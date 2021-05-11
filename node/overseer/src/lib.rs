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
//! An `Overseer` is something that allows spawning/stopping and overseing
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

use std::fmt::{self, Debug};
use std::pin::Pin;
use std::sync::{atomic::{self, AtomicUsize}, Arc};
use std::task::Poll;
use std::time::Duration;
use std::collections::{hash_map, HashMap};

use futures::channel::oneshot;
use futures::{
	poll, select,
	future::BoxFuture,
	stream::{self, FuturesUnordered, Fuse},
	Future, FutureExt, StreamExt,
};
use futures_timer::Delay;

use polkadot_primitives::v1::{Block, BlockId,BlockNumber, Hash, ParachainHost};
use client::{BlockImportNotification, BlockchainEvents, FinalityNotification};
use sp_api::{ApiExt, ProvideRuntimeApi};

use polkadot_subsystem::messages::{
	CandidateValidationMessage, CandidateBackingMessage,
	CandidateSelectionMessage, ChainApiMessage, StatementDistributionMessage,
	AvailabilityDistributionMessage, BitfieldSigningMessage, BitfieldDistributionMessage,
	ProvisionerMessage, RuntimeApiMessage,
	AvailabilityStoreMessage, NetworkBridgeMessage, AllMessages, CollationGenerationMessage,
	CollatorProtocolMessage, AvailabilityRecoveryMessage, ApprovalDistributionMessage,
	ApprovalVotingMessage, GossipSupportMessage,
};
pub use polkadot_subsystem::{
	Subsystem, SubsystemContext, SubsystemSender, OverseerSignal, FromOverseer, SubsystemError,
	SubsystemResult, SpawnedSubsystem, ActiveLeavesUpdate, ActivatedLeaf, DummySubsystem, jaeger,
};
use polkadot_node_subsystem_util::{TimeoutExt, metrics::{self, prometheus}, metered, Metronome};
use polkadot_node_primitives::SpawnNamed;
use polkadot_procmacro_overseer_subsystems_gen::AllSubsystemsGen;


mod metrics;

use crate::metrics::*;

// A graceful `Overseer` teardown time delay.
const STOP_DELAY: u64 = 1;
// Target for logs.
const LOG_TARGET: &'static str = "parachain::overseer";

trait MapSubsystem<T> {
	type Output;

	fn map_subsystem(&self, sub: T) -> Self::Output;
}

impl<F, T, U> MapSubsystem<T> for F where F: Fn(T) -> U {
	type Output = U;

	fn map_subsystem(&self, sub: T) -> U {
		(self)(sub)
	}
}

/// Whether a header supports parachain consensus or not.
pub trait HeadSupportsParachains {
	/// Return true if the given header supports parachain consensus. Otherwise, false.
	fn head_supports_parachains(&self, head: &Hash) -> bool;
}

impl<Client> HeadSupportsParachains for Arc<Client> where
	Client: ProvideRuntimeApi<Block>,
	Client::Api: ParachainHost<Block>,
{
	fn head_supports_parachains(&self, head: &Hash) -> bool {
		let id = BlockId::Hash(*head);
		self.runtime_api().has_api::<dyn ParachainHost<Block>>(&id).unwrap_or(false)
	}
}

/// A type of messages that are sent from [`Subsystem`] to [`Overseer`].
///
/// Used to launch jobs.
enum ToOverseer {
	/// A message that wraps something the `Subsystem` is desiring to
	/// spawn on the overseer and a `oneshot::Sender` to signal the result
	/// of the spawn.
	SpawnJob {
		name: &'static str,
		s: BoxFuture<'static, ()>,
	},

	/// Same as `SpawnJob` but for blocking tasks to be executed on a
	/// dedicated thread pool.
	SpawnBlockingJob {
		name: &'static str,
		s: BoxFuture<'static, ()>,
	},
}



impl Debug for ToOverseer {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			ToOverseer::SpawnJob { .. } => write!(f, "OverseerMessage::Spawn(..)"),
			ToOverseer::SpawnBlockingJob { .. } => write!(f, "OverseerMessage::SpawnBlocking(..)")
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
		BlockInfo {
			hash: n.hash,
			parent_hash: n.header.parent_hash,
			number: n.header.number,
		}
	}
}

impl From<FinalityNotification<Block>> for BlockInfo {
	fn from(n: FinalityNotification<Block>) -> Self {
		BlockInfo {
			hash: n.hash,
			parent_hash: n.header.parent_hash,
			number: n.header.number,
		}
	}
}

/// Some event from the outer world.
enum Event {
	BlockImported(BlockInfo),
	BlockFinalized(BlockInfo),
	MsgToSubsystem(AllMessages),
	ExternalRequest(ExternalRequest),
	Stop,
}

/// Some request from outer world.
enum ExternalRequest {
	WaitForActivation {
		hash: Hash,
		response_channel: oneshot::Sender<SubsystemResult<()>>,
	},
}



/// Glues together the [`Overseer`] and `BlockchainEvents` by forwarding
/// import and finality notifications into the [`OverseerHandler`].
///
/// [`Overseer`]: struct.Overseer.html
/// [`OverseerHandler`]: struct.OverseerHandler.html
pub async fn forward_events<P: BlockchainEvents<Block>>(
	client: Arc<P>,
	mut handler: OverseerHandler,
) {
	let mut finality = client.finality_notification_stream();
	let mut imports = client.import_notification_stream();

	loop {
		select! {
			f = finality.next() => {
				match f {
					Some(block) => {
						handler.block_finalized(block.into()).await;
					}
					None => break,
				}
			},
			i = imports.next() => {
				match i {
					Some(block) => {
						handler.block_imported(block.into()).await;
					}
					None => break,
				}
			},
			complete => break,
		}
	}
}


type SubsystemIncomingMessages = SubsystemIncomingMessages<OverseerSignal>;

#[derive(Debug, Default, Clone)]
struct SignalsReceived(Arc<AtomicUsize>);

impl SignalsReceived {
	fn load(&self) -> usize {
		self.0.load(atomic::Ordering::SeqCst)
	}

	fn inc(&self) {
		self.0.fetch_add(1, atomic::Ordering::SeqCst);
	}
}

/// A context type that is given to the [`Subsystem`] upon spawning.
/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
/// or to spawn it's [`SubsystemJob`]s.
///
/// [`Overseer`]: struct.Overseer.html
/// [`Subsystem`]: trait.Subsystem.html
/// [`SubsystemJob`]: trait.SubsystemJob.html
#[derive(Debug)]
pub struct OverseerSubsystemContext<M>{
	signals: metered::MeteredReceiver<OverseerSignal>,
	messages: SubsystemIncomingMessages<M>,
	to_subsystems: OverseerSubsystemSender,
	to_overseer: metered::UnboundedMeteredSender<ToOverseer>,
	signals_received: SignalsReceived,
	pending_incoming: Option<(usize, M)>,
	metrics: Metrics,
}

impl<M> OverseerSubsystemContext<M> {
	/// Create a new `OverseerSubsystemContext`.
	fn new(
		signals: metered::MeteredReceiver<OverseerSignal>,
		messages: SubsystemIncomingMessages<M>,
		to_subsystems: ChannelsOut,
		to_overseer: metered::UnboundedMeteredSender<ToOverseer>,
		metrics: Metrics,
	) -> Self {
		let signals_received = SignalsReceived::default();
		OverseerSubsystemContext {
			signals,
			messages,
			to_subsystems: OverseerSubsystemSender {
				channels: to_subsystems,
				signals_received: signals_received.clone(),
			},
			to_overseer,
			signals_received,
			pending_incoming: None,
			metrics,
		 }
	}

	/// Create a new `OverseerSubsystemContext` with no metering.
	///
	/// Intended for tests.
	#[allow(unused)]
	fn new_unmetered(
		signals: metered::MeteredReceiver<OverseerSignal>,
		messages: SubsystemIncomingMessages<M>,
		to_subsystems: ChannelsOut,
		to_overseer: metered::UnboundedMeteredSender<ToOverseer>,
	) -> Self {
		let metrics = Metrics::default();
		OverseerSubsystemContext::new(signals, messages, to_subsystems, to_overseer, metrics)
	}
}

#[async_trait::async_trait]
impl<M: Send + 'static> SubsystemContext for OverseerSubsystemContext<M> {
	type Message = M;
	type Sender = OverseerSubsystemSender;

	async fn try_recv(&mut self) -> Result<Option<FromOverseer<M>>, ()> {
		match poll!(self.recv()) {
			Poll::Ready(msg) => Ok(Some(msg.map_err(|_| ())?)),
			Poll::Pending => Ok(None),
		}
	}

	async fn recv(&mut self) -> SubsystemResult<FromOverseer<M>> {
		loop {
			// If we have a message pending an overseer signal, we only poll for signals
			// in the meantime.
			if let Some((needs_signals_received, msg)) = self.pending_incoming.take() {
				if needs_signals_received <= self.signals_received.load() {
					return Ok(FromOverseer::Communication { msg });
				} else {
					self.pending_incoming = Some((needs_signals_received, msg));

					// wait for next signal.
					let signal = self.signals.next().await
						.ok_or(SubsystemError::Context(
							"No more messages in rx queue to process"
							.to_owned()
						))?;

					self.signals_received.inc();
					return Ok(FromOverseer::Signal(signal))
				}
			}

			let mut await_message = self.messages.next().fuse();
			let mut await_signal = self.signals.next().fuse();
			let signals_received = self.signals_received.load();
			let pending_incoming = &mut self.pending_incoming;

			// Otherwise, wait for the next signal or incoming message.
			let from_overseer = futures::select_biased! {
				signal = await_signal => {
					let signal = signal
						.ok_or(SubsystemError::Context(
							"No more messages in rx queue to process"
							.to_owned()
						))?;

					FromOverseer::Signal(signal)
				}
				msg = await_message => {
					let packet = msg
						.ok_or(SubsystemError::Context(
							"No more messages in rx queue to process"
							.to_owned()
						))?;

					if packet.signals_received > signals_received {
						// wait until we've received enough signals to return this message.
						*pending_incoming = Some((packet.signals_received, packet.message));
						continue;
					} else {
						// we know enough to return this message.
						FromOverseer::Communication { msg: packet.message}
					}
				}
			};

			if let FromOverseer::Signal(_) = from_overseer {
				self.signals_received.inc();
			}

			return Ok(from_overseer);
		}
	}

	async fn spawn(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
		-> SubsystemResult<()>
	{
		self.to_overseer.send(ToOverseer::SpawnJob {
			name,
			s,
		}).await.map_err(Into::into)
	}

	async fn spawn_blocking(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
		-> SubsystemResult<()>
	{
		self.to_overseer.send(ToOverseer::SpawnBlockingJob {
			name,
			s,
		}).await.map_err(Into::into)
	}

	fn sender(&mut self) -> &mut OverseerSubsystemSender {
		&mut self.to_subsystems
	}
}

/// A subsystem that we oversee.
///
/// Ties together the [`Subsystem`] itself and it's running instance
/// (which may be missing if the [`Subsystem`] is not running at the moment
/// for whatever reason).
///
/// [`Subsystem`]: trait.Subsystem.html
struct OverseenSubsystem<M> {
	instance: Option<SubsystemInstance<M>>,
}

impl<M> OverseenSubsystem<M> {
	/// Send a message to the wrapped subsystem.
	///
	/// If the inner `instance` is `None`, nothing is happening.
	async fn send_message(&mut self, msg: M) -> SubsystemResult<()> {
		const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);

		if let Some(ref mut instance) = self.instance {
			match instance.tx_bounded.send(MessagePacket {
				signals_received: instance.signals_received,
				message: msg.into()
			}).timeout(MESSAGE_TIMEOUT).await
			{
				None => {
					tracing::error!(target: LOG_TARGET, "Subsystem {} appears unresponsive.", instance.name);
					Err(SubsystemError::SubsystemStalled(instance.name))
				}
				Some(res) => res.map_err(Into::into),
			}
		} else {
			Ok(())
		}
	}

	/// Send a signal to the wrapped subsystem.
	///
	/// If the inner `instance` is `None`, nothing is happening.
	async fn send_signal(&mut self, signal: OverseerSignal) -> SubsystemResult<()> {
		const SIGNAL_TIMEOUT: Duration = Duration::from_secs(10);

		if let Some(ref mut instance) = self.instance {
			match instance.tx_signal.send(signal).timeout(SIGNAL_TIMEOUT).await {
				None => {
					tracing::error!(target: LOG_TARGET, "Subsystem {} appears unresponsive.", instance.name);
					Err(SubsystemError::SubsystemStalled(instance.name))
				}
				Some(res) => {
					let res = res.map_err(Into::into);
					if res.is_ok() {
						instance.signals_received += 1;
					}
					res
				}
			}
		} else {
			Ok(())
		}
	}
}

#[derive(Clone)]
struct SubsystemMeters {
	bounded: metered::Meter,
	unbounded: metered::Meter,
	signals: metered::Meter,
}

impl SubsystemMeters {
	fn read(&self) -> SubsystemMeterReadouts {
		SubsystemMeterReadouts {
			bounded: self.bounded.read(),
			unbounded: self.unbounded.read(),
			signals: self.signals.read(),
		}
	}
}

struct SubsystemMeterReadouts {
	bounded: metered::Readout,
	unbounded: metered::Readout,
	signals: metered::Readout,
}

/// The `Overseer` itself.
#[overlord(event=Event, signal=OverseerSignal, gen=AllMessages)]
pub struct Overseer<S, SupportsParachains> {
	#[subsystem(no_dispatch, CandidateValidationMessage)]
	candidate_validation,

	#[subsystem(no_dispatch, CandidateBackingMessage)]
	candidate_backing,

	#[subsystem(no_dispatch, CandidateSelectionMessage)]
	candidate_selection,

	#[subsystem(StatementDistributionMessage)]
	statement_distribution,

	#[subsystem(no_dispatch, AvailabilityDistributionMessage)]
	availability_distribution,

	#[subsystem(no_dispatch, AvailabilityRecoveryMessage)]
	availability_recovery,

	#[subsystem(no_dispatch, BitfieldSigningMessage)]
	bitfield_signing,

	#[subsystem(BitfieldDistributionMessage)]
	bitfield_distribution,

	#[subsystem(no_dispatch, ProvisionerMessage)]
	provisioner,

	#[subsystem(no_dispatch, RuntimeApiMessage)]
	runtime_api,

	#[subsystem(no_dispatch, blocking, AvailabilityStoreMessage)]
	availability_store,

	#[subsystem(no_dispatch, NetworkBridgeMessage)]
	network_bridge,

	#[subsystem(no_dispatch, blocking, ChainApiMessage)]
	chain_api,

	#[subsystem(no_dispatch, CollationGenerationMessage)]
	collation_generation,

	#[subsystem(no_dispatch, CollatorProtocolMessage)]
	collator_protocol,

	#[subsystem(ApprovalDistributionMessage)]
	approval_distribution,

	#[subsystem(no_dispatch, blocking, ApprovalVotingMessage)]
	approval_voting,

	#[subsystem(no_dispatch, GossipSupportMessage)]
	gossip_support,


	/// External listeners waiting for a hash to be in the active-leave set.
	activation_external_listeners: HashMap<Hash, Vec<oneshot::Sender<SubsystemResult<()>>>>,

	/// Stores the [`jaeger::Span`] per active leaf.
	span_per_active_leaf: HashMap<Hash, Arc<jaeger::Span>>,

	/// A set of leaves that `Overseer` starts working with.
	///
	/// Drained at the beginning of `run` and never used again.
	leaves: Vec<(Hash, BlockNumber)>,

	/// The set of the "active leaves".
	active_leaves: HashMap<Hash, BlockNumber>,

	/// An implementation for checking whether a header supports parachain consensus.
	supports_parachains: SupportsParachains,

	/// Various Prometheus metrics.
	metrics: Metrics,
}


impl<S, SupportsParachains> Overseer<S, SupportsParachains>
where
	S: SpawnNamed,
	SupportsParachains: HeadSupportsParachains,
{
	/// Create a new instance of the `Overseer` with a fixed set of [`Subsystem`]s.
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
	///                      `spawn` a `job`
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
	/// For the sake of siroute_messagerseer::{Overseer, HeadSupportsParachains, AllSubsystems};
	/// # use polkadot_primitives::v1::Hash;
	/// # use polkadot_subsystem::{
	/// #     Subsystem, DummySubsystem, SpawnedSubsystem, SubsystemContext,
	/// #     messages::CandidateValidationMessage,
	/// # };
	///
	/// struct ValidationSubsystem;
	///
	/// impl<C> Subsystem<C> for ValidationSubsystem
	///     where C: SubsystemContext<Message=CandidateValidationMessage>
	/// {
	///     fn start(
	///         self,
	///         mut ctx: C,
	///     ) -> SpawnedSubsystem {
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
	/// let all_subsystems = AllSubsystems::<()>::dummy().replace_candidate_validation(ValidationSubsystem);
	/// let (overseer, _handler) = Overseer::new(
	///     vec![],
	///     all_subsystems,
	///     None,
	///     AlwaysSupportsParachains,
	///     spawner,
	/// ).unwrap();
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
	/// # }); }
	/// ```
	pub fn new<CV, CB, CS, SD, AD, AR, BS, BD, P, RA, AS, NB, CA, CG, CP, ApD, ApV, GS>(
		leaves: impl IntoIterator<Item = BlockInfo>,
		all_subsystems: AllSubsystems<CV, CB, CS, SD, AD, AR, BS, BD, P, RA, AS, NB, CA, CG, CP, ApD, ApV, GS>,
		prometheus_registry: Option<&prometheus::Registry>,
		supports_parachains: SupportsParachains,
		mut s: S,
	) -> SubsystemResult<(Self, OverseerHandler)>
	where
		CV: Subsystem<OverseerSubsystemContext<CandidateValidationMessage>> + Send,
		CB: Subsystem<OverseerSubsystemContext<CandidateBackingMessage>> + Send,
		CS: Subsystem<OverseerSubsystemContext<CandidateSelectionMessage>> + Send,
		SD: Subsystem<OverseerSubsystemContext<StatementDistributionMessage>> + Send,
		AD: Subsystem<OverseerSubsystemContext<AvailabilityDistributionMessage>> + Send,
		AR: Subsystem<OverseerSubsystemContext<AvailabilityRecoveryMessage>> + Send,
		BS: Subsystem<OverseerSubsystemContext<BitfieldSigningMessage>> + Send,
		BD: Subsystem<OverseerSubsystemContext<BitfieldDistributionMessage>> + Send,
		P: Subsystem<OverseerSubsystemContext<ProvisionerMessage>> + Send,
		RA: Subsystem<OverseerSubsystemContext<RuntimeApiMessage>> + Send,
		AS: Subsystem<OverseerSubsystemContext<AvailabilityStoreMessage>> + Send,
		NB: Subsystem<OverseerSubsystemContext<NetworkBridgeMessage>> + Send,
		CA: Subsystem<OverseerSubsystemContext<ChainApiMessage>> + Send,
		CG: Subsystem<OverseerSubsystemContext<CollationGenerationMessage>> + Send,
		CP: Subsystem<OverseerSubsystemContext<CollatorProtocolMessage>> + Send,
		ApD: Subsystem<OverseerSubsystemContext<ApprovalDistributionMessage>> + Send,
		ApV: Subsystem<OverseerSubsystemContext<ApprovalVotingMessage>> + Send,
		GS: Subsystem<OverseerSubsystemContext<GossipSupportMessage>> + Send,
	{

		let (overseer, handler) = Overseer::builder()
			.metrics(<Metrics as metrics::Metrics>::register(prometheus_registry)?)
			.active_leaves(leaves
				.into_iter()
				.map(|BlockInfo { hash, parent_hash: _, number }| (hash, number))
				.collect())
			.supports_parachains(supports_parachains),
			.build(s);
		{
			struct ExtractNameAndMeters;
			impl<'a, T: 'a> MapSubsystem<&'a OverseenSubsystem<T>> for ExtractNameAndMeters {
				type Output = (&'static str, SubsystemMeters);

				fn map_subsystem(&self, subsystem: &'a OverseenSubsystem<T>) -> Self::Output {
					let instance = subsystem.instance.as_ref()
						.expect("Extraction is done directly after spawning when subsystems\
						have not concluded; qed");

					(
						instance.name,
						instance.meters.clone(),
					)
				}
			}

			let subsystem_meters = overseer.subsystems().map_subsystems(ExtractNameAndMeters);
			let metronome_metrics = metrics.clone();
			let metronome = Metronome::new(std::time::Duration::from_millis(950))
				.for_each(move |_| {
					let subsystem_meters = subsystem_meters.as_ref()
						.map_subsystems(|&(name, ref meters): &(_, SubsystemMeters)| (name, meters.read()));

					// We combine the amount of messages from subsystems to the overseer
					// as well as the amount of messages from external sources to the overseer
					// into one to_overseer value.
					metronome_metrics.channel_fill_level_snapshot(subsystem_meters);

					async move {
						()
					}
				});
			s.spawn("metrics_metronome", Box::pin(metronome));
		}


		Ok((this, handler))
	}

	/// Run the `Overseer`.
	#[tracing::instrument(skip(self), fields(subsystem = LOG_TARGET))]
	pub async fn run(mut self) -> SubsystemResult<()> {
		let mut update = ActiveLeavesUpdate::default();

		for (hash, number) in std::mem::take(&mut self.leaves) {
			let _ = self.active_leaves.insert(hash, number);
			if let Some(span) = self.on_head_activated(&hash, None) {
				update.activated.push(ActivatedLeaf {
					hash,
					number,
					span,
				});
			}
		}

		if !update.is_empty() {
			self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await?;
		}

		loop {
			select! {
				msg = self.events_rx.next().fuse() => {
					let msg = if let Some(msg) = msg {
						msg
					} else {
						continue
					};

					match msg {
						Event::MsgToSubsystem(msg) => {
							self.route_message(msg.into()).await?;
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
				msg = self.to_overseer_rx.next() => {
					let msg = match msg {
						Some(m) => m,
						None => {
							// This is a fused stream so we will shut down after receiving all
							// shutdown notifications.
							continue
						}
					};

					match msg {
						ToOverseer::SpawnJob { name, s } => {
							self.spawn_job(name, s);
						}
						ToOverseer::SpawnBlockingJob { name, s } => {
							self.spawn_blocking_job(name, s);
						}
					}
				},
				res = self.running_subsystems.next().fuse() => {
					let finished = if let Some(finished) = res {
						finished
					} else {
						continue
					};

					tracing::error!(target: LOG_TARGET, subsystem = ?finished, "subsystem finished unexpectedly");
					self.stop().await;
					return finished;
				},
			}
		}
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	async fn block_imported(&mut self, block: BlockInfo) -> SubsystemResult<()> {
		match self.active_leaves.entry(block.hash) {
			hash_map::Entry::Vacant(entry) => entry.insert(block.number),
			hash_map::Entry::Occupied(entry) => {
				debug_assert_eq!(*entry.get(), block.number);
				return Ok(());
			}
		};

		let mut update = match self.on_head_activated(&block.hash, Some(block.parent_hash)) {
			Some(span) => ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: block.hash,
				number: block.number,
				span
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
			self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await
		} else {
			Ok(())
		}
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	async fn block_finalized(&mut self, block: BlockInfo) -> SubsystemResult<()> {
		let mut update = ActiveLeavesUpdate::default();

		self.active_leaves.retain(|h, n| {
			if *n <= block.number {
				update.deactivated.push(*h);
				false
			} else {
				true
			}
		});

		for deactivated in &update.deactivated {
			self.on_head_deactivated(deactivated)
		}

		self.broadcast_signal(OverseerSignal::BlockFinalized(block.hash, block.number)).await?;

		// If there are no leaves being deactivated, we don't need to send an update.
		//
		// Our peers will be informed about our finalized block the next time we activating/deactivating some leaf.
		if !update.is_empty() {
			self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await?;
		}

		Ok(())
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	async fn broadcast_signal(&mut self, signal: OverseerSignal) -> SubsystemResult<()> {
		self.subsystems.candidate_validation.send_signal(signal.clone()).await?;
		self.subsystems.candidate_backing.send_signal(signal.clone()).await?;
		self.subsystems.candidate_selection.send_signal(signal.clone()).await?;
		self.subsystems.statement_distribution.send_signal(signal.clone()).await?;
		self.subsystems.availability_distribution.send_signal(signal.clone()).await?;
		self.subsystems.availability_recovery.send_signal(signal.clone()).await?;
		self.subsystems.bitfield_signing.send_signal(signal.clone()).await?;
		self.subsystems.bitfield_distribution.send_signal(signal.clone()).await?;
		self.subsystems.provisioner.send_signal(signal.clone()).await?;
		self.subsystems.runtime_api.send_signal(signal.clone()).await?;
		self.subsystems.availability_store.send_signal(signal.clone()).await?;
		self.subsystems.network_bridge.send_signal(signal.clone()).await?;
		self.subsystems.chain_api.send_signal(signal.clone()).await?;
		self.subsystems.collator_protocol.send_signal(signal.clone()).await?;
		self.subsystems.collation_generation.send_signal(signal.clone()).await?;
		self.subsystems.approval_distribution.send_signal(signal.clone()).await?;
		self.subsystems.approval_voting.send_signal(signal.clone()).await?;
		self.subsystems.gossip_support.send_signal(signal).await?;

		Ok(())
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	async fn route_message(&mut self, msg: AllMessages) -> SubsystemResult<()> {
		self.metrics.on_message_relayed();
		match msg {
			AllMessages::CandidateValidation(msg) => {
				self.subsystems.candidate_validation.send_message(msg).await?;
			},
			AllMessages::CandidateBacking(msg) => {
				self.subsystems.candidate_backing.send_message(msg).await?;
			},
			AllMessages::CandidateSelection(msg) => {
				self.subsystems.candidate_selection.send_message(msg).await?;
			},
			AllMessages::StatementDistribution(msg) => {
				self.subsystems.statement_distribution.send_message(msg).await?;
			},
			AllMessages::AvailabilityDistribution(msg) => {
				self.subsystems.availability_distribution.send_message(msg).await?;
			},
			AllMessages::AvailabilityRecovery(msg) => {
				self.subsystems.availability_recovery.send_message(msg).await?;
			},
			AllMessages::BitfieldDistribution(msg) => {
				self.subsystems.bitfield_distribution.send_message(msg).await?;
			},
			AllMessages::BitfieldSigning(msg) => {
				self.subsystems.bitfield_signing.send_message(msg).await?;
			},
			AllMessages::Provisioner(msg) => {
				self.subsystems.provisioner.send_message(msg).await?;
			},
			AllMessages::RuntimeApi(msg) => {
				self.subsystems.runtime_api.send_message(msg).await?;
			},
			AllMessages::AvailabilityStore(msg) => {
				self.subsystems.availability_store.send_message(msg).await?;
			},
			AllMessages::NetworkBridge(msg) => {
				self.subsystems.network_bridge.send_message(msg).await?;
			},
			AllMessages::ChainApi(msg) => {
				self.subsystems.chain_api.send_message(msg).await?;
			},
			AllMessages::CollationGeneration(msg) => {
				self.subsystems.collation_generation.send_message(msg).await?;
			},
			AllMessages::CollatorProtocol(msg) => {
				self.subsystems.collator_protocol.send_message(msg).await?;
			},
			AllMessages::ApprovalDistribution(msg) => {
				self.subsystems.approval_distribution.send_message(msg).await?;
			},
			AllMessages::ApprovalVoting(msg) => {
				self.subsystems.approval_voting.send_message(msg).await?;
			},
			AllMessages::GossipSupport(msg) => {
				self.subsystems.gossip_support.send_message(msg).await?;
			},
		}

		Ok(())
	}

	/// Handles a header activation. If the header's state doesn't support the parachains API,
	/// this returns `None`.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn on_head_activated(&mut self, hash: &Hash, parent_hash: Option<Hash>)
		-> Option<Arc<jaeger::Span>>
	{
		if !self.supports_parachains.head_supports_parachains(hash) {
			return None;
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
		Some(span)
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn on_head_deactivated(&mut self, hash: &Hash) {
		self.metrics.on_head_deactivated();
		self.activation_external_listeners.remove(hash);
		self.span_per_active_leaf.remove(hash);
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn clean_up_external_listeners(&mut self) {
		self.activation_external_listeners.retain(|_, v| {
			// remove dead listeners
			v.retain(|c| !c.is_canceled());
			!v.is_empty()
		})
	}

	#[tracing::instrument(level = "trace", skip(self, request), fields(subsystem = LOG_TARGET))]
	fn handle_external_request(&mut self, request: ExternalRequest) {
		match request {
			ExternalRequest::WaitForActivation { hash, response_channel } => {
				if self.active_leaves.get(&hash).is_some() {
					// it's fine if the listener is no longer interested
					let _ = response_channel.send(Ok(()));
				} else {
					self.activation_external_listeners.entry(hash).or_default().push(response_channel);
				}
			}
		}
	}

	fn spawn_job(&mut self, name: &'static str, j: BoxFuture<'static, ()>) {
		self.s.spawn(name, j);
	}

	fn spawn_blocking_job(&mut self, name: &'static str, j: BoxFuture<'static, ()>) {
		self.s.spawn_blocking(name, j);
	}
}


#[cfg(test)]
mod tests;
