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
//! [implementers-guide](https://github.com/paritytech/polkadot/blob/master/roadmap/implementers-guide/guide.md).
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

use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use std::collections::HashSet;

use futures::channel::{mpsc, oneshot};
use futures::{
	pending, poll, select,
	future::BoxFuture,
	stream::{self, FuturesUnordered},
	Future, FutureExt, SinkExt, StreamExt,
};
use futures_timer::Delay;
use streamunordered::{StreamYield, StreamUnordered};

use polkadot_primitives::v1::{Block, BlockNumber, Hash};
use client::{BlockImportNotification, BlockchainEvents, FinalityNotification};

use polkadot_subsystem::messages::{
	CandidateValidationMessage, CandidateBackingMessage,
	CandidateSelectionMessage, StatementDistributionMessage,
	AvailabilityDistributionMessage, BitfieldSigningMessage, BitfieldDistributionMessage,
	ProvisionerMessage, PoVDistributionMessage, RuntimeApiMessage,
	AvailabilityStoreMessage, NetworkBridgeMessage, AllMessages,
};
pub use polkadot_subsystem::{
	Subsystem, SubsystemContext, OverseerSignal, FromOverseer, SubsystemError, SubsystemResult,
	SpawnedSubsystem, ActiveLeavesUpdate,
};
use polkadot_node_primitives::SpawnNamed;


// A capacity of bounded channels inside the overseer.
const CHANNEL_CAPACITY: usize = 1024;
// A graceful `Overseer` teardown time delay.
const STOP_DELAY: u64 = 1;

/// A type of messages that are sent from [`Subsystem`] to [`Overseer`].
///
/// It wraps a system-wide [`AllMessages`] type that represents all possible
/// messages in the system.
///
/// [`AllMessages`]: enum.AllMessages.html
/// [`Subsystem`]: trait.Subsystem.html
/// [`Overseer`]: struct.Overseer.html
enum ToOverseer {
	/// This is a message sent by a `Subsystem`.
	SubsystemMessage(AllMessages),

	/// A message that wraps something the `Subsystem` is desiring to
	/// spawn on the overseer and a `oneshot::Sender` to signal the result
	/// of the spawn.
	SpawnJob {
		name: &'static str,
		s: BoxFuture<'static, ()>,
	},
}

/// An event telling the `Overseer` on the particular block
/// that has been imported or finalized.
///
/// This structure exists solely for the purposes of decoupling
/// `Overseer` code from the client code and the necessity to call
/// `HeaderBackend::block_number_from_id()`.
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

/// Some event from outer world.
enum Event {
	BlockImported(BlockInfo),
	BlockFinalized(BlockInfo),
	MsgToSubsystem(AllMessages),
	Stop,
}

/// A handler used to communicate with the [`Overseer`].
///
/// [`Overseer`]: struct.Overseer.html
#[derive(Clone)]
pub struct OverseerHandler {
	events_tx: mpsc::Sender<Event>,
}

impl OverseerHandler {
	/// Inform the `Overseer` that that some block was imported.
	pub async fn block_imported(&mut self, block: BlockInfo) -> SubsystemResult<()> {
		self.events_tx.send(Event::BlockImported(block)).await?;

		Ok(())
	}

	/// Send some message to one of the `Subsystem`s.
	pub async fn send_msg(&mut self, msg: AllMessages) -> SubsystemResult<()> {
		self.events_tx.send(Event::MsgToSubsystem(msg)).await?;

		Ok(())
	}

	/// Inform the `Overseer` that that some block was finalized.
	pub async fn block_finalized(&mut self, block: BlockInfo) -> SubsystemResult<()> {
		self.events_tx.send(Event::BlockFinalized(block)).await?;

		Ok(())
	}

	/// Tell `Overseer` to shutdown.
	pub async fn stop(&mut self) -> SubsystemResult<()> {
		self.events_tx.send(Event::Stop).await?;

		Ok(())
	}
}

/// Glues together the [`Overseer`] and `BlockchainEvents` by forwarding
/// import and finality notifications into the [`OverseerHandler`].
///
/// [`Overseer`]: struct.Overseer.html
/// [`OverseerHandler`]: struct.OverseerHandler.html
pub async fn forward_events<P: BlockchainEvents<Block>>(
	client: Arc<P>,
	mut handler: OverseerHandler,
) -> SubsystemResult<()> {
	let mut finality = client.finality_notification_stream();
	let mut imports = client.import_notification_stream();

	loop {
		select! {
			f = finality.next() => {
				match f {
					Some(block) => {
						handler.block_finalized(block.into()).await?;
					}
					None => break,
				}
			},
			i = imports.next() => {
				match i {
					Some(block) => {
						handler.block_imported(block.into()).await?;
					}
					None => break,
				}
			},
			complete => break,
		}
	}

	Ok(())
}

impl Debug for ToOverseer {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			ToOverseer::SubsystemMessage(msg) => {
				write!(f, "OverseerMessage::SubsystemMessage({:?})", msg)
			}
			ToOverseer::SpawnJob { .. } => write!(f, "OverseerMessage::Spawn(..)")
		}
	}
}

/// A running instance of some [`Subsystem`].
///
/// [`Subsystem`]: trait.Subsystem.html
struct SubsystemInstance<M> {
	tx: mpsc::Sender<FromOverseer<M>>,
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
	rx: mpsc::Receiver<FromOverseer<M>>,
	tx: mpsc::Sender<ToOverseer>,
}

#[async_trait::async_trait]
impl<M: Send + 'static> SubsystemContext for OverseerSubsystemContext<M> {
	type Message = M;

	async fn try_recv(&mut self) -> Result<Option<FromOverseer<M>>, ()> {
		match poll!(self.rx.next()) {
			Poll::Ready(Some(msg)) => Ok(Some(msg)),
			Poll::Ready(None) => Err(()),
			Poll::Pending => Ok(None),
		}
	}

	async fn recv(&mut self) -> SubsystemResult<FromOverseer<M>> {
		self.rx.next().await.ok_or(SubsystemError)
	}

	async fn spawn(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
		-> SubsystemResult<()>
	{
		self.tx.send(ToOverseer::SpawnJob {
			name,
			s,
		}).await?;

		Ok(())
	}

	async fn send_message(&mut self, msg: AllMessages) -> SubsystemResult<()> {
		self.tx.send(ToOverseer::SubsystemMessage(msg)).await?;

		Ok(())
	}

	async fn send_messages<T>(&mut self, msgs: T) -> SubsystemResult<()>
		where T: IntoIterator<Item = AllMessages> + Send, T::IntoIter: Send
	{
		let mut msgs = stream::iter(msgs.into_iter().map(ToOverseer::SubsystemMessage).map(Ok));
		self.tx.send_all(&mut msgs).await?;

		Ok(())
	}
}

/// A subsystem compatible with the overseer - one which can be run in the context of the
/// overseer.
pub type CompatibleSubsystem<M> = Box<dyn Subsystem<OverseerSubsystemContext<M>> + Send>;

/// A subsystem that we oversee.
///
/// Ties together the [`Subsystem`] itself and it's running instance
/// (which may be missing if the [`Subsystem`] is not running at the moment
/// for whatever reason).
///
/// [`Subsystem`]: trait.Subsystem.html
#[allow(dead_code)]
struct OverseenSubsystem<M> {
	instance: Option<SubsystemInstance<M>>,
}

/// The `Overseer` itself.
pub struct Overseer<S: SpawnNamed> {
	/// A candidate validation subsystem.
	candidate_validation_subsystem: OverseenSubsystem<CandidateValidationMessage>,

	/// A candidate backing subsystem.
	candidate_backing_subsystem: OverseenSubsystem<CandidateBackingMessage>,

	/// A candidate selection subsystem.
	candidate_selection_subsystem: OverseenSubsystem<CandidateSelectionMessage>,

	/// A statement distribution subsystem.
	statement_distribution_subsystem: OverseenSubsystem<StatementDistributionMessage>,

	/// An availability distribution subsystem.
	availability_distribution_subsystem: OverseenSubsystem<AvailabilityDistributionMessage>,

	/// A bitfield signing subsystem.
	bitfield_signing_subsystem: OverseenSubsystem<BitfieldSigningMessage>,

	/// A bitfield distribution subsystem.
	bitfield_distribution_subsystem: OverseenSubsystem<BitfieldDistributionMessage>,

	/// A provisioner subsystem.
	provisioner_subsystem: OverseenSubsystem<ProvisionerMessage>,

	/// A PoV distribution subsystem.
	pov_distribution_subsystem: OverseenSubsystem<PoVDistributionMessage>,

	/// A runtime API subsystem.
	runtime_api_subsystem: OverseenSubsystem<RuntimeApiMessage>,

	/// An availability store subsystem.
	availability_store_subsystem: OverseenSubsystem<AvailabilityStoreMessage>,

	/// A network bridge subsystem.
	network_bridge_subsystem: OverseenSubsystem<NetworkBridgeMessage>,


	/// Spawner to spawn tasks to.
	s: S,

	/// Here we keep handles to spawned subsystems to be notified when they terminate.
	running_subsystems: FuturesUnordered<BoxFuture<'static, ()>>,

	/// Gather running subsystms' outbound streams into one.
	running_subsystems_rx: StreamUnordered<mpsc::Receiver<ToOverseer>>,

	/// Events that are sent to the overseer from the outside world
	events_rx: mpsc::Receiver<Event>,

	/// A set of leaves that `Overseer` starts working with.
	///
	/// Drained at the beginning of `run` and never used again.
	leaves: Vec<(Hash, BlockNumber)>,

	/// The set of the "active leaves".
	active_leaves: HashSet<(Hash, BlockNumber)>,
}

/// This struct is passed as an argument to create a new instance of an [`Overseer`].
///
/// As any entity that satisfies the interface may act as a [`Subsystem`] this allows
/// mocking in the test code:
///
/// Each [`Subsystem`] is supposed to implement some interface that is generic over
/// message type that is specific to this [`Subsystem`]. At the moment not all
/// subsystems are implemented and the rest can be mocked with the [`DummySubsystem`].
///
/// [`Subsystem`]: trait.Subsystem.html
/// [`DummySubsystem`]: struct.DummySubsystem.html
pub struct AllSubsystems<CV, CB, CS, SD, AD, BS, BD, P, PoVD, RA, AS, NB> {
	/// A candidate validation subsystem.
	pub candidate_validation: CV,
	/// A candidate backing subsystem.
	pub candidate_backing: CB,
	/// A candidate selection subsystem.
	pub candidate_selection: CS,
	/// A statement distribution subsystem.
	pub statement_distribution: SD,
	/// An availability distribution subsystem.
	pub availability_distribution: AD,
	/// A bitfield signing subsystem.
	pub bitfield_signing: BS,
	/// A bitfield distribution subsystem.
	pub bitfield_distribution: BD,
	/// A provisioner subsystem.
	pub provisioner: P,
	/// A PoV distribution subsystem.
	pub pov_distribution: PoVD,
	/// A runtime API subsystem.
	pub runtime_api: RA,
	/// An availability store subsystem.
	pub availability_store: AS,
	/// A network bridge subsystem.
	pub network_bridge: NB,
}

impl<S> Overseer<S>
where
	S: SpawnNamed,
{
	/// Create a new intance of the `Overseer` with a fixed set of [`Subsystem`]s.
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
	/// # use polkadot_overseer::{Overseer, AllSubsystems};
	/// # use polkadot_subsystem::{
	/// #     Subsystem, DummySubsystem, SpawnedSubsystem, SubsystemContext,
	/// #     messages::CandidateValidationMessage,
	/// # };
	///
	/// struct ValidationSubsystem;
	///
	/// impl<C> Subsystem<C> for ValidationSubsystem
	/// 	where C: SubsystemContext<Message=CandidateValidationMessage>
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
	/// let spawner = sp_core::testing::TaskExecutor::new();
	/// let all_subsystems = AllSubsystems {
	///     candidate_validation: ValidationSubsystem,
	///     candidate_backing: DummySubsystem,
	///     candidate_selection: DummySubsystem,
	///     statement_distribution: DummySubsystem,
	///     availability_distribution: DummySubsystem,
	///     bitfield_signing: DummySubsystem,
	///     bitfield_distribution: DummySubsystem,
	///     provisioner: DummySubsystem,
	///     pov_distribution: DummySubsystem,
	///     runtime_api: DummySubsystem,
	///     availability_store: DummySubsystem,
	///     network_bridge: DummySubsystem,
	/// };
	/// let (overseer, _handler) = Overseer::new(
	///     vec![],
	///     all_subsystems,
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
	pub fn new<CV, CB, CS, SD, AD, BS, BD, P, PoVD, RA, AS, NB>(
		leaves: impl IntoIterator<Item = BlockInfo>,
		all_subsystems: AllSubsystems<CV, CB, CS, SD, AD, BS, BD, P, PoVD, RA, AS, NB>,
		mut s: S,
	) -> SubsystemResult<(Self, OverseerHandler)>
	where
		CV: Subsystem<OverseerSubsystemContext<CandidateValidationMessage>> + Send,
		CB: Subsystem<OverseerSubsystemContext<CandidateBackingMessage>> + Send,
		CS: Subsystem<OverseerSubsystemContext<CandidateSelectionMessage>> + Send,
		SD: Subsystem<OverseerSubsystemContext<StatementDistributionMessage>> + Send,
		AD: Subsystem<OverseerSubsystemContext<AvailabilityDistributionMessage>> + Send,
		BS: Subsystem<OverseerSubsystemContext<BitfieldSigningMessage>> + Send,
		BD: Subsystem<OverseerSubsystemContext<BitfieldDistributionMessage>> + Send,
		P: Subsystem<OverseerSubsystemContext<ProvisionerMessage>> + Send,
		PoVD: Subsystem<OverseerSubsystemContext<PoVDistributionMessage>> + Send,
		RA: Subsystem<OverseerSubsystemContext<RuntimeApiMessage>> + Send,
		AS: Subsystem<OverseerSubsystemContext<AvailabilityStoreMessage>> + Send,
		NB: Subsystem<OverseerSubsystemContext<NetworkBridgeMessage>> + Send,
	{
		let (events_tx, events_rx) = mpsc::channel(CHANNEL_CAPACITY);

		let handler = OverseerHandler {
			events_tx: events_tx.clone(),
		};

		let mut running_subsystems_rx = StreamUnordered::new();
		let mut running_subsystems = FuturesUnordered::new();

		let candidate_validation_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.candidate_validation,
		)?;

		let candidate_backing_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.candidate_backing,
		)?;

		let candidate_selection_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.candidate_selection,
		)?;

		let statement_distribution_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.statement_distribution,
		)?;

		let availability_distribution_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.availability_distribution,
		)?;

		let bitfield_signing_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.bitfield_signing,
		)?;

		let bitfield_distribution_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.bitfield_distribution,
		)?;

		let provisioner_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.provisioner,
		)?;

		let pov_distribution_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.pov_distribution,
		)?;

		let runtime_api_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.runtime_api,
		)?;

		let availability_store_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.availability_store,
		)?;

		let network_bridge_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			all_subsystems.network_bridge,
		)?;

		let active_leaves = HashSet::new();

		let leaves = leaves
			.into_iter()
			.map(|BlockInfo { hash, parent_hash: _, number }| (hash, number))
			.collect();

		let this = Self {
			candidate_validation_subsystem,
			candidate_backing_subsystem,
			candidate_selection_subsystem,
			statement_distribution_subsystem,
			availability_distribution_subsystem,
			bitfield_signing_subsystem,
			bitfield_distribution_subsystem,
			provisioner_subsystem,
			pov_distribution_subsystem,
			runtime_api_subsystem,
			availability_store_subsystem,
			network_bridge_subsystem,
			s,
			running_subsystems,
			running_subsystems_rx,
			events_rx,
			leaves,
			active_leaves,
		};

		Ok((this, handler))
	}

	// Stop the overseer.
	async fn stop(mut self) {
		if let Some(ref mut s) = self.candidate_validation_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		if let Some(ref mut s) = self.candidate_backing_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		if let Some(ref mut s) = self.candidate_selection_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		if let Some(ref mut s) = self.statement_distribution_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		if let Some(ref mut s) = self.availability_distribution_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		if let Some(ref mut s) = self.bitfield_distribution_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		if let Some(ref mut s) = self.provisioner_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		if let Some(ref mut s) = self.pov_distribution_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		if let Some(ref mut s) = self.runtime_api_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		if let Some(ref mut s) = self.availability_distribution_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		if let Some(ref mut s) = self.network_bridge_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		let mut stop_delay = Delay::new(Duration::from_secs(STOP_DELAY)).fuse();

		loop {
			select! {
				_ = self.running_subsystems.next() => {
					if self.running_subsystems.is_empty() {
						break;
					}
				},
				_ = stop_delay => break,
				complete => break,
			}
		}
	}

	/// Run the `Overseer`.
	pub async fn run(mut self) -> SubsystemResult<()> {
		let leaves = std::mem::take(&mut self.leaves);
		let mut update = ActiveLeavesUpdate::default();

		for leaf in leaves.into_iter() {
			update.activated.push(leaf.0);
			self.active_leaves.insert(leaf);
		}

		self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await?;

		loop {
			while let Poll::Ready(Some(msg)) = poll!(&mut self.events_rx.next()) {
				match msg {
					Event::MsgToSubsystem(msg) => {
						self.route_message(msg).await;
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
				}
			}

			while let Poll::Ready(Some((StreamYield::Item(msg), _))) = poll!(
				&mut self.running_subsystems_rx.next()
			) {
				match msg {
					ToOverseer::SubsystemMessage(msg) => self.route_message(msg).await,
					ToOverseer::SpawnJob { name, s } => {
						self.spawn_job(name, s);
					}
				}
			}

			// Some subsystem exited? It's time to panic.
			if let Poll::Ready(Some(finished)) = poll!(self.running_subsystems.next()) {
				log::error!("Subsystem finished unexpectedly {:?}", finished);
				self.stop().await;
				return Err(SubsystemError);
			}

			// Looks like nothing is left to be polled, let's take a break.
			pending!();
		}
	}

	async fn block_imported(&mut self, block: BlockInfo) -> SubsystemResult<()> {
		let mut update = ActiveLeavesUpdate::default();

		if let Some(parent) = self.active_leaves.take(&(block.parent_hash, block.number - 1)) {
			update.deactivated.push(parent.0);
		}

		if !self.active_leaves.contains(&(block.hash, block.number)) {
			update.activated.push(block.hash);
			self.active_leaves.insert((block.hash, block.number));
		}

		self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await?;

		Ok(())
	}

	async fn block_finalized(&mut self, block: BlockInfo) -> SubsystemResult<()> {
		let mut update = ActiveLeavesUpdate::default();

		self.active_leaves.retain(|(h, n)| {
			if *n <= block.number {
				update.deactivated.push(*h);
				false
			} else {
				true
			}
		});

		self.broadcast_signal(OverseerSignal::ActiveLeaves(update)).await?;

		self.broadcast_signal(OverseerSignal::BlockFinalized(block.hash)).await?;

		Ok(())
	}

	async fn broadcast_signal(&mut self, signal: OverseerSignal) -> SubsystemResult<()> {
		if let Some(ref mut s) = self.candidate_validation_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal.clone())).await?;
		}

		if let Some(ref mut s) = self.candidate_backing_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal.clone())).await?;
		}

		if let Some(ref mut s) = self.candidate_selection_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal.clone())).await?;
		}

		if let Some(ref mut s) = self.statement_distribution_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal.clone())).await?;
		}

		if let Some(ref mut s) = self.availability_distribution_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal.clone())).await?;
		}

		if let Some(ref mut s) = self.bitfield_distribution_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal.clone())).await?;
		}

		if let Some(ref mut s) = self.provisioner_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal.clone())).await?;
		}

		if let Some(ref mut s) = self.pov_distribution_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal.clone())).await?;
		}

		if let Some(ref mut s) = self.runtime_api_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal.clone())).await?;
		}

		if let Some(ref mut s) = self.availability_store_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal.clone())).await?;
		}

		if let Some(ref mut s) = self.network_bridge_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal)).await?;
		}

		Ok(())
	}

	async fn route_message(&mut self, msg: AllMessages) {
		match msg {
			AllMessages::CandidateValidation(msg) => {
				if let Some(ref mut s) = self.candidate_validation_subsystem.instance {
					let _= s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
			AllMessages::CandidateBacking(msg) => {
				if let Some(ref mut s) = self.candidate_backing_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
			AllMessages::CandidateSelection(msg) => {
				if let Some(ref mut s) = self.candidate_selection_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
			AllMessages::StatementDistribution(msg) => {
				if let Some(ref mut s) = self.statement_distribution_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
			AllMessages::AvailabilityDistribution(msg) => {
				if let Some(ref mut s) = self.availability_distribution_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
			AllMessages::BitfieldDistribution(msg) => {
				if let Some(ref mut s) = self.bitfield_distribution_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
			AllMessages::BitfieldSigning(msg) => {
				if let Some(ref mut s) = self.bitfield_signing_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication{ msg }).await;
				}
			}
			AllMessages::Provisioner(msg) => {
				if let Some(ref mut s) = self.provisioner_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
			AllMessages::PoVDistribution(msg) => {
				if let Some(ref mut s) = self.pov_distribution_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
			AllMessages::RuntimeApi(msg) => {
				if let Some(ref mut s) = self.runtime_api_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
			AllMessages::AvailabilityStore(msg) => {
				if let Some(ref mut s) = self.availability_store_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
			AllMessages::NetworkBridge(msg) => {
				if let Some(ref mut s) = self.network_bridge_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
		}
	}

	fn spawn_job(&mut self, name: &'static str, j: BoxFuture<'static, ()>) {
		self.s.spawn(name, j);
	}
}

fn spawn<S: SpawnNamed, M: Send + 'static>(
	spawner: &mut S,
	futures: &mut FuturesUnordered<BoxFuture<'static, ()>>,
	streams: &mut StreamUnordered<mpsc::Receiver<ToOverseer>>,
	s: impl Subsystem<OverseerSubsystemContext<M>>,
) -> SubsystemResult<OverseenSubsystem<M>> {
	let (to_tx, to_rx) = mpsc::channel(CHANNEL_CAPACITY);
	let (from_tx, from_rx) = mpsc::channel(CHANNEL_CAPACITY);
	let ctx = OverseerSubsystemContext { rx: to_rx, tx: from_tx };
	let SpawnedSubsystem { future, name } = s.start(ctx);

	let (tx, rx) = oneshot::channel();

	let fut = Box::pin(async move {
		future.await;
		let _ = tx.send(());
	});

	spawner.spawn(name, fut);

	streams.push(from_rx);
	futures.push(Box::pin(rx.map(|_| ())));

	let instance = Some(SubsystemInstance {
		tx: to_tx,
	});

	Ok(OverseenSubsystem {
		instance,
	})
}


#[cfg(test)]
mod tests {
	use futures::{executor, pin_mut, select, channel::mpsc, FutureExt};

	use polkadot_primitives::v1::{BlockData, PoV};
	use polkadot_subsystem::DummySubsystem;
	use super::*;


	struct TestSubsystem1(mpsc::Sender<usize>);

	impl<C> Subsystem<C> for TestSubsystem1
		where C: SubsystemContext<Message=CandidateValidationMessage>
	{
		fn start(self, mut ctx: C) -> SpawnedSubsystem {
			let mut sender = self.0;
			SpawnedSubsystem {
				name: "test-subsystem-1",
				future: Box::pin(async move {
					let mut i = 0;
					loop {
						match ctx.recv().await {
							Ok(FromOverseer::Communication { .. }) => {
								let _ = sender.send(i).await;
								i += 1;
								continue;
							}
							Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => return,
							Err(_) => return,
							_ => (),
						}
					}
				}),
			}
		}
	}

	struct TestSubsystem2(mpsc::Sender<usize>);

	impl<C> Subsystem<C> for TestSubsystem2
		where C: SubsystemContext<Message=CandidateBackingMessage>
	{
		fn start(self, mut ctx: C) -> SpawnedSubsystem {
			let sender = self.0.clone();
			SpawnedSubsystem {
				name: "test-subsystem-2",
				future: Box::pin(async move {
					let _sender = sender;
					let mut c: usize = 0;
					loop {
						if c < 10 {
							let (tx, _) = oneshot::channel();
							ctx.send_message(
								AllMessages::CandidateValidation(
									CandidateValidationMessage::ValidateFromChainState(
										Default::default(),
										PoV {
											block_data: BlockData(Vec::new()),
										}.into(),
										tx,
									)
								)
							).await.unwrap();
							c += 1;
							continue;
						}
						match ctx.try_recv().await {
							Ok(Some(FromOverseer::Signal(OverseerSignal::Conclude))) => {
								break;
							}
							Ok(Some(_)) => {
								continue;
							}
							Err(_) => return,
							_ => (),
						}
						pending!();
					}
				}),
			}
		}
	}

	struct TestSubsystem4;

	impl<C> Subsystem<C> for TestSubsystem4
		where C: SubsystemContext<Message=CandidateBackingMessage>
	{
		fn start(self, mut _ctx: C) -> SpawnedSubsystem {
			SpawnedSubsystem {
				name: "test-subsystem-4",
				future: Box::pin(async move {
					// Do nothing and exit.
				}),
			}
		}
	}

	// Checks that a minimal configuration of two jobs can run and exchange messages.
	#[test]
	fn overseer_works() {
		let spawner = sp_core::testing::TaskExecutor::new();

		executor::block_on(async move {
			let (s1_tx, mut s1_rx) = mpsc::channel(64);
			let (s2_tx, mut s2_rx) = mpsc::channel(64);

			let all_subsystems = AllSubsystems {
				candidate_validation: TestSubsystem1(s1_tx),
				candidate_backing: TestSubsystem2(s2_tx),
				candidate_selection: DummySubsystem,
				statement_distribution: DummySubsystem,
				availability_distribution: DummySubsystem,
				bitfield_signing: DummySubsystem,
				bitfield_distribution: DummySubsystem,
				provisioner: DummySubsystem,
				pov_distribution: DummySubsystem,
				runtime_api: DummySubsystem,
				availability_store: DummySubsystem,
				network_bridge: DummySubsystem,
			};
			let (overseer, mut handler) = Overseer::new(
				vec![],
				all_subsystems,
				spawner,
			).unwrap();
			let overseer_fut = overseer.run().fuse();

			pin_mut!(overseer_fut);

			let mut s1_results = Vec::new();
			let mut s2_results = Vec::new();

			loop {
				select! {
					a = overseer_fut => break,
					s1_next = s1_rx.next() => {
						match s1_next {
							Some(msg) => {
								s1_results.push(msg);
								if s1_results.len() == 10 {
									handler.stop().await.unwrap();
								}
							}
							None => break,
						}
					},
					s2_next = s2_rx.next() => {
						match s2_next {
							Some(msg) => s2_results.push(s2_next),
							None => break,
						}
					},
					complete => break,
				}
			}

			assert_eq!(s1_results, (0..10).collect::<Vec<_>>());
		});
	}

	// Spawn a subsystem that immediately exits.
	//
	// Should immediately conclude the overseer itself with an error.
	#[test]
	fn overseer_panics_on_subsystem_exit() {
		let spawner = sp_core::testing::TaskExecutor::new();

		executor::block_on(async move {
			let (s1_tx, _) = mpsc::channel(64);
			let all_subsystems = AllSubsystems {
				candidate_validation: TestSubsystem1(s1_tx),
				candidate_backing: TestSubsystem4,
				candidate_selection: DummySubsystem,
				statement_distribution: DummySubsystem,
				availability_distribution: DummySubsystem,
				bitfield_signing: DummySubsystem,
				bitfield_distribution: DummySubsystem,
				provisioner: DummySubsystem,
				pov_distribution: DummySubsystem,
				runtime_api: DummySubsystem,
				availability_store: DummySubsystem,
				network_bridge: DummySubsystem,
			};
			let (overseer, _handle) = Overseer::new(
				vec![],
				all_subsystems,
				spawner,
			).unwrap();
			let overseer_fut = overseer.run().fuse();
			pin_mut!(overseer_fut);

			select! {
				res = overseer_fut => assert!(res.is_err()),
				complete => (),
			}
		})
	}

	struct TestSubsystem5(mpsc::Sender<OverseerSignal>);

	impl<C> Subsystem<C> for TestSubsystem5
		where C: SubsystemContext<Message=CandidateValidationMessage>
	{
		fn start(self, mut ctx: C) -> SpawnedSubsystem {
			let mut sender = self.0.clone();

			SpawnedSubsystem {
				name: "test-subsystem-5",
				future: Box::pin(async move {
					loop {
						match ctx.try_recv().await {
							Ok(Some(FromOverseer::Signal(OverseerSignal::Conclude))) => break,
							Ok(Some(FromOverseer::Signal(s))) => {
								sender.send(s).await.unwrap();
								continue;
							},
							Ok(Some(_)) => continue,
							Err(_) => return,
							_ => (),
						}
						pending!();
					}
				}),
			}
		}
	}

	struct TestSubsystem6(mpsc::Sender<OverseerSignal>);

	impl<C> Subsystem<C> for TestSubsystem6
		where C: SubsystemContext<Message=CandidateBackingMessage>
	{
		fn start(self, mut ctx: C) -> SpawnedSubsystem {
			let mut sender = self.0.clone();

			SpawnedSubsystem {
				name: "test-subsystem-6",
				future: Box::pin(async move {
					loop {
						match ctx.try_recv().await {
							Ok(Some(FromOverseer::Signal(OverseerSignal::Conclude))) => break,
							Ok(Some(FromOverseer::Signal(s))) => {
								sender.send(s).await.unwrap();
								continue;
							},
							Ok(Some(_)) => continue,
							Err(_) => return,
							_ => (),
						}
						pending!();
					}
				}),
			}
		}
	}

	// Tests that starting with a defined set of leaves and receiving
	// notifications on imported blocks triggers expected `StartWork` and `StopWork` heartbeats.
	#[test]
	fn overseer_start_stop_works() {
		let spawner = sp_core::testing::TaskExecutor::new();

		executor::block_on(async move {
			let first_block_hash = [1; 32].into();
			let second_block_hash = [2; 32].into();
			let third_block_hash = [3; 32].into();

			let first_block = BlockInfo {
				hash: first_block_hash,
				parent_hash: [0; 32].into(),
				number: 1,
			};
			let second_block = BlockInfo {
				hash: second_block_hash,
				parent_hash: first_block_hash,
				number: 2,
			};
			let third_block = BlockInfo {
				hash: third_block_hash,
				parent_hash: second_block_hash,
				number: 3,
			};

			let (tx_5, mut rx_5) = mpsc::channel(64);
			let (tx_6, mut rx_6) = mpsc::channel(64);
			let all_subsystems = AllSubsystems {
				candidate_validation: TestSubsystem5(tx_5),
				candidate_backing: TestSubsystem6(tx_6),
				candidate_selection: DummySubsystem,
				statement_distribution: DummySubsystem,
				availability_distribution: DummySubsystem,
				bitfield_signing: DummySubsystem,
				bitfield_distribution: DummySubsystem,
				provisioner: DummySubsystem,
				pov_distribution: DummySubsystem,
				runtime_api: DummySubsystem,
				availability_store: DummySubsystem,
				network_bridge: DummySubsystem,
			};
			let (overseer, mut handler) = Overseer::new(
				vec![first_block],
				all_subsystems,
				spawner,
			).unwrap();

			let overseer_fut = overseer.run().fuse();
			pin_mut!(overseer_fut);

			let mut ss5_results = Vec::new();
			let mut ss6_results = Vec::new();

			handler.block_imported(second_block).await.unwrap();
			handler.block_imported(third_block).await.unwrap();

			let expected_heartbeats = vec![
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(first_block_hash)),
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					activated: [second_block_hash].as_ref().into(),
					deactivated: [first_block_hash].as_ref().into(),
				}),
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					activated: [third_block_hash].as_ref().into(),
					deactivated: [second_block_hash].as_ref().into(),
				}),
			];

			loop {
				select! {
					res = overseer_fut => {
						assert!(res.is_ok());
						break;
					},
					res = rx_5.next() => {
						if let Some(res) = res {
							ss5_results.push(res);
						}
					}
					res = rx_6.next() => {
						if let Some(res) = res {
							ss6_results.push(res);
						}
					}
					complete => break,
				}

				if ss5_results.len() == expected_heartbeats.len() &&
					ss6_results.len() == expected_heartbeats.len() {
						handler.stop().await.unwrap();
				}
			}

			assert_eq!(ss5_results, expected_heartbeats);
			assert_eq!(ss6_results, expected_heartbeats);
		});
	}

	// Tests that starting with a defined set of leaves and receiving
	// notifications on imported blocks triggers expected `StartWork` and `StopWork` heartbeats.
	#[test]
	fn overseer_finalize_works() {
		let spawner = sp_core::testing::TaskExecutor::new();

		executor::block_on(async move {
			let first_block_hash = [1; 32].into();
			let second_block_hash = [2; 32].into();
			let third_block_hash = [3; 32].into();

			let first_block = BlockInfo {
				hash: first_block_hash,
				parent_hash: [0; 32].into(),
				number: 1,
			};
			let second_block = BlockInfo {
				hash: second_block_hash,
				parent_hash: [42; 32].into(),
				number: 2,
			};
			let third_block = BlockInfo {
				hash: third_block_hash,
				parent_hash: second_block_hash,
				number: 3,
			};

			let (tx_5, mut rx_5) = mpsc::channel(64);
			let (tx_6, mut rx_6) = mpsc::channel(64);

			let all_subsystems = AllSubsystems {
				candidate_validation: TestSubsystem5(tx_5),
				candidate_backing: TestSubsystem6(tx_6),
				candidate_selection: DummySubsystem,
				statement_distribution: DummySubsystem,
				availability_distribution: DummySubsystem,
				bitfield_signing: DummySubsystem,
				bitfield_distribution: DummySubsystem,
				provisioner: DummySubsystem,
				pov_distribution: DummySubsystem,
				runtime_api: DummySubsystem,
				availability_store: DummySubsystem,
				network_bridge: DummySubsystem,
			};
			// start with two forks of different height.
			let (overseer, mut handler) = Overseer::new(
				vec![first_block, second_block],
				all_subsystems,
				spawner,
			).unwrap();

			let overseer_fut = overseer.run().fuse();
			pin_mut!(overseer_fut);

			let mut ss5_results = Vec::new();
			let mut ss6_results = Vec::new();

			// this should stop work on both forks we started with earlier.
			handler.block_finalized(third_block).await.unwrap();

			let expected_heartbeats = vec![
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					activated: [first_block_hash, second_block_hash].as_ref().into(),
					..Default::default()
				}),
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					deactivated: [first_block_hash, second_block_hash].as_ref().into(),
					..Default::default()
				}),
				OverseerSignal::BlockFinalized(third_block_hash),
			];

			loop {
				select! {
					res = overseer_fut => {
						assert!(res.is_ok());
						break;
					},
					res = rx_5.next() => {
						if let Some(res) = res {
							ss5_results.push(res);
						}
					}
					res = rx_6.next() => {
						if let Some(res) = res {
							ss6_results.push(res);
						}
					}
					complete => break,
				}

				if ss5_results.len() == expected_heartbeats.len() &&
					ss6_results.len() == expected_heartbeats.len() {
						handler.stop().await.unwrap();
				}
			}

			assert_eq!(ss5_results.len(), expected_heartbeats.len());
			assert_eq!(ss6_results.len(), expected_heartbeats.len());

			// Notifications on finality for multiple blocks at once
			// may be received in different orders.
			for expected in expected_heartbeats {
				assert!(ss5_results.contains(&expected));
				assert!(ss6_results.contains(&expected));
			}
		});
	}
}
