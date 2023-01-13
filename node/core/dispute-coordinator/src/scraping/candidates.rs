use polkadot_primitives::v2::{BlockNumber, CandidateHash};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Keeps `CandidateHash` in reference counted way.
/// Each `insert` saves a value with `reference count == 1` or increases the reference
/// count if the value already exists.
/// Each `remove` decreases the reference count for the corresponding `CandidateHash`.
/// If the reference count reaches 0 - the value is removed.
struct RefCountedCandidates {
	candidates: HashMap<CandidateHash, usize>,
}

impl RefCountedCandidates {
	pub fn new() -> Self {
		Self { candidates: HashMap::new() }
	}
	// If `CandidateHash` doesn't exist in the `HashMap` it is created and its reference
	// count is set to 1.
	// If `CandidateHash` already exists in the `HashMap` its reference count is increased.
	pub fn insert(&mut self, candidate: CandidateHash) {
		*self.candidates.entry(candidate).or_default() += 1;
	}

	// If a `CandidateHash` with reference count equals to 1 is about to be removed - the
	// candidate is dropped from the container too.
	// If a `CandidateHash` with reference count biger than 1 is about to be removed - the
	// reference count is decreased and the candidate remains in the container.
	pub fn remove(&mut self, candidate: &CandidateHash) {
		match self.candidates.get_mut(candidate) {
			Some(v) if *v > 1 => *v -= 1,
			Some(v) => {
				assert!(*v == 1);
				self.candidates.remove(candidate);
			},
			None => {},
		}
	}

	pub fn contains(&self, candidate: &CandidateHash) -> bool {
		self.candidates.contains_key(&candidate)
	}
}

#[cfg(test)]
mod ref_counted_candidates_tests {
	use super::*;
	use polkadot_primitives::v2::{BlakeTwo256, HashT};

	#[test]
	fn element_is_removed_when_refcount_reaches_zero() {
		let mut container = RefCountedCandidates::new();

		let zero = CandidateHash(BlakeTwo256::hash(&vec![0]));
		let one = CandidateHash(BlakeTwo256::hash(&vec![1]));
		// add two separate candidates
		container.insert(zero); // refcount == 1
		container.insert(one);

		// and increase the reference count for the first
		container.insert(zero); // refcount == 2

		assert!(container.contains(&zero));
		assert!(container.contains(&one));

		// remove once -> refcount == 1
		container.remove(&zero);
		assert!(container.contains(&zero));
		assert!(container.contains(&one));

		// remove once -> refcount == 0
		container.remove(&zero);
		assert!(!container.contains(&zero));
		assert!(container.contains(&one));

		// remove the other element
		container.remove(&one);
		assert!(!container.contains(&zero));
		assert!(!container.contains(&one));
	}
}

/// Keeps track of scraped candidates. Supports `insert`, `remove_up_to_height` and `contains`
/// operations.
pub struct ScrapedCandidates {
	/// Main data structure which keeps the candidates we know about. `contains` does lookups only here.
	candidates: RefCountedCandidates,
	/// Keeps track at which block number a candidate was inserted. Used in `remove_up_to_height`.
	/// Without this tracking we won't be able to remove all candidates before block X.
	candidates_by_block_number: BTreeMap<BlockNumber, HashSet<CandidateHash>>,
}

impl ScrapedCandidates {
	pub fn new() -> Self {
		Self {
			candidates: RefCountedCandidates::new(),
			candidates_by_block_number: BTreeMap::new(),
		}
	}

	pub fn contains(&self, candidate_hash: &CandidateHash) -> bool {
		self.candidates.contains(candidate_hash)
	}

	// Removes all candidates up to a given height. The candidates at the block height are NOT removed.
	pub fn remove_up_to_height(&mut self, height: &BlockNumber) {
		let not_stale = self.candidates_by_block_number.split_off(&height);
		let stale = std::mem::take(&mut self.candidates_by_block_number);
		self.candidates_by_block_number = not_stale;
		for candidates in stale.values() {
			for c in candidates {
				self.candidates.remove(c);
			}
		}
	}

	pub fn insert(&mut self, block_number: BlockNumber, candidate_hash: CandidateHash) {
		self.candidates.insert(candidate_hash);
		self.candidates_by_block_number
			.entry(block_number)
			.or_default()
			.insert(candidate_hash);
	}

	// Used only for tests to verify the pruning doesn't leak data.
	#[cfg(test)]
	pub fn candidates_by_block_number_is_empty(&self) -> bool {
		self.candidates_by_block_number.is_empty()
	}
}

#[cfg(test)]
mod scraped_candidates_tests {
	use super::*;
	use polkadot_primitives::v2::{BlakeTwo256, HashT};

	#[test]
	fn stale_candidates_are_removed() {
		let mut candidates = ScrapedCandidates::new();
		let target = CandidateHash(BlakeTwo256::hash(&vec![1, 2, 3]));
		candidates.insert(1, target);

		assert!(candidates.contains(&target));

		candidates.remove_up_to_height(&2);
		assert!(!candidates.contains(&target));
		assert!(candidates.candidates_by_block_number_is_empty());
	}
}
