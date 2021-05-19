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

/// How many slots are stack-reserved for active leaves updates
///
/// If there are fewer than this number of slots, then we've wasted some stack space.
/// If there are greater than this number of slots, then we fall back to a heap vector.
const ACTIVE_LEAVES_SMALLVEC_CAPACITY: usize = 8;

/// Activated leaf.
#[derive(Debug, Clone)]
pub struct ActivatedLeaf {
	/// The block hash.
	pub hash: Hash,
	/// The block number.
	pub number: BlockNumber,
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
	Jaeger(#[from] jaeger::JaegerError),

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

impl SubsystemError {
	/// Adds a `str` as `origin` to the given error `err`.
	pub fn with_origin<E: 'static + Send + Sync + std::error::Error>(origin: &'static str, err: E) -> Self {
		Self::FromOrigin { origin, source: Box::new(err) }
	}
}

/// A `Result` type that wraps [`SubsystemError`].
///
/// [`SubsystemError`]: struct.SubsystemError.html
pub type SubsystemResult<T> = Result<T, SubsystemError>;
