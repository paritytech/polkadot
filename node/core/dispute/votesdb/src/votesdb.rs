// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.
//
// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! VotesDB
//!
//! A private storage to track votes, from backing or secondary checking or explicit dispute
//! votes and derive `VoteEvent`s from it.
//!
//! Storage layout within the kv db is as follows:
//!
//! Tracks the waterlevel up to which session index pruning was already completed.
//! ```text
//! vote/prune/waterlevel
//! ```
//!
//! Tracks all validators that voted for a particular candidate.
//! The actual `Vote` is stored there.
//! ```text
//! vote/s_{session_index}/c_{candidate_hash}/v_{validator_index}
//! ```
//!
//! If the path exists the validator voted for that particular candidate.
//! Stores an `Option<()>` as a marker, should never have a `Some(())` value.
//! ```text
//! vote/s_{session_index}/v_{validator_index}/c_{candidate_hash}
//! ```
//!
//! Common prefixes based on the session allows for fast and pain free deletion.
//!
//!

// TODO prevent repeated sending of dispute detections or resolutions

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

	#[error("Sending QueryValidators query failed")]
	QueryValidatorsSendQuery(#[source] SubsystemError),

	#[error("Response channel to obtain QueryValidators failed")]
	QueryValidatorsResponseChannel(#[source] oneshot::Canceled),

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
	if let Err(e) = db.write(column, v.as_slice()) {
		tracing::warn!(target: TARGET, err = ?e, "Error writing to the votes db store");
		None
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

///////////

/// Track up to which point all data was pruned.
const OLDEST_SESSION_SLOT_ENTRY: &[u8] = b"vote/prune/waterlevel";

/// Obtain the vote for a particular validator for a particular session and candidate hash.
#[inline(always)]
fn derive_key_per_hash(session: SessionIndex, candidate_hash: CandidateHash) -> String {
	format!(
		"vote/s_{session_index}/c_{candidate_hash}",
		session_index = session,
		candidate_hash = candidate_hash,
		validator_index = validator
	)
}

/// A prefix with keys per validator.
#[inline(always)]
fn derive_key_per_validator(session: SessionIndex, validator: ValidatorIndex) -> String {
	format!(
		"vote/s_{session_index}/v_{validator_index}",
		session_index = session,
		candidate_hash = candidate_hash,
		validator_index = validator
	)
}

/// Derive the prefix key for pruning.
#[inline(always)]
fn derive_prune_prefix(session: SessionIndex) -> String {
	format!(
		"vote/s_{session_index}",
		session_index = session,
	)
}

/// Derive the prefix key for collecting all votes of a particular validator
/// Contains a `Vec<(SessionIndex, ValidatorIndex, Vote)>` as value.
#[inline(always)]
fn derive_disputes_per_validator_prefix(validator: ValidatorId) -> String {
	format!(
		"validator/{validator}/participations",
		validator = validator
	)
}

////////////////////////////////////////////////////////////////////////////////////////

/// Returns the oldest session index for which entries are not pruned yet.
fn oldest_session_waterlevel(db: &Arc<dyn KeyValueDB>) -> SessionIndex {
	read_db(db, columns::DATA, OLDEST_SESSION_SLOT_ENTRY).unwrap_or_default()
}

/// Update the oldest stored session index index entry.
fn update_oldest_session_waterlevel(db: &Arc<dyn KeyValueDB>, current_oldest: SessionIndex, new_oldest: SessionIndex) -> SessionIndex {
	let new_oldest = current_oldest.max(new_oldest.saturating_sub(1));
	write_db(db, columns::DATA, OLDEST_SESSION_SLOT_ENTRY, new_oldest);
	new_oldest
}

/// Remove all votes that we stored in the db that are related to any
/// session index before the provided `session`.
fn prune_votes_older_than_session(db: &Arc<dyn KeyValueDB>, session: SessionIndex) -> Result<()> {
	let mut oldest_session: SessionIndex = oldest_session_waterlevel(db);
	if oldest_session >= session {
		return Ok(())
	}

	let mut cleanup_transaction = db.transaction();
	let mut n = 0;

	for cursor_session in oldest_session..session {

		let prefix = derive_prune_prefix(cursor_session);
		for (key, value) in db.iter_with_prefix(DATA, prefix.as_bytes()) {
			log::trace!("Pruning {}", cursor_session);
			cleanup_transaction.erase(key);

			// use checkpoint submits
			n += 1_u16;
			if n > MAX_ITEMS_PER_DB_TRANSACTION {
				db.write_transaction(cleanup_transaction);

				oldest_session = update_oldest_session_waterlevel(db, oldest_session, session_cursor);

				cleanup_transaction = db.transaction();
				n = 0_u16;
			}
		}
	}

	db.write_transaction(cleanup_transaction);
	let _ = update_oldest_session_waterlevel(db, oldest_session, session);

	Ok(())
}


/// Resolve validator to the set of indices and validator ids the validator was active
async fn resolve_validator(db: &Arc<dyn KeyValueDB>, validator: ValidatorId) -> Option<Vec<(SessionIndex,ValidatorIndex)>> {
	read_db(db, columns::DATA,derive_disputes_per_validator_prefix(validator)).map(|v| {
		v.into_iter().map(|(session, validator, _vote)| { (session, validator)}).collect()
	})
}

/// Resolve a validator from a defined set to it's public key.
async fn resolve_validator_reverse(validator: (SessionIndex,ValidatorIndex)) -> Option<ValidatorId> {
	unimplemented!("resolve (SessionIndex,ValidatorIndex) -> Option<ValidatorId>")
}

/// Helper.
async fn resolve_resolve(validator: (SessionIndex,ValidatorIndex)) -> Option<Vec<(SessionIndex,ValidatorIndex)>> {
	let validator_pub = resolve_validator_reverse(validator)?;
	resolve_validator(validator_pub)
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
	/// No particularly interesting event, successfully stored the vote.
	Stored,
	/// This is the first set of votes that was stored for this dispute
	DisputeDetected {
		candidate: CandidateHash,
		votes: Vec<Vote>,
	},
	/// A validator tried to vote twice
	DoubleVote {
		/// The candidate in question.
		candidate: CandidateHash,
		/// In which session the candidate was included.
		session: SessionIndex,
		/// The validator that created the inconsistent votes.
		validator: ValidatorIndex,
		/// All votes cast by this validator.
		votes: Vec<Vote>,
	},
	/// Either side of the votes has reached a super majority
	SupermajorityReached {
		quorum: CandidateQuorum,
	},
	/// Discard an obsolete vote
	ObsoleteVoteDiscarded {
		candidate: CandidateHash,
	},
}

/// Query the runtime for the validator set for a particular relay parent.
// TODO make this based on the session index.
async fn query_validator_set<Context>(
	ctx: &mut Context,
	hash: Hash,
) -> Result<Vec<ValidatorId>>
where
	Context: SubsystemContext<Message = VotesDbMessage>,
{
	let (tx, rx) = oneshot::channel();
	let query_validators = AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		hash,
		RuntimeApiRequest::Session (tx),
	));

	ctx.send_message(query_validators)
		.await
		.map_err(|e| Error::QueryValidatorsSendQuery(e))?;
	rx.await
		.map_err(|e| Error::QueryValidatorsResponseChannel(e))?
		.map_err(|e| Error::QueryValidators(e))
}

/// Assumes double votes are already accounted for and are NOT present in the
/// count of `pro` and `con`.
fn check_supermajority(validator_count: usize, pro: usize, con: usize) -> Option<CandidateQuorum> {
	if validator_count == 0 {
		log::warn!(target: TARGET, "Encountered a validator set with 0 validators");
		return None
	}
	if validator_count < pro + con {
		log::warn!(target: TARGET, "More votes cast than validators in validator set");
		return None
	}

	let threshold = validator_count - validator_count / 3;

	let pro_wins = pro_votes >= threshold;
	let con_wins = con_votes >= threshold;

	if pro_wins && con_wins {
		unreachable!("In no circumstance can opposing parties achieve a supermajority");
	} else if pro_wins {
		Some(CandidateQuorum::Valid)
	} else if con_wins {
		Some(CandidateQuorum::Invalid)
	} else {
		None
	}
}

/// Obtain all votes for a particular dispute indentified
/// by session and candidate hash.
fn lookup_all_votes_for_dispute(
	db: &Arc<dyn KeyValueDB>,
	session: SessionIndex,
	candidate_hash: CandidateHash,
) -> Result<Vec<Vote>> {
	let prefix = derive_key_per_hash(sesssion, candidate_hash);
	let cast_votes = db.iter_with_prefix(columns::DATA, prefix.as_bytes())
		.filter_map(|(_key, value)| {
			let v: Vote = v.decode().ok()?;
			Some(v)
		}).collect::<Vec<Vote>>();
	Ok(cast_votes)
}

/// Check if for a particular candidate a supermajority was reached.
async fn supermajority_reached<Context>(
	ctx: &mut Context,
	db: &Arc<dyn KeyValueDB>,
	session: SessionIndex,
	candidate_hash: CandidateHash
) -> Result<Option<CandidateQuorum>>
where
	Context: SubsystemContext<Message = VotesDBMessage>,
{
	debug_assert!(session >= oldest_session_waterlevel(session));

	// TODO consider lazily cashing the validator set
	let validators = query_validator_set(ctx, session).await?;

	let n = validators.len();

	let votes: Vec<Vote> = lookup_all_votes_for_dispute(db, session, candidate_hash)?;

	let mut pro = 0_u64;
	let mut con = 0_u64;
	for vote in votes {
		if vote.is_negative() {
			con += 1_u64;
		} else {
			pro += 1_u64;
		}
	}

	Ok(check_supermajority(n, pro, con))
}

/// returns a map of  `VoteEvents` which can either be `DoubleVote`, `Stored`,
fn store_votes_inner<Context>(
	ctx: &mut Context,
	db: &Arc<dyn KeyValueDB>,
	votes: &[Vote],
) -> Result<HashMap<(CandidateHash, SessionIndex, ValidatorIndex), VoteEvent>>
where
	Context: SubsystemContext<Message = VotesDBMessage>,
{
	let oldest_allowed = get_pivot();

	let mut transaction = DBTransaction::with_capacity(votes.len());
	// events per input vote
	let events: Vec<VoteEvent> = votes.into_iter()
		.filter_map(|vote| {
			if vote.session() < oldest_allowed {
				log::warn!("Dropping request to store ancient votes.");
				return Err(Error::ObsoleteVote)
			}

			let k = derive_key(session, vote.validator());

			let event = if let Some(previous_vote) = read_db(db, columns::DATA, k) {
				let previous_vote: Vote = previous_vote.decode()
					.expect("Database entries are all created from this module and thus must decode. qed");

				if previous_vote != vote {
					VoteEvent::DoubleVote {
						candidate: vote.candidate_hash(),
						session: vote.session(),
						validator: vote.validator(),
						votes: vec![previous_vote, vote],
					}

					// TODO clarify if a double vote means two opposing votes (pro and con)
					// TODO or also two different vote kinds where both are positive
				} else {
					// if the votes are equivalent, just avoid the transaction element
					VoteEvent::Success
				}
			} else {
				let v = vote.encode();
				transaction.put(columns::DATA, k ,v);
				VoteEvent::Success
			};
			Some(event)
	}).collect();

	db.write_transaction(transaction)?;

	Ok(events)
}


/// Checks a set of given candidats whether the dispute
/// already reached the required votes for a supermajority.
///
/// All the disputes that reached the required quorum, will be
/// represented in the returned `HashMap`.
pub async fn check_for_supermajority<Context>(
	ctx: Context,
	db: &Arc<dyn KeyValueDB>,
	votes: &[Vote],
) -> Result<HashMap<(SessionIndex, CandidateHash), CandidateQuorum>>
where
	Context: SubsystemContext<Message = VotesDBMessage>,
{
	let mut acc = HashSet::with_capacity(votes.len());
	for (session, candidate_hash) in votes.into_iter().map(|vote| (vote.session(), vote.candidate_hash())) {
		if let Some(quorum) = supermajority_reached(ctx, db, session, candidate_hash).await {
			let _ = acc.insert((session, candidate_hash), quorum);
		}
	}
	Ok(acc)
}


/// Obtain a list of disput votes cast in which the validator participated.
///
/// Note: Due to the storage limitation this limited to `SESSION_COUNT_BEFORE_DROP` sessions.
pub fn find_all_disputes_validator_participated(
	db: &Arc<dyn KeyValueDB>,
	session: SessionIndex,
	validator: ValidatorIndex
) -> Result<Vec<Vote>>
where
	Context: SubsystemContext<Message = VotesDBMessage>,
{
	let validator_pub = unimplemented!("lookup ValidatorId by session and validator index");
	let prefix = derive_disputes_per_validator_prefix(validator_pub);
	let participated_disputes = db.iter_with_prefix(columns::DATA, prefix.as_bytes())
		.filter_map(|(_key, value)| {
			let v: Vote = v.decode().ok()?;
			Some(v)
		}).collect::<Vec<Vote>>();
	Ok(participated_disputes)
}

/// The votes db subsystem.
pub struct VotesDB {
	metrics: Metrics,
	inner: Arc<dyn KeyValueDB>,
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

	pub async fn on_session_change(&mut self) -> Result<()> {
		let current_session = unimplemented!("current session index")
			.saturating_sub(SESSION_COUNT_BEFORE_DROP);

		prune_votes_older_than_session(&self.inner, current_session);
		Ok(())
	}

	/// Store a vote into the persistent DB
	pub async fn store_vote<Context>(ctx: Context, vote: impl Into<Vec<Vote>>) -> Result<()>
	where
		Context: SubsystemContext<Message = VotesDBMessage>,
	{
		let votes: Vec<Vote> = votes.into();

		let votes = store_votes_inner(&self.inner, votes.as_slice())?;

		let reached_quorums = check_for_supermajority::<Context>(ctx, db, votes.values()).await?;

		let attestation = unimplemented!("attestion retriveal missing");
		let response = unimplemented!("attestion retriveal missing");
		for reached_quorum in reached_quorums {
			ctx.send_message(AllMessages::DisputeParticipation(
				DisputeParticipationMessage::Detection {
					candidate,
					relay_parent,
					session,
				}
			)).await
		}
		Ok(())
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
					if let Err(e) = query(db, validator).await {
						log::warn!(target: TARGET, "Failed to query disputes validator {} pariticpated", validator)
					}
				}

				FromOverseer::Communication {
					msg: VotesDBMessage::StoreVote { vote },
				} => {
					if let Err(e) = store_vote(ctx, db, vote).await {
						log::warn!(target: TARGET, "Failed to store disputes vote pariticpated")
					}
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(hash)) => {
					// TODO investigate if cleanup depends on finalization or not
				}
				FromOverseer::Signal(OverseerSignal::Conclude) => {
					trace!(target: TARGET, "Conclude");
					return Ok(());
				}
				FromOverseer::Signal(_) => {}
			}
		}
	}
}

#[cfg(test)]
mod tests;
