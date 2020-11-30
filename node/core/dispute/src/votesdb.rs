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

//! The bitfield distribution
//!
//! In case this node is a validator, gossips its own signed availability bitfield
//! for a particular relay parent.
//! Independently of that, gossips on received messages from peers to other interested peers.

use parity_scale_codec::{Decode, Encode};
use futures::{channel::oneshot, FutureExt};

use log::{trace, warn};
use polkadot_subsystem::messages::*;
use polkadot_subsystem::{
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemResult,
};
use polkadot_node_subsystem_util::{
	metrics::{self, prometheus},
};
use polkadot_primitives::v1::{Hash, SignedAvailabilityBitfield, SigningContext, ValidatorId};
use polkadot_node_network_protocol::{v1 as protocol_v1, PeerId, NetworkBridgeEvent, View, ReputationChange};
use std::collections::{HashMap, HashSet};
use futures::{select, channel::oneshot, FutureExt};
use kvdb_rocksdb::{Database, DatabaseConfig};
use kvdb::{KeyValueDB, DBTransaction};


mod columns {
	pub const DATA: u32 = 0;
	pub const NUM_COLUMNS: u32 = 1;
}


#[derive(Debug, thiserror::Error)]
enum Error {
	Io(#[from] io::Error),	
	Oneshot(#[from] oneshot::Canceled),
	Subsystem(#[from] SubsystemError),
}


/// Data used to track information of peers and relay parents the
/// overseer ordered us to work on.
#[derive(Default, Clone)]
struct ProtocolState {
	active_leaves_set: HashSet<Hash>,
}


const TARGET: &'static str = "vodb";

/// The bitfield distribution subsystem.
pub struct VotesDB {
	metrics: Metrics,
}

impl VotesDB {
	/// Create a new instance of the `VotesDB` subsystem.
	pub fn new_on_disk(config: Config, metrics: Metrics) -> io::Result<Self> {
		let mut db_config = DatabaseConfig::with_columns(columns::NUM_COLUMNS);

		let path = config.path.to_str().ok_or_else(|| io::Error::new(
			io::ErrorKind::Other,
			format!("Bad database path: {:?}", config.path),
		))?;

		let db = Database::open(&db_config, &path)?;

		Ok(Self {
			inner: Arc::new(db),
			metrics,
		})
    }

	#[cfg(test)]
	fn new_in_memory(inner: Arc<dyn KeyValueDB>, metrics: Metrics) -> Self {
		Self {
			inner,
			metrics,
		}
	}

	/// Start processing work as passed on from the Overseer.
	async fn run<Context>(self, mut ctx: Context) -> SubsystemResult<()>
	where
		Context: SubsystemContext<Message = VotesDBMessage>,
	{
		// work: process incoming messages from the overseer and process accordingly.
		let mut state = ProtocolState::default();
		loop {
			let message = ctx.recv().await?;
			match message {
				FromOverseer::Communication {
					msg: VotesDBMessage::Query (session, validator),
				} => {
                    trace!(target: TARGET, "Query ");
						// .await?;
				}
				
				FromOverseer::Communication {
					msg: VotesDBMessage::RegisterVote{ gossip_msg },
				} => {
                    trace!(target: TARGET, "Query v2");
                    // .await?;
				}
				FromOverseer::Signal(
					OverseerSignal::ActiveLeaves(
						ActiveLeavesUpdate { activated, deactivated })) => {
					for relay_parent in deactivated {
						trace!(target: TARGET, "Stop {:?}", relay_parent);
						state.active_leaves_set.remove(relay_parent)
					}
					for relay_parent in activated {
						trace!(target: TARGET, "Start {:?}", relay_parent);

						state.active_leaves_set.insert(relay_parent)
					}
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(hash)) => {
					trace!(target: TARGET, "Block finalized {:?}", hash);
				}
				FromOverseer::Signal(OverseerSignal::Conclude) => {
					trace!(target: TARGET, "Conclude");
					return Ok(());
				}
			}
		}
	}
}

#[cfg(test)]
mod tests;
