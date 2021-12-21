// Copyright 2021 Parity Technologies (UK) Ltd.
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

use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

use polkadot_primitives::v1::{CandidateHash, CandidateReceipt, SessionIndex};

use crate::real::ordering::CandidateComparator;

#[cfg(test)]
mod tests;

/// How many potential garbage disputes we want to queue, before starting to drop requests.
#[cfg(not(test))]
const BEST_EFFORT_QUEUE_SIZE: usize = 100;
#[cfg(test)]
const BEST_EFFORT_QUEUE_SIZE: usize = 3;

/// How many priority disputes can be queued.
///
/// Once the queue exceeds that size, we will start to drop the newest participation requests in
/// the queue. Note that for each vote import the request will be re-added, if there is free
/// capacity. This limit just serves as a safe guard, it is not expected to ever really be reached.
///
/// For 100 parachains, this would allow for every single candidate in 100 blocks on
/// two forks to get disputed, which should be plenty to deal with any realistic attack.
#[cfg(not(test))]
const PRIORITY_QUEUE_SIZE: usize = 20_000;
#[cfg(test)]
const PRIORITY_QUEUE_SIZE: usize = 2;

/// Type for counting how often a candidate was added to the best effort queue.
type BestEffortCount = u32;

/// Queues for dispute participation.
pub struct Queues {
	/// Set of best effort participation requests.
	///
	/// Note that as size is limited to `BEST_EFFORT_QUEUE_SIZE` we simply do a linear search for
	/// the entry with the highest `added_count` to determine what dispute to participate next in.
	///
	/// This mechanism leads to an amplifying effect - the more validators already participated,
	/// the more likely it becomes that more validators will participate soon, which should lead to
	/// a quick resolution of disputes, even in the best effort queue.
	best_effort: HashMap<CandidateHash, BestEffortEntry>,

	/// Priority queue.
	///
	/// In the priority queue, we have a strict ordering of candidates and participation will
	/// happen in that order.
	priority: BTreeMap<CandidateComparator, ParticipationRequest>,
}

/// A dispute participation request that can be queued.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ParticipationRequest {
	candidate_hash: CandidateHash,
	candidate_receipt: CandidateReceipt,
	session: SessionIndex,
	n_validators: usize,
}

/// Entry for the best effort queue.
struct BestEffortEntry {
	req: ParticipationRequest,
	/// How often was the above request added to the queue.
	added_count: BestEffortCount,
}

/// What can go wrong when queuing a request.
#[derive(Debug, Error)]
pub enum Error {
	#[error("Request could not be queued, because best effort queue was already full.")]
	BestEffortFull,
	#[error("Request could not be queued, because priority queue was already full.")]
	PriorityFull,
}

impl ParticipationRequest {
	/// Create a new `ParticipationRequest` to be queued.
	pub fn new(
		candidate_receipt: CandidateReceipt,
		session: SessionIndex,
		n_validators: usize,
	) -> Self {
		Self { candidate_hash: candidate_receipt.hash(), candidate_receipt, session, n_validators }
	}

	pub fn candidate_receipt(&'_ self) -> &'_ CandidateReceipt {
		&self.candidate_receipt
	}
	pub fn candidate_hash(&'_ self) -> &'_ CandidateHash {
		&self.candidate_hash
	}
	pub fn session(&self) -> SessionIndex {
		self.session
	}
	pub fn n_validators(&self) -> usize {
		self.n_validators
	}
	pub fn into_candidate_info(self) -> (CandidateHash, CandidateReceipt) {
		let Self { candidate_hash, candidate_receipt, .. } = self;
		(candidate_hash, candidate_receipt)
	}
}

impl Queues {
	/// Create new `Queues`.
	pub fn new() -> Self {
		Self { best_effort: HashMap::new(), priority: BTreeMap::new() }
	}

	/// Will put message in queue, either priority or best effort depending on whether a
	/// `CandidateComparator` was provided or not.
	///
	/// If the message was already previously present on best effort, it will be moved to priority
	/// if a `CandidateComparator` has been passed now, otherwise the `added_count` on the best
	/// effort queue will be bumped.
	///
	/// Returns error in case a queue was found full already.
	pub fn queue(
		&mut self,
		comparator: Option<CandidateComparator>,
		req: ParticipationRequest,
	) -> Result<(), Error> {
		debug_assert!(comparator
			.map(|c| c.matches_candidate(req.candidate_hash()))
			.unwrap_or(true));

		if let Some(comparator) = comparator {
			if self.priority.len() >= PRIORITY_QUEUE_SIZE {
				return Err(Error::PriorityFull)
			}
			// Remove any best effort entry:
			self.best_effort.remove(&req.candidate_hash);
			self.priority.insert(comparator, req);
		} else {
			if self.best_effort.len() >= BEST_EFFORT_QUEUE_SIZE {
				return Err(Error::BestEffortFull)
			}
			// Note: The request might have been added to priority in a previous call already, we
			// take care of that case in `dequeue` (more efficient).
			self.best_effort
				.entry(req.candidate_hash)
				.or_insert(BestEffortEntry { req, added_count: 0 })
				.added_count += 1;
		}
		Ok(())
	}

	/// Get the next best request for dispute participation
	///
	/// if any.  Priority queue is always considered first, then the best effort queue based on
	/// `added_count`.
	pub fn dequeue(&mut self) -> Option<ParticipationRequest> {
		if let Some(req) = self.pop_priority() {
			// In case a candidate became best effort over time, we might have it also queued in
			// the best effort queue - get rid of any such entry:
			self.best_effort.remove(req.candidate_hash());
			return Some(req)
		}
		self.pop_best_effort()
	}

	/// Get the next best from the best effort queue.
	///
	/// If there are multiple best - just pick one.
	fn pop_best_effort(&mut self) -> Option<ParticipationRequest> {
		let best = self.best_effort.iter().reduce(|(hash1, entry1), (hash2, entry2)| {
			if entry1.added_count > entry2.added_count {
				(hash1, entry1)
			} else {
				(hash2, entry2)
			}
		});
		if let Some((best_hash, _)) = best {
			let best_hash = best_hash.clone();
			self.best_effort.remove(&best_hash).map(|e| e.req)
		} else {
			None
		}
	}

	/// Get best priority queue entry.
	fn pop_priority(&mut self) -> Option<ParticipationRequest> {
		// Once https://github.com/rust-lang/rust/issues/62924 is there, we can use a simple:
		// priority.pop_first().
		if let Some((comparator, _)) = self.priority.iter().next() {
			let comparator = comparator.clone();
			self.priority.remove(&comparator)
		} else {
			None
		}
	}
}
