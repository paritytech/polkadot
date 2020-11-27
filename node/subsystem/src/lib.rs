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

use polkadot_primitives::v1::{Hash, PoV};
use async_trait::async_trait;
use smallvec::SmallVec;
use thiserror::Error;

use crate::messages::AllMessages;

pub mod errors;
pub mod messages;

/// How many slots are stack-reserved for active leaves updates
///
/// If there are fewer than this number of slots, then we've wasted some stack space.
/// If there are greater than this number of slots, then we fall back to a heap vector.
const ACTIVE_LEAVES_SMALLVEC_CAPACITY: usize = 8;

/// Changes in the set of active leaves: the parachain heads which we care to work on.
///
/// Note that the activated and deactivated fields indicate deltas, not complete sets.
#[derive(Clone, Debug, Default, Eq)]
pub struct ActiveLeavesUpdate {
	/// New relay chain block hashes of interest.
	pub activated: SmallVec<[Hash; ACTIVE_LEAVES_SMALLVEC_CAPACITY]>,
	/// Relay chain block hashes no longer of interest.
	pub deactivated: SmallVec<[Hash; ACTIVE_LEAVES_SMALLVEC_CAPACITY]>,
}

impl ActiveLeavesUpdate {
	/// Create a ActiveLeavesUpdate with a single activated hash
	pub fn start_work(hash: Hash) -> Self {
		Self { activated: [hash][..].into(), ..Default::default() }
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
			&& self.activated.iter().all(|a| other.activated.contains(a))
			&& self.deactivated.iter().all(|a| other.deactivated.contains(a))
	}
}

/// Signals sent by an overseer to a subsystem.
#[derive(PartialEq, Clone, Debug)]
pub enum OverseerSignal {
	/// Subsystems should adjust their jobs to start and stop work on appropriate block hashes.
	ActiveLeaves(ActiveLeavesUpdate),
	/// `Subsystem` is informed of a finalized block by its block hash.
	BlockFinalized(Hash),
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
#[derive(Error, Debug)]
pub enum SubsystemError {
	/// A notification connection is no longer valid.
	#[error(transparent)]
	NotifyCancellation(#[from] oneshot::Canceled),

	/// Queue does not accept another item.
	#[error(transparent)]
	QueueError(#[from] mpsc::SendError),

	/// An attempt to spawn a futures task did not succeed.
	#[error(transparent)]
	TaskSpawn(#[from] futures::task::SpawnError),

	/// An infallable error.
	#[error(transparent)]
	Infallible(#[from] std::convert::Infallible),

	/// Prometheus had a problem
	#[error(transparent)]
	Prometheus(#[from] substrate_prometheus_endpoint::PrometheusError),

	/// An other error lacking particular type information.
	#[error("Failed to {0}")]
	Context(String),

	/// Per origin (or subsystem) annotations to wrap an error.
	#[error("Error originated in {origin}")]
	FromOrigin {
		/// An additional anotation tag for the origin of `source`.
		origin: &'static str,
		/// The wrapped error. Marked as source for tracking the error chain.
		#[source] source: Box<dyn std::error::Error + Send>
	},
}

impl SubsystemError {
	/// Adds a `str` as `origin` to the given error `err`.
	pub fn with_origin<E: 'static + Send + std::error::Error>(origin: &'static str, err: E) -> Self {
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

	/// Spawn a blocking child task on the executor's dedicated thread pool.
	async fn spawn_blocking(
		&mut self,
		name: &'static str,
		s: Pin<Box<dyn Future<Output = ()> + Send>>,
	) -> SubsystemResult<()>;

	/// Send a direct message to some other `Subsystem`, routed based on message type.
	async fn send_message(&mut self, msg: AllMessages);

	/// Send multiple direct messages to other `Subsystem`s, routed based on message type.
	async fn send_messages<T>(&mut self, msgs: T)
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

// Jaeger integration.
//
// See <https://www.jaegertracing.io/> for an introduction.
//
// The easiest way to try Jaeger is:
//
// - Start a docker container with the all-in-one docker image (see below).
// - Open your browser and navigate to <https://localhost:16686> to acces the UI.
//
// The all-in-one docker image can be started with:
//
// ```not_rust
// docker run -d --name jaeger \
//  -e COLLECTOR_ZIPKIN_HTTP_PORT=9411 \
//  -p 5775:5775/udp \
//  -p 6831:6831/udp \
//  -p 6832:6832/udp \
//  -p 5778:5778 \
//  -p 16686:16686 \
//  -p 14268:14268 \
//  -p 14250:14250 \
//  -p 9411:9411 \
//  jaegertracing/all-in-one:1.21
// ```
//

/// Shortcut for [`hash_span`] with the hash of the `PoV`.
pub fn pov_span(pov: &PoV, span_name: &str) -> mick_jaeger::Span {
	hash_span(&pov.hash(), span_name)
}

/// Creates a `Span` referring to the given hash. All spans created with [`hash_span`] with the
/// same hash (even from multiple different nodes) will be visible in the same view on Jaeger.
pub fn hash_span(hash: &Hash, span_name: &str) -> mick_jaeger::Span {
	let trace_id = {
		let mut buf = [0u8; 16];
		buf.copy_from_slice(&hash.as_ref()[0..16]);
		std::num::NonZeroU128::new(u128::from_be_bytes(buf)).unwrap()
	};

	TRACES_IN.lock().unwrap().as_ref().unwrap().span(trace_id, span_name)
}

/// Must be called otherwise the other jaeger-related functions will panic.
///
/// THIS API IS REALLY BAD AND SHOULDN'T BE FOUND IN THE MASTER BRANCH
pub fn init_jaeger(node_name: &str) {
	let (traces_in, mut traces_out) = mick_jaeger::init(mick_jaeger::Config {
		service_name: format!("polkadot-{}", node_name),
	});

	let jaeger_agent: std::net::SocketAddr = "127.0.0.1:6831".parse().unwrap();

	// Spawn a background task that pulls span information and sends them on the network.
	async_std::task::spawn(async move {
		let udp_socket = async_std::net::UdpSocket::bind("0.0.0.0:0").await.unwrap();

		loop {
			let buf = traces_out.next().await;
			// UDP sending errors happen only either if the API is misused (in which case
			// panicking is desirable) or in case of missing priviledge, in which case a
			// panic is preferable in order to inform the user.
			udp_socket.send_to(&buf, jaeger_agent).await.unwrap();
		}
	});

	*TRACES_IN.lock().unwrap() = Some(traces_in);
}

lazy_static::lazy_static! {
	static ref TRACES_IN: std::sync::Mutex<Option<std::sync::Arc<mick_jaeger::TracesIn>>> = std::sync::Mutex::new(None);
}
