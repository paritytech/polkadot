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

//! `V1` database for the dispute coordinator.

use polkadot_node_primitives::DisputeStatus;
use polkadot_node_subsystem::{SubsystemError, SubsystemResult};
use polkadot_node_subsystem_util::database::{DBTransaction, Database};
use polkadot_primitives::v2::{
	CandidateHash, CandidateReceipt, Hash, InvalidDisputeStatementKind, SessionIndex,
	ValidDisputeStatementKind, ValidatorIndex, ValidatorSignature,
};

use std::sync::Arc;

use parity_scale_codec::{Decode, Encode};

use crate::{
	backend::{Backend, BackendWriteOp, OverlayedBackend},
	error::{FatalError, FatalResult},
	metrics::Metrics,
	LOG_TARGET,
};

const RECENT_DISPUTES_KEY: &[u8; 15] = b"recent-disputes";
const EARLIEST_SESSION_KEY: &[u8; 16] = b"earliest-session";
const CANDIDATE_VOTES_SUBKEY: &[u8; 15] = b"candidate-votes";
/// Until what session have votes been cleaned up already?
const CLEANED_VOTES_WATERMARK_KEY: &[u8; 23] = b"cleaned-votes-watermark";

/// Restrict number of cleanup operations.
///
/// On the first run we are starting at session 0 going up all the way to the current session -
/// this should not be done at once, but rather in smaller batches so nodes won't get stalled by
/// this.
///
/// 300 is with session duration of 1 hour and 30 parachains around <3_000_000 key purges in the worst
/// case. Which is already quite a lot, at the same time we have around 21_000 sessions on
/// Kusama. This means at 300 purged sessions per session, cleaning everything up will take
/// around 3 days. Depending on how severe disk usage becomes, we might want to bump the batch
/// size, at the cost of risking issues at session boundaries (performance).
#[cfg(test)]
const MAX_CLEAN_BATCH_SIZE: u32 = 10;
#[cfg(not(test))]
const MAX_CLEAN_BATCH_SIZE: u32 = 300;

pub struct DbBackend {
	inner: Arc<dyn Database>,
	config: ColumnConfiguration,
	metrics: Metrics,
}

impl DbBackend {
	pub fn new(db: Arc<dyn Database>, config: ColumnConfiguration, metrics: Metrics) -> Self {
		Self { inner: db, config, metrics }
	}

	/// Cleanup old votes.
	///
	/// Should be called whenever a new earliest session gets written.
	fn add_vote_cleanup_tx(
		&mut self,
		tx: &mut DBTransaction,
		earliest_session: SessionIndex,
	) -> FatalResult<()> {
		// Cleanup old votes in db:
		let watermark = load_cleaned_votes_watermark(&*self.inner, &self.config)?.unwrap_or(0);
		let clean_until = if earliest_session.saturating_sub(watermark) > MAX_CLEAN_BATCH_SIZE {
			watermark + MAX_CLEAN_BATCH_SIZE
		} else {
			earliest_session
		};
		gum::trace!(
			target: LOG_TARGET,
			?watermark,
			?clean_until,
			?earliest_session,
			?MAX_CLEAN_BATCH_SIZE,
			"WriteEarliestSession"
		);

		for index in watermark..clean_until {
			gum::trace!(
			target: LOG_TARGET,
			?index,
			encoded = ?candidate_votes_session_prefix(index),
			"Cleaning votes for session index"
			);
			tx.delete_prefix(self.config.col_dispute_data, &candidate_votes_session_prefix(index));
		}
		// New watermark:
		tx.put_vec(self.config.col_dispute_data, CLEANED_VOTES_WATERMARK_KEY, clean_until.encode());
		Ok(())
	}
}

impl Backend for DbBackend {
	/// Load the earliest session, if any.
	fn load_earliest_session(&self) -> SubsystemResult<Option<SessionIndex>> {
		load_earliest_session(&*self.inner, &self.config)
	}

	/// Load the recent disputes, if any.
	fn load_recent_disputes(&self) -> SubsystemResult<Option<RecentDisputes>> {
		load_recent_disputes(&*self.inner, &self.config)
	}

	/// Load the candidate votes for the specific session-candidate pair, if any.
	fn load_candidate_votes(
		&self,
		session: SessionIndex,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateVotes>> {
		load_candidate_votes(&*self.inner, &self.config, session, candidate_hash)
	}

	/// Atomically writes the list of operations, with later operations taking precedence over
	/// prior.
	///
	/// This also takes care of purging old votes (of obsolete sessions).
	fn write<I>(&mut self, ops: I) -> FatalResult<()>
	where
		I: IntoIterator<Item = BackendWriteOp>,
	{
		let mut tx = DBTransaction::new();
		// Make sure the whole process is timed, including the actual transaction flush:
		let mut cleanup_timer = None;
		for op in ops {
			match op {
				BackendWriteOp::WriteEarliestSession(session) => {
					cleanup_timer = match cleanup_timer.take() {
						None => Some(self.metrics.time_vote_cleanup()),
						Some(t) => Some(t),
					};
					self.add_vote_cleanup_tx(&mut tx, session)?;

					// Actually write the earliest session.
					tx.put_vec(
						self.config.col_dispute_data,
						EARLIEST_SESSION_KEY,
						session.encode(),
					);
				},
				BackendWriteOp::WriteRecentDisputes(recent_disputes) => {
					tx.put_vec(
						self.config.col_dispute_data,
						RECENT_DISPUTES_KEY,
						recent_disputes.encode(),
					);
				},
				BackendWriteOp::WriteCandidateVotes(session, candidate_hash, votes) => {
					gum::trace!(target: LOG_TARGET, ?session, "Writing candidate votes");
					tx.put_vec(
						self.config.col_dispute_data,
						&candidate_votes_key(session, &candidate_hash),
						votes.encode(),
					);
				},
				BackendWriteOp::DeleteCandidateVotes(session, candidate_hash) => {
					tx.delete(
						self.config.col_dispute_data,
						&candidate_votes_key(session, &candidate_hash),
					);
				},
			}
		}

		self.inner.write(tx).map_err(FatalError::DbWriteFailed)
	}
}

fn candidate_votes_key(session: SessionIndex, candidate_hash: &CandidateHash) -> [u8; 15 + 4 + 32] {
	let mut buf = [0u8; 15 + 4 + 32];
	buf[..15].copy_from_slice(CANDIDATE_VOTES_SUBKEY);

	// big-endian encoding is used to ensure lexicographic ordering.
	buf[15..][..4].copy_from_slice(&session.to_be_bytes());
	candidate_hash.using_encoded(|s| buf[(15 + 4)..].copy_from_slice(s));

	buf
}

fn candidate_votes_session_prefix(session: SessionIndex) -> [u8; 15 + 4] {
	let mut buf = [0u8; 15 + 4];
	buf[..15].copy_from_slice(CANDIDATE_VOTES_SUBKEY);

	// big-endian encoding is used to ensure lexicographic ordering.
	buf[15..][..4].copy_from_slice(&session.to_be_bytes());
	buf
}

/// Column configuration information for the DB.
#[derive(Debug, Clone)]
pub struct ColumnConfiguration {
	/// The column in the key-value DB where data is stored.
	pub col_dispute_data: u32,
	/// The column in the key-value DB where session data is stored.
	pub col_session_data: u32,
}

/// Tracked votes on candidates, for the purposes of dispute resolution.
#[derive(Debug, Clone, Encode, Decode)]
pub struct CandidateVotes {
	/// The receipt of the candidate itself.
	pub candidate_receipt: CandidateReceipt,
	/// Votes of validity, sorted by validator index.
	pub valid: Vec<(ValidDisputeStatementKind, ValidatorIndex, ValidatorSignature)>,
	/// Votes of invalidity, sorted by validator index.
	pub invalid: Vec<(InvalidDisputeStatementKind, ValidatorIndex, ValidatorSignature)>,
}

impl From<CandidateVotes> for polkadot_node_primitives::CandidateVotes {
	fn from(db_votes: CandidateVotes) -> polkadot_node_primitives::CandidateVotes {
		polkadot_node_primitives::CandidateVotes {
			candidate_receipt: db_votes.candidate_receipt,
			valid: db_votes.valid.into_iter().map(|(kind, i, sig)| (i, (kind, sig))).collect(),
			invalid: db_votes.invalid.into_iter().map(|(kind, i, sig)| (i, (kind, sig))).collect(),
		}
	}
}

impl From<polkadot_node_primitives::CandidateVotes> for CandidateVotes {
	fn from(primitive_votes: polkadot_node_primitives::CandidateVotes) -> CandidateVotes {
		CandidateVotes {
			candidate_receipt: primitive_votes.candidate_receipt,
			valid: primitive_votes
				.valid
				.into_iter()
				.map(|(i, (kind, sig))| (kind, i, sig))
				.collect(),
			invalid: primitive_votes.invalid.into_iter().map(|(i, (k, sig))| (k, i, sig)).collect(),
		}
	}
}

/// The mapping for recent disputes; any which have not yet been pruned for being ancient.
pub type RecentDisputes = std::collections::BTreeMap<(SessionIndex, CandidateHash), DisputeStatus>;

/// Errors while accessing things from the DB.
#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Io(#[from] std::io::Error),
	#[error(transparent)]
	Codec(#[from] parity_scale_codec::Error),
}

impl From<Error> for crate::error::Error {
	fn from(err: Error) -> Self {
		match err {
			Error::Io(io) => Self::Io(io),
			Error::Codec(e) => Self::Codec(e),
		}
	}
}

/// Result alias for DB errors.
pub type Result<T> = std::result::Result<T, Error>;

fn load_decode<D: Decode>(
	db: &dyn Database,
	col_dispute_data: u32,
	key: &[u8],
) -> Result<Option<D>> {
	match db.get(col_dispute_data, key)? {
		None => Ok(None),
		Some(raw) => D::decode(&mut &raw[..]).map(Some).map_err(Into::into),
	}
}

/// Load the candidate votes for the specific session-candidate pair, if any.
pub(crate) fn load_candidate_votes(
	db: &dyn Database,
	config: &ColumnConfiguration,
	session: SessionIndex,
	candidate_hash: &CandidateHash,
) -> SubsystemResult<Option<CandidateVotes>> {
	load_decode(db, config.col_dispute_data, &candidate_votes_key(session, candidate_hash))
		.map_err(|e| SubsystemError::with_origin("dispute-coordinator", e))
}

/// Load the earliest session, if any.
pub(crate) fn load_earliest_session(
	db: &dyn Database,
	config: &ColumnConfiguration,
) -> SubsystemResult<Option<SessionIndex>> {
	load_decode(db, config.col_dispute_data, EARLIEST_SESSION_KEY)
		.map_err(|e| SubsystemError::with_origin("dispute-coordinator", e))
}

/// Load the recent disputes, if any.
pub(crate) fn load_recent_disputes(
	db: &dyn Database,
	config: &ColumnConfiguration,
) -> SubsystemResult<Option<RecentDisputes>> {
	load_decode(db, config.col_dispute_data, RECENT_DISPUTES_KEY)
		.map_err(|e| SubsystemError::with_origin("dispute-coordinator", e))
}

/// Maybe prune data in the DB based on the provided session index.
///
/// This is intended to be called on every block, and as such will be used to populate the DB on
/// first launch. If the on-disk data does not need to be pruned, only a single storage read
/// will be performed.
///
/// If one or more ancient sessions are pruned, all metadata on candidates within the ancient
/// session will be deleted.
pub(crate) fn note_earliest_session(
	overlay_db: &mut OverlayedBackend<'_, impl Backend>,
	new_earliest_session: SessionIndex,
) -> SubsystemResult<()> {
	match overlay_db.load_earliest_session()? {
		None => {
			// First launch - write new-earliest.
			overlay_db.write_earliest_session(new_earliest_session);
		},
		Some(prev_earliest) if new_earliest_session > prev_earliest => {
			// Prune all data in the outdated sessions.
			overlay_db.write_earliest_session(new_earliest_session);

			// Clear recent disputes metadata.
			{
				let mut recent_disputes = overlay_db.load_recent_disputes()?.unwrap_or_default();

				let lower_bound = (new_earliest_session, CandidateHash(Hash::repeat_byte(0x00)));

				let new_recent_disputes = recent_disputes.split_off(&lower_bound);
				// Any remanining disputes are considered ancient and must be pruned.
				let pruned_disputes = recent_disputes;

				if pruned_disputes.len() != 0 {
					overlay_db.write_recent_disputes(new_recent_disputes);
					// Note: Deleting old candidate votes is handled in `write` based on the earliest session.
				}
			}
		},
		Some(_) => {
			// nothing to do.
		},
	}

	Ok(())
}

/// Until what session votes have been cleaned up already.
///
/// That is the db has already been purged of votes for sessions older than the returned
/// `SessionIndex`.
fn load_cleaned_votes_watermark(
	db: &dyn Database,
	config: &ColumnConfiguration,
) -> FatalResult<Option<SessionIndex>> {
	load_decode(db, config.col_dispute_data, CLEANED_VOTES_WATERMARK_KEY)
		.map_err(|e| FatalError::DbReadFailed(e))
}

#[cfg(test)]
mod tests {

	use super::*;
	use ::test_helpers::{dummy_candidate_receipt, dummy_hash};
	use polkadot_node_primitives::DISPUTE_WINDOW;
	use polkadot_primitives::v2::{Hash, Id as ParaId};

	fn make_db() -> DbBackend {
		let db = kvdb_memorydb::create(1);
		let db = polkadot_node_subsystem_util::database::kvdb_impl::DbAdapter::new(db, &[0]);
		let store = Arc::new(db);
		let config = ColumnConfiguration { col_dispute_data: 0, col_session_data: 1 };
		DbBackend::new(store, config, Metrics::default())
	}

	#[test]
	fn max_clean_batch_size_is_honored() {
		let mut backend = make_db();

		let mut overlay_db = OverlayedBackend::new(&backend);
		let current_session = MAX_CLEAN_BATCH_SIZE + DISPUTE_WINDOW.get() + 3;
		let earliest_session = current_session - DISPUTE_WINDOW.get();

		overlay_db.write_earliest_session(0);
		let candidate_hash = CandidateHash(Hash::repeat_byte(1));

		for session in 0..current_session + 1 {
			overlay_db.write_candidate_votes(
				session,
				candidate_hash,
				CandidateVotes {
					candidate_receipt: dummy_candidate_receipt(dummy_hash()),
					valid: Vec::new(),
					invalid: Vec::new(),
				},
			);
		}
		assert!(overlay_db.load_candidate_votes(0, &candidate_hash).unwrap().is_some());
		assert!(overlay_db
			.load_candidate_votes(MAX_CLEAN_BATCH_SIZE - 1, &candidate_hash)
			.unwrap()
			.is_some());
		assert!(overlay_db
			.load_candidate_votes(MAX_CLEAN_BATCH_SIZE, &candidate_hash)
			.unwrap()
			.is_some());

		// Cleanup only works for votes that have been written already - so write.
		let write_ops = overlay_db.into_write_ops();
		backend.write(write_ops).unwrap();

		let mut overlay_db = OverlayedBackend::new(&backend);

		gum::trace!(target: LOG_TARGET, ?current_session, "Noting current session");
		note_earliest_session(&mut overlay_db, earliest_session).unwrap();

		let write_ops = overlay_db.into_write_ops();
		backend.write(write_ops).unwrap();

		let mut overlay_db = OverlayedBackend::new(&backend);

		assert!(overlay_db
			.load_candidate_votes(MAX_CLEAN_BATCH_SIZE - 1, &candidate_hash)
			.unwrap()
			.is_none());
		// After batch size votes should still be there:
		assert!(overlay_db
			.load_candidate_votes(MAX_CLEAN_BATCH_SIZE, &candidate_hash)
			.unwrap()
			.is_some());

		let current_session = current_session + 1;
		let earliest_session = earliest_session + 1;

		note_earliest_session(&mut overlay_db, earliest_session).unwrap();

		let write_ops = overlay_db.into_write_ops();
		backend.write(write_ops).unwrap();

		let overlay_db = OverlayedBackend::new(&backend);

		// All should be gone now:
		assert!(overlay_db
			.load_candidate_votes(earliest_session - 1, &candidate_hash)
			.unwrap()
			.is_none());
		// Earliest session should still be there:
		assert!(overlay_db
			.load_candidate_votes(earliest_session, &candidate_hash)
			.unwrap()
			.is_some());
		// Old current session should still be there as well:
		assert!(overlay_db
			.load_candidate_votes(current_session - 1, &candidate_hash)
			.unwrap()
			.is_some());
	}

	#[test]
	fn overlay_pre_and_post_commit_consistency() {
		let mut backend = make_db();

		let mut overlay_db = OverlayedBackend::new(&backend);

		overlay_db.write_earliest_session(0);
		overlay_db.write_earliest_session(1);

		overlay_db.write_recent_disputes(
			vec![((0, CandidateHash(Hash::repeat_byte(0))), DisputeStatus::Active)]
				.into_iter()
				.collect(),
		);

		overlay_db.write_recent_disputes(
			vec![((1, CandidateHash(Hash::repeat_byte(1))), DisputeStatus::Active)]
				.into_iter()
				.collect(),
		);

		overlay_db.write_candidate_votes(
			1,
			CandidateHash(Hash::repeat_byte(1)),
			CandidateVotes {
				candidate_receipt: dummy_candidate_receipt(dummy_hash()),
				valid: Vec::new(),
				invalid: Vec::new(),
			},
		);
		overlay_db.write_candidate_votes(
			1,
			CandidateHash(Hash::repeat_byte(1)),
			CandidateVotes {
				candidate_receipt: {
					let mut receipt = dummy_candidate_receipt(dummy_hash());
					receipt.descriptor.para_id = ParaId::from(5_u32);

					receipt
				},
				valid: Vec::new(),
				invalid: Vec::new(),
			},
		);

		// Test that overlay returns the correct values before committing.
		assert_eq!(overlay_db.load_earliest_session().unwrap().unwrap(), 1);

		assert_eq!(
			overlay_db.load_recent_disputes().unwrap().unwrap(),
			vec![((1, CandidateHash(Hash::repeat_byte(1))), DisputeStatus::Active),]
				.into_iter()
				.collect()
		);

		assert_eq!(
			overlay_db
				.load_candidate_votes(1, &CandidateHash(Hash::repeat_byte(1)))
				.unwrap()
				.unwrap()
				.candidate_receipt
				.descriptor
				.para_id,
			ParaId::from(5),
		);

		let write_ops = overlay_db.into_write_ops();
		backend.write(write_ops).unwrap();

		// Test that subsequent writes were written.
		assert_eq!(backend.load_earliest_session().unwrap().unwrap(), 1);

		assert_eq!(
			backend.load_recent_disputes().unwrap().unwrap(),
			vec![((1, CandidateHash(Hash::repeat_byte(1))), DisputeStatus::Active),]
				.into_iter()
				.collect()
		);

		assert_eq!(
			backend
				.load_candidate_votes(1, &CandidateHash(Hash::repeat_byte(1)))
				.unwrap()
				.unwrap()
				.candidate_receipt
				.descriptor
				.para_id,
			ParaId::from(5),
		);
	}

	#[test]
	fn overlay_preserves_candidate_votes_operation_order() {
		let mut backend = make_db();

		let mut overlay_db = OverlayedBackend::new(&backend);

		overlay_db.write_candidate_votes(
			1,
			CandidateHash(Hash::repeat_byte(1)),
			CandidateVotes {
				candidate_receipt: dummy_candidate_receipt(Hash::random()),
				valid: Vec::new(),
				invalid: Vec::new(),
			},
		);

		let receipt = dummy_candidate_receipt(dummy_hash());

		overlay_db.write_candidate_votes(
			1,
			CandidateHash(Hash::repeat_byte(1)),
			CandidateVotes {
				candidate_receipt: receipt.clone(),
				valid: Vec::new(),
				invalid: Vec::new(),
			},
		);

		let write_ops = overlay_db.into_write_ops();
		backend.write(write_ops).unwrap();

		assert_eq!(
			backend
				.load_candidate_votes(1, &CandidateHash(Hash::repeat_byte(1)))
				.unwrap()
				.unwrap()
				.candidate_receipt,
			receipt,
		);
	}

	#[test]
	fn note_earliest_session_prunes_old() {
		let mut backend = make_db();

		let hash_a = CandidateHash(Hash::repeat_byte(0x0a));
		let hash_b = CandidateHash(Hash::repeat_byte(0x0b));
		let hash_c = CandidateHash(Hash::repeat_byte(0x0c));
		let hash_d = CandidateHash(Hash::repeat_byte(0x0d));

		let prev_earliest_session = 0;
		let new_earliest_session = 5;
		let current_session = 5 + DISPUTE_WINDOW.get();

		let super_old_no_dispute = 1;
		let very_old = 3;
		let slightly_old = 4;
		let very_recent = current_session - 1;

		let blank_candidate_votes = || CandidateVotes {
			candidate_receipt: dummy_candidate_receipt(dummy_hash()),
			valid: Vec::new(),
			invalid: Vec::new(),
		};

		let mut overlay_db = OverlayedBackend::new(&backend);
		overlay_db.write_earliest_session(prev_earliest_session);
		overlay_db.write_recent_disputes(
			vec![
				((very_old, hash_a), DisputeStatus::Active),
				((slightly_old, hash_b), DisputeStatus::Active),
				((new_earliest_session, hash_c), DisputeStatus::Active),
				((very_recent, hash_d), DisputeStatus::Active),
			]
			.into_iter()
			.collect(),
		);

		overlay_db.write_candidate_votes(super_old_no_dispute, hash_a, blank_candidate_votes());
		overlay_db.write_candidate_votes(very_old, hash_a, blank_candidate_votes());

		overlay_db.write_candidate_votes(slightly_old, hash_b, blank_candidate_votes());

		overlay_db.write_candidate_votes(new_earliest_session, hash_c, blank_candidate_votes());

		overlay_db.write_candidate_votes(very_recent, hash_d, blank_candidate_votes());

		let write_ops = overlay_db.into_write_ops();
		backend.write(write_ops).unwrap();

		let mut overlay_db = OverlayedBackend::new(&backend);
		note_earliest_session(&mut overlay_db, new_earliest_session).unwrap();

		assert_eq!(overlay_db.load_earliest_session().unwrap(), Some(new_earliest_session));

		assert_eq!(
			overlay_db.load_recent_disputes().unwrap().unwrap(),
			vec![
				((new_earliest_session, hash_c), DisputeStatus::Active),
				((very_recent, hash_d), DisputeStatus::Active),
			]
			.into_iter()
			.collect(),
		);

		// Votes are only cleaned up after actual write:
		let write_ops = overlay_db.into_write_ops();
		backend.write(write_ops).unwrap();

		let overlay_db = OverlayedBackend::new(&backend);

		assert!(overlay_db
			.load_candidate_votes(super_old_no_dispute, &hash_a)
			.unwrap()
			.is_none());
		assert!(overlay_db.load_candidate_votes(very_old, &hash_a).unwrap().is_none());
		assert!(overlay_db.load_candidate_votes(slightly_old, &hash_b).unwrap().is_none());
		assert!(overlay_db
			.load_candidate_votes(new_earliest_session, &hash_c)
			.unwrap()
			.is_some());
		assert!(overlay_db.load_candidate_votes(very_recent, &hash_d).unwrap().is_some());
	}
}
