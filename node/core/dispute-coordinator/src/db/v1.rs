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

//! V1 database for the dispute coordinator.

use polkadot_primitives::v1::{
	CandidateReceipt, ValidDisputeStatementKind, InvalidDisputeStatementKind, ValidatorIndex,
	ValidatorSignature, SessionIndex, CandidateHash,
};

use kvdb::{KeyValueDB, DBTransaction};
use parity_scale_codec::{Encode, Decode};

const ACTIVE_DISPUTES_KEY: &[u8; 15] = b"active-disputes";
const EARLIEST_SESSION_KEY: &[u8; 16] = b"earliest-session";
const CANDIDATE_VOTES_SUBKEY: &[u8; 15] = b"candidate-votes";

// It would be nice to draw this from the chain state, but we have no tools for it right now.
// On Polkadot this is 2 days, and on Kusama it's 12 hours.
const DISPUTE_WINDOW: SessionIndex = 12;

fn candidate_votes_key(session: SessionIndex, candidate_hash: &CandidateHash) -> [u8; 15 + 4 + 32] {
	let mut buf = [0u8; 15 + 4 + 32];
	buf[..15].copy_from_slice(CANDIDATE_VOTES_SUBKEY);
	session.using_encoded(|s| buf[15..][..4].copy_from_slice(s));
	candidate_hash.using_encoded(|s| buf[(15 + 4)..].copy_from_slice(s));

	buf
}

/// Column configuration information for the DB.
#[derive(Debug, Clone)]
pub struct ColumnConfiguration {
	/// The column in the key-value DB where data is stored.
	pub col_data: u32,
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

/// Meta-key for tracking active disputes.
#[derive(Debug, Default, Clone, Encode, Decode, PartialEq)]
pub struct ActiveDisputes {
	/// All disputed candidates, sorted by session index and then by candidate hash.
	pub disputed: Vec<(SessionIndex, CandidateHash)>,
}

/// Errors while accessing things from the DB.
#[derive(Debug, derive_more::From, derive_more::Display)]
pub enum Error {
	Io(std::io::Error),
	InvalidDecoding(parity_scale_codec::Error),
}

impl std::error::Error for Error {}

/// Result alias for DB errors.
pub type Result<T> = std::result::Result<T, Error>;

fn load_decode<D: Decode>(db: &dyn KeyValueDB, col_data: u32, key: &[u8])
	-> Result<Option<D>>
{
	match db.get(col_data, key)? {
		None => Ok(None),
		Some(raw) => D::decode(&mut &raw[..])
			.map(Some)
			.map_err(Into::into),
	}
}

/// Load the candidate votes for the identified candidate under the given hash.
pub(crate) fn load_candidate_votes(
	db: &dyn KeyValueDB,
	config: &ColumnConfiguration,
	session: SessionIndex,
	candidate_hash: &CandidateHash,
) -> Result<Option<CandidateVotes>> {
	load_decode(db, config.col_data, &candidate_votes_key(session, candidate_hash))
}

/// Load the earliest session, if any.
pub(crate) fn load_earliest_session(
	db: &dyn KeyValueDB,
	config: &ColumnConfiguration,
) -> Result<Option<SessionIndex>> {
	load_decode(db, config.col_data, EARLIEST_SESSION_KEY)
}

/// Load the active disputes, if any.
pub(crate) fn load_active_disputes(
	db: &dyn KeyValueDB,
	config: &ColumnConfiguration,
) -> Result<Option<ActiveDisputes>> {
	load_decode(db, config.col_data, ACTIVE_DISPUTES_KEY)
}

/// An atomic transaction to be commited to the underlying DB.
#[derive(Debug, Default, Clone)]
pub(crate) struct Transaction {
	earliest_session: Option<SessionIndex>,
	active_disputes: Option<ActiveDisputes>,
	write_candidate_votes: Vec<(SessionIndex, CandidateHash, CandidateVotes)>,
	delete_candidate_votes: Vec<(SessionIndex, CandidateHash)>,
}

impl Transaction {
	/// Prepare a write to the 'earliest session' field of the DB.
	///
	/// Later calls to this function will override earlier ones.
	pub(crate) fn put_earliest_session(&mut self, session: SessionIndex) {
		self.earliest_session = Some(session);
	}

	/// Prepare a write to the active disputes stored in the DB.
	///
	/// Later calls to this function will override earlier ones.
	pub(crate) fn put_active_disputes(&mut self, active: ActiveDisputes) {
		self.active_disputes = Some(active);
	}


	/// Prepare a write of the candidate votes under the indicated candidate.
	///
	/// Later calls to this function for the same candidate will override earlier ones.
	/// Any calls to this function will be overridden by deletions of the same candidate.
	pub(crate) fn put_candidate_votes(
		&mut self,
		session: SessionIndex,
		candidate_hash: CandidateHash,
		votes: CandidateVotes,
	) {
		self.write_candidate_votes.push((session, candidate_hash, votes))
	}

	/// Prepare a deletion of the candidate votes under the indicated candidate.
	///
	/// Any calls to this function will override writes to the same candidate.
	pub(crate) fn delete_candidate_votes(
		&mut self,
		session: SessionIndex,
		candidate_hash: CandidateHash,
	) {
		self.delete_candidate_votes.push((session, candidate_hash))
	}

	/// Write the transaction atomically to the DB.
	pub(crate) fn write(self, db: &dyn KeyValueDB, config: &ColumnConfiguration) -> Result<()> {
		let mut tx = DBTransaction::new();

		if let Some(s) = self.earliest_session {
			tx.put_vec(config.col_data, EARLIEST_SESSION_KEY, s.encode());
		}

		if let Some(a) = self.active_disputes {
			tx.put_vec(config.col_data, ACTIVE_DISPUTES_KEY, a.encode());
		}

		for (session, candidate_hash, votes) in self.write_candidate_votes {
			tx.put_vec(config.col_data, &candidate_votes_key(session, &candidate_hash), votes.encode());
		}

		for (session, candidate_hash) in self.delete_candidate_votes {
			tx.delete(config.col_data, &candidate_votes_key(session, &candidate_hash));
		}

		db.write(tx).map_err(Into::into)
	}
}

/// Maybe prune data in the DB based on the provided session index.
///
/// This is intended to be called on every block, and as such will be used to populate the DB on
/// first launch. If the on-disk data does not need to be pruned, only a single storage read
/// will be performed.
///
/// If one or more ancient sessions are pruned, all metadata on candidates within the ancient
/// session will be deleted.
pub(crate) fn note_current_session(
	store: &dyn KeyValueDB,
	config: &ColumnConfiguration,
	current_session: SessionIndex,
) -> Result<()> {
	let new_earliest = current_session.saturating_sub(DISPUTE_WINDOW);
	let mut tx = Transaction::default();

	match load_earliest_session(store, config)? {
		None => {
			// First launch - write new-earliest.
			tx.put_earliest_session(new_earliest);
		}
		Some(prev_earliest) if new_earliest > prev_earliest => {
			// Prune all data in the outdated sessions.
			tx.put_earliest_session(new_earliest);

			// Clear active disputes metadata.
			{
				let mut active_disputes = load_active_disputes(store, config)?.unwrap_or_default();
				let prune_up_to = active_disputes.disputed.iter()
					.take_while(|s| s.0 < new_earliest)
					.count();

				if prune_up_to > 0 {
					let _ = active_disputes.disputed.drain(..prune_up_to);
					tx.put_active_disputes(active_disputes);
				}
			}

			// Clear all disputes with session less than the new earliest kept.
			{
				// TODO [now]
			}
		}
		Some(_) => {
			// nothing to do.
		}
	};

	tx.write(store, config)
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_primitives::v1::{Hash, Id as ParaId};

	#[test]
	fn candidate_votes_key_works() {
		let session = 4;
		let candidate = CandidateHash(Hash::repeat_byte(0x01));

		let key = candidate_votes_key(session, &candidate);

		assert_eq!(&key[0..15], CANDIDATE_VOTES_SUBKEY);
		assert_eq!(&key[15..19], &[0x04, 0x00, 0x00, 0x00]);
		assert_eq!(&key[19..51], candidate.0.as_bytes());
	}

	#[test]
	fn db_transaction() {
		let store = kvdb_memorydb::create(1);
		let config = ColumnConfiguration { col_data: 0 };

		{
			let mut tx = Transaction::default();

			tx.put_earliest_session(0);
			tx.put_earliest_session(1);

			tx.put_active_disputes(ActiveDisputes {
				disputed: vec![
					(0, CandidateHash(Hash::repeat_byte(0))),
				],
			});

			tx.put_active_disputes(ActiveDisputes {
				disputed: vec![
					(1, CandidateHash(Hash::repeat_byte(1))),
				],
			});

			tx.put_candidate_votes(
				1,
				CandidateHash(Hash::repeat_byte(1)),
				CandidateVotes {
					candidate_receipt: Default::default(),
					valid: Vec::new(),
					invalid: Vec::new(),
				},
			);
			tx.put_candidate_votes(
				1,
				CandidateHash(Hash::repeat_byte(1)),
				CandidateVotes {
					candidate_receipt: {
						let mut receipt = CandidateReceipt::default();
						receipt.descriptor.para_id = 5.into();

						receipt
					},
					valid: Vec::new(),
					invalid: Vec::new(),
				},
			);

			tx.write(&store, &config).unwrap();
		}

		// Test that subsequent writes were written.
		{
			assert_eq!(
				load_earliest_session(&store, &config).unwrap().unwrap(),
				1,
			);

			assert_eq!(
				load_active_disputes(&store, &config).unwrap().unwrap(),
				ActiveDisputes {
					disputed: vec![
						(1, CandidateHash(Hash::repeat_byte(1))),
					],
				},
			);

			assert_eq!(
				load_candidate_votes(
					&store,
					&config,
					1,
					&CandidateHash(Hash::repeat_byte(1))
				).unwrap().unwrap().candidate_receipt.descriptor.para_id,
				ParaId::from(5),
			);
		}
	}

	#[test]
	fn db_deletes_supersede_writes() {
		let store = kvdb_memorydb::create(1);
		let config = ColumnConfiguration { col_data: 0 };

		{
			let mut tx = Transaction::default();
			tx.put_candidate_votes(
				1,
				CandidateHash(Hash::repeat_byte(1)),
				CandidateVotes {
					candidate_receipt: Default::default(),
					valid: Vec::new(),
					invalid: Vec::new(),
				}
			);

			tx.write(&store, &config).unwrap();
		}

		assert_eq!(
			load_candidate_votes(
				&store,
				&config,
				1,
				&CandidateHash(Hash::repeat_byte(1))
			).unwrap().unwrap().candidate_receipt.descriptor.para_id,
			ParaId::from(0),
		);

		{
			let mut tx = Transaction::default();
			tx.put_candidate_votes(
				1,
				CandidateHash(Hash::repeat_byte(1)),
				CandidateVotes {
					candidate_receipt: {
						let mut receipt = CandidateReceipt::default();
						receipt.descriptor.para_id = 5.into();

						receipt
					},
					valid: Vec::new(),
					invalid: Vec::new(),
				}
			);

			tx.delete_candidate_votes(1, CandidateHash(Hash::repeat_byte(1)));

			tx.write(&store, &config).unwrap();
		}

		assert!(
			load_candidate_votes(
				&store,
				&config,
				1,
				&CandidateHash(Hash::repeat_byte(1))
			).unwrap().is_none()
		);
	}
}
