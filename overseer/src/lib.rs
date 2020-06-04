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
//! [implementors-guide](https://github.com/paritytech/polkadot/blob/master/roadmap/implementors-guide/guide.md).
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
use std::task::Poll;
use std::time::Duration;
use std::collections::HashSet;

use futures::channel::{mpsc, oneshot};
use futures::{
	pending, poll, select,
	future::{BoxFuture, RemoteHandle},
	stream::FuturesUnordered,
	task::{Spawn, SpawnError, SpawnExt},
	Future, FutureExt, SinkExt, StreamExt,
};
use futures_timer::Delay;
use streamunordered::{StreamYield, StreamUnordered};

use polkadot_primitives::{Block, BlockNumber, Hash};
use client::{BlockImportNotification, BlockchainEvents, FinalityNotification};

/// An error type that describes faults that may happen
///
/// These are:
///   * Channels being closed
///   * Subsystems dying when they are not expected to
///   * Subsystems not dying when they are told to die
///   * etc.
#[derive(Debug)]
pub struct SubsystemError;

impl From<mpsc::SendError> for SubsystemError {
	fn from(_: mpsc::SendError) -> Self {
		Self
	}
}

impl From<oneshot::Canceled> for SubsystemError {
	fn from(_: oneshot::Canceled) -> Self {
		Self
	}
}

impl From<SpawnError> for SubsystemError {
    fn from(_: SpawnError) -> Self {
		Self
    }
}

/// A `Result` type that wraps [`SubsystemError`].
///
/// [`SubsystemError`]: struct.SubsystemError.html
pub type SubsystemResult<T> = Result<T, SubsystemError>;

/// An asynchronous subsystem task that runs inside and being overseen by the [`Overseer`].
///
/// In essence it's just a newtype wrapping a `BoxFuture`.
///
/// [`Overseer`]: struct.Overseer.html
pub struct SpawnedSubsystem(pub BoxFuture<'static, ()>);

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
		s: BoxFuture<'static, ()>,
		res: oneshot::Sender<SubsystemResult<()>>,
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

/// Some message that is sent from one of the `Subsystem`s to the outside world.
pub enum OutboundMessage {
	SubsystemMessage {
		msg: AllMessages,
	}
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
	client: P,
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
struct SubsystemInstance<M: Debug> {
	tx: mpsc::Sender<FromOverseer<M>>,
}

/// A context type that is given to the [`Subsystem`] upon spawning.
/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
/// or to spawn it's [`SubsystemJob`]s.
///
/// [`Overseer`]: struct.Overseer.html
/// [`Subsystem`]: trait.Subsystem.html
/// [`SubsystemJob`]: trait.SubsystemJob.html
pub struct SubsystemContext<M: Debug>{
	rx: mpsc::Receiver<FromOverseer<M>>,
	tx: mpsc::Sender<ToOverseer>,
}

/// A signal used by [`Overseer`] to communicate with the [`Subsystem`]s.
///
/// [`Overseer`]: struct.Overseer.html
/// [`Subsystem`]: trait.Subsystem.html
#[derive(PartialEq, Clone, Debug)]
pub enum OverseerSignal {
	/// `Subsystem` should start working.
	StartWork(Hash),
	/// `Subsystem` should stop working.
	StopWork(Hash),
	/// Conclude the work of the `Overseer` and all `Subsystem`s.
	Conclude,
}

#[derive(Debug)]
/// A message type used by the Validation [`Subsystem`].
///
/// [`Subsystem`]: trait.Subsystem.html
pub enum ValidationSubsystemMessage {
	ValidityAttestation,
}

#[derive(Debug)]
/// A message type used by the CandidateBacking [`Subsystem`].
///
/// [`Subsystem`]: trait.Subsystem.html
pub enum CandidateBackingSubsystemMessage {
	RegisterBackingWatcher,
	Second,
}

/// A message type tying together all message types that are used across [`Subsystem`]s.
///
/// [`Subsystem`]: trait.Subsystem.html
#[derive(Debug)]
pub enum AllMessages {
	Validation(ValidationSubsystemMessage),
	CandidateBacking(CandidateBackingSubsystemMessage),
}

/// A message type that a [`Subsystem`] receives from the [`Overseer`].
/// It wraps siglans from the [`Overseer`] and messages that are circulating
/// between subsystems.
///
/// It is generic over over the message type `M` that a particular `Subsystem` may use.
///
/// [`Overseer`]: struct.Overseer.html
/// [`Subsystem`]: trait.Subsystem.html
#[derive(Debug)]
pub enum FromOverseer<M: Debug> {
	/// Signal from the `Overseer`.
	Signal(OverseerSignal),

	/// Some other `Subsystem`'s message.
	Communication {
		msg: M,
	},
}

impl<M: Debug> SubsystemContext<M> {
	/// Try to asyncronously receive a message.
	///
	/// This has to be used with caution, if you loop over this without
	/// using `pending!()` macro you will end up with a busy loop!
	pub async fn try_recv(&mut self) -> Result<Option<FromOverseer<M>>, ()> {
		match poll!(self.rx.next()) {
			Poll::Ready(Some(msg)) => Ok(Some(msg)),
			Poll::Ready(None) => Err(()),
			Poll::Pending => Ok(None),
		}
	}

	/// Receive a message.
	pub async fn recv(&mut self) -> SubsystemResult<FromOverseer<M>> {
		self.rx.next().await.ok_or(SubsystemError)
	}

	/// Spawn a child task on the executor.
	pub async fn spawn(&mut self, s: Pin<Box<dyn Future<Output = ()> + Send>>) -> SubsystemResult<()> {
		let (tx, rx) = oneshot::channel();
		self.tx.send(ToOverseer::SpawnJob {
			s,
			res: tx,
		}).await?;

		rx.await?
	}

	/// Send a direct message to some other `Subsystem`, routed based on message type.
	pub async fn send_msg(&mut self, msg: AllMessages) -> SubsystemResult<()> {
		self.tx.send(ToOverseer::SubsystemMessage(msg)).await?;

		Ok(())
	}

	fn new(rx: mpsc::Receiver<FromOverseer<M>>, tx: mpsc::Sender<ToOverseer>) -> Self {
		Self {
			rx,
			tx,
		}
	}
}

/// A trait that describes the [`Subsystem`]s that can run on the [`Overseer`].
///
/// It is generic over the message type circulating in the system.
/// The idea that we want some type contaning persistent state that
/// can spawn actually running subsystems when asked to.
///
/// [`Overseer`]: struct.Overseer.html
/// [`Subsystem`]: trait.Subsystem.html
pub trait Subsystem<M: Debug> {
	/// Start this `Subsystem` and return `SpawnedSubsystem`.
	fn start(&mut self, ctx: SubsystemContext<M>) -> SpawnedSubsystem;
}

/// A subsystem that we oversee.
///
/// Ties together the [`Subsystem`] itself and it's running instance
/// (which may be missing if the [`Subsystem`] is not running at the moment
/// for whatever reason).
///
/// [`Subsystem`]: trait.Subsystem.html
#[allow(dead_code)]
struct OverseenSubsystem<M: Debug> {
	subsystem: Box<dyn Subsystem<M> + Send>,
	instance: Option<SubsystemInstance<M>>,
}

/// The `Overseer` itself.
pub struct Overseer<S: Spawn> {
	/// A validation subsystem
	validation_subsystem: OverseenSubsystem<ValidationSubsystemMessage>,

	/// A candidate backing subsystem
	candidate_backing_subsystem: OverseenSubsystem<CandidateBackingSubsystemMessage>,

	/// Spawner to spawn tasks to.
	s: S,

	/// Here we keep handles to spawned subsystems to be notified when they terminate.
	running_subsystems: FuturesUnordered<RemoteHandle<()>>,

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

impl<S> Overseer<S>
where
	S: Spawn,
{
	/// Create a new intance of the `Overseer` with a fixed set of [`Subsystem`]s.
	///
	/// Each [`Subsystem`] is passed to this function as an explicit parameter
	/// and is supposed to implement some interface that is generic over message type
	/// that is specific to this [`Subsystem`]. At the moment there are only two
	/// subsystems:
	///   * Validation
	///   * CandidateBacking
	///
	/// As any entity that satisfies the interface may act as a [`Subsystem`] this allows
	/// mocking in the test code:
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
	/// Here, we create two mock subsystems and start the `Overseer` with them. For the sake
	/// of simplicity the termination of the example is done with a timeout.
	/// ```
	/// # use std::time::Duration;
	/// # use futures::{executor, pin_mut, select, FutureExt};
	/// # use futures_timer::Delay;
	/// # use overseer::{
	/// #     Overseer, Subsystem, SpawnedSubsystem, SubsystemContext,
	/// #     ValidationSubsystemMessage, CandidateBackingSubsystemMessage,
	/// # };
	///
	/// struct ValidationSubsystem;
	/// impl Subsystem<ValidationSubsystemMessage> for ValidationSubsystem {
	///     fn start(
	///         &mut self,
	///         mut ctx: SubsystemContext<ValidationSubsystemMessage>,
	///     ) -> SpawnedSubsystem {
	///         SpawnedSubsystem(Box::pin(async move {
	///             loop {
	///                 Delay::new(Duration::from_secs(1)).await;
	///             }
	///         }))
	///     }
	/// }
	///
	/// struct CandidateBackingSubsystem;
	/// impl Subsystem<CandidateBackingSubsystemMessage> for CandidateBackingSubsystem {
	///     fn start(
	///         &mut self,
	///         mut ctx: SubsystemContext<CandidateBackingSubsystemMessage>,
	///     ) -> SpawnedSubsystem {
	///         SpawnedSubsystem(Box::pin(async move {
	///             loop {
	///                 Delay::new(Duration::from_secs(1)).await;
	///             }
	///         }))
	///     }
	/// }
	///
	/// # fn main() { executor::block_on(async move {
	/// let spawner = executor::ThreadPool::new().unwrap();
	/// let (overseer, _handler) = Overseer::new(
	///     vec![],
	///     Box::new(ValidationSubsystem),
	///     Box::new(CandidateBackingSubsystem),
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
	pub fn new(
		leaves: impl IntoIterator<Item = BlockInfo>,
		validation: Box<dyn Subsystem<ValidationSubsystemMessage> + Send>,
		candidate_backing: Box<dyn Subsystem<CandidateBackingSubsystemMessage> + Send>,
		mut s: S,
	) -> SubsystemResult<(Self, OverseerHandler)> {
		let (events_tx, events_rx) = mpsc::channel(CHANNEL_CAPACITY);

		let handler = OverseerHandler {
			events_tx: events_tx.clone(),
		};

		let mut running_subsystems_rx = StreamUnordered::new();
		let mut running_subsystems = FuturesUnordered::new();

		let validation_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			validation,
		)?;

		let candidate_backing_subsystem = spawn(
			&mut s,
			&mut running_subsystems,
			&mut running_subsystems_rx,
			candidate_backing,
		)?;

		let active_leaves = HashSet::new();

		let leaves = leaves
			.into_iter()
			.map(|BlockInfo { hash, parent_hash: _, number }| (hash, number))
			.collect();

		let this = Self {
			validation_subsystem,
			candidate_backing_subsystem,
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
		if let Some(ref mut s) = self.validation_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		}

		if let Some(ref mut s) = self.candidate_backing_subsystem.instance {
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

		for leaf in leaves.into_iter() {
			self.broadcast_signal(OverseerSignal::StartWork(leaf.0)).await?;
			self.active_leaves.insert(leaf);
		}

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
					ToOverseer::SpawnJob { s, res } => {
						let s = self.spawn_job(s);

						let _ = res.send(s);
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
		if let Some(parent) = self.active_leaves.take(&(block.parent_hash, block.number - 1)) {
			self.broadcast_signal(OverseerSignal::StopWork(parent.0)).await?;
		}

		if !self.active_leaves.contains(&(block.hash, block.number)) {
			self.broadcast_signal(OverseerSignal::StartWork(block.hash)).await?;
			self.active_leaves.insert((block.hash, block.number));
		}

		Ok(())
	}

	async fn block_finalized(&mut self, block: BlockInfo) -> SubsystemResult<()> {
		let mut stop_these = Vec::new();

		self.active_leaves.retain(|(h, n)| {
			if *n <= block.number {
				stop_these.push(*h);
				false
			} else {
				true
			}
		});

		for hash in stop_these.into_iter() {
			self.broadcast_signal(OverseerSignal::StopWork(hash)).await?
		}

		Ok(())
	}

	async fn broadcast_signal(&mut self, signal: OverseerSignal) -> SubsystemResult<()> {
		if let Some(ref mut s) = self.validation_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal.clone())).await?;
		}

		if let Some(ref mut s) = self.candidate_backing_subsystem.instance {
			s.tx.send(FromOverseer::Signal(signal)).await?;
		}

		Ok(())
	}

	async fn route_message(&mut self, msg: AllMessages) {
		match msg {
			AllMessages::Validation(msg) => {
				if let Some(ref mut s) = self.validation_subsystem.instance {
					let _= s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
			AllMessages::CandidateBacking(msg) => {
				if let Some(ref mut s) = self.candidate_backing_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
		}
	}

	fn spawn_job(&mut self, j: BoxFuture<'static, ()>) -> SubsystemResult<()> {
		self.s.spawn(j).map_err(|_| SubsystemError)
	}
}

fn spawn<S: Spawn, M: Debug>(
	spawner: &mut S,
	futures: &mut FuturesUnordered<RemoteHandle<()>>,
	streams: &mut StreamUnordered<mpsc::Receiver<ToOverseer>>,
	mut s: Box<dyn Subsystem<M> + Send>,
) -> SubsystemResult<OverseenSubsystem<M>> {
	let (to_tx, to_rx) = mpsc::channel(CHANNEL_CAPACITY);
	let (from_tx, from_rx) = mpsc::channel(CHANNEL_CAPACITY);
	let ctx = SubsystemContext::new(to_rx, from_tx);
	let f = s.start(ctx);

	let handle = spawner.spawn_with_handle(f.0)?;

	streams.push(from_rx);
	futures.push(handle);

	let instance = Some(SubsystemInstance {
		tx: to_tx,
	});

	Ok(OverseenSubsystem {
		subsystem: s,
		instance,
	})
}

#[cfg(test)]
mod tests {
	use futures::{executor, pin_mut, select, channel::mpsc, FutureExt};
	use super::*;

	struct TestSubsystem1(mpsc::Sender<usize>);

	impl Subsystem<ValidationSubsystemMessage> for TestSubsystem1 {
		fn start(&mut self, mut ctx: SubsystemContext<ValidationSubsystemMessage>) -> SpawnedSubsystem {
			let mut sender = self.0.clone();
			SpawnedSubsystem(Box::pin(async move {
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
			}))
		}
	}

	struct TestSubsystem2(mpsc::Sender<usize>);

	impl Subsystem<CandidateBackingSubsystemMessage> for TestSubsystem2 {
		fn start(&mut self, mut ctx: SubsystemContext<CandidateBackingSubsystemMessage>) -> SpawnedSubsystem {
			SpawnedSubsystem(Box::pin(async move {
				let mut c: usize = 0;
				loop {
					if c < 10 {
						ctx.send_msg(
							AllMessages::Validation(
								ValidationSubsystemMessage::ValidityAttestation
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
			}))
		}
	}

	struct TestSubsystem4;

	impl Subsystem<CandidateBackingSubsystemMessage> for TestSubsystem4 {
		fn start(&mut self, mut _ctx: SubsystemContext<CandidateBackingSubsystemMessage>) -> SpawnedSubsystem {
			SpawnedSubsystem(Box::pin(async move {
				// Do nothing and exit.
			}))
		}
	}

	// Checks that a minimal configuration of two jobs can run and exchange messages.
	#[test]
	fn overseer_works() {
		let spawner = executor::ThreadPool::new().unwrap();

		executor::block_on(async move {
			let (s1_tx, mut s1_rx) = mpsc::channel(64);
			let (s2_tx, mut s2_rx) = mpsc::channel(64);

			let (overseer, mut handler) = Overseer::new(
				vec![],
				Box::new(TestSubsystem1(s1_tx)),
				Box::new(TestSubsystem2(s2_tx)),
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
	fn overseer_panics_on_sybsystem_exit() {
		let spawner = executor::ThreadPool::new().unwrap();

		executor::block_on(async move {
			let (s1_tx, _) = mpsc::channel(64);
			let (overseer, _handle) = Overseer::new(
				vec![],
				Box::new(TestSubsystem1(s1_tx)),
				Box::new(TestSubsystem4),
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

	impl Subsystem<ValidationSubsystemMessage> for TestSubsystem5 {
		fn start(&mut self, mut ctx: SubsystemContext<ValidationSubsystemMessage>) -> SpawnedSubsystem {
			let mut sender = self.0.clone();

			SpawnedSubsystem(Box::pin(async move {
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
			}))
		}
	}

	struct TestSubsystem6(mpsc::Sender<OverseerSignal>);

	impl Subsystem<CandidateBackingSubsystemMessage> for TestSubsystem6 {
		fn start(&mut self, mut ctx: SubsystemContext<CandidateBackingSubsystemMessage>) -> SpawnedSubsystem {
			let mut sender = self.0.clone();

			SpawnedSubsystem(Box::pin(async move {
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
			}))
		}
	}

	// Tests that starting with a defined set of leaves and receiving
	// notifications on imported blocks triggers expected `StartWork` and `StopWork` heartbeats.
	#[test]
	fn overseer_start_stop_works() {
		let spawner = executor::ThreadPool::new().unwrap();

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

			let (overseer, mut handler) = Overseer::new(
				vec![first_block],
				Box::new(TestSubsystem5(tx_5)),
				Box::new(TestSubsystem6(tx_6)),
				spawner,
			).unwrap();

			let overseer_fut = overseer.run().fuse();
			pin_mut!(overseer_fut);

			let mut ss5_results = Vec::new();
			let mut ss6_results = Vec::new();

			handler.block_imported(second_block).await.unwrap();
			handler.block_imported(third_block).await.unwrap();

			let expected_heartbeats = vec![
				OverseerSignal::StartWork(first_block_hash),
				OverseerSignal::StopWork(first_block_hash),
				OverseerSignal::StartWork(second_block_hash),
				OverseerSignal::StopWork(second_block_hash),
				OverseerSignal::StartWork(third_block_hash),
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
		let spawner = executor::ThreadPool::new().unwrap();

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

			// start with two forks of different height.
			let (overseer, mut handler) = Overseer::new(
				vec![first_block, second_block],
				Box::new(TestSubsystem5(tx_5)),
				Box::new(TestSubsystem6(tx_6)),
				spawner,
			).unwrap();

			let overseer_fut = overseer.run().fuse();
			pin_mut!(overseer_fut);

			let mut ss5_results = Vec::new();
			let mut ss6_results = Vec::new();

			// this should stop work on both forks we started with earlier.
			handler.block_finalized(third_block).await.unwrap();

			let expected_heartbeats = vec![
				OverseerSignal::StartWork(first_block_hash),
				OverseerSignal::StartWork(second_block_hash),
				OverseerSignal::StopWork(first_block_hash),
				OverseerSignal::StopWork(second_block_hash),
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
