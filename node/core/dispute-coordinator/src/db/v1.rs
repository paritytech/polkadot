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

fn candidate_votes_key(session: SessionIndex, candidate_hash: &CandidateHash) -> [u8; 4 + 15 + 32] {
	let mut buf = [0u8; 4 + 15 + 32];
	session.using_encoded(|s| buf[0..4].copy_from_slice(s));
	buf[4..][..15].copy_from_slice(CANDIDATE_VOTES_SUBKEY);
	candidate_hash.using_encoded(|s| buf[(4 + 15)..].copy_from_slice(s));

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
#[derive(Debug, Clone, Encode, Decode)]
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

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_primitives::v1::Hash;

	#[test]
	fn candidate_votes_key_works() {
		let session = 4;
		let candidate = CandidateHash(Hash::repeat_byte(0x01));

		let key = candidate_votes_key(session, &candidate);

		assert_eq!(&key[0..4], &[0x04, 0x00, 0x00, 0x00]);
		assert_eq!(&key[4..19], CANDIDATE_VOTES_SUBKEY);
		assert_eq!(&key[19..51], candidate.0.as_bytes());
	}
}
