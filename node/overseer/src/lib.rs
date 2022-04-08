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
	collections::{hash_map, BTreeMap, HashMap},
	fmt::{self, Debug},
	pin::Pin,
	sync::Arc,
	time::Duration,
};

use futures::{channel::oneshot, future::BoxFuture, select, Future, FutureExt, Stream, StreamExt};
use lru::LruCache;

use client::{BlockImportNotification, BlockchainEvents, FinalityNotification};
use polkadot_primitives::{
	v1::{
		CandidateCommitments, CandidateEvent,
		CommittedCandidateReceipt, CoreState, GroupRotationInfo, Hash, Header, Id,
		InboundDownwardMessage, InboundHrmpMessage, OccupiedCoreAssumption,
		PersistedValidationData, ScrapedOnChainVotes, SessionIndex, ValidationCode,
		ValidationCodeHash, ValidatorId, ValidatorIndex, ValidatorSignature,
	},
	v2::{Block, BlockId, BlockNumber, Hash, ParachainHost, ParachainHost, PvfCheckStatement, SessionInfo},
};
use sp_api::{ApiExt, ProvideRuntimeApi};

use polkadot_node_network_protocol::v1 as protocol_v1;
use polkadot_node_subsystem_types::messages::{
	ApprovalDistributionMessage, ApprovalVotingMessage, AvailabilityDistributionMessage,
	AvailabilityRecoveryMessage, AvailabilityStoreMessage, BitfieldDistributionMessage,
	BitfieldSigningMessage, CandidateBackingMessage, CandidateValidationMessage, ChainApiMessage,
	ChainSelectionMessage, CollationGenerationMessage, CollatorProtocolMessage,
	DisputeCoordinatorMessage, DisputeDistributionMessage, GossipSupportMessage,
	NetworkBridgeEvent, NetworkBridgeMessage, ProvisionerMessage, PvfCheckerMessage,
	RuntimeApiMessage, StatementDistributionMessage,
};
pub use polkadot_node_subsystem_types::{
	errors::{SubsystemError, SubsystemResult},
	jaeger, ActivatedLeaf, ActiveLeavesUpdate, LeafStatus, OverseerSignal,
};

use sp_api::ApiError;
use sp_authority_discovery::AuthorityDiscoveryApi;
use sp_consensus_babe::{
	AuthorityId, BabeApi, BabeGenesisConfiguration, Epoch, EquivocationProof,
	OpaqueKeyOwnershipProof, Slot,
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
	overlord, FromOverseer, MapSubsystem, MessagePacket, SignalsReceived, SpawnNamed, Subsystem,
	SubsystemContext, SubsystemIncomingMessages, SubsystemInstance, SubsystemMeterReadouts,
	SubsystemMeters, SubsystemSender, TimeoutExt, ToOverseer,
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
	Client: OverseerRuntimeClient,
{
	fn head_supports_parachains(&self, head: &Hash) -> bool {
		let id = BlockId::Hash(*head);
		// Check that the `ParachainHost` runtime api is at least with version 1 present on chain.
		self.api_version_parachain_host(&id).ok().flatten().unwrap_or(0) >= 1
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

/// Glues together the [`Overseer`] and `BlockchainEvents` by forwarding
/// import and finality notifications into the [`OverseerHandle`].
pub async fn forward_collator_events<P: client::BlockchainRPCEvents<Block>>(
	client: Arc<P>,
	mut handle: Handle,
) {
	let mut finality = client.finality_notification_stream().fuse();
	let mut imports = client.import_notification_stream().fuse();

	loop {
		select! {
			f = finality.next() => {
				match f {
					Some(header) => {
						/// TODO Implement into
						let block_info = BlockInfo {
							hash: header.hash(),
							parent_hash: header.parent_hash,
							number: header.number
						};
						handle.block_finalized(block_info).await;
					}
					None => break,
				}
			},
			i = imports.next() => {
				match i {
					Some(header) => {
						let block_info = BlockInfo {
							hash: header.hash(),
							parent_hash: header.parent_hash,
							number: header.number
						};
						handle.block_imported(block_info).await;
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
#[overlord(
	gen=AllMessages,
	event=Event,
	signal=OverseerSignal,
	error=SubsystemError,
	network=NetworkBridgeEvent<protocol_v1::ValidationProtocol>,
)]
pub struct Overseer<SupportsParachains> {
	#[subsystem(no_dispatch, CandidateValidationMessage)]
	candidate_validation: CandidateValidation,

	#[subsystem(no_dispatch, PvfCheckerMessage)]
	pvf_checker: PvfChecker,

	#[subsystem(no_dispatch, CandidateBackingMessage)]
	candidate_backing: CandidateBacking,

	#[subsystem(StatementDistributionMessage)]
	statement_distribution: StatementDistribution,

	#[subsystem(no_dispatch, AvailabilityDistributionMessage)]
	availability_distribution: AvailabilityDistribution,

	#[subsystem(no_dispatch, AvailabilityRecoveryMessage)]
	availability_recovery: AvailabilityRecovery,

	#[subsystem(blocking, no_dispatch, BitfieldSigningMessage)]
	bitfield_signing: BitfieldSigning,

	#[subsystem(BitfieldDistributionMessage)]
	bitfield_distribution: BitfieldDistribution,

	#[subsystem(no_dispatch, ProvisionerMessage)]
	provisioner: Provisioner,

	#[subsystem(no_dispatch, blocking, RuntimeApiMessage)]
	runtime_api: RuntimeApi,

	#[subsystem(no_dispatch, blocking, AvailabilityStoreMessage)]
	availability_store: AvailabilityStore,

	#[subsystem(no_dispatch, NetworkBridgeMessage)]
	network_bridge: NetworkBridge,

	#[subsystem(no_dispatch, blocking, ChainApiMessage)]
	chain_api: ChainApi,

	#[subsystem(no_dispatch, CollationGenerationMessage)]
	collation_generation: CollationGeneration,

	#[subsystem(no_dispatch, CollatorProtocolMessage)]
	collator_protocol: CollatorProtocol,

	#[subsystem(ApprovalDistributionMessage)]
	approval_distribution: ApprovalDistribution,

	#[subsystem(no_dispatch, blocking, ApprovalVotingMessage)]
	approval_voting: ApprovalVoting,

	#[subsystem(GossipSupportMessage)]
	gossip_support: GossipSupport,

	#[subsystem(no_dispatch, blocking, DisputeCoordinatorMessage)]
	dispute_coordinator: DisputeCoordinator,

	#[subsystem(no_dispatch, DisputeDistributionMessage)]
	dispute_distribution: DisputeDistribution,

	#[subsystem(no_dispatch, blocking, ChainSelectionMessage)]
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

pub trait OverseerRuntimeClient {
	fn validators(&self, at: &BlockId) -> Result<Vec<ValidatorId>, ApiError>;

	fn validator_groups(
		&self,
		at: &BlockId,
	) -> Result<(Vec<Vec<ValidatorIndex>>, GroupRotationInfo<BlockNumber>), ApiError>;

	/// Yields information on all availability cores as relevant to the child block.
	/// Cores are either free or occupied. Free cores can have paras assigned to them.
	fn availability_cores(
		&self,
		at: &BlockId,
	) -> Result<Vec<CoreState<Hash, BlockNumber>>, ApiError>;

	/// Yields the persisted validation data for the given `ParaId` along with an assumption that
	/// should be used if the para currently occupies a core.
	///
	/// Returns `None` if either the para is not registered or the assumption is `Freed`
	/// and the para already occupies a core.
	fn persisted_validation_data(
		&self,
		at: &BlockId,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<PersistedValidationData<Hash, BlockNumber>>, ApiError>;

	/// Returns the persisted validation data for the given `ParaId` along with the corresponding
	/// validation code hash. Instead of accepting assumption about the para, matches the validation
	/// data hash against an expected one and yields `None` if they're not equal.
	fn assumed_validation_data(
		&self,
		at: &BlockId,
		para_id: Id,
		expected_persisted_validation_data_hash: Hash,
	) -> Result<Option<(PersistedValidationData<Hash, BlockNumber>, ValidationCodeHash)>, ApiError>;

	/// Checks if the given validation outputs pass the acceptance criteria.
	fn check_validation_outputs(
		&self,
		at: &BlockId,
		para_id: Id,
		outputs: CandidateCommitments,
	) -> Result<bool, ApiError>;

	/// Returns the session index expected at a child of the block.
	///
	/// This can be used to instantiate a `SigningContext`.
	fn session_index_for_child(&self, at: &BlockId) -> Result<SessionIndex, ApiError>;

	/// Fetch the validation code used by a para, making the given `OccupiedCoreAssumption`.
	///
	/// Returns `None` if either the para is not registered or the assumption is `Freed`
	/// and the para already occupies a core.
	fn validation_code(
		&self,
		at: &BlockId,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<ValidationCode>, ApiError>;

	/// Get the receipt of a candidate pending availability. This returns `Some` for any paras
	/// assigned to occupied cores in `availability_cores` and `None` otherwise.
	fn candidate_pending_availability(
		&self,
		at: &BlockId,
		para_id: Id,
	) -> Result<Option<CommittedCandidateReceipt<Hash>>, ApiError>;

	/// Get a vector of events concerning candidates that occurred within a block.
	fn candidate_events(&self, at: &BlockId) -> Result<Vec<CandidateEvent<Hash>>, ApiError>;

	/// Get all the pending inbound messages in the downward message queue for a para.
	fn dmq_contents(
		&self,
		at: &BlockId,
		recipient: Id,
	) -> Result<Vec<InboundDownwardMessage<BlockNumber>>, ApiError>;

	/// Get the contents of all channels addressed to the given recipient. Channels that have no
	/// messages in them are also included.
	fn inbound_hrmp_channels_contents(
		&self,
		at: &BlockId,
		recipient: Id,
	) -> Result<BTreeMap<Id, Vec<InboundHrmpMessage<BlockNumber>>>, ApiError>;

	/// Get the validation code from its hash.
	fn validation_code_by_hash(
		&self,
		at: &BlockId,
		hash: ValidationCodeHash,
	) -> Result<Option<ValidationCode>, ApiError>;

	/// Scrape dispute relevant from on-chain, backing votes and resolved disputes.
	fn on_chain_votes(&self, at: &BlockId) -> Result<Option<ScrapedOnChainVotes<Hash>>, ApiError>;

	/***** Added in v2 *****/

	/// Get the session info for the given session, if stored.
	///
	/// NOTE: This function is only available since parachain host version 2.
	fn session_info(
		&self,
		at: &BlockId,
		index: SessionIndex,
	) -> Result<Option<SessionInfo>, ApiError>;

	/// Get the session info for the given session, if stored.
	///
	/// NOTE: This function is only available since parachain host version 2.
	fn session_info_before_version_2(
		&self,
		at: &BlockId,
		index: SessionIndex,
	) -> Result<Option<polkadot_primitives::v1::SessionInfo>, ApiError>;

	/// Submits a PVF pre-checking statement into the transaction pool.
	///
	/// NOTE: This function is only available since parachain host version 2.
	fn submit_pvf_check_statement(
		&self,
		at: &BlockId,
		stmt: PvfCheckStatement,
		signature: ValidatorSignature,
	) -> Result<(), ApiError>;

	/// Returns code hashes of PVFs that require pre-checking by validators in the active set.
	///
	/// NOTE: This function is only available since parachain host version 2.
	fn pvfs_require_precheck(&self, at: &BlockId) -> Result<Vec<ValidationCodeHash>, ApiError>;

	/// Fetch the hash of the validation code used by a para, making the given `OccupiedCoreAssumption`.
	///
	/// NOTE: This function is only available since parachain host version 2.
	fn validation_code_hash(
		&self,
		at: &BlockId,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<ValidationCodeHash>, ApiError>;

	/// ===BABE===
	///
	///
	/// Return the genesis configuration for BABE. The configuration is only read on genesis.
	fn configuration(&self, at: &BlockId) -> Result<BabeGenesisConfiguration, ApiError>;

	/// Returns the slot that started the current epoch.
	fn current_epoch_start(&self, at: &BlockId) -> Result<Slot, ApiError>;

	/// Returns information regarding the current epoch.
	fn current_epoch(&self, at: &BlockId) -> Result<Epoch, ApiError>;

	/// Returns information regarding the next epoch (which was already
	/// previously announced).
	fn next_epoch(&self, at: &BlockId) -> Result<Epoch, ApiError>;

	/// Generates a proof of key ownership for the given authority in the
	/// current epoch. An example usage of this module is coupled with the
	/// session historical module to prove that a given authority key is
	/// tied to a given staking identity during a specific session. Proofs
	/// of key ownership are necessary for submitting equivocation reports.
	/// NOTE: even though the API takes a `slot` as parameter the current
	/// implementations ignores this parameter and instead relies on this
	/// method being called at the correct block height, i.e. any point at
	/// which the epoch for the given slot is live on-chain. Future
	/// implementations will instead use indexed data through an offchain
	/// worker, not requiring older states to be available.
	fn generate_key_ownership_proof(
		&self,
		at: &BlockId,
		slot: Slot,
		authority_id: AuthorityId,
	) -> Result<Option<OpaqueKeyOwnershipProof>, ApiError>;

	/// Submits an unsigned extrinsic to report an equivocation. The caller
	/// must provide the equivocation proof and a key ownership proof
	/// (should be obtained using `generate_key_ownership_proof`). The
	/// extrinsic will be unsigned and should only be accepted for local
	/// authorship (not to be broadcast to the network). This method returns
	/// `None` when creation of the extrinsic fails, e.g. if equivocation
	/// reporting is disabled for the given runtime (i.e. this method is
	/// hardcoded to return `None`). Only useful in an offchain context.
	fn submit_report_equivocation_unsigned_extrinsic(
		&self,
		at: &BlockId,
		equivocation_proof: EquivocationProof<Header>,
		key_owner_proof: OpaqueKeyOwnershipProof,
	) -> Result<Option<()>, ApiError>;

	fn authorities(
		&self,
		at: &BlockId,
	) -> std::result::Result<Vec<sp_authority_discovery::AuthorityId>, ApiError>;

	fn api_version_parachain_host(&self, at: &BlockId) -> Result<Option<u32>, ApiError>;
}

impl<T> OverseerRuntimeClient for T
where
	T: ProvideRuntimeApi<Block>,
	T::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
{
	fn validators(&self, at: &BlockId) -> Result<Vec<ValidatorId>, ApiError> {
		self.runtime_api().validators(at)
	}

	fn validator_groups(
		&self,
		at: &BlockId,
	) -> Result<(Vec<Vec<ValidatorIndex>>, GroupRotationInfo<BlockNumber>), ApiError> {
		self.runtime_api().validator_groups(at)
	}

	fn availability_cores(
		&self,
		at: &BlockId,
	) -> Result<Vec<CoreState<Hash, BlockNumber>>, ApiError> {
		self.runtime_api().availability_cores(at)
	}

	fn persisted_validation_data(
		&self,
		at: &BlockId,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<PersistedValidationData<Hash, BlockNumber>>, ApiError> {
		self.runtime_api().persisted_validation_data(at, para_id, assumption)
	}

	fn assumed_validation_data(
		&self,
		at: &BlockId,
		para_id: Id,
		expected_persisted_validation_data_hash: Hash,
	) -> Result<Option<(PersistedValidationData<Hash, BlockNumber>, ValidationCodeHash)>, ApiError>
	{
		self.runtime_api().assumed_validation_data(
			at,
			para_id,
			expected_persisted_validation_data_hash,
		)
	}

	fn check_validation_outputs(
		&self,
		at: &BlockId,
		para_id: Id,
		outputs: CandidateCommitments,
	) -> Result<bool, ApiError> {
		self.runtime_api().check_validation_outputs(at, para_id, outputs)
	}

	fn session_index_for_child(&self, at: &BlockId) -> Result<SessionIndex, ApiError> {
		self.runtime_api().session_index_for_child(at)
	}

	fn validation_code(
		&self,
		at: &BlockId,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<ValidationCode>, ApiError> {
		self.runtime_api().validation_code(at, para_id, assumption)
	}

	fn candidate_pending_availability(
		&self,
		at: &BlockId,
		para_id: Id,
	) -> Result<Option<CommittedCandidateReceipt<Hash>>, ApiError> {
		self.runtime_api().candidate_pending_availability(at, para_id)
	}

	fn candidate_events(&self, at: &BlockId) -> Result<Vec<CandidateEvent<Hash>>, ApiError> {
		self.runtime_api().candidate_events(at)
	}

	fn dmq_contents(
		&self,
		at: &BlockId,
		recipient: Id,
	) -> Result<Vec<InboundDownwardMessage<BlockNumber>>, ApiError> {
		self.runtime_api().dmq_contents(at, recipient)
	}

	fn inbound_hrmp_channels_contents(
		&self,
		at: &BlockId,
		recipient: Id,
	) -> Result<BTreeMap<Id, Vec<InboundHrmpMessage<BlockNumber>>>, ApiError> {
		self.runtime_api().inbound_hrmp_channels_contents(at, recipient)
	}

	fn validation_code_by_hash(
		&self,
		at: &BlockId,
		hash: ValidationCodeHash,
	) -> Result<Option<ValidationCode>, ApiError> {
		self.runtime_api().validation_code_by_hash(at, hash)
	}

	fn on_chain_votes(&self, at: &BlockId) -> Result<Option<ScrapedOnChainVotes<Hash>>, ApiError> {
		self.runtime_api().on_chain_votes(at)
	}

	fn session_info(
		&self,
		at: &BlockId,
		index: SessionIndex,
	) -> Result<Option<SessionInfo>, ApiError> {
		self.runtime_api().session_info(at, index)
	}

	fn submit_pvf_check_statement(
		&self,
		at: &BlockId,
		stmt: PvfCheckStatement,
		signature: ValidatorSignature,
	) -> Result<(), ApiError> {
		self.runtime_api().submit_pvf_check_statement(at, stmt, signature)
	}

	fn pvfs_require_precheck(&self, at: &BlockId) -> Result<Vec<ValidationCodeHash>, ApiError> {
		self.runtime_api().pvfs_require_precheck(at)
	}

	fn validation_code_hash(
		&self,
		at: &BlockId,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<ValidationCodeHash>, ApiError> {
		self.runtime_api().validation_code_hash(at, para_id, assumption)
	}

	fn configuration(&self, at: &BlockId) -> Result<BabeGenesisConfiguration, ApiError> {
		self.runtime_api().configuration(at)
	}

	fn current_epoch_start(&self, at: &BlockId) -> Result<Slot, ApiError> {
		self.runtime_api().current_epoch_start(at)
	}

	fn current_epoch(&self, at: &BlockId) -> Result<Epoch, ApiError> {
		self.runtime_api().current_epoch(at)
	}

	fn next_epoch(&self, at: &BlockId) -> Result<Epoch, ApiError> {
		self.runtime_api().next_epoch(at)
	}

	fn generate_key_ownership_proof(
		&self,
		at: &BlockId,
		slot: Slot,
		authority_id: AuthorityId,
	) -> Result<Option<OpaqueKeyOwnershipProof>, ApiError> {
		self.runtime_api().generate_key_ownership_proof(at, slot, authority_id)
	}

	fn submit_report_equivocation_unsigned_extrinsic(
		&self,
		at: &BlockId,
		equivocation_proof: EquivocationProof<Header>,
		key_owner_proof: OpaqueKeyOwnershipProof,
	) -> Result<Option<()>, ApiError> {
		self.runtime_api().submit_report_equivocation_unsigned_extrinsic(
			at,
			equivocation_proof,
			key_owner_proof,
		)
	}

	fn authorities(
		&self,
		at: &BlockId,
	) -> std::result::Result<Vec<sp_authority_discovery::AuthorityId>, ApiError> {
		self.runtime_api().authorities(at)
	}

	fn api_version_parachain_host(&self, at: &BlockId) -> Result<Option<u32>, ApiError> {
		self.runtime_api().api_version::<dyn ParachainHost<Block>>(at)
	}

	fn session_info_before_version_2(
		&self,
		at: &BlockId,
		index: SessionIndex,
	) -> Result<Option<polkadot_primitives::v1::SessionInfo>, ApiError> {
		self.runtime_api().session_info_before_version_2(at, index)
	}
}
