// Copyright 2022 Parity Technologies (UK) Ltd.
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

use std::collections::HashMap;

use polkadot_node_primitives::SignedDisputeStatement;
use polkadot_primitives::v2::{ValidatorIndex, CandidateReceipt, CandidateHash};


struct Batch {
	/// The actual candidate this batch is concerned with.
	candidate_receipt: CandidateReceipt,

	/// Cache `CandidateHash` to do efficient sanity checks.
	candidate_hash: CandidateHash,

	/// All valid votes received in this batch so far.
	///
	/// We differentiate between valid and invalid votes, so we can detect (and drop) duplicates,
	/// while still allowing validators to equivocate.
	///
	/// Detecting and rejecting duplicats is crucial in order to effectively enforce
	/// `MIN_KEEP_BATCH_ALIVE_VOTES` per `BATCH_COLLECTING_INTERVAL`. If we would count duplicates
	/// here, the mechanism would be broken.
	valid_votes: HashMap<ValidatorIndex, SignedDisputeStatement>,

	/// All invalid votes received in this batch so far.
	invalid_votes: HashMap<ValidatorIndex, SignedDisputeStatement>,

	/// How many votes have been batched in the last `BATCH_COLLECTING_INTERVAL`?
	votes_batched_since_last_tick: u32,
}

impl Batch {
	/// Create a new empty batch based on the given `CandidateReceipt`.
	pub fn new(candidate_receipt: CandidateReceipt) -> Self {
		Self {
			candidate_hash: candidate_receipt.hash(),
			candidate_receipt,
			valid_votes: HashMap::new(),
			invalid_votes: HashMap::new(),
			votes_batched_since_last_tick: 0,
		}
	}

	/// Import votes into the batch.
	pub fn import_votes(&mut self, receipt: CandidateReceipt, valid_vote: (SignedDisputeStatement, ValidatorIndex), invalid_vote: (SignedDisputeStatement, ValidatorIndex)) {
		debug_assert!(valid_vote.0.candidate_hash() == invalid_vote.0.candidate_hash());
		debug_assert!(valid_vote.0.candidate_hash() == &self.candidate_hash);

		if self.valid_votes.insert(valid_vote.1, valid_vote.0).is_none() {
			self.votes_batched_since_last_tick += 1;
		}
		if self.invalid_votes.insert(invalid_vote.1, invalid_vote.0).is_none() {
			self.votes_batched_since_last_tick += 1;
		}
	}
}
