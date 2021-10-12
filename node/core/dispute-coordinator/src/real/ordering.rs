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

/// Provider of `CandidateComparator` for candidates.
struct OrderingProvider {
	/// Currently cached comparators for candidates.
	cached_comparators: HashMap<CandidateHash, CandidateComparator>,
}

struct CandidateComparator {
  included_in: BlockNumber,
  para_id: ParaId,
  relay_parent: Hash
}

impl Ord for CandidateComparator {
  fn cmp(&self, other: &Self) -> Ordering {
      match self.included_in.cmp(other.block_number) {
        Ordering::Equal => (),
        o => return o,
      }
      match self.block_number.cmp(other.para_id) {
        Ordering::Equal => (),
        o => return o,
      }
      self.relay_parent.cmp(other.relay_parent)
    }
}

impl OrderingProvider {
	/// Check whether a candidate is included on chains known to this node.
	async fn is_known_included(&mut self, candidate: &CandidateReceipt) -> bool {}
	/// Retrive a candidate comparator if available.
	///
	/// If not available, we can treat disputes concerning this candidate with low priority and
	/// should use spam slots for such disputes. 
	async fn candidate_comparator(&mut self, candidate: &CandidateReceipt) -> Option<CandidateComparator> {
	}

	/// Query active leaves for any candidate `CandidateEvent::CandidateIncluded` events.
	///
	/// and updates current heads, so we can query candidates for all active heads.
	fn process_active_leaves_update(&mut self, update: &ActiveLeavesUpdate) {}
}



/// Queue

// - On import - check for ordering for the given candidate hash
// - If present - import into priority queue based on given `CandidateComparator`.
// - If not push on best effort queue (if participation is required)


/// Import


/// Chain import


/// Dispute distribution on startup - ordering irrelevant, just distribute what has not yet been
/// distributed.
///
///
///


/// Components/modules:
///

// Ordering: Get Ordering for candidates
// 
// participation/Queues - might be owned by participation:
//
//    /// Will put message in queue, either priority or best effort depending on whether ordering
//    delivers a comparator or not.
//	- fn queue(&mut self, ordering: &mut OrderingProvider, msg: DisputeParticiationMessage)
// Priority queue for participation:
//  - fn dequeue(&mut self) -> Option<DisputeParticiationMessage>
//	- Put on queue
//
//	Best effort queue:
//
//	- Put on queue
//
//
// participation:
//
//   struct Participation {
//		running_participations: HashSet<CandidateHash>,
//   }
//   // removes candidate from running_participations & dequeues the next participation.
//    
//   fn handle_local_statement(session, candidate_hash) 
//	 - Keep track of in-flight participation tasks - watch for `IssueLocalStatement`.
//	 - Process queues (priority in order and then best effort in order)
//	 - Maintain a configured number of parallel participation tasks.
//
//
//	spam_slots:
//
//  struct SpamSlots {
//		slots: HashMap<(SessionIndex, ValidatorIndex), u32>,
//		unconfirmed_candidates: HashMap<(SessionIndex, CandidateHash), HashSet<ValidatorIndex>>,
//  }
//		- fn recover_from_state() -> Self {}
//		- fn add_unconfirmed(&mut self, CandidateHash, ValidatorIndex)
//		- fn confirm(CandidateHash)
