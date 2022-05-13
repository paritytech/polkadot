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

//! We want to batch imports together over some time in order to greatly improve import
//! performance.
//!
//! The dispute coordinator is importing a lot of votes all the time, due to backing and approval
//! votes. The import of a single vote is currently very inefficent, mostly because all the votes
//! are read, updated and written for every single new vote. An easy way to make this more
//! efficient is to batch up imports, before doing the actual import. This way this massive
//! overhead is reduced to e.g. one read/update/write per 30 statements, instead of for each one.
//!
//! Care must be taken with batching to not delay dispute propagation too much.



/// Batches statements for a candidate.
struct Batch {
	/// The candidate hash to batch up statements for.
	candidate_hash: CandidateHash,
	/// Candidate receipt if available.
	candidate_receipt: Option<CandidateReceipt>,
	/// The session the candidate appeared in.
	session: SessionIndex,
	/// Accumulated statements.
	statements: BTreeSet<(SignedDisputeStatement, ValidatorIndex)>,
}


/// Batch together statements as they are flying in.
struct Batcher {
	/// Available batches.
	batches: HashMap<CandidateHash, Batch>,
	/// Keep a list of candidates whe recently have seen invalid votes for.
	///
	/// If a dispute is triggered we would like to forward this quickly, at the same time we
	/// definitely want to batch up votes coming from dispute-distribution as well, as a quick
	/// import of votes is crucial to properly handle spam.
	///
	/// To resolve this dilemma, we use a compromise: When we first encounter an invalid vote for a
	/// candidate, likely a dispute got raised and we will forward votes immediately. In order to
	/// know that it is the first time we encounter an invalid vote, we keep track of last seen
	/// invalid votes here (so we know, even after the batch got flushed).
	///
	/// Note: It is not possible that we see an invalid vote without a valid vote, as
	/// dispute-distribution always batches up two opposing votes.
	last_seen_invalid: LruCache<Candidatehash, ()>,
}

struct BatchEntry {
	/// The actual batch.
	batch: Batch,
	/// When was this batch inserted the first time.
	inserted: Instant,
	has_first_invalid_vote: bool,
}

impl Batcher {

	/// Add a batch of imports.
	///
	/// If a batch for a the candidate is already present, the existing batch will be combined with
	/// the added one.
	pub fn add_batch(&mut self, batch: Batch) {
		match self.batches.entry() {
			Entry::Occupied(occupied) => {
				occupied.get_mut().merge(batch);
			}
			Entry::VacantEntry(vacant) => {
				vacant.insert(batch);
			}
		}
	}
}

impl Batch {
	/// Merge this batch with another batch, consuming it.
	///
	/// Note: this call is only sound if the other `Batch` is for the same `candidate_hash`.
	pub fn merge(&mut self, other: Self) {
		debug_assert!(self.candidate_hash == other.candidate_hash);
		let Self { candidate_hash, candidate_receipt, session, statements } = other;
		self.candidate_receipt = self.candidate_receipt.take().or(candidate_receipt);
		self.statements.append(statements);
	}
}
