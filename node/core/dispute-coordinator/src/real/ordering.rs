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

use std::{
	cmp::{Ord, Ordering, PartialOrd},
	collections::{BTreeMap, HashMap, HashSet},
};

use polkadot_node_subsystem::{ActivatedLeaf, ActiveLeavesUpdate, SubsystemSender};
use polkadot_node_subsystem_util::runtime::get_candidate_events;
use polkadot_primitives::v1::{
	BlockNumber, CandidateEvent, CandidateHash, CandidateReceipt, Hash, Id,
};

use super::error::Result;

/// Provider of `CandidateComparator` for candidates.
pub struct OrderingProvider {
	/// Currently cached comparators for candidates.
	cached_comparators: HashMap<CandidateHash, CandidateComparator>,
	/// including block -> `CandidateHash`
	///
	/// We need this to clean up `cached_comparators` on `ActiveLeavesUpdate`.
	candidates_by_block_number: BTreeMap<BlockNumber, HashSet<CandidateHash>>,
}

/// Comparator for ordering of disputes for candidates.
///
/// This comparator makes it possible to order disputes based on age and to ensure some fairness
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
pub struct CandidateComparator {
	/// Relay chain block number the candidate got included in.
	included_block_number: BlockNumber,
	/// Para id, used for ordering parachains within same relay chain block.
	para_id: Id,
	/// The hash of the relay chain block the candidate got included in.
	included_block_hash: Hash,
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
		match self.included_block_number.cmp(&other.included_block_number) {
			Ordering::Equal => (),
			o => return o,
		}
		match self.para_id.cmp(&other.para_id) {
			Ordering::Equal => (),
			o => return o,
		}
		self.included_block_hash.cmp(&other.included_block_hash)
	}
}

impl OrderingProvider {
	/// Create a properly initialized `OrderingProvider`.
	pub async fn new<Sender: SubsystemSender>(
		sender: &mut Sender,
		initial_head: ActivatedLeaf,
	) -> Result<Self> {
		let mut s = Self {
			cached_comparators: HashMap::new(),
			candidates_by_block_number: BTreeMap::new(),
		};
		let update =
			ActiveLeavesUpdate { activated: Some(initial_head), deactivated: Default::default() };
		s.process_active_leaves_update(sender, &update).await?;
		Ok(s)
	}

	/// Check whether a candidate is included on chains known to this node.
	pub async fn is_known_included(&mut self, candidate: &CandidateReceipt) -> bool {
		self.cached_comparators.contains_key(&candidate.hash())
	}

	/// Retrieve a candidate comparator if available.
	///
	/// If not available, we can treat disputes concerning this candidate with low priority and
	/// should use spam slots for such disputes.
	pub fn candidate_comparator<'a>(
		&'a mut self,
		candidate: &CandidateReceipt,
	) -> Option<&'a CandidateComparator> {
		self.cached_comparators.get(&candidate.hash())
	}

	/// Query active leaves for any candidate `CandidateEvent::CandidateIncluded` events.
	///
	/// and updates current heads, so we can query candidates for all non finalized blocks.
	pub async fn process_active_leaves_update<Sender: SubsystemSender>(
		&mut self,
		sender: &mut Sender,
		update: &ActiveLeavesUpdate,
	) -> Result<()> {
		if let Some(activated) = update.activated.as_ref() {
			// Get included events:
			let included = get_candidate_events(sender, activated.hash)
				.await?
				.into_iter()
				.filter_map(|ev| match ev {
					CandidateEvent::CandidateIncluded(receipt, _, _, _) => Some(receipt),
					_ => None,
				});
			for receipt in included {
				let candidate_hash = receipt.hash();
				let comparator = CandidateComparator {
					included_block_number: activated.number,
					included_block_hash: activated.hash,
					para_id: receipt.descriptor.para_id,
				};
				self.cached_comparators.insert(candidate_hash, comparator);
				self.candidates_by_block_number
					.entry(activated.number)
					.or_default()
					.insert(candidate_hash);
			}
		}

		Ok(())
	}

	/// Prune finalized candidates.
	///
	/// Once a candidate lives in a relay chain block that's behind the finalized chain/got
	/// finalized, we can treat it as low priority.
	pub fn process_finalized_block(&mut self, finalized: &BlockNumber) {
		let not_finalized = self.candidates_by_block_number.split_off(finalized);
		let finalized = std::mem::take(&mut self.candidates_by_block_number);
		self.candidates_by_block_number = not_finalized;
		// Clean up finalized:
		for finalized_candidate in finalized.into_values().flatten() {
			self.cached_comparators.remove(&finalized_candidate);
		}
	}
}
