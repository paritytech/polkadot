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
//! This subsystem will be the point which produce dispute votes, eiuther positive or negative, based on locally-observed
//! validation results as well as a sink for votes received by other subsystems. When importing a dispute vote from
//! another node, this will trigger the dispute participation subsystem to recover and validate the block and call
//! back to this subsystem.

use std::collections::HashMap;
use std::sync::Arc;

use polkadot_node_primitives::CandidateVotes;
use polkadot_node_subsystem::{
	messages::{
		RuntimeApiRequest, DisputeCoordinatorMessage,
	},
	Subsystem, SubsystemContext, SubsystemResult, FromOverseer, OverseerSignal, SpawnedSubsystem,
	SubsystemError,
	errors::{ChainApiError, RuntimeApiError},
};
use polkadot_primitives::v1::{SessionIndex, CandidateHash};

use futures::prelude::*;
use kvdb::KeyValueDB;
use parity_scale_codec::Error as CodecError;
use sc_keystore::LocalKeystore;

mod db;

const LOG_TARGET: &str = "parachain::dispute-coordinator";

struct State {
	keystore: Arc<LocalKeystore>,
	overlay: HashMap<(SessionIndex, CandidateHash), CandidateVotes>,
	highest_session: Option<SessionIndex>,
}

/// Configuration for the dispute coordinator subsystem.
#[derive(Debug, Clone, Copy)]
pub struct Config {
	/// The data column in the store to use for dispute data.
	pub col_data: u32,
}

/// An implementation of the dispute coordinator subsystem.
pub struct DisputeCoordinatorSubsystem {
	config: Config,
	store: Arc<dyn KeyValueDB>,
	keystore: Arc<LocalKeystore>,
}

impl DisputeCoordinatorSubsystem {
	/// Create a new instance of the subsystem.
	pub fn new(
		store: Arc<dyn KeyValueDB>,
		config: Config,
		keystore: Arc<LocalKeystore>,
	) -> Self {
		DisputeCoordinatorSubsystem { store, config, keystore }
	}
}

impl<Context> Subsystem<Context> for DisputeCoordinatorSubsystem
	where Context: SubsystemContext<Message = DisputeCoordinatorMessage>
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = run(self, ctx)
			.map(|_| Ok(()))
			.boxed();

		SpawnedSubsystem {
			name: "dispute-coordinator-subsystem",
			future,
		}
	}
}

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
	#[error(transparent)]
	RuntimeApi(#[from] RuntimeApiError),

	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Io(#[from] std::io::Error),

	#[error(transparent)]
	Oneshot(#[from] futures::channel::oneshot::Canceled),

	#[error(transparent)]
	Subsystem(#[from] SubsystemError),

	#[error(transparent)]
	Codec(#[from] CodecError),
}

impl From<db::v1::Error> for Error {
	fn from(err: db::v1::Error) -> Self {
		match err {
			db::v1::Error::Io(io) => Self::Io(io),
			db::v1::Error::Codec(e) => Self::Codec(e),
		}
	}
}

impl Error {
	fn trace(&self) {
		match self {
			// don't spam the log with spurious errors
			Self::RuntimeApi(_) |
			Self::Oneshot(_) => tracing::debug!(target: LOG_TARGET, err = ?self),
			// it's worth reporting otherwise
			_ => tracing::warn!(target: LOG_TARGET, err = ?self),
		}
	}
}

async fn run<Context>(mut subsystem: DisputeCoordinatorSubsystem, mut ctx: Context)
	where Context: SubsystemContext<Message = DisputeCoordinatorMessage>
{
	loop {
		let res = run_iteration(&mut ctx, &mut subsystem).await;
		match res {
			Err(e) => {
				e.trace();

				if let Error::Subsystem(SubsystemError::Context(_)) = e {
					break;
				}
			}
			Ok(true) => {
				tracing::info!(target: LOG_TARGET, "received `Conclude` signal, exiting");
				break;
			}
			Ok(false) => continue,
		}
	}
}

// Run the subsystem until an error is encountered or a `conclude` signal is received.
// Most errors are non-fatal and should lead to another call to this function.
//
// A return value of `true` indicates that an exit should be made, while a return value of
// `false` indicates that another iteration should be performed.
async fn run_iteration<Context>(ctx: &mut Context, subsystem: &mut DisputeCoordinatorSubsystem)
	-> Result<bool, Error>
	where Context: SubsystemContext<Message = DisputeCoordinatorMessage>
{
	unimplemented!()
}
