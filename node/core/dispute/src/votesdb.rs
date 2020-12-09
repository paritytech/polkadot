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

//! The VotesDB
//!
//! A private storage to track votes, from backing or secondary checking or explicit dispute
//! votes and derive `VoteEvent`s from it.

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
const SESSION_COUNT_BEFORE_DROP: u32 = 100;

/// Number of transactions to batch up.
const MAX_ITEMS_PER_DB_TRANSACTION: u16 = 1024_u16;

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
enum Error {
	#[error(transparent)]
	Io(#[from] io::Error),

	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),

	#[error(transparent)]
	Subsystem(#[from] SubsystemError),

	#[error("Attempted to store an obsolete vote")]
	ObsoleteVote,
}


/// Data used to track information of peers and relay parents the
/// overseer ordered us to work on.
#[derive(Default, Clone)]
struct ProtocolState {
	active_leaves_set: HashSet<Hash>,
}


const TARGET: &'static str = "votesdb";

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
const OLDEST_SESSION_SLOT_ENTRY: &[u8] = b"prune/pivot";

#[inline(always)]
fn derive_key(prefix: &str, session: SessionIndex, validator: ValidatorIndex) -> String {
	format!("vote/s_{session_index}/v_{validator_index}", session_index, validator_index=validator)
}
#[inline(always)]
fn derive_prefix(prefix: &str, session: SessionIndex) -> String {
	format!("vote/s_{session_index}", session_index)
}


/// Returns the oldest session index for which entries are not pruned yet.
fn get_oldest_session(db: &Arc<dyn KeyValueDB>) -> SessionIndex {
	read_db(db, columns::DATA, OLDEST_SESSION_SLOT_ENTRY).unwrap_or_default()
}

/// Update the oldest stored session index index entry.
fn update_oldest_session(db: &Arc<dyn KeyValueDB>, current_oldest: SessionIndex, new_oldest: SessionIndex) -> SessionIndex {
	let new_oldest = current_oldest.max(new_oldest.saturating_sub(1));
	write_db(db, columns::DATA, OLDEST_SESSION_SLOT_ENTRY, new_oldest);
	new_oldest
}

/// Remove all votes that we stored in the db that are related to any
/// session index before the provided `session`.
fn prune_votes_older_than_session(db: &Arc<dyn KeyValueDB>, session: SessionIndex) -> Result<()> {
	let mut oldest_session: SessionIndex = oldest_session(db);
	if oldest_session >= session {
		return Ok(())
	}

	let mut cleanup_transaction = db.transaction();
	let mut n = 0;

	for cursor_session in oldest_session..session {

		let prefix = derive_prefix(cursor_session);
		for (key, value) in db.iter_with_prefix(DATA, prefix.as_bytes()) {
			log::trace!("Pruning {}", cursor_session);
			cleanup_transaction.erase(key);

			// use checkpoint submits
			n += 1_u16;
			if n > MAX_ITEMS_PER_DB_TRANSACTION {
				db.write_transaction(cleanup_transaction);

				oldest_session = update_oldest_session(db, oldest_session, session_cursor);

				cleanup_transaction = db.transaction();
				n = 0_u16;
			}
		}
	}

	db.write_transaction(cleanup_transaction);
	let _ = update_oldest_session(db, oldest_session, session);

	Ok(())
}


/// A vote cast by another validator.
#[derive(Debug, Clone, Encode, Decode, Eq, PartialEq)]
enum Vote {
	Backing { candidate: BackedCandidate },
	ApprovalCheck { sfs: SignedFullStatement },
	DisputePositive { sfs: SignedFullStatement },
	DisputeNegative { sfs: SignedFullStatement },
}

impl Vote {
	/// Determines if the vote is a vote that supports the validity of this block.
	pub fn positive(&self) -> bool {
		match self {
			Self::Backing { .. } => true,
			Self::ApprovalCheck { .. } => true,
			Self::DisputePositive { .. } => true,
			Self::DisputeNegative { .. } => false,
		}
	}

	/// A vote that challenges the validity of a candidate.
	#[inline(always)]
	pub fn negative(&self) -> bool {
		!self.positive()
	}

	pub fn backed_candidate(&self) -> &BackedCandidate {
		match self {
			Self::Backing { backed_candidate } => backed_candidate,
			Self::ApprovalCheck { sfs } => sfs.validator_index,
			Self::DisputePositive { sfs } => sfs.validator_index,
			Self::DisputeNegative { sfs } => sfs.validator_index,
		}
	}

	/// Obtain the vote's validator indices.
	pub fn validators(&self) -> Vec<ValidatorIndex> {
		self.backed_candidate().validator_indices.iter().map(|validator_index| {
			*validator_index as ValidatorIndex
		}).collect()
	}
	
	pub fn candidate_hash(&self) -> CandidateHash {
		
	}
}

#[derive(Debug, Clone, Copy)]
enum CandidateQuorum {
	/// The backed candidate is deemed valid.
	Valid,
	/// Invalid candidate block.
	Invalid,
}

/// Output of the vote store action.
#[derive(Debug, Clone)]
enum VoteEvent {
	Stored,
	/// This is the first set of votes that was stored for this dispute
	DisputeDetected {
		candidate: CandidateHash,
		votes: Vec<Vote>,
	},
	/// A validator tried to vote twice
	DoubleVote {
		candidate: CandidateHash,
		validator: ValidatorId,
	},
	/// Either side of the votes has reached a super majority
	SupermajorityReached{
		quorum: CandidateQuorumResult,
	},
	/// Discard an obsolete vote
	ObsoleteVoteDiscarded {
		candidate: CandidateHash,
	},
}

fn check_for_supermajority(db: &Arc<dyn KeyValueDB>, session: SessionIndex) -> Option<CandidateQuorum> {
	
	
}

fn store_votes(db: &Arc<dyn KeyValueDB>, session: SessionIndex, votes: &[Vote]) -> Result<Vec<VoteEvent>>> {
	if session < get_pivot() {
		log::warn!("Dropping request to store ancient votes.");
		return Err(Error::ObsoleteVote)
	}
	let mut transaction = DBTransaction::with_capacity(votes.len());
	let events: Vec<VoteEvent> = votes.into_iter().map(|vote| {
		for vote in votes {
		}
		let k = derive_key(session, vote.validator());

		if let Some(previous_vote) = read_db(db, columns::DATA, k) {
			let previous_vote: Vote = previous_vote.decode()
				.expect("Database entries are all created from this module and thus must decode. qed");

			if previous_vote != vote {
				unimplemented!("Derive a set of votes")
				// VoteEvent::DoubleVote {
				// 	validator: vote.validator(),
				// 	votes: vec![previous_vote, vote],
				// }
				
				// TODO clarify if a double vote means two opposing votes (pro and con)
				// TODO or also two different vote kinds where both are positive
			} else {
				// if the votes are equivalent, just avoid the transaction element
				VoteEvent::Success
			}
		} else {
			let v = vote.encode();
			transaction.put(columns::DATA, k ,v);
			if supermajority_reached {
				VoteEvent::SupermajorityReached
			} else {
				VoteEvent::Stored
			}
		}
	}).collect();

	db.write_transaction(transaction)?;

	Ok(events)
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
