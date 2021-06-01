// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Subsystem trait definitions and message types.
//!
//! Node-side logic for Polkadot is mostly comprised of Subsystems, which are discrete components
//! that communicate via message-passing. They are coordinated by an overseer, provided by a
//! separate crate.

#![warn(missing_docs)]

use std::{pin::Pin, sync::Arc, fmt};

use futures::prelude::*;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;

use polkadot_primitives::v1::{Hash, BlockNumber};
use async_trait::async_trait;
use smallvec::SmallVec;

pub mod errors;
pub mod messages;

pub use polkadot_node_jaeger as jaeger;
pub use jaeger::*;

use self::messages::AllMessages;

/// How many slots are stack-reserved for active leaves updates
///
/// If there are fewer than this number of slots, then we've wasted some stack space.
/// If there are greater than this number of slots, then we fall back to a heap vector.
const ACTIVE_LEAVES_SMALLVEC_CAPACITY: usize = 8;


/// The status of an activated leaf.
#[derive(Debug, Clone)]
pub enum LeafStatus {
	/// A leaf is fresh when it's the first time the leaf has been encountered.
	/// Most leaves should be fresh.
	Fresh,
	/// A leaf is stale when it's encountered for a subsequent time. This will happen
	/// when the chain is reverted or the fork-choice rule abandons some chain.
	Stale,
}

impl LeafStatus {
	/// Returns a bool indicating fresh status.
	pub fn is_fresh(&self) -> bool {
		match *self {
			LeafStatus::Fresh => true,
			LeafStatus::Stale => false,
		}
	}

	/// Returns a bool indicating stale status.
	pub fn is_stale(&self) -> bool {
		match *self {
			LeafStatus::Fresh => false,
			LeafStatus::Stale => true,
		}
	}
}

/// Activated leaf.
#[derive(Debug, Clone)]
pub struct ActivatedLeaf {
	/// The block hash.
	pub hash: Hash,
	/// The block number.
	pub number: BlockNumber,
	/// The status of the leaf.
	pub status: LeafStatus,
	/// An associated [`jaeger::Span`].
	///
	/// NOTE: Each span should only be kept active as long as the leaf is considered active and should be dropped
	/// when the leaf is deactivated.
	pub span: Arc<jaeger::Span>,
}

/// Changes in the set of active leaves: the parachain heads which we care to work on.
///
/// Note that the activated and deactivated fields indicate deltas, not complete sets.
#[derive(Clone, Default)]
pub struct ActiveLeavesUpdate {
	/// New relay chain blocks of interest.
	pub activated: SmallVec<[ActivatedLeaf; ACTIVE_LEAVES_SMALLVEC_CAPACITY]>,
	/// Relay chain block hashes no longer of interest.
	pub deactivated: SmallVec<[Hash; ACTIVE_LEAVES_SMALLVEC_CAPACITY]>,
}

impl ActiveLeavesUpdate {
	/// Create a ActiveLeavesUpdate with a single activated hash
	pub fn start_work(activated: ActivatedLeaf) -> Self {
		Self { activated: [activated][..].into(), ..Default::default() }
	}

	/// Create a ActiveLeavesUpdate with a single deactivated hash
	pub fn stop_work(hash: Hash) -> Self {
		Self { deactivated: [hash][..].into(), ..Default::default() }
	}

	/// Is this update empty and doesn't contain any information?
	pub fn is_empty(&self) -> bool {
		self.activated.is_empty() && self.deactivated.is_empty()
	}
}

impl PartialEq for ActiveLeavesUpdate {
	/// Equality for `ActiveLeavesUpdate` doesnt imply bitwise equality.
	///
	/// Instead, it means equality when `activated` and `deactivated` are considered as sets.
	fn eq(&self, other: &Self) -> bool {
		self.activated.len() == other.activated.len() && self.deactivated.len() == other.deactivated.len()
			&& self.activated.iter().all(|a| other.activated.iter().any(|o| a.hash == o.hash))
			&& self.deactivated.iter().all(|a| other.deactivated.contains(a))
	}
}

impl fmt::Debug for ActiveLeavesUpdate {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		struct Activated<'a>(&'a [ActivatedLeaf]);
		impl fmt::Debug for Activated<'_> {
			fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
				f.debug_list().entries(self.0.iter().map(|e| e.hash)).finish()
			}
		}

		f.debug_struct("ActiveLeavesUpdate")
			.field("activated", &Activated(&self.activated))
			.field("deactivated", &self.deactivated)
			.finish()
	}
}

/// Signals sent by an overseer to a subsystem.
#[derive(PartialEq, Clone, Debug)]
pub enum OverseerSignal {
	/// Subsystems should adjust their jobs to start and stop work on appropriate block hashes.
	ActiveLeaves(ActiveLeavesUpdate),
	/// `Subsystem` is informed of a finalized block by its block hash and number.
	BlockFinalized(Hash, BlockNumber),
	/// Conclude the work of the `Overseer` and all `Subsystem`s.
	Conclude,
}

/// A message type that a subsystem receives from an overseer.
/// It wraps signals from an overseer and messages that are circulating
/// between subsystems.
///
/// It is generic over over the message type `M` that a particular `Subsystem` may use.
#[derive(Debug)]
pub enum FromOverseer<M> {
	/// Signal from the `Overseer`.
	Signal(OverseerSignal),

	/// Some other `Subsystem`'s message.
	Communication {
		/// Contained message
		msg: M,
	},
}


/// An error type that describes faults that may happen
///
/// These are:
///   * Channels being closed
///   * Subsystems dying when they are not expected to
///   * Subsystems not dying when they are told to die
///   * etc.
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum SubsystemError {
	#[error(transparent)]
	NotifyCancellation(#[from] oneshot::Canceled),

	#[error(transparent)]
	QueueError(#[from] mpsc::SendError),

	#[error(transparent)]
	TaskSpawn(#[from] futures::task::SpawnError),

	#[error(transparent)]
	Infallible(#[from] std::convert::Infallible),

	#[error(transparent)]
	Prometheus(#[from] substrate_prometheus_endpoint::PrometheusError),

	#[error(transparent)]
	Jaeger(#[from] JaegerError),

	#[error("Failed to {0}")]
	Context(String),

	#[error("Subsystem stalled: {0}")]
	SubsystemStalled(&'static str),

	/// Per origin (or subsystem) annotations to wrap an error.
	#[error("Error originated in {origin}")]
	FromOrigin {
		/// An additional annotation tag for the origin of `source`.
		origin: &'static str,
		/// The wrapped error. Marked as source for tracking the error chain.
		#[source] source: Box<dyn 'static + std::error::Error + Send + Sync>
	},
}

impl SubsystemError {
	/// Adds a `str` as `origin` to the given error `err`.
	pub fn with_origin<E: 'static + Send + Sync + std::error::Error>(origin: &'static str, err: E) -> Self {
		Self::FromOrigin { origin, source: Box::new(err) }
	}
}

/// An asynchronous subsystem task..
///
/// In essence it's just a newtype wrapping a `BoxFuture`.
pub struct SpawnedSubsystem {
	/// Name of the subsystem being spawned.
	pub name: &'static str,
	/// The task of the subsystem being spawned.
	pub future: BoxFuture<'static, SubsystemResult<()>>,
}

/// A `Result` type that wraps [`SubsystemError`].
///
/// [`SubsystemError`]: struct.SubsystemError.html
pub type SubsystemResult<T> = Result<T, SubsystemError>;

/// A sender used by subsystems to communicate with other subsystems.
///
/// Each clone of this type may add more capacity to the bounded buffer, so clones should
/// be used sparingly.
#[async_trait]
pub trait SubsystemSender: Send + Clone + 'static {
	/// Send a direct message to some other `Subsystem`, routed based on message type.
	async fn send_message(&mut self, msg: AllMessages);

	/// Send multiple direct messages to other `Subsystem`s, routed based on message type.
	async fn send_messages<T>(&mut self, msgs: T)
		where T: IntoIterator<Item = AllMessages> + Send, T::IntoIter: Send;

	/// Send a message onto the unbounded queue of some other `Subsystem`, routed based on message
	/// type.
	///
	/// This function should be used only when there is some other bounding factor on the messages
	/// sent with it. Otherwise, it risks a memory leak.
	fn send_unbounded_message(&mut self, msg: AllMessages);
}

/// A context type that is given to the [`Subsystem`] upon spawning.
/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
/// or spawn jobs.
///
/// [`Overseer`]: struct.Overseer.html
/// [`SubsystemJob`]: trait.SubsystemJob.html
#[async_trait]
pub trait SubsystemContext: Send + Sized + 'static {
	/// The message type of this context. Subsystems launched with this context will expect
	/// to receive messages of this type.
	type Message: Send;

	/// The message sender type of this context. Clones of the sender should be used sparingly.
	type Sender: SubsystemSender;

	/// Try to asynchronously receive a message.
	///
	/// This has to be used with caution, if you loop over this without
	/// using `pending!()` macro you will end up with a busy loop!
	async fn try_recv(&mut self) -> Result<Option<FromOverseer<Self::Message>>, ()>;

	/// Receive a message.
	async fn recv(&mut self) -> SubsystemResult<FromOverseer<Self::Message>>;

	/// Spawn a child task on the executor.
	async fn spawn(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>) -> SubsystemResult<()>;

	/// Spawn a blocking child task on the executor's dedicated thread pool.
	async fn spawn_blocking(
		&mut self,
		name: &'static str,
		s: Pin<Box<dyn Future<Output = ()> + Send>>,
	) -> SubsystemResult<()>;

	/// Get a mutable reference to the sender.
	fn sender(&mut self) -> &mut Self::Sender;

	/// Send a direct message to some other `Subsystem`, routed based on message type.
	async fn send_message(&mut self, msg: AllMessages) {
		self.sender().send_message(msg).await
	}

	/// Send multiple direct messages to other `Subsystem`s, routed based on message type.
	async fn send_messages<T>(&mut self, msgs: T)
		where T: IntoIterator<Item = AllMessages> + Send, T::IntoIter: Send
	{
		self.sender().send_messages(msgs).await
	}


	/// Send a message onto the unbounded queue of some other `Subsystem`, routed based on message
	/// type.
	///
	/// This function should be used only when there is some other bounding factor on the messages
	/// sent with it. Otherwise, it risks a memory leak.
	///
	/// Generally, for this method to be used, these conditions should be met:
	/// * There is a communication cycle between subsystems
	/// * One of the parts of the cycle has a clear bound on the number of messages produced.
	fn send_unbounded_message(&mut self, msg: AllMessages) {
		self.sender().send_unbounded_message(msg)
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
pub trait Subsystem<C: SubsystemContext> {
	/// Start this `Subsystem` and return `SpawnedSubsystem`.
	fn start(self, ctx: C) -> SpawnedSubsystem;
}

/// A dummy subsystem that implements [`Subsystem`] for all
/// types of messages. Used for tests or as a placeholder.
pub struct DummySubsystem;

impl<C: SubsystemContext> Subsystem<C> for DummySubsystem
where
	C::Message: std::fmt::Debug
{
	fn start(self, mut ctx: C) -> SpawnedSubsystem {
		let future = Box::pin(async move {
			loop {
				match ctx.recv().await {
					Err(_) => return Ok(()),
					Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => return Ok(()),
					Ok(overseer_msg) => {
						tracing::debug!(
							target: "dummy-subsystem",
							"Discarding a message sent from overseer {:?}",
							overseer_msg
						);
						continue;
					}
				}
			}
		});

		SpawnedSubsystem {
			name: "dummy-subsystem",
			future,
		}
	}
}
