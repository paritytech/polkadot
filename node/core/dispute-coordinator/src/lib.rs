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

//! Implements the dispute coordinator subsystem.
//!
//! This is the central subsystem of the node-side components which participate in disputes.
//! This subsystem wraps a database which tracks all statements observed by all validators over some window of sessions.
//! Votes older than this session window are pruned.
//!
//! This subsystem will be the point which produce dispute votes, either positive or negative, based on locally-observed
//! validation results as well as a sink for votes received by other subsystems. When importing a dispute vote from
//! another node, this will trigger the dispute participation subsystem to recover and validate the block and call
//! back to this subsystem.

/// Metrics types.
mod metrics;

/// Common error types for this subsystem.
mod error;

/// Status tracking of disputes (`DisputeStatus`).
mod status;

/// Dummy implementation.
mod dummy;
/// The real implementation.
mod real;

use kvdb::KeyValueDB;
use metrics::Metrics;
use polkadot_node_subsystem::{
	messages::DisputeCoordinatorMessage, overseer, SpawnedSubsystem, SubsystemContext,
	SubsystemError,
};
use sc_keystore::LocalKeystore;
use std::sync::Arc;

pub use self::real::Config;

pub(crate) const LOG_TARGET: &str = "parachain::dispute-coordinator";

/// The disputes coordinator subsystem, abstracts `dummy` and `real` implementations.
pub enum DisputeCoordinatorSubsystem {
	Dummy(dummy::DisputeCoordinatorSubsystem),
	Real(real::DisputeCoordinatorSubsystem),
}

impl DisputeCoordinatorSubsystem {
	/// Create a new dummy instance.
	pub fn dummy() -> Self {
		DisputeCoordinatorSubsystem::Dummy(dummy::DisputeCoordinatorSubsystem::new())
	}

	/// Create a new instance of the subsystem.
	pub fn new(
		store: Arc<dyn KeyValueDB>,
		config: real::Config,
		keystore: Arc<LocalKeystore>,
		metrics: Metrics,
	) -> Self {
		DisputeCoordinatorSubsystem::Real(real::DisputeCoordinatorSubsystem::new(
			store, config, keystore, metrics,
		))
	}
}

impl<Context> overseer::Subsystem<Context, SubsystemError> for DisputeCoordinatorSubsystem
where
	Context: SubsystemContext<Message = DisputeCoordinatorMessage>,
	Context: overseer::SubsystemContext<Message = DisputeCoordinatorMessage>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		match self {
			DisputeCoordinatorSubsystem::Dummy(dummy) => dummy.start(ctx),
			DisputeCoordinatorSubsystem::Real(real) => real.start(ctx),
		}
	}
}
