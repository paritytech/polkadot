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

use std::{cmp::Ordering, collections::BTreeMap};

use futures::channel::oneshot;
use polkadot_node_subsystem::{messages::ChainApiMessage, overseer};
use polkadot_primitives::v2::{BlockNumber, CandidateHash, CandidateReceipt, Hash, SessionIndex};

use crate::{
	error::{FatalError, FatalResult, Result},
	LOG_TARGET,
};

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

/// Queues for dispute participation.
/// In both queues we have a strict ordering of candidates and participation will
/// happen in that order. Refer to `CandidateComparator` for details on the ordering.
pub struct Queues {
	/// Set of best effort participation requests.
	best_effort: BTreeMap<CandidateComparator, ParticipationRequest>,

	/// Priority queue.
	priority: BTreeMap<CandidateComparator, ParticipationRequest>,
}

/// A dispute participation request that can be queued.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ParticipationRequest {
	candidate_hash: CandidateHash,
	candidate_receipt: CandidateReceipt,
	session: SessionIndex,
}

/// Whether a `ParticipationRequest` should be put on best-effort or the priority queue.
#[derive(Debug)]
pub enum ParticipationPriority {
	BestEffort,
	Priority,
}

impl ParticipationPriority {
	/// Create `ParticipationPriority` with either `Priority`
	///
	/// or `BestEffort`.
	pub fn with_priority_if(is_priority: bool) -> Self {
		if is_priority {
			Self::Priority
		} else {
			Self::BestEffort
		}
	}

	/// Whether or not this is a priority entry.
	///
	/// If false, it is best effort.
	pub fn is_priority(&self) -> bool {
		match self {
			Self::Priority => true,
			Self::BestEffort => false,
		}
	}
}

/// What can go wrong when queuing a request.
#[derive(Debug, thiserror::Error)]
pub enum QueueError {
	#[error("Request could not be queued, because best effort queue was already full.")]
	BestEffortFull,
	#[error("Request could not be queued, because priority queue was already full.")]
	PriorityFull,
}

impl ParticipationRequest {
	/// Create a new `ParticipationRequest` to be queued.
	pub fn new(candidate_receipt: CandidateReceipt, session: SessionIndex) -> Self {
		Self { candidate_hash: candidate_receipt.hash(), candidate_receipt, session }
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
	pub fn into_candidate_info(self) -> (CandidateHash, CandidateReceipt) {
		let Self { candidate_hash, candidate_receipt, .. } = self;
		(candidate_hash, candidate_receipt)
	}
}

impl Queues {
	/// Create new `Queues`.
	pub fn new() -> Self {
		Self { best_effort: BTreeMap::new(), priority: BTreeMap::new() }
	}

	/// Will put message in queue, either priority or best effort depending on priority.
	///
	/// If the message was already previously present on best effort, it will be moved to priority
	/// if it is considered priority now.
	///
	/// Returns error in case a queue was found full already.
	pub async fn queue(
		&mut self,
		sender: &mut impl overseer::DisputeCoordinatorSenderTrait,
		priority: ParticipationPriority,
		req: ParticipationRequest,
	) -> Result<()> {
		let comparator = CandidateComparator::new(sender, &req.candidate_receipt).await?;

		self.queue_with_comparator(comparator, priority, req)?;
		Ok(())
	}

	/// Get the next best request for dispute participation if any.
	/// First the priority queue is considered and then the best effort one.
	pub fn dequeue(&mut self) -> Option<ParticipationRequest> {
		if let Some(req) = self.pop_priority() {
			return Some(req.1)
		}
		self.pop_best_effort().map(|d| d.1)
	}

	fn queue_with_comparator(
		&mut self,
		comparator: CandidateComparator,
		priority: ParticipationPriority,
		req: ParticipationRequest,
	) -> std::result::Result<(), QueueError> {
		if priority.is_priority() {
			if self.priority.len() >= PRIORITY_QUEUE_SIZE {
				return Err(QueueError::PriorityFull)
			}
			// Remove any best effort entry:
			self.best_effort.remove(&comparator);
			self.priority.insert(comparator, req);
		} else {
			if self.priority.contains_key(&comparator) {
				// The candidate is already in priority queue - don't
				// add in in best effort too.
				return Ok(())
			}
			if self.best_effort.len() >= BEST_EFFORT_QUEUE_SIZE {
				return Err(QueueError::BestEffortFull)
			}
			self.best_effort.insert(comparator, req);
		}
		Ok(())
	}

	/// Get best from the best effort queue.
	fn pop_best_effort(&mut self) -> Option<(CandidateComparator, ParticipationRequest)> {
		return Self::pop_impl(&mut self.best_effort)
	}

	/// Get best priority queue entry.
	fn pop_priority(&mut self) -> Option<(CandidateComparator, ParticipationRequest)> {
		return Self::pop_impl(&mut self.priority)
	}

	// `pop_best_effort` and `pop_priority` do the same but on different `BTreeMap`s. This function has
	// the extracted implementation
	fn pop_impl(
		target: &mut BTreeMap<CandidateComparator, ParticipationRequest>,
	) -> Option<(CandidateComparator, ParticipationRequest)> {
		// Once https://github.com/rust-lang/rust/issues/62924 is there, we can use a simple:
		// target.pop_first().
		if let Some((comparator, _)) = target.iter().next() {
			let comparator = *comparator;
			target
				.remove(&comparator)
				.map(|participation_request| (comparator, participation_request))
		} else {
			None
		}
	}
}

/// `Comparator` for ordering of disputes for candidates.
///
/// This `comparator` makes it possible to order disputes based on age and to ensure some fairness
/// between chains in case of equally old disputes.
///
/// Objective ordering between nodes is important in case of lots disputes, so nodes will pull in
/// the same direction and work on resolving the same disputes first. This ensures that we will
/// conclude some disputes, even if there are lots of them. While any objective ordering would
/// suffice for this goal, ordering by age ensures we are not only resolving disputes, but also
/// resolve the oldest one first, which are also the most urgent and important ones to resolve.
///
/// Note: That by `oldest` we mean oldest in terms of relay chain block number, for any block
/// number that has not yet been finalized. If a block has been finalized already it should be
/// treated as low priority when it comes to disputes, as even in the case of a negative outcome,
/// we are already too late. The ordering mechanism here serves to prevent this from happening in
/// the first place.
#[derive(Copy, Clone)]
#[cfg_attr(test, derive(Debug))]
struct CandidateComparator {
	/// Block number of the relay parent. It's wrapped in an `Option<>` because there are cases when
	/// it can't be obtained. For example when the node is lagging behind and new leaves are received
	/// with a slight delay. Candidates with unknown relay parent are treated with the lowest priority.
	///
	/// The order enforced by `CandidateComparator` is important because we want to participate in
	/// the oldest disputes first.
	///
	/// Note: In theory it would make more sense to use the `BlockNumber` of the including
	/// block, as inclusion time is the actual relevant event when it comes to ordering. The
	/// problem is, that a candidate can get included multiple times on forks, so the `BlockNumber`
	/// of the including block is not unique. We could theoretically work around that problem, by
	/// just using the lowest `BlockNumber` of all available including blocks - the problem is,
	/// that is not stable. If a new fork appears after the fact, we would start ordering the same
	/// candidate differently, which would result in the same candidate getting queued twice.
	relay_parent_block_number: Option<BlockNumber>,
	/// By adding the `CandidateHash`, we can guarantee a unique ordering across candidates with the
	/// same relay parent block number. Candidates without `relay_parent_block_number` are ordered by
	/// the `candidate_hash` (and treated with the lowest priority, as already mentioned).
	candidate_hash: CandidateHash,
}

impl CandidateComparator {
	/// Create a candidate comparator based on given (fake) values.
	///
	/// Useful for testing.
	#[cfg(test)]
	pub fn new_dummy(block_number: Option<BlockNumber>, candidate_hash: CandidateHash) -> Self {
		Self { relay_parent_block_number: block_number, candidate_hash }
	}

	/// Create a candidate comparator for a given candidate.
	///
	/// Returns:
	///	- `Ok(CandidateComparator{Some(relay_parent_block_number), candidate_hash})` when the
	/// 	relay parent can be obtained. This is the happy case.
	/// - `Ok(CandidateComparator{None, candidate_hash})` in case the candidate's relay parent
	/// 	can't be obtained.
	///	- `FatalError` in case the chain API call fails with an unexpected error.
	pub async fn new(
		sender: &mut impl overseer::DisputeCoordinatorSenderTrait,
		candidate: &CandidateReceipt,
	) -> FatalResult<Self> {
		let candidate_hash = candidate.hash();
		let n = get_block_number(sender, candidate.descriptor().relay_parent).await?;

		if n.is_none() {
			gum::warn!(
				target: LOG_TARGET,
				candidate_hash = ?candidate_hash,
				"Candidate's relay_parent could not be found via chain API - `CandidateComparator` \
				with an empty relay parent block number will be provided!"
			);
		}

		Ok(CandidateComparator { relay_parent_block_number: n, candidate_hash })
	}
}

impl PartialEq for CandidateComparator {
	fn eq(&self, other: &CandidateComparator) -> bool {
		Ordering::Equal == self.cmp(other)
	}
}

impl Eq for CandidateComparator {}

impl PartialOrd for CandidateComparator {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for CandidateComparator {
	fn cmp(&self, other: &Self) -> Ordering {
		return match (self.relay_parent_block_number, other.relay_parent_block_number) {
			(None, None) => {
				// No relay parents for both -> compare hashes
				self.candidate_hash.cmp(&other.candidate_hash)
			},
			(Some(self_relay_parent_block_num), Some(other_relay_parent_block_num)) => {
				match self_relay_parent_block_num.cmp(&other_relay_parent_block_num) {
					// if the relay parent is the same for both -> compare hashes
					Ordering::Equal => self.candidate_hash.cmp(&other.candidate_hash),
					// if not - return the result from comparing the relay parent block numbers
					o => return o,
				}
			},
			(Some(_), None) => {
				// Candidates with known relay parents are always with priority
				Ordering::Less
			},
			(None, Some(_)) => {
				// Ditto
				Ordering::Greater
			},
		}
	}
}

async fn get_block_number(
	sender: &mut impl overseer::DisputeCoordinatorSenderTrait,
	relay_parent: Hash,
) -> FatalResult<Option<BlockNumber>> {
	let (tx, rx) = oneshot::channel();
	sender.send_message(ChainApiMessage::BlockNumber(relay_parent, tx)).await;
	rx.await
		.map_err(|_| FatalError::ChainApiSenderDropped)?
		.map_err(FatalError::ChainApiAncestors)
}
