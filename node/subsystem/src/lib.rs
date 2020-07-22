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

use std::pin::Pin;

use futures::prelude::*;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;

use polkadot_primitives::v1::Hash;
use async_trait::async_trait;

use crate::messages::AllMessages;

pub mod messages;
pub mod util;
#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers;

/// Signals sent by an overseer to a subsystem.
#[derive(PartialEq, Clone, Debug)]
pub enum OverseerSignal {
	/// `Subsystem` should start working on block-based work, given by the relay-chain block hash.
	StartWork(Hash),
	/// `Subsystem` should stop working on block-based work specified by the relay-chain block hash.
	StopWork(Hash),
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
#[derive(Debug, PartialEq, Eq)]
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

impl From<futures::task::SpawnError> for SubsystemError {
    fn from(_: futures::task::SpawnError) -> Self {
		Self
    }
}

impl From<std::convert::Infallible> for SubsystemError {
	fn from(e: std::convert::Infallible) -> Self {
		match e {}
	}
}

/// An asynchronous subsystem task..
///
/// In essence it's just a newtype wrapping a `BoxFuture`.
pub struct SpawnedSubsystem {
	/// Name of the subsystem being spawned.
	pub name: &'static str,
	/// The task of the subsystem being spawned.
	pub future: BoxFuture<'static, ()>,
}

/// A `Result` type that wraps [`SubsystemError`].
///
/// [`SubsystemError`]: struct.SubsystemError.html
pub type SubsystemResult<T> = Result<T, SubsystemError>;

/// A context type that is given to the [`Subsystem`] upon spawning.
/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
/// or spawn jobs.
///
/// [`Overseer`]: struct.Overseer.html
/// [`SubsystemJob`]: trait.SubsystemJob.html
#[async_trait]
pub trait SubsystemContext: Send + 'static {
	/// The message type of this context. Subsystems launched with this context will expect
	/// to receive messages of this type.
	type Message: Send;

	/// Try to asynchronously receive a message.
	///
	/// This has to be used with caution, if you loop over this without
	/// using `pending!()` macro you will end up with a busy loop!
	async fn try_recv(&mut self) -> Result<Option<FromOverseer<Self::Message>>, ()>;

	/// Receive a message.
	async fn recv(&mut self) -> SubsystemResult<FromOverseer<Self::Message>>;

	/// Spawn a child task on the executor.
	async fn spawn(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>) -> SubsystemResult<()>;

	/// Send a direct message to some other `Subsystem`, routed based on message type.
	async fn send_message(&mut self, msg: AllMessages) -> SubsystemResult<()>;

	/// Send multiple direct messages to other `Subsystem`s, routed based on message type.
	async fn send_messages<T>(&mut self, msgs: T) -> SubsystemResult<()>
		where T: IntoIterator<Item = AllMessages> + Send, T::IntoIter: Send;
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

impl<C: SubsystemContext> Subsystem<C> for DummySubsystem {
	fn start(self, mut ctx: C) -> SpawnedSubsystem {
		let future = Box::pin(async move {
			loop {
				match ctx.recv().await {
					Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => return,
					Err(_) => return,
					_ => continue,
				}
			}
		});

		SpawnedSubsystem {
			name: "DummySubsystem",
			future,
		}
	}
}
