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

pub use overseer_gen_proc_macro::*;
#[doc(hidden)]
pub use tracing;
#[doc(hidden)]
pub use metered;
#[doc(hidden)]
pub use sp_core::traits::SpawnNamed;

#[doc(hidden)]
pub use futures::{
	select,
	future::{
		Fuse, Future, BoxFuture
	},
	stream::{
		select, FuturesUnordered
	},
	channel::{mpsc, oneshot},
};
#[doc(hidden)]
pub use async_trait::async_trait;

#[doc(hidden)]
pub use std::time::Duration;
use std::sync::atomic::{self, AtomicUsize};
use std::sync::Arc;

/// A type of messages that are sent from [`Subsystem`] to [`Overseer`].
///
/// Used to launch jobs.
pub enum ToOverseer {
	/// A message that wraps something the `Subsystem` is desiring to
	/// spawn on the overseer and a `oneshot::Sender` to signal the result
	/// of the spawn.
	SpawnJob {
		/// Name of the task to spawn which be shown in jaeger and tracing logs.
		name: &'static str,
		/// The future to execute.
		s: BoxFuture<'static, ()>,
	},

	/// Same as `SpawnJob` but for blocking tasks to be executed on a
	/// dedicated thread pool.
	SpawnBlockingJob {
		/// Name of the task to spawn which be shown in jaeger and tracing logs.
		name: &'static str,
		/// The future to execute.
		s: BoxFuture<'static, ()>,
	},
}



/// A helper trait to map a subsystem to smth. else.
pub(crate) trait MapSubsystem<T> {
	type Output;

	fn map_subsystem(&self, sub: T) -> Self::Output;
}

impl<F, T, U> MapSubsystem<T> for F where F: Fn(T) -> U {
	type Output = U;

	fn map_subsystem(&self, sub: T) -> U {
		(self)(sub)
	}
}

/// A wrapping type for messages.
// FIXME XXX elaborate the purpose of this.
#[derive(Debug)]
pub struct MessagePacket<T> {
	signals_received: usize,
	message: T,
}

/// Create a packet from its parts.
pub fn make_packet<T>(signals_received: usize, message: T) -> MessagePacket<T> {
	MessagePacket {
		signals_received,
		message,
	}
}

/// Incoming messages from both the bounded and unbounded channel.
pub type SubsystemIncomingMessages<M> = ::futures::stream::Select<
	self::metered::MeteredReceiver<MessagePacket<M>>,
	self::metered::UnboundedMeteredReceiver<MessagePacket<M>>,
>;


/// Meter to count the received signals in total.
// XXX FIXME is there a necessity for this? Seems redundant to `ReadOuts`
#[derive(Debug, Default, Clone)]
pub struct SignalsReceived(Arc<AtomicUsize>);

impl SignalsReceived {
	/// Load the current value of received signals.
	pub fn load(&self) -> usize {
		// off by a few is ok
		self.0.load(atomic::Ordering::Relaxed)
	}

	/// Increase the number of signals by one.
	pub fn inc(&self) {
		self.0.fetch_add(1, atomic::Ordering::Acquire);
	}
}



/// A trait to support the origin annotation
/// such that errors across subsystems can be easier tracked.
pub trait AnnotateErrorOrigin: 'static + Send + Sync + std::error::Error {
	fn with_origin(self, origin: &'static str) -> Self;
}


/// An asynchronous subsystem task..
///
/// In essence it's just a newtype wrapping a `BoxFuture`.
pub struct SpawnedSubsystem<E>
where
E: std::error::Error
 + 'static
 + Send
 + From<SubsystemError>
{
	/// Name of the subsystem being spawned.
	pub name: &'static str,
	/// The task of the subsystem being spawned.
	pub future: BoxFuture<'static, Result<(), E>>,
}


/// An error type that describes faults that may happen
///
/// These are:
///   * Channels being closed
///   * Subsystems dying when they are not expected to
///   * Subsystems not dying when they are told to die
///   * etc.
// FIXME XXX make generic over the source error of FromOrigin
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

	#[error("Failed to {0}")]
	Context(String),

	#[error("Subsystem stalled: {0}")]
	SubsystemStalled(&'static str),

	/// Per origin (or subsystem) annotations to wrap an error.
	#[error("Error originated in {origin}")]
	FromOrigin {
		/// An additional anotation tag for the origin of `source`.
		origin: &'static str,
		/// The wrapped error. Marked as source for tracking the error chain.
		#[source] source: Box<dyn 'static + std::error::Error + Send + Sync>
	},
}

pub type SubsystemResult<T> = std::result::Result<T, SubsystemError>;

/// Collection of meters related to a subsystem.
#[derive(Clone)]
pub struct SubsystemMeters {
	#[allow(missing_docs)]
	pub bounded: metered::Meter,
	#[allow(missing_docs)]
	pub unbounded: metered::Meter,
	#[allow(missing_docs)]
	pub signals: metered::Meter,
}

impl SubsystemMeters {
	/// Read the values of all subsystem `Meter`s.
	pub fn read(&self) -> SubsystemMeterReadouts {
		SubsystemMeterReadouts {
			bounded: self.bounded.read(),
			unbounded: self.unbounded.read(),
			signals: self.signals.read(),
		}
	}
}


/// Set of readouts of the `Meter`s of a subsystem.
pub struct SubsystemMeterReadouts {
	#[allow(missing_docs)]
	pub bounded: metered::Readout,
	#[allow(missing_docs)]
	pub unbounded: metered::Readout,
	#[allow(missing_docs)]
	pub signals: metered::Readout,
}

#[cfg(test)]
mod tests;
