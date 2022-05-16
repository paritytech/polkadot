// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! # Overseer
//!
//! `overseer` implements the Overseer architecture described in the
//! [implementers-guide](https://w3f.github.io/parachain-implementers-guide/node/index.html).
//! For the motivations behind implementing the overseer itself you should
//! check out that guide, documentation in this crate will be mostly discussing
//! technical stuff.
//!
//! An `Overseer` is something that allows spawning/stopping and overseeing
//! asynchronous tasks as well as establishing a well-defined and easy to use
//! protocol that the tasks can use to communicate with each other. It is desired
//! that this protocol is the only way tasks communicate with each other, however
//! at this moment there are no foolproof guards against other ways of communication.
//!
//! The `Overseer` is instantiated with a pre-defined set of `Subsystems` that
//! share the same behavior from `Overseer`'s point of view.
//!
//! ```text
//!                              +-----------------------------+
//!                              |         Overseer            |
//!                              +-----------------------------+
//!
//!             ................|  Overseer "holds" these and uses |..............
//!             .                  them to (re)start things                      .
//!             .                                                                .
//!             .  +-------------------+                +---------------------+  .
//!             .  |   Subsystem1      |                |   Subsystem2        |  .
//!             .  +-------------------+                +---------------------+  .
//!             .           |                                       |            .
//!             ..................................................................
//!                         |                                       |
//!                       start()                                 start()
//!                         V                                       V
//!             ..................| Overseer "runs" these |.......................
//!             .  +--------------------+               +---------------------+  .
//!             .  | SubsystemInstance1 |               | SubsystemInstance2  |  .
//!             .  +--------------------+               +---------------------+  .
//!             ..................................................................
//! ```

// #![deny(unused_results)]
// unused dependencies can not work for test and examples at the same time
// yielding false positives
#![warn(missing_docs)]

use std::{
	collections::{hash_map, HashMap},
	fmt::{self, Debug},
	pin::Pin,
	sync::Arc,
	time::Duration,
};

use futures::{channel::oneshot, future::BoxFuture, select, Future, FutureExt, StreamExt};
use lru::LruCache;

use client::{BlockImportNotification, BlockchainEvents, FinalityNotification};
use polkadot_primitives::{
	runtime_api::ParachainHost,
	v2::{Block, BlockId, BlockNumber, Hash},
};
use sp_api::{ApiExt, ProvideRuntimeApi};

use polkadot_node_subsystem_types::messages::{
	ApprovalDistributionMessage, ApprovalVotingMessage, AvailabilityDistributionMessage,
	AvailabilityRecoveryMessage, AvailabilityStoreMessage, BitfieldDistributionMessage,
	BitfieldSigningMessage, CandidateBackingMessage, CandidateValidationMessage, ChainApiMessage,
	ChainSelectionMessage, CollationGenerationMessage, CollatorProtocolMessage,
	DisputeCoordinatorMessage, DisputeDistributionMessage, GossipSupportMessage,
	NetworkBridgeMessage, ProvisionerMessage, PvfCheckerMessage, RuntimeApiMessage,
	StatementDistributionMessage,
};
pub use polkadot_node_subsystem_types::{
	errors::{SubsystemError, SubsystemResult},
	jaeger, ActivatedLeaf, ActiveLeavesUpdate, LeafStatus, OverseerSignal,
};

pub mod metrics;
pub use self::metrics::Metrics as OverseerMetrics;

/// A dummy subsystem, mostly useful for placeholders and tests.
pub mod dummy;
pub use self::dummy::DummySubsystem;

pub use polkadot_node_metrics::{
	metrics::{prometheus, Metrics as MetricsTrait},
	Metronome,
};

use parity_util_mem::MemoryAllocationTracker;

pub use polkadot_overseer_gen as gen;
pub use polkadot_overseer_gen::{
	contextbounds, orchestra, subsystem, FromOverseer, MapSubsystem, MessagePacket, SignalsReceived,
	SpawnNamed, Subsystem, SubsystemContext, SubsystemIncomingMessages, SubsystemInstance,
	SubsystemMeterReadouts, SubsystemMeters, SubsystemSender, TimeoutExt, ToOverseer,
};

/// Store 2 days worth of blocks, not accounting for forks,
/// in the LRU cache. Assumes a 6-second block time.
pub const KNOWN_LEAVES_CACHE_SIZE: usize = 2 * 24 * 3600 / 6;

#[cfg(test)]
mod tests;

/// Whether a header supports parachain consensus or not.
pub trait HeadSupportsParachains {
	/// Return true if the given header supports parachain consensus. Otherwise, false.
	fn head_supports_parachains(&self, head: &Hash) -> bool;
}

impl<Client> HeadSupportsParachains for Arc<Client>
where
	Client: ProvideRuntimeApi<Block>,
	Client::Api: ParachainHost<Block>,
{
	fn head_supports_parachains(&self, head: &Hash) -> bool {
		let id = BlockId::Hash(*head);
		// Check that the `ParachainHost` runtime api is at least with version 1 present on chain.
		self.runtime_api()
			.api_version::<dyn ParachainHost<Block>>(&id)
			.ok()
			.flatten()
			.unwrap_or(0) >=
			1
	}
}

/// A handle used to communicate with the [`Overseer`].
///
/// [`Overseer`]: struct.Overseer.html
#[derive(Clone)]
pub struct Handle(OverseerHandle);

impl Handle {
	/// Create a new [`Handle`].
	pub fn new(raw: OverseerHandle) -> Self {
		Self(raw)
	}

	/// Inform the `Overseer` that that some block was imported.
	pub async fn block_imported(&mut self, block: BlockInfo) {
		self.send_and_log_error(Event::BlockImported(block)).await
	}

	/// Send some message to one of the `Subsystem`s.
	pub async fn send_msg(&mut self, msg: impl Into<AllMessages>, origin: &'static str) {
		self.send_and_log_error(Event::MsgToSubsystem { msg: msg.into(), origin }).await
	}

	/// Send a message not providing an origin.
	#[inline(always)]
	pub async fn send_msg_anon(&mut self, msg: impl Into<AllMessages>) {
		self.send_msg(msg, "").await
	}

	/// Inform the `Overseer` that some block was finalized.
	pub async fn block_finalized(&mut self, block: BlockInfo) {
		self.send_and_log_error(Event::BlockFinalized(block)).await
	}

	/// Wait for a block with the given hash to be in the active-leaves set.
	///
	/// The response channel responds if the hash was activated and is closed if the hash was deactivated.
	/// Note that due the fact the overseer doesn't store the whole active-leaves set, only deltas,
	/// the response channel may never return if the hash was deactivated before this call.
	/// In this case, it's the caller's responsibility to ensure a timeout is set.
	pub async fn wait_for_activation(
		&mut self,
		hash: Hash,
		response_channel: oneshot::Sender<SubsystemResult<()>>,
	) {
		self.send_and_log_error(Event::ExternalRequest(ExternalRequest::WaitForActivation {
			hash,
			response_channel,
		}))
		.await;
	}

	/// Tell `Overseer` to shutdown.
	pub async fn stop(&mut self) {
		self.send_and_log_error(Event::Stop).await;
	}

	/// Most basic operation, to stop a server.
	async fn send_and_log_error(&mut self, event: Event) {
		if self.0.send(event).await.is_err() {
			gum::info!(target: LOG_TARGET, "Failed to send an event to Overseer");
		}
	}
}

/// An event telling the `Overseer` on the particular block
/// that has been imported or finalized.
///
/// This structure exists solely for the purposes of decoupling
/// `Overseer` code from the client code and the necessity to call
/// `HeaderBackend::block_number_from_id()`.
#[derive(Debug, Clone)]
pub struct BlockInfo {
	/// hash of the block.
	pub hash: Hash,
	/// hash of the parent block.
	pub parent_hash: Hash,
	/// block's number.
	pub number: BlockNumber,
}

impl From<BlockImportNotification<Block>> for BlockInfo {
	fn from(n: BlockImportNotification<Block>) -> Self {
		BlockInfo { hash: n.hash, parent_hash: n.header.parent_hash, number: n.header.number }
	}
}

impl From<FinalityNotification<Block>> for BlockInfo {
	fn from(n: FinalityNotification<Block>) -> Self {
		BlockInfo { hash: n.hash, parent_hash: n.header.parent_hash, number: n.header.number }
	}
}

/// An event from outside the overseer scope, such
/// as the substrate framework or user interaction.
pub enum Event {
	/// A new block was imported.
	BlockImported(BlockInfo),
	/// A block was finalized with i.e. babe or another consensus algorithm.
	BlockFinalized(BlockInfo),
	/// Message as sent to a subsystem.
	MsgToSubsystem {
		/// The actual message.
		msg: AllMessages,
		/// The originating subsystem name.
		origin: &'static str,
	},
	/// A request from the outer world.
	ExternalRequest(ExternalRequest),
	/// Stop the overseer on i.e. a UNIX signal.
	Stop,
}

/// Some request from outer world.
pub enum ExternalRequest {
	/// Wait for the activation of a particular hash
	/// and be notified by means of the return channel.
	WaitForActivation {
		/// The relay parent for which activation to wait for.
		hash: Hash,
		/// Response channel to await on.
		response_channel: oneshot::Sender<SubsystemResult<()>>,
	},
}

/// Glues together the [`Overseer`] and `BlockchainEvents` by forwarding
/// import and finality notifications into the [`OverseerHandle`].
pub async fn forward_events<P: BlockchainEvents<Block>>(client: Arc<P>, mut handle: Handle) {
	let mut finality = client.finality_notification_stream();
	let mut imports = client.import_notification_stream();

	loop {
		select! {
			f = finality.next() => {
				match f {
					Some(block) => {
						handle.block_finalized(block.into()).await;
					}
					None => break,
				}
			},
			i = imports.next() => {
				match i {
					Some(block) => {
						handle.block_imported(block.into()).await;
					}
					None => break,
				}
			},
			complete => break,
		}
	}
}

/// Create a new instance of the [`Overseer`] with a fixed set of [`Subsystem`]s.
///
/// This returns the overseer along with an [`OverseerHandle`] which can
/// be used to send messages from external parts of the codebase.
///
/// The [`OverseerHandle`] returned from this function is connected to
/// the returned [`Overseer`].
///
/// ```text
///                  +------------------------------------+
///                  |            Overseer                |
///                  +------------------------------------+
///                    /            |             |      \
///      ................. subsystems...................................
///      . +-----------+    +-----------+   +----------+   +---------+ .
///      . |           |    |           |   |          |   |         | .
///      . +-----------+    +-----------+   +----------+   +---------+ .
///      ...............................................................
///                              |
///                        probably `spawn`
///                            a `job`
///                              |
///                              V
///                         +-----------+
///                         |           |
///                         +-----------+
///
/// ```
///
/// [`Subsystem`]: trait.Subsystem.html
///
/// # Example
///
/// The [`Subsystems`] may be any type as long as they implement an expected interface.
/// Here, we create a mock validation subsystem and a few dummy ones and start the `Overseer` with them.
/// For the sake of simplicity the termination of the example is done with a timeout.
/// ```
/// # use std::time::Duration;
/// # use futures::{executor, pin_mut, select, FutureExt};
/// # use futures_timer::Delay;
/// # use polkadot_primitives::v2::Hash;
/// # use polkadot_overseer::{
/// # 	self as overseer,
/// #   OverseerSignal,
/// # 	SubsystemSender as _,
/// # 	AllMessages,
/// # 	HeadSupportsParachains,
/// # 	Overseer,
/// # 	SubsystemError,
/// # 	gen::{
/// # 		SubsystemContext,
/// # 		FromOverseer,
/// # 		SpawnedSubsystem,
/// # 	},
/// # };
/// # use polkadot_node_subsystem_types::messages::{
/// # 	CandidateValidationMessage, CandidateBackingMessage,
/// # 	NetworkBridgeMessage,
/// # };
///
/// struct ValidationSubsystem;
///
/// impl<Ctx> overseer::Subsystem<Ctx, SubsystemError> for ValidationSubsystem
/// where
///     Ctx: overseer::SubsystemContext<
///				Message=CandidateValidationMessage,
///				AllMessages=AllMessages,
///				Signal=OverseerSignal,
///				Error=SubsystemError,
///			>,
/// {
///     fn start(
///         self,
///         mut ctx: Ctx,
///     ) -> SpawnedSubsystem<SubsystemError> {
///         SpawnedSubsystem {
///             name: "validation-subsystem",
///             future: Box::pin(async move {
///                 loop {
///                     Delay::new(Duration::from_secs(1)).await;
///                 }
///             }),
///         }
///     }
/// }
///
/// # fn main() { executor::block_on(async move {
///
/// struct AlwaysSupportsParachains;
/// impl HeadSupportsParachains for AlwaysSupportsParachains {
///      fn head_supports_parachains(&self, _head: &Hash) -> bool { true }
/// }
/// let spawner = sp_core::testing::TaskExecutor::new();
/// let (overseer, _handle) = dummy_overseer_builder(spawner, AlwaysSupportsParachains, None)
///		.unwrap()
///		.replace_candidate_validation(|_| ValidationSubsystem)
///		.build()
///		.unwrap();
///
/// let timer = Delay::new(Duration::from_millis(50)).fuse();
///
/// let overseer_fut = overseer.run().fuse();
/// pin_mut!(timer);
/// pin_mut!(overseer_fut);
///
/// select! {
///     _ = overseer_fut => (),
///     _ = timer => (),
/// }
/// #
/// # 	});
/// # }
/// ```
#[orchestra(
	gen=AllMessages,
	event=Event,
	signal=OverseerSignal,
	error=SubsystemError,
	message_capacity=2048,
)]
pub struct Overseer<SupportsParachains> {
	#[subsystem(CandidateValidationMessage, sends: [
		RuntimeApiMessage,
	])]
	candidate_validation: CandidateValidation,

	#[subsystem(PvfCheckerMessage, sends: [
		CandidateValidationMessage,
		RuntimeApiMessage,
	])]
	pvf_checker: PvfChecker,

	#[subsystem(CandidateBackingMessage, sends: [
		CandidateValidationMessage,
		CollatorProtocolMessage,
		AvailabilityDistributionMessage,
		AvailabilityStoreMessage,
		StatementDistributionMessage,
		ProvisionerMessage,
		RuntimeApiMessage,
		DisputeCoordinatorMessage,
	])]
	candidate_backing: CandidateBacking,

	#[subsystem(StatementDistributionMessage, sends: [
		NetworkBridgeMessage,
		CandidateBackingMessage,
		RuntimeApiMessage,
	])]
	statement_distribution: StatementDistribution,

	#[subsystem(AvailabilityDistributionMessage, sends: [
		AvailabilityStoreMessage,
		AvailabilityRecoveryMessage,
		ChainApiMessage,
		RuntimeApiMessage,
		NetworkBridgeMessage,
	])]
	availability_distribution: AvailabilityDistribution,

	#[subsystem(AvailabilityRecoveryMessage, sends: [
		NetworkBridgeMessage,
		RuntimeApiMessage,
		AvailabilityStoreMessage,
	])]
	availability_recovery: AvailabilityRecovery,

	#[subsystem(blocking, BitfieldSigningMessage, sends: [
		AvailabilityStoreMessage,
		RuntimeApiMessage,
		BitfieldDistributionMessage,
	])]
	bitfield_signing: BitfieldSigning,

	#[subsystem(BitfieldDistributionMessage, sends: [
		RuntimeApiMessage,
		NetworkBridgeMessage,
		ProvisionerMessage,
	])]
	bitfield_distribution: BitfieldDistribution,

	#[subsystem(ProvisionerMessage, sends: [
		RuntimeApiMessage,
		CandidateBackingMessage,
		ChainApiMessage,
		DisputeCoordinatorMessage,
	])]
	provisioner: Provisioner,

	#[subsystem(blocking, RuntimeApiMessage, sends: [])]
	runtime_api: RuntimeApi,

	#[subsystem(blocking, AvailabilityStoreMessage, sends: [
		ChainApiMessage,
		RuntimeApiMessage,
	])]
	availability_store: AvailabilityStore,

	#[subsystem(NetworkBridgeMessage, sends: [
		BitfieldDistributionMessage,
		StatementDistributionMessage,
		ApprovalDistributionMessage,
		GossipSupportMessage,
		DisputeDistributionMessage,
		CollationGenerationMessage,
		CollatorProtocolMessage,
	])]
	network_bridge: NetworkBridge,

	#[subsystem(blocking, ChainApiMessage, sends: [])]
	chain_api: ChainApi,

	#[subsystem(CollationGenerationMessage, sends: [
		RuntimeApiMessage,
		CollatorProtocolMessage,
	])]
	collation_generation: CollationGeneration,

	#[subsystem(CollatorProtocolMessage, sends: [
		NetworkBridgeMessage,
		RuntimeApiMessage,
		CandidateBackingMessage,
	])]
	collator_protocol: CollatorProtocol,

	#[subsystem(ApprovalDistributionMessage, sends: [
		NetworkBridgeMessage,
		ApprovalVotingMessage,
	])]
	approval_distribution: ApprovalDistribution,

	#[subsystem(blocking, ApprovalVotingMessage, sends: [
		RuntimeApiMessage,
		ChainApiMessage,
		ChainSelectionMessage,
		DisputeCoordinatorMessage,
		AvailabilityRecoveryMessage,
		ApprovalDistributionMessage,
		CandidateValidationMessage,
	])]
	approval_voting: ApprovalVoting,

	#[subsystem(GossipSupportMessage, sends: [
		NetworkBridgeMessage,
		RuntimeApiMessage,
		ChainSelectionMessage,
	])]
	gossip_support: GossipSupport,

	#[subsystem(blocking, DisputeCoordinatorMessage, sends: [
		RuntimeApiMessage,
		ChainApiMessage,
		DisputeDistributionMessage,
		CandidateValidationMessage,
		AvailabilityStoreMessage,
		AvailabilityRecoveryMessage,
	])]
	dispute_coordinator: DisputeCoordinator,

	#[subsystem(DisputeDistributionMessage, sends: [
		RuntimeApiMessage,
		DisputeCoordinatorMessage,
		NetworkBridgeMessage,
	])]
	dispute_distribution: DisputeDistribution,

	#[subsystem(blocking, ChainSelectionMessage, sends: [ChainApiMessage])]
	chain_selection: ChainSelection,

	/// External listeners waiting for a hash to be in the active-leave set.
	pub activation_external_listeners: HashMap<Hash, Vec<oneshot::Sender<SubsystemResult<()>>>>,

	/// Stores the [`jaeger::Span`] per active leaf.
	pub span_per_active_leaf: HashMap<Hash, Arc<jaeger::Span>>,

	/// A set of leaves that `Overseer` starts working with.
	///
	/// Drained at the beginning of `run` and never used again.
	pub leaves: Vec<(Hash, BlockNumber)>,

	/// The set of the "active leaves".
	pub active_leaves: HashMap<Hash, BlockNumber>,

	/// An implementation for checking whether a header supports parachain consensus.
	pub supports_parachains: SupportsParachains,

	/// An LRU cache for keeping track of relay-chain heads that have already been seen.
	pub known_leaves: LruCache<Hash, ()>,

	/// Various Prometheus metrics.
	pub metrics: OverseerMetrics,
}

/// Spawn the metrics metronome task.
pub fn spawn_metronome_metrics<S, SupportsParachains>(
	overseer: &mut Overseer<S, SupportsParachains>,
	metronome_metrics: OverseerMetrics,
) -> Result<(), SubsystemError>
where
	S: SpawnNamed,
	SupportsParachains: HeadSupportsParachains,
{
	struct ExtractNameAndMeters;

	impl<'a, T: 'a> MapSubsystem<&'a OverseenSubsystem<T>> for ExtractNameAndMeters {
		type Output = Option<(&'static str, SubsystemMeters)>;

		fn map_subsystem(&self, subsystem: &'a OverseenSubsystem<T>) -> Self::Output {
			subsystem
				.instance
				.as_ref()
				.map(|instance| (instance.name, instance.meters.clone()))
		}
	}
	let subsystem_meters = overseer.map_subsystems(ExtractNameAndMeters);

	let collect_memory_stats: Box<dyn Fn(&OverseerMetrics) + Send> =
		match MemoryAllocationTracker::new() {
			Ok(memory_stats) =>
				Box::new(move |metrics: &OverseerMetrics| match memory_stats.snapshot() {
					Ok(memory_stats_snapshot) => {
						gum::trace!(
							target: LOG_TARGET,
							"memory_stats: {:?}",
							&memory_stats_snapshot
						);
						metrics.memory_stats_snapshot(memory_stats_snapshot);
					},
					Err(e) =>
						gum::debug!(target: LOG_TARGET, "Failed to obtain memory stats: {:?}", e),
				}),
			Err(_) => {
				gum::debug!(
					target: LOG_TARGET,
					"Memory allocation tracking is not supported by the allocator.",
				);

				Box::new(|_| {})
			},
		};

	let metronome = Metronome::new(std::time::Duration::from_millis(950)).for_each(move |_| {
		collect_memory_stats(&metronome_metrics);

		// We combine the amount of messages from subsystems to the overseer
		// as well as the amount of messages from external sources to the overseer
		// into one `to_overseer` value.
		metronome_metrics.channel_metrics_snapshot(
			subsystem_meters
				.iter()
				.cloned()
				.filter_map(|x| x)
				.map(|(name, ref meters)| (name, meters.read())),
		);

		futures::future::ready(())
	});
	overseer
		.spawner()
		.spawn("metrics-metronome", Some("overseer"), Box::pin(metronome));

	Ok(())
}

impl<S, SupportsParachains> Overseer<S, SupportsParachains>
where
	SupportsParachains: HeadSupportsParachains,
	S: SpawnNamed,
{
	/// Stop the `Overseer`.
	async fn stop(mut self) {
		let _ = self.wait_terminate(OverseerSignal::Conclude, Duration::from_secs(1_u64)).await;
	}

	/// Run the `Overseer`.
	pub async fn run(mut self) -> SubsystemResult<()> {
		let metrics = self.metrics.clone();
		spawn_metronome_metrics(&mut self, metrics)?;

		// Notify about active leaves on startup before starting the loop
		for (hash, number) in std::mem::take(&mut self.leaves) {
			let _ = self.active_leaves.insert(hash, number);
			if let Some((span, status)) = self.on_head_activated(&hash, None) {
				let update =
					ActiveLeavesUpdate::start_work(ActivatedLeaf { hash, number, status, span });
				self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await?;
			}
		}

		loop {
			select! {
				msg = self.events_rx.select_next_some() => {
					match msg {
						Event::MsgToSubsystem { msg, origin } => {
							self.route_message(msg.into(), origin).await?;
							self.metrics.on_message_relayed();
						}
						Event::Stop => {
							self.stop().await;
							return Ok(());
						}
						Event::BlockImported(block) => {
							self.block_imported(block).await?;
						}
						Event::BlockFinalized(block) => {
							self.block_finalized(block).await?;
						}
						Event::ExternalRequest(request) => {
							self.handle_external_request(request);
						}
					}
				},
				msg = self.to_overseer_rx.select_next_some() => {
					match msg {
						ToOverseer::SpawnJob { name, subsystem, s } => {
							self.spawn_job(name, subsystem, s);
						}
						ToOverseer::SpawnBlockingJob { name, subsystem, s } => {
							self.spawn_blocking_job(name, subsystem, s);
						}
					}
				},
				res = self.running_subsystems.select_next_some() => {
					gum::error!(
						target: LOG_TARGET,
						subsystem = ?res,
						"subsystem finished unexpectedly",
					);
					self.stop().await;
					return res;
				},
			}
		}
	}

	async fn block_imported(&mut self, block: BlockInfo) -> SubsystemResult<()> {
		match self.active_leaves.entry(block.hash) {
			hash_map::Entry::Vacant(entry) => entry.insert(block.number),
			hash_map::Entry::Occupied(entry) => {
				debug_assert_eq!(*entry.get(), block.number);
				return Ok(())
			},
		};

		let mut update = match self.on_head_activated(&block.hash, Some(block.parent_hash)) {
			Some((span, status)) => ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: block.hash,
				number: block.number,
				status,
				span,
			}),
			None => ActiveLeavesUpdate::default(),
		};

		if let Some(number) = self.active_leaves.remove(&block.parent_hash) {
			debug_assert_eq!(block.number.saturating_sub(1), number);
			update.deactivated.push(block.parent_hash);
			self.on_head_deactivated(&block.parent_hash);
		}

		self.clean_up_external_listeners();

		if !update.is_empty() {
			self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await?;
		}
		Ok(())
	}

	async fn block_finalized(&mut self, block: BlockInfo) -> SubsystemResult<()> {
		let mut update = ActiveLeavesUpdate::default();

		self.active_leaves.retain(|h, n| {
			// prune all orphaned leaves, but don't prune
			// the finalized block if it is itself a leaf.
			if *n <= block.number && *h != block.hash {
				update.deactivated.push(*h);
				false
			} else {
				true
			}
		});

		for deactivated in &update.deactivated {
			self.on_head_deactivated(deactivated)
		}

		self.broadcast_signal(OverseerSignal::BlockFinalized(block.hash, block.number))
			.await?;

		// If there are no leaves being deactivated, we don't need to send an update.
		//
		// Our peers will be informed about our finalized block the next time we activating/deactivating some leaf.
		if !update.is_empty() {
			self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await?;
		}

		Ok(())
	}

	/// Handles a header activation. If the header's state doesn't support the parachains API,
	/// this returns `None`.
	fn on_head_activated(
		&mut self,
		hash: &Hash,
		parent_hash: Option<Hash>,
	) -> Option<(Arc<jaeger::Span>, LeafStatus)> {
		if !self.supports_parachains.head_supports_parachains(hash) {
			return None
		}

		self.metrics.on_head_activated();
		if let Some(listeners) = self.activation_external_listeners.remove(hash) {
			for listener in listeners {
				// it's fine if the listener is no longer interested
				let _ = listener.send(Ok(()));
			}
		}

		let mut span = jaeger::Span::new(*hash, "leaf-activated");

		if let Some(parent_span) = parent_hash.and_then(|h| self.span_per_active_leaf.get(&h)) {
			span.add_follows_from(&*parent_span);
		}

		let span = Arc::new(span);
		self.span_per_active_leaf.insert(*hash, span.clone());

		let status = if let Some(_) = self.known_leaves.put(*hash, ()) {
			LeafStatus::Stale
		} else {
			LeafStatus::Fresh
		};

		Some((span, status))
	}

	fn on_head_deactivated(&mut self, hash: &Hash) {
		self.metrics.on_head_deactivated();
		self.activation_external_listeners.remove(hash);
		self.span_per_active_leaf.remove(hash);
	}

	fn clean_up_external_listeners(&mut self) {
		self.activation_external_listeners.retain(|_, v| {
			// remove dead listeners
			v.retain(|c| !c.is_canceled());
			!v.is_empty()
		})
	}

	fn handle_external_request(&mut self, request: ExternalRequest) {
		match request {
			ExternalRequest::WaitForActivation { hash, response_channel } => {
				// We use known leaves here because the `WaitForActivation` message
				// is primarily concerned about leaves which subsystems have simply
				// not been made aware of yet. Anything in the known leaves set,
				// even if stale, has been activated in the past.
				if self.known_leaves.peek(&hash).is_some() {
					// it's fine if the listener is no longer interested
					let _ = response_channel.send(Ok(()));
				} else {
					self.activation_external_listeners
						.entry(hash)
						.or_default()
						.push(response_channel);
				}
			},
		}
	}

	fn spawn_job(
		&mut self,
		task_name: &'static str,
		subsystem_name: Option<&'static str>,
		j: BoxFuture<'static, ()>,
	) {
		self.spawner.spawn(task_name, subsystem_name, j);
	}

	fn spawn_blocking_job(
		&mut self,
		task_name: &'static str,
		subsystem_name: Option<&'static str>,
		j: BoxFuture<'static, ()>,
	) {
		self.spawner.spawn_blocking(task_name, subsystem_name, j);
	}
}
