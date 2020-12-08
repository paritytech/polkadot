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

/// The number of sessions to store the information on
/// a particular dispute, this includes open or shut
/// remote or local disputes.
pub const SESSION_COUNT_BEFORE_DROP: u32 = 100;


#[derive(Debug, thiserror::Error)]
enum Error {
	#[error(transparent)]
	Io(#[from] io::Error),

	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),

	#[error(transparent)]
	Subsystem(#[from] SubsystemError),
}


/// Data used to track information of peers and relay parents the
/// overseer ordered us to work on.
#[derive(Default, Clone)]
struct ProtocolState {
	active_leaves_set: HashSet<Hash>,
}


const TARGET: &'static str = "votesdb";


// /// Get a value by key.
// fn get(&self, col: u32, key: &[u8]) -> io::Result<Option<DBValue>>;

// /// Get the first value matching the given prefix.
// fn get_by_prefix(&self, col: u32, prefix: &[u8]) -> Option<Box<[u8]>>;

// /// Write a transaction of changes to the backing store.
// fn write(&self, transaction: DBTransaction) -> io::Result<()>;

// /// Iterate over the data for a given column.
// fn iter<'a>(&'a self, col: u32) -> Box<dyn Iterator<Item = (Box<[u8]>, Box<[u8]>)> + 'a>;

// /// Iterate over the data for a given column, returning all key/value pairs
// /// where the key starts with the given prefix.
// fn iter_with_prefix<'a>(
// 	&'a self,
// 	col: u32,
// 	prefix: &'a [u8],
// ) -> Box<dyn Iterator<Item = (Box<[u8]>, Box<[u8]>)> + 'a>;

// /// Attempt to replace this database with a new one located at the given path.
// fn restore(&self, new_db: &str) -> io::Result<()>;

// /// Query statistics.
// ///
// /// Not all kvdb implementations are able or expected to implement this, so by
// /// default, empty statistics is returned. Also, not all kvdb implementation
// /// can return every statistic or configured to do so (some statistics gathering
// /// may impede the performance and might be off by default).
// fn io_stats(&self, _kind: IoStatsKind) -> IoStats {
// 	IoStats::empty()
// }



fn write_db<D: Encode>(
	db: &Arc<dyn KeyValueDB>,
	column: u32,
	key: &[u8],
	value: D,
) {
	let v = value.encode();
	match db.write(column, v.as_slice()) {
		Ok(None) => None,
		Err(e) => {
			tracing::warn!(target: TARGET, err = ?e, "Error writing to the votes db store");
			None
		}
	}
}

fn read_db<D: Decode>(
	db: &Arc<dyn KeyValueDB>,
	column: u32,
	key: &[u8],
) -> Option<D> {
	match db.get(column, key) {
		Ok(Some(raw)) => {
			let res = D::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed");
			Some(res)
		}
		Ok(None) => None,
		Err(e) => {
			tracing::warn!(target: TARGET, err = ?e, "Error reading from the votes db store");
			None
		}
	}
}


// Storage format prefix: "vote/s_{session_index}/v_{validator_index}"

/// Track up to which point all data was pruned.
const PIVOT_KEY: &[u8] = b"prune/pivot";

#[inline(always)]
fn derive_key(prefix: &str, session: SessionIndex, validator: ValidatorIndex) -> String {
	format!("vote/s_{session_index}/v_{validator_index}", session_index, validator_index=validator)
}
#[inline(always)]
fn derive_prefix(prefix: &str, session: SessionIndex) -> String {
	format!("vote/s_{session_index}", session_index)
}

fn get_pivot(db: &Arc<dyn KeyValueDB>) -> SessionIndex {
	read_db(db, columns::DATA, PIVOT_KEY).unwrap_or_default()
}

fn prune_votes_before_session(db: &Arc<dyn KeyValueDB>, session: SessionIndex) -> Result<()> {
	let pruned_up_to_session_index: SessionIndex = get_pivot(db);

	let mut cleanup_transaction = db.transaction();
	let mut n = 0;
	const MAX_ITEMS_PER_TRANSACTION: u16 = 128_u16;
	for cursor_session in pruned_up_to_session_index..session {
		let prefix = derive_prefix(cursor_session);
		for (key, value) in db.iter_with_prefix(DATA, prefix.as_bytes()) {
			log::trace!("Pruning {}", cursor_session);
			cleanup_transaction.erase(key);

			// use checkpoint submits
			n += 1u16;
			if n > MAX_ITEMS_PRE_TRANSACTION {
				db.write_transaction(cleanup_transaction);

				write_db(db, columns::DATA, PIVOT_KEY, pruned_up_to_session.max(cursor_session - 1));

				cleanup_transaction = db.transaction();
				n = 0;
			}
		}
		// always track our state so we don't ever have to start over
	}

	db.write_transaction(cleanup_transaction);
	write_db(db, columns::DATA, PIVOT_KEY, session - 1);

	Ok(())
}

fn store_votes(db: &Arc<dyn KeyValueDB>, session: SessionIndex, votes: &[Vote]) -> Result<()> {
	if session < get_pivot() {
		log::warn!("Dropping request to store ancient votes.");
		Err(TODO)
	}
	let mut transaction = DBTransaction::with_capacity(votes.len());
	for vote in votes {
		let k = derive_key(session, vote.validator());
		let v = vote.encode();
		transaction.put(columns::DATA, k ,v);

		// TODO detect double votes
	}

	db.write_transaction(transaction)?;

	Ok(())
}


pub async fn on_session_change(current_session: SessionIndex) -> Result<()> {

	// TODO lookup all stored session indices that are less than the curren
	// and drop all associated data
	Ok(())
}




pub async fn store_(current_session: SessionIndex) -> Result<()> {

	Ok(())
}

pub async fn query() -> Result<()> {

}
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
