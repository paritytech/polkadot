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
//! To spawn something the `Overseer` needs to know what actually needs to be spawn.
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
//!             .  +-------------------+                +---------------------+  .
//!             .  | SubsystemInstance1|                | SubsystemInstance2  |  .
//!             .  +-------------------+                +---------------------+  .
//!             ..................................................................
//! ```

use std::fmt::Debug;
use std::pin::Pin;
use std::collections::{HashSet, HashMap};
use std::task::Poll;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::channel::{mpsc, oneshot};
use futures::{
	pending, poll,
	task::{Spawn, SpawnExt},
	Future, FutureExt, SinkExt, StreamExt,
};


// TODO: a better way to get unique IDs for jobs
fn get_id() -> usize {
	static COUNTER:AtomicUsize = AtomicUsize::new(1);
	COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// An error type that describes faults that may happen
///
/// These are:
///   * Channels being closed
///   * Subsystems dying when they are not expected to
///   * Subsystems not dying when they are told to die
///   * etc.
// TODO: populate with actual error cases.
#[derive(Debug)]
pub struct SubsystemError;

/// A `Result` type that wraps `SubsystemError` and an empty type on success.
// TODO: Proper success type.
pub type SubsystemResult = Result<(), SubsystemError>;

/// An asynchronous job that runs inside and being overseen by the `Overseer`.
///
/// In essence it's just a newtype wrapping a pinned `Future` dyn trait object.
pub struct SubsystemJob(pub Pin<Box<dyn Future<Output = ()> + Send + 'static>>);

/// A type of messages that are used inside the `Overseer`.
///
/// It is generic over some `T` that is intended to be a message type
/// being used by the subsystems running on the `Overseer`. Most likely
/// this type will be one large `enum` covering all possible messages in
/// the system.
enum OverseerMessage<T: Debug> {
	/// This is a message generated by a `Subsystem`.
	/// Wraps the messge itself and has an optional `to` of
	/// someone who can receive this message.
	///
	/// If that `to` is present the message will be targetedly sent to the intended
	/// receiver. The most obvious use case of this is communicating with children.
	SubsystemMessage {
		to: Option<SubsystemId>,
		msg: T,
	},
	/// A message that wraps something the `Subsystem` is desiring to
	/// spawn on the overseer and a `oneshot::Sender` to signal the result
	/// of the spawn.
	SpawnChild {
		s: Box<dyn Subsystem<T> + Send>,
		res: oneshot::Sender<Result<SubsystemId, SubsystemError>>,
	},
}

impl<T: Debug> Debug for OverseerMessage<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			OverseerMessage::SubsystemMessage { to, msg } => {
				write!(f, "OverseerMessage::SubsystemMessage{{ to: {:?}, msg: {:?} }}", to, msg)
			}
			OverseerMessage::SpawnChild { .. } => write!(f, "OverseerMessage::Spawn(..)")
		}
	}
}

/// A running instance of some `Subsystem`.
struct SubsystemInstance<M: Debug> {
	/// We talk to the `Overseer` over this channel.
	rx: mpsc::Receiver<OverseerMessage<M>>,
	/// The `Overseer` talks to use over this channel.
	tx: mpsc::Sender<M>,
	/// Exit signal.
	_signal: exit_future::Signal,
}

/// An `id` that is given to any `SubsystemInstance` for identification.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SubsystemId(usize);

/// A context type that is given to the `Subsystem` upon spawning
/// that can be used by `Subsystem` to communicate with the outside world.
pub struct SubsystemContext<M: Debug>{
	rx: mpsc::Receiver<M>,
	tx: mpsc::Sender<OverseerMessage<M>>,
}

impl<M: Debug> SubsystemContext<M> {
	/// Try to asyncronously receive a message.
	///
	/// This has to be used with caution, if you loop over this without
	/// using `pending!()` macro you will end up with a busy loop!
	pub async fn try_recv(&mut self) -> Result<Option<M>, ()> {
		match poll!(self.rx.next()) {
			Poll::Ready(Some(msg)) => Ok(Some(msg)),
			Poll::Ready(None) => Err(()),
			Poll::Pending => Ok(None),
		}
	}

	/// Receive a message.
	pub async fn recv(&mut self) -> Result<M, SubsystemError> {
		self.rx.next().await.ok_or(SubsystemError)
	}

	/// Send a message to whom it may concern.
	///
	/// The message will be broadcasted to all other `Subsystem`s that can
	/// receive it.
	pub async fn send_msg(&mut self, msg: M) {
		let _ = self.tx.send(OverseerMessage::SubsystemMessage{
			to: None,
			msg,
		}).await;
	}

	/// Spawn a child `Subsystem` on the executor and get it's `SubsystemId` upon success.
	pub async fn spawn(&mut self, s: Box<dyn Subsystem<M> + Send>) -> Result<SubsystemId, SubsystemError> {
		let (tx, rx) = oneshot::channel();
		let _ = self.tx.send(OverseerMessage::SpawnChild {
			s,
			res: tx,
		}).await;

		rx.await.unwrap_or_else(|_| Err(SubsystemError))
	}

	/// Send a direct message to some other `Subsystem` you know `SubsystemId` of.
	pub async fn send_msg_to(&mut self, to: SubsystemId, msg: M) {
		let _ = self.tx.send(OverseerMessage::SubsystemMessage{
			to: Some(to),
			msg,
		}).await;
	}

	fn new(rx: mpsc::Receiver<M>, tx: mpsc::Sender<OverseerMessage<M>>) -> Self {
		Self {
			rx,
			tx,
		}
	}
}

/// A trait that describes the `Subsystems` that can run on the `Overseer`.
///
/// It is generic over the message type circulating in the system.
/// The idea that we want some type contaning persistent state that
/// can start actually running jobs when asked to.
pub trait Subsystem<M: Debug> {
	/// Start this `Subsystem` and return `SubsystemJob`.
	fn start(&mut self, ctx: SubsystemContext<M>) -> SubsystemJob;
	/// If this `Subsystem` want to receive this message.
	///
	/// By default receive all messages.
	fn can_recv_msg(&self, _msg: &M) -> bool { true }
}

/// A subsystem that we oversee.
///
/// Ties together the `Subsystem` itself and it's running instance
/// (which may be missing if the `Subsystem` is not running at the moment
/// for whatever reason).
struct OverseenSubsystem<M: Debug> {
	subsystem: Box<dyn Subsystem<M> + Send>,
	instance: Option<SubsystemInstance<M>>,
}

/// The `Overseer` itself.
pub struct Overseer<M: Debug, S: Spawn> {
	/// All `Subsystem`s by their respective `SubsystemId`s.
	subsystems: HashMap<SubsystemId, OverseenSubsystem<M>>,

	/// The actual poor man's process tree.
	///
	/// Needed (among other things) to stop a running `Job` along
	/// with all it's children.
	id_to_children: HashMap<SubsystemId, HashSet<SubsystemId>>,

	/// Spawner to spawn tasks to.
	s: S,
}

impl<M: Debug + Clone, S: Spawn> Overseer<M, S> {
	/// Create a new intance of the `Overseer` with some initial set of `Subsystems.
	///
	/// The `Subsystems` submitted to this call will act as a level 1 in the "process tree":
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
	///                        something else
	///                              |
	///                              V
	///                         +-----------+
	///                         |           |
	///                         +-----------+
	///
	/// ```
	pub fn new<I: IntoIterator<Item = Box<dyn Subsystem<M> + Send>>>(subsystems: I, s: S) -> Self {
		let mut this = Self {
			subsystems: HashMap::new(),
			id_to_children: HashMap::new(),
			s,
		};

		for s in subsystems.into_iter() {
			let _ = this.spawn(s);
		}

		this
	}

	/// Run the `Overseer`.
	// TODO: we have to
	//   * Give out to the user some handler to communicate with the `Overseer`
	//     to tell it to do things such as `Start` `Stop` or `Spawn`
	//   * Actually implement stopping of the `Overseer`, atm it's unstoppable.
	pub async fn run(mut self) {
		loop {
			// Upon iteration of the loop we will be collecting all the messages
			// that need dispatching (if any).
			let mut msgs = Vec::default();

			for (id, s) in self.subsystems.iter_mut() {
				if let Some(s) = &mut s.instance {
					while let Poll::Ready(Some(msg)) = poll!(&mut s.rx.next()) {
						log::info!("Received message from subsystem {:?}", msg);
						msgs.push((*id, msg));
					}
				}
			}

			// Do the message dispatching be it broadcasting or direct messages.
			//
			// TODO: this desperately need refactoring.
			for msg in msgs.into_iter() {
				match msg.1 {
					OverseerMessage::SubsystemMessage{ to, msg: m } => {
						match to {
							Some(to) => {
								if let Some(subsystem) = self.subsystems.get_mut(&to) {
									if let Some(ref mut i) = subsystem.instance {
										let _ = i.tx.send(m).await;
									}
								}
							}
							None => {
								for (id, s) in self.subsystems.iter_mut() {
									// Don't send messages back to the sender.
									if msg.0 == *id {
										continue;
									}

									if s.subsystem.can_recv_msg(&m) {
										if let Some(ref mut i) = s.instance {
											let _ = i.tx.send(m.clone()).await;
										}
									}
								}
							}
						}
					}
					OverseerMessage::SpawnChild { s, res } => {
						log::info!("Spawn message");

						let s = self.spawn(s);

						if let Ok(id) = s {
							match self.id_to_children.get_mut(&msg.0) {
								Some(ref mut v) => {
									v.insert(msg.0);
								}
								None => {
									let mut hs = HashSet::new();
									hs.insert(id);
									self.id_to_children.insert(msg.0, hs);
								}
							}
						}
						let _ = res.send(s);
					}
				}
			}

			// Log our "poor man's" process tree as it is at the moment.
			log::trace!("Pids {:?}", self.id_to_children);

			// Looks like nothing is left to be polled, let's take a break.
			pending!();
		}
	}

	fn spawn(&mut self, mut s: Box<dyn Subsystem<M> + Send>) -> Result<SubsystemId, SubsystemError> {
		let (to_tx, to_rx) = mpsc::channel(1024);
		let (from_tx, from_rx) = mpsc::channel(1024);
		let ctx = SubsystemContext::new(to_rx, from_tx);
		let f = s.start(ctx);

		let (signal, exit) = exit_future::signal();
		self.s.spawn(futures::future::select(exit, f.0).map(|_| ())).unwrap();

		let instance = Some(SubsystemInstance {
			rx: from_rx,
			tx: to_tx,
			_signal: signal,
		});

		let id = SubsystemId(get_id());
		self.subsystems.insert(id, OverseenSubsystem {
			subsystem: s,
			instance,
		});

		Ok(id)
	}
}


#[cfg(test)]
mod tests {
	use futures::{executor, pin_mut, select, channel::mpsc, FutureExt};
	use super::*;

	// TODO: Can the test types and the tests themselves be simplified
	// to avoid all this message collection to compare results to the desired ones?
	struct TestSubsystem1(mpsc::Sender<usize>);

	impl Subsystem<usize> for TestSubsystem1 {
		fn start(&mut self, mut ctx: SubsystemContext<usize>) -> SubsystemJob {
			let mut sender = self.0.clone();
			SubsystemJob(Box::pin(async move {
				loop {
					match ctx.recv().await {
						Ok(msg) => {
							let _ = sender.send(msg).await;
							continue;
						}
					    Err(_) => return,
					}
				}
			}))
		}
	}

	struct TestSubsystem2(mpsc::Sender<usize>);

	impl Subsystem<usize> for TestSubsystem2 {
		fn start(&mut self, mut ctx: SubsystemContext<usize>) -> SubsystemJob {
			SubsystemJob(Box::pin(async move {
				let mut c = 0;
				loop {
					if c < 10 {
						ctx.send_msg(c).await;
						c += 1;
						continue;
					}
					match ctx.try_recv().await {
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

	struct TestSubsystem3(Option<oneshot::Sender<usize>>);

	impl Subsystem<usize> for TestSubsystem3 {
		fn start(&mut self, mut ctx: SubsystemContext<usize>) -> SubsystemJob {
			let oneshot = self.0.take().unwrap();

			SubsystemJob(Box::pin(async move {
				let (tx, mut rx) = mpsc::channel(1024);

				let s1 = Box::new(TestSubsystem1(tx));

				let s1_id = ctx.spawn(s1).await.unwrap();

				let mut c = 0;
				loop {
					if c < 10 {
						ctx.send_msg_to(s1_id, c).await;
						assert_eq!(rx.next().await, Some(c));
						c += 1;
						continue;
					}
					break;
				}

				let _ = oneshot.send(c);

				// just stay around for longer
				loop {
					match ctx.try_recv().await {
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

			let subsystems: Vec<Box<dyn Subsystem<usize> + Send>> = vec![
				Box::new(TestSubsystem1(s1_tx)),
				Box::new(TestSubsystem2(s2_tx)),
			];
			let overseer = Overseer::new(subsystems, spawner);
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
									break;
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

	// Test that spawning a subsystem and sending it a direct message works
	#[test]
	fn overseer_spawn_works() {
		let spawner = executor::ThreadPool::new().unwrap();

		executor::block_on(async move {
			let (tx, rx) = oneshot::channel();
			let subsystems: Vec<Box<dyn Subsystem<usize> + Send>> = vec![
				Box::new(TestSubsystem3(Some(tx))),
			];
			let overseer = Overseer::new(subsystems, spawner);
			let overseer_fut = overseer.run().fuse();

			let mut rx = rx.fuse();
			pin_mut!(overseer_fut);

			loop {
				select! {
					a = overseer_fut => break,
					result = rx => {
						assert_eq!(result.unwrap(), 10);
						break;
					}
				}
			}
		});
	}
}
