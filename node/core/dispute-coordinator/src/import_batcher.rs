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
//!
//! ## Goals for Execution of batches
//!
//! Batching is easy, but when have we batched enough and should do the actual import?
//! A number of goals for a policy come to mind:
//!
//! 1. Even out load - we'd like to avoid huge spikes, and idling. For best performance, load should
//! be spread out as evenly as possible.
//! 2. No problematic losses on crash. As long as we don't persist batches, we can not batch for
//!    too long to avoid problematic data loss.
//! 3. Batches should be signficant, so they actually reduce load on imports.
//! 4. We don't want to slow down dispute resolution.
//! 5. Limit memory usage. If we keep batching for lots of candidates for a significant amount of
//!    time, we might end up consuming quite a lot of memory.
//! 6. Statements are expected to come in waves: Avoid flushing out a batch right in the middle of
//!    a wave if possible, but rather once it is done. This will result in the best performance.
//!
//! ## Picked solution
//!
//! We keep three sets of batches: `active_batches` and `inactive_batches` and `finished_batches`.
//!
//! Every `DETERMINE_FINISHED_TIME` we append `inactive_batches` to `finished_batches`, marking them
//! as ready for doing the actual import. The we move `active_batches` to `inactive_batches`.
//!
//! On every import we move the corresponding batch from `inactive_batches` to `active_batches` (if
//! it is not already). Therefore as long as imports are happening within
//! `DETERMINE_FINISHED_TIME`, we consider the batch active and continue batching.
//!
//! This should take care of 3 and 6.
//!
//! To account for 2 and 5 we also count how often a batch "survived" out our
//! `DETERMINE_FINISHED_TIME` timer and move it to finished after `MAX_INACTIVE_SURVIVALS`.
//!
//! To account for 4, we move a batch to `finished_batches` immediately if it contains an invalid
//! vote for the first time (we keep track of recent invalid votes for candidates for this).
//!
//! We are going to punt on 1 for now. Batching obviously concentrates load, we could even it out
//! again by spawning the actual import in its own task, processing messages via an incoming
//! channel, but as the purpose of this batching is to reduce the load significantly, we will try
//! synchronous import in a first iteration. Factoring out an importer task is left as future
//! optimization work, if deemed necessary.
use std::{time::{Instant, Duration}, collections::{BTreeSet, HashMap, HashSet, hash_map::Entry}};

use lru::LruCache;
use polkadot_node_primitives::SignedDisputeStatement;
use polkadot_node_subsystem::messages::ImportStatementsResult;
use polkadot_primitives::v2::{CandidateHash, CandidateReceipt, SessionIndex, ValidatorIndex};


/// Poll time for inactive/finished batches.
///
/// Every `DETERMINE_FINISHED_TIME` a timer triggers doing the following:
///
/// 1. Move `inactive_batches` to `finished_batches` - ready for consumption by imports.
/// 2. Move `active_batches` to `inactive_batches`
const DETERMINE_FINISHED_TIME: Duration = Duration::from_millis(3000);


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
	/// Confirmations pending.
	///
	/// NOTE: As statements are already checked and valid, the import can only 
	pending_confirmations: Vec<oneshot::Sender<ImportStatementsResult>>,
}


/// Batch together statements as they are flying in.
struct Batcher {
	/// Batches considered actively importing.
	active_batches: HashMap<CandidateHash, Batch>,

	/// Batches considered ready for getting imported.
	inactive_batches: HashMap<CandidateHash, Batch>,
	
	/// Batches that should be considered inactive (are ready for import) no matter what.
	///
	/// If a batch contains a first invalid vote, it will be added here also if a batch survived
	/// one activity check it will be added here, so it gets imported on the next check.
	enforced_inactive: HashSet<CandidateHash>,

	/// Keep a list of candidates we recently have seen invalid votes for.
	///
	/// If a dispute is triggered we would like to forward this quickly, at the same time we
	/// definitely want to batch up votes coming from dispute-distribution as well, as a quick
	/// import of votes is crucial to properly handle spam.
	///
	/// To resolve this dilemma, we use a compromise: When we first encounter an invalid vote for a
	/// candidate, likely a dispute got raised and we will forward votes immediately (on next
	/// sample). In order to know that it is the first time we encounter an invalid vote, we keep
	/// track of last seen invalid votes here (so we know, even after the batch got flushed).
	///
	/// Note: It is not possible that we see an invalid vote without a valid vote, as
	/// dispute-distribution always batches up two opposing votes.
	last_seen_invalid: LruCache<CandidateHash, ()>,
}

impl Batcher {

	/// Add a batch of imports.
	///
	/// If a batch for a the candidate is already present, the existing batch will be combined with
	/// the added one.
	pub fn add_batch(&mut self, batch: Batch) {
		// First invalid statement?
		let new_invalid = batch.contains_invalid_vote() && !self.last_seen_invalid.contains(&batch.candidate_hash);
		if new_invalid {
			self.last_seen_invalid.put(batch.candidate_hash, ());
			self.enforced_inactive.insert(batch.candidate_hash);
		}

		let batch = if let Some(inactive) = self.inactive_batches.remove(&batch.candidate_hash) {
			inactive.merge(batch);
			inactive
		} else {
			batch
		};
		
		match self.active_batches.entry(batch.candidate_hash) {
			Entry::Occupied(occupied) => {
				occupied.get_mut().merge(batch);
			}
			Entry::Vacant(vacant) => {
				vacant.insert(batch);
			}
		}
	}
}

impl Batch {
	pub fn new(candidate_hash: CandidateHash, candidate_receipt: CandidateReceipt, session: SessionIndex, statements: Vec<(SignedDisputeStatement, ValidatorIndex)>, pending_confirmation: Option<oneshot::Sender<ImportStatementsResult>>) -> Self {
		Self {
			candidate_hash,
			candidate_recipt,
			session,
			statements: statements.into_iter().collect(),
			pending_confirmations: pendint_confirmation.into_iter().collect()
		}
	}
	/// Merge this batch with another batch, consuming it.
	///
	/// Note: this call is only sound if the other `Batch` is for the same `candidate_hash`.
	fn merge(&mut self, other: Self) {
		debug_assert!(self.candidate_hash == other.candidate_hash);
		let Self { candidate_hash, candidate_receipt, session, statements } = other;
		self.candidate_receipt = self.candidate_receipt.take().or(candidate_receipt);
		self.statements.append(statements);
	}

	/// Determine whether this batch contains an invalid vote.
	///
	/// NOTE: For simplicity this does a linear search, which should be fine if run on incoming
	/// batches, which (as they have not been batched together) usually only contain 1 to 2
	/// statements.
	fn contains_invalid_vote(&self) -> bool {
		self.statements.iter().find(|(statement, _)| statement.statement().indicates_invalidity()).is_some()
	}
}
