// Copyright 2021 Parity Technologies (UK) Ltd.
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
//!             .  | SubsystemInstance1 | <-- bidir --> | SubsystemInstance2  |  .
//!             .  +--------------------+               +---------------------+  .
//!             ..................................................................
//! ```

// #![deny(unused_results)]
// unused dependencies can not work for test and examples at the same time
// yielding false positives
#![deny(missing_docs)]
#![deny(unused_crate_dependencies)]

pub use polkadot_overseer_gen_proc_macro::overlord;

#[doc(hidden)]
pub use gum;
#[doc(hidden)]
pub use metered;
#[doc(hidden)]
pub use polkadot_node_primitives::SpawnNamed;

#[doc(hidden)]
pub use async_trait::async_trait;
#[doc(hidden)]
pub use futures::{
	self,
	channel::{mpsc, oneshot},
	future::{BoxFuture, Fuse, Future},
	poll, select,
	stream::{self, select, FuturesUnordered},
	task::{Context, Poll},
	FutureExt, StreamExt,
};
#[doc(hidden)]
pub use std::pin::Pin;

use std::sync::{
	atomic::{self, AtomicUsize},
	Arc,
};
#[doc(hidden)]
pub use std::time::Duration;

#[doc(hidden)]
pub use futures_timer::Delay;

pub use polkadot_node_network_protocol::WrongVariant;

use std::fmt;

#[cfg(test)]
mod tests;

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
		/// Subsystem of the task to spawn which be shown in jaeger and tracing logs.
		subsystem: Option<&'static str>,
		/// The future to execute.
		s: BoxFuture<'static, ()>,
	},

	/// Same as `SpawnJob` but for blocking tasks to be executed on a
	/// dedicated thread pool.
	SpawnBlockingJob {
		/// Name of the task to spawn which be shown in jaeger and tracing logs.
		name: &'static str,
		/// Subsystem of the task to spawn which be shown in jaeger and tracing logs.
		subsystem: Option<&'static str>,
		/// The future to execute.
		s: BoxFuture<'static, ()>,
	},
}

impl fmt::Debug for ToOverseer {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::SpawnJob { name, subsystem, .. } => {
				writeln!(f, "SpawnJob{{ {}, {} ..}}", name, subsystem.unwrap_or("default"))
			},
			Self::SpawnBlockingJob { name, subsystem, .. } => {
				writeln!(f, "SpawnBlockingJob{{ {}, {} ..}}", name, subsystem.unwrap_or("default"))
			},
		}
	}
}

/// A helper trait to map a subsystem to smth. else.
pub trait MapSubsystem<T> {
	/// The output type of the mapping.
	type Output;

	/// Consumes a `T` per subsystem, and maps it to `Self::Output`.
	fn map_subsystem(&self, sub: T) -> Self::Output;
}

impl<F, T, U> MapSubsystem<T> for F
where
	F: Fn(T) -> U,
{
	type Output = U;

	fn map_subsystem(&self, sub: T) -> U {
		(self)(sub)
	}
}

/// A wrapping type for messages.
///
/// Includes a counter to synchronize signals with messages,
/// such that no inconsistent message sequences are prevented.
#[derive(Debug)]
pub struct MessagePacket<T> {
	/// Signal level at the point of reception.
	///
	/// Required to assure signals were consumed _before_
	/// consuming messages that are based on the assumption
	/// that a certain signal was assumed.
	pub signals_received: usize,
	/// The message to be sent/consumed.
	pub message: T,
}

/// Create a packet from its parts.
pub fn make_packet<T>(signals_received: usize, message: T) -> MessagePacket<T> {
	MessagePacket { signals_received, message }
}

/// Incoming messages from both the bounded and unbounded channel.
pub type SubsystemIncomingMessages<M> = self::stream::Select<
	self::metered::MeteredReceiver<MessagePacket<M>>,
	self::metered::UnboundedMeteredReceiver<MessagePacket<M>>,
>;

/// Watermark to track the received signals.
#[derive(Debug, Default, Clone)]
pub struct SignalsReceived(Arc<AtomicUsize>);

impl SignalsReceived {
	/// Load the current value of received signals.
	pub fn load(&self) -> usize {
		// It's imperative that we prevent reading a stale value from memory because of reordering.
		// Memory barrier to ensure that no reads or writes in the current thread before this load are reordered.
		// All writes in other threads using release semantics become visible to the current thread.
		self.0.load(atomic::Ordering::Acquire)
	}

	/// Increase the number of signals by one.
	pub fn inc(&self) {
		self.0.fetch_add(1, atomic::Ordering::AcqRel);
	}
}

/// A trait to support the origin annotation
/// such that errors across subsystems can be easier tracked.
pub trait AnnotateErrorOrigin: 'static + Send + Sync + std::error::Error {
	/// Annotate the error with a origin `str`.
	///
	/// Commonly this is used to create nested enum variants.
	///
	/// ```rust,ignore
	/// E::WithOrigin("I am originally from Cowtown.", E::Variant)
	/// ```
	fn with_origin(self, origin: &'static str) -> Self;
}

/// An asynchronous subsystem task..
///
/// In essence it's just a new type wrapping a `BoxFuture`.
pub struct SpawnedSubsystem<E>
where
	E: std::error::Error + Send + Sync + 'static + From<self::OverseerError>,
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
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum OverseerError {
	#[error(transparent)]
	NotifyCancellation(#[from] oneshot::Canceled),

	#[error(transparent)]
	QueueError(#[from] mpsc::SendError),

	#[error("Failed to spawn task {0}")]
	TaskSpawn(&'static str),

	#[error(transparent)]
	Infallible(#[from] std::convert::Infallible),

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
		#[source]
		source: Box<dyn 'static + std::error::Error + Send + Sync>,
	},
}

/// Alias for a result with error type `OverseerError`.
pub type OverseerResult<T> = std::result::Result<T, self::OverseerError>;

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

/// A running instance of some [`Subsystem`].
///
/// [`Subsystem`]: trait.Subsystem.html
///
/// `M` here is the inner message type, and _not_ the generated `enum AllMessages` or `#message_wrapper` type.
pub struct SubsystemInstance<Message, Signal> {
	/// Send sink for `Signal`s to be sent to a subsystem.
	pub tx_signal: crate::metered::MeteredSender<Signal>,
	/// Send sink for `Message`s to be sent to a subsystem.
	pub tx_bounded: crate::metered::MeteredSender<MessagePacket<Message>>,
	/// All meters of the particular subsystem instance.
	pub meters: SubsystemMeters,
	/// The number of signals already received.
	/// Required to assure messages and signals
	/// are processed correctly.
	pub signals_received: usize,
	/// Name of the subsystem instance.
	pub name: &'static str,
}

/// A message type that a subsystem receives from an overseer.
/// It wraps signals from an overseer and messages that are circulating
/// between subsystems.
///
/// It is generic over over the message type `M` that a particular `Subsystem` may use.
#[derive(Debug)]
pub enum FromOverseer<Message, Signal> {
	/// Signal from the `Overseer`.
	Signal(Signal),

	/// Some other `Subsystem`'s message.
	Communication {
		/// Contained message
		msg: Message,
	},
}

impl<Signal, Message> From<Signal> for FromOverseer<Message, Signal> {
	fn from(signal: Signal) -> Self {
		Self::Signal(signal)
	}
}

/// A context type that is given to the [`Subsystem`] upon spawning.
/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
/// or spawn jobs.
///
/// [`Overseer`]: struct.Overseer.html
/// [`SubsystemJob`]: trait.SubsystemJob.html
#[async_trait::async_trait]
pub trait SubsystemContext: Send + 'static {
	/// The message type of this context. Subsystems launched with this context will expect
	/// to receive messages of this type. Commonly uses the wrapping `enum` commonly called
	/// `AllMessages`.
	type Message: std::fmt::Debug + Send + 'static;
	/// And the same for signals.
	type Signal: std::fmt::Debug + Send + 'static;
	/// The overarching messages `enum` for this particular subsystem.
	type OutgoingMessages: Send + 'static;
	/// The sender type as provided by `sender()` and underlying.
	type Sender: SubsystemSender<Self::OutgoingMessages> + Send + 'static;
	/// The error type.
	type Error: ::std::error::Error + ::std::convert::From<OverseerError> + Sync + Send + 'static;

	/// Try to asynchronously receive a message.
	///
	/// This has to be used with caution, if you loop over this without
	/// using `pending!()` macro you will end up with a busy loop!
	async fn try_recv(&mut self) -> Result<Option<FromOverseer<Self::Message, Self::Signal>>, ()>;

	/// Receive a message.
	async fn recv(&mut self) -> Result<FromOverseer<Self::Message, Self::Signal>, Self::Error>;

	/// Spawn a child task on the executor.
	fn spawn(
		&mut self,
		name: &'static str,
		s: ::std::pin::Pin<Box<dyn crate::Future<Output = ()> + Send>>,
	) -> Result<(), Self::Error>;

	/// Spawn a blocking child task on the executor's dedicated thread pool.
	fn spawn_blocking(
		&mut self,
		name: &'static str,
		s: ::std::pin::Pin<Box<dyn crate::Future<Output = ()> + Send>>,
	) -> Result<(), Self::Error>;

	/// Send a direct message to some other `Subsystem`, routed based on message type.
	async fn send_message<X>(&mut self, msg: X)
	where
		Self::OutgoingMessages: From<X>,
		X: Send,
	{
		self.sender().send_message(<Self::OutgoingMessages>::from(msg)).await
	}

	/// Send multiple direct messages to other `Subsystem`s, routed based on message type.
	async fn send_messages<X, T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = X> + Send,
		T::IntoIter: Send,
		Self::OutgoingMessages: From<X>,
		X: Send,
	{
		self.sender()
			.send_messages(msgs.into_iter().map(|x| <Self::OutgoingMessages>::from(x)))
			.await
	}

	/// Send a message using the unbounded connection.
	fn send_unbounded_message<X>(&mut self, msg: X)
	where
		Self::OutgoingMessages: From<X>,
		X: Send,
	{
		self.sender().send_unbounded_message(Self::OutgoingMessages::from(msg))
	}

	/// Obtain the sender.
	fn sender(&mut self) -> &mut Self::Sender;
}

/// A trait that describes the [`Subsystem`]s that can run on the [`Overseer`].
///
/// It is generic over the message type circulating in the system.
/// The idea that we want some type containing persistent state that
/// can spawn actually running subsystems when asked.
///
/// [`Overseer`]: struct.Overseer.html
/// [`Subsystem`]: trait.Subsystem.html
pub trait Subsystem<Ctx, E>
where
	Ctx: SubsystemContext,
	E: std::error::Error + Send + Sync + 'static + From<self::OverseerError>,
{
	/// Start this `Subsystem` and return `SpawnedSubsystem`.
	fn start(self, ctx: Ctx) -> SpawnedSubsystem<E>;
}

/// Sender end of a channel to interface with a subsystem.
#[async_trait::async_trait]
pub trait SubsystemSender<Message>: Send + Clone + 'static {
	/// Send a direct message to some other `Subsystem`, routed based on message type.
	async fn send_message(&mut self, msg: Message);

	/// Send multiple direct messages to other `Subsystem`s, routed based on message type.
	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = Message> + Send,
		T::IntoIter: Send;

	/// Send a message onto the unbounded queue of some other `Subsystem`, routed based on message
	/// type.
	///
	/// This function should be used only when there is some other bounding factor on the messages
	/// sent with it. Otherwise, it risks a memory leak.
	fn send_unbounded_message(&mut self, msg: Message);
}

/// A future that wraps another future with a `Delay` allowing for time-limited futures.
#[pin_project::pin_project]
pub struct Timeout<F: Future> {
	#[pin]
	future: F,
	#[pin]
	delay: Delay,
}

/// Extends `Future` to allow time-limited futures.
pub trait TimeoutExt: Future {
	/// Adds a timeout of `duration` to the given `Future`.
	/// Returns a new `Future`.
	fn timeout(self, duration: Duration) -> Timeout<Self>
	where
		Self: Sized,
	{
		Timeout { future: self, delay: Delay::new(duration) }
	}
}

impl<F> TimeoutExt for F where F: Future {}

impl<F> Future for Timeout<F>
where
	F: Future,
{
	type Output = Option<F::Output>;

	fn poll(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Self::Output> {
		let this = self.project();

		if this.delay.poll(ctx).is_ready() {
			return Poll::Ready(None)
		}

		if let Poll::Ready(output) = this.future.poll(ctx) {
			return Poll::Ready(Some(output))
		}

		Poll::Pending
	}
}
