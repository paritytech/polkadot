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
//! To spawn something the `Overseer` needs to know what actually needs to be spawned.
//! This is solved by splitting the actual type of the subsystem from the type that
//! is being asyncronously run on the `Overseer`. What we need from the subsystem
//! is the ability to return some `Future` object that the `Overseer` can run and
//! dispatch messages to/from it. Let's take a look at the simplest case with two
//! `Subsystems`:
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

use futures::channel::{mpsc, oneshot};
use futures::{
	pending, poll, select,
	future::{BoxFuture, RemoteHandle},
	stream::FuturesUnordered,
	task::{Spawn, SpawnExt},
	Future, FutureExt, SinkExt, StreamExt,
};
use futures_timer::Delay;

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

/// A `Result` type that wraps [`SubsystemError`].
///
/// [`Overseer`]: struct.SubsystemError.html
pub type SubsystemResult<T> = Result<T, SubsystemError>;

/// An asynchronous subsystem task that runs inside and being overseen by the [`Overseer`].
///
/// In essence it's just a newtype wrapping a `BoxFuture`.
///
/// [`Overseer`]: struct.Overseer.html
pub struct SpawnedSubsystem(pub BoxFuture<'static, ()>);

// A capacity of bounded channels inside the overseer.
const CHANNEL_CAPACITY: usize = 1024;

/// A type of messages that are sent from [`Subsystem`] to [`Overseer`].
///
/// It is generic over some `M` that is intended to be a message type
/// being used by the subsystems running on the `Overseer`. Most likely
/// this type will be one large `enum` covering all possible messages in
/// the system.
/// It is also generic over `I` that is entended to be a type identifying
/// different subsystems, again most likely this is a one large `enum`
/// covering all possible subsystem kinds.
///
/// [`Subsystem`]: trait.Subsystem.html
/// [`Overseer`]: struct.Overseer.html
enum ToOverseer {
	/// This is a message generated by a `Subsystem`.
	/// Wraps the messge itself and has an optional `to` of
	/// someone who can receive this message.
	///
	/// If that `to` is present the message will be targetedly sent to the intended
	/// receiver. The most obvious use case of this is communicating with children.
	SubsystemMessage(AllMessages),

	/// A message that wraps something the `Subsystem` is desiring to
	/// spawn on the overseer and a `oneshot::Sender` to signal the result
	/// of the spawn.
	SpawnJob {
		s: BoxFuture<'static, ()>,
		res: oneshot::Sender<SubsystemResult<()>>,
	},
}

/// Some event from outer world.
enum Event {
	BlockImport,
	BlockFinalized,
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
pub struct OverseerHandler {
	events_tx: mpsc::Sender<Event>,
}

impl OverseerHandler {
	/// Inform the `Overseer` that that some block was imported.
	pub async fn block_imported(&mut self) -> SubsystemResult<()> {
		self.events_tx.send(Event::BlockImport).await?;

		Ok(())
	}

	/// Send some message to one of the `Subsystem`s.
	pub async fn send_msg(&mut self, msg: AllMessages) -> SubsystemResult<()> {
		self.events_tx.send(Event::MsgToSubsystem(msg)).await?;

		Ok(())
	}

	/// Inform the `Overseer` that that some block was finalized.
	pub async fn block_finalized(&mut self) -> SubsystemResult<()> {
		self.events_tx.send(Event::BlockFinalized).await?;

		Ok(())
	}

	/// Tell `Overseer` to shutdown.
	pub async fn stop(&mut self) -> SubsystemResult<()> {
		self.events_tx.send(Event::Stop).await?;

		Ok(())
	}
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
	rx: mpsc::Receiver<ToOverseer>,
	tx: mpsc::Sender<FromOverseer<M>>,
}

/// A context type that is given to the [`Subsystem`] upon spawning.
/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
/// or to spawn it's `SubsystemJob`s.
///
/// [`Overseer`]: struct.Overseer.html
/// [`Subsystem`]: trait.Subsystem.html
/// [`SubsystemJob`]: trait.SubsystemJob.html
pub struct SubsystemContext<M: Debug>{
	rx: mpsc::Receiver<FromOverseer<M>>,
	tx: mpsc::Sender<ToOverseer>,
}

/// A signal used by [`Overseer`] to communicate with the [`Subsystems`].
///
/// [`Overseer`]: struct.Overseer.html
/// [`Subsystem`]: trait.Subsystem.html
#[derive(Debug)]
pub enum OverseerSignal {
	/// `Subsystem` should start working.
	StartWork,
	/// `Subsystem` should stop working.
	StopWork,
}

/// All known subsystems
pub enum AllSubsystems {
	Validation,
	CandidateBacking,
}

#[derive(Debug)]
pub enum ValidationSubsystemMessage {
	ValidityAttestation,
}

#[derive(Debug)]
pub enum CandidateBackingSubsystemMessage {
	RegisterBackingWatcher,
	Second,
}

/// A message type tying together all message types that are used across `Subsystem`s.
#[derive(Debug)]
pub enum AllMessages {
	Validation(ValidationSubsystemMessage),
	CandidateBacking(CandidateBackingSubsystemMessage),
}

/// A message type that a [`Subsystem`] receives from the [`Overseer`].
/// It wraps siglans from the `Oveseer` and messages that are circulating 
/// between subsystems.
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

	/// Spawn a child `Subsystem` on the executor and get it's `I`d upon success.
	pub async fn spawn(&mut self, s: Pin<Box<dyn Future<Output = ()> + Send>>) -> SubsystemResult<()> {
		let (tx, rx) = oneshot::channel();
		self.tx.send(ToOverseer::SpawnJob {
			s,
			res: tx,
		}).await?;

		rx.await?
	}

	/// Send a direct message to some other `Subsystem` you know the `I`d of.
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

	/// Events that are sent to the overseer from the outside world
	events_rx: mpsc::Receiver<Event>,
}

impl<S> Overseer<S>
where
	S: Spawn,
{
	/// Create a new intance of the `Overseer` with some initial set of [`Subsystem`]s.
	///
	///
	/// ```text
	///                  +------------------------------------+
	///                  |            Overseer                |
	///                  +------------------------------------+
	///                    /            |             |      \
	///      ................. subsystems[..] ..............................
	///      . +-----------+    +-----------+   +----------+   +---------+ .
	///      . |           |    |           |   |          |   |         | .
	///      . +-----------+    +-----------+   +----------+   +---------+ .
	///      ...............................................................
	///                              |
	///                        probably `spawn`
	///                        a `job`
	///                              |
	///                              V
	///                         +-----------+
	///                         |           |
	///                         +-----------+
	///
	/// ```
	///
	/// [`Subsystem`]: trait.Subsystem.html
	pub fn new(
		validation: Box<dyn Subsystem<ValidationSubsystemMessage> + Send>,
		candidate_backing: Box<dyn Subsystem<CandidateBackingSubsystemMessage> + Send>,
		mut s: S,
	) -> (Self, OverseerHandler) {
		let (events_tx, events_rx) = mpsc::channel(CHANNEL_CAPACITY);

		let handler = OverseerHandler {
			events_tx: events_tx.clone(),
		};

		let (h_v, validation_subsystem) = Self::spawn(&mut s, validation).unwrap();
		let (h_c, candidate_backing_subsystem) = Self::spawn(&mut s, candidate_backing).unwrap();

		let running_subsystems = vec![h_v, h_c].into_iter().collect();

		let this = Self {
			validation_subsystem,
			candidate_backing_subsystem,
			s,
			running_subsystems,
			events_rx,
		};

		(this, handler)
	}

	// Stop the overseer.
	async fn stop(mut self) {
		if let Some(ref mut s) = self.validation_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::StopWork)).await;
		}

		if let Some(ref mut s) = self.candidate_backing_subsystem.instance {
			let _ = s.tx.send(FromOverseer::Signal(OverseerSignal::StopWork)).await;
		}

		let mut stop_delay = Delay::new(Duration::from_secs(1)).fuse();

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
	pub async fn run(mut self) {
		loop {
			// Upon iteration of the loop we will be collecting all the messages
			// that need dispatching (if any).
			let mut msgs = Vec::new();
			let mut to_spawn = Vec::new();

			while let Poll::Ready(Some(msg)) = poll!(&mut self.events_rx.next()) {
				match msg {
				    Event::MsgToSubsystem(msg) => {
						msgs.push(msg);
					}
				    Event::Stop => return self.stop().await,
					_ => ()
				}
			}

			if let Some(s) = &mut self.candidate_backing_subsystem.instance {
				while let Poll::Ready(Some(msg)) = poll!(&mut s.rx.next()) {
					match msg {
					    ToOverseer::SubsystemMessage(msg) => msgs.push(msg),
					    ToOverseer::SpawnJob { s, res } => to_spawn.push((s, res)),
					}
				}
			}

			if let Some(s) = &mut self.validation_subsystem.instance {
				while let Poll::Ready(Some(msg)) = poll!(&mut s.rx.next()) {
					match msg {
					    ToOverseer::SubsystemMessage(msg) => msgs.push(msg),
					    ToOverseer::SpawnJob { s, res } => to_spawn.push((s, res)),
					}
				}
			}

			for msg in msgs.into_iter() {
				self.route_message(msg).await;
			}

			for msg in to_spawn.into_iter() {
				let s = self.spawn_job(msg.0);

				let _ = msg.1.send(s);
			}

			// Some subsystem exited? It's time to panic.
			if let Poll::Ready(Some(finished)) = poll!(self.running_subsystems.next()) {
				panic!("Subsystem finished unexpectedly {:?}", finished);
			}

			// Looks like nothing is left to be polled, let's take a break.
			pending!();
		}
	}

	async fn route_message(&mut self, msg: AllMessages) {
		match msg {
		    AllMessages::Validation(msg) => {
				if let Some(ref mut s) = self.validation_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
		    AllMessages::CandidateBacking(msg) => {
				if let Some(ref mut s) = self.candidate_backing_subsystem.instance {
					let _ = s.tx.send(FromOverseer::Communication { msg }).await;
				}
			}
		}
	}

	fn spawn<M: Debug>(
		spawner: &mut S,
		mut s: Box<dyn Subsystem<M> + Send>,
	) -> SubsystemResult<(RemoteHandle<()>, OverseenSubsystem<M>)> {
		let (to_tx, to_rx) = mpsc::channel(CHANNEL_CAPACITY);
		let (from_tx, from_rx) = mpsc::channel(CHANNEL_CAPACITY);
		let ctx = SubsystemContext::new(to_rx, from_tx);
		let f = s.start(ctx);

		let handle = spawner.spawn_with_handle(f.0)
			.expect("We need to be able to successfully spawn all subsystems");

		let instance = Some(SubsystemInstance {
			rx: from_rx,
			tx: to_tx,
		});

		Ok(
			(
				handle,
				OverseenSubsystem {
					subsystem: s,
					instance,
				}
			)
		)
	}

	fn spawn_job(&mut self, j: BoxFuture<'static, ()>) -> SubsystemResult<()> {
		self.s.spawn(j).map_err(|_| SubsystemError)
	}
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
						Ok(FromOverseer::Signal(OverseerSignal::StopWork)) => return,
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
						Ok(Some(FromOverseer::Signal(OverseerSignal::StopWork))) => {
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
	// The first job a number of messages that are re-broadcasted to the second job that
	// in it's turn send them to the test code to collect the results and compare them to
	// the expected ones.
	#[test]
	fn overseer_works() {
		let spawner = executor::ThreadPool::new().unwrap();

		executor::block_on(async move {
			let (s1_tx, mut s1_rx) = mpsc::channel(64);
			let (s2_tx, mut s2_rx) = mpsc::channel(64);

			let (overseer, mut handler) = Overseer::new(
				Box::new(TestSubsystem1(s1_tx)),
				Box::new(TestSubsystem2(s2_tx)),
				spawner,
			);
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

	// Spawn a subsystem that immediately exits. This should panic:
	//
	// Subsystems are long-lived worker tasks that are in charge of performing
	// some particular kind of work. All subsystems can communicate with each
	// other via a well-defined protocol.
	#[test]
	#[should_panic]
	fn overseer_panics_on_sybsystem_exit() {
		let spawner = executor::ThreadPool::new().unwrap();

		executor::block_on(async move {
			let (s1_tx, _) = mpsc::channel(64);
			let (overseer, _) = Overseer::new(
				Box::new(TestSubsystem1(s1_tx)),
				Box::new(TestSubsystem4),
				spawner,
			);
			let overseer_fut = overseer.run().fuse();
			pin_mut!(overseer_fut);

			loop {
				select! {
					a = overseer_fut => break,
					complete => break,
				}
			}
		})
	}
}
