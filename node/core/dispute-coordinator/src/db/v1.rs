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
