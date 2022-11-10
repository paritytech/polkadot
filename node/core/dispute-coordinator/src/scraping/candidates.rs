use polkadot_primitives::v2::{BlockNumber, CandidateHash};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Keeps `CandidateHash` in reference counted way.
/// Each `insert` saves a value with `reference count == 1` or increases the reference
/// count if the value already exists.
/// Each `remove` decreases the reference count for the corresponding `CandidateHash`.
/// If the reference count reaches 0 - the value is removed.
pub struct RefCountedCandidates {
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

/// Keeps track of backed candidates. Supports `insert`, `remove`, `remove_up_to_height`
/// and `contains` operations.
/// This structure has got an undesired side effect. If a candidate is removed explicitly
/// with `remove` its entries will remain in `backed_candidates_by_block_number` until
/// `remove_up_to_height` is called. This means we will keep data in
/// `backed_candidates_by_block_number` a bit longer than necessary.
/// Unfortunately fixing this is not trivial. `backed_candidates` uses reference counting
/// because a candidate can be backed on multiple block heights. So `remove` doesn't necessary
/// removes the candidate - it just decreases its reference count. For this reason it is not
/// a good idea to remove any entries in `backed_candidates_by_block_number` when `remove` is
/// called.
/// One approach for a fix is to introduce `BTreeMap<CandidateHash, HashSet<BlockNumber>>` to
/// keep track on which candidates exist at a specific key in `backed_candidates_by_block_number`.
/// However when removing a candidate with `remove` we don't know (and don't care) at which block
/// height it was backed. So no precise removal is possible. This means that we should either
/// a) don't do any cleanup in `backed_candidates_by_block_number`
/// b) add the 2nd `BTreeMap` mentioned above and delete a random block from
///    `backed_candidates_by_block_number` on each removal.
/// Option a) sounds more reasonable to me - if we can't do the right thing we'd better do nothing.
pub struct BackedCandidates {
	/// Main data structure which keeps the backed candidates we know about. `contains` does
	/// lookups only here.
	backed_candidates: RefCountedCandidates,
	/// Keeps track at which block number a candidate was backed. Used in `remove_up_to_height`.
	/// Without this tracking we won't be able to 'remove all candidates included in block X and before'.
	backed_candidates_by_block_number: BTreeMap<BlockNumber, HashSet<CandidateHash>>,
}

impl BackedCandidates {
	pub fn new() -> Self {
		Self {
			backed_candidates: RefCountedCandidates::new(),
			backed_candidates_by_block_number: BTreeMap::new(),
		}
	}

	pub fn contains(&self, candidate_hash: &CandidateHash) -> bool {
		self.backed_candidates.contains(candidate_hash)
	}

	// Removes all candidates up to a given height. The candidates at the block height are NOT removed.
	pub fn remove_up_to_height(&mut self, height: &BlockNumber) {
		let not_stale = self.backed_candidates_by_block_number.split_off(&height);
		let stale = std::mem::take(&mut self.backed_candidates_by_block_number);
		self.backed_candidates_by_block_number = not_stale;
		for candidates in stale.values() {
			for c in candidates {
				self.backed_candidates.remove(c);
			}
		}
	}

	// Removes a single candidate
	pub fn remove(&mut self, candidate_hash: &CandidateHash) {
		self.backed_candidates.remove(candidate_hash);
	}

	pub fn insert(&mut self, block_number: BlockNumber, candidate_hash: CandidateHash) {
		self.backed_candidates.insert(candidate_hash);
		self.backed_candidates_by_block_number
			.entry(block_number)
			.or_default()
			.insert(candidate_hash);
	}

	// Used only for tests to verify the pruning doesn't leak data.
	#[cfg(test)]
	pub fn backed_candidates_by_block_number_is_empty(&self) -> bool {
		self.backed_candidates_by_block_number.is_empty()
	}

	#[cfg(test)]
	pub fn backed_candidates_by_block_number_has_key(&self, key: &BlockNumber) -> bool {
		self.backed_candidates_by_block_number.contains_key(key)
	}
}

#[cfg(test)]
mod backed_candidates_tests {
	use super::*;
	use polkadot_primitives::v2::{BlakeTwo256, HashT};

	#[test]
	fn explicitly_removed_are_cleaned_from_block_mapping() {
		let mut candidates = BackedCandidates::new();
		let target = CandidateHash(BlakeTwo256::hash(&vec![1, 2, 3]));
		candidates.insert(1, target);

		assert!(candidates.contains(&target));

		candidates.remove(&target);
		assert!(!candidates.contains(&target));
		assert!(candidates.backed_candidates_by_block_number_has_key(&1)); // undesired side effect

		candidates.remove_up_to_height(&2);
		assert!(!candidates.backed_candidates_by_block_number_has_key(&1));
		assert!(candidates.backed_candidates_by_block_number_is_empty());
	}

	#[test]
	fn stale_candidates_are_removed() {
		let mut candidates = BackedCandidates::new();
		let target = CandidateHash(BlakeTwo256::hash(&vec![1, 2, 3]));
		candidates.insert(1, target);

		assert!(candidates.contains(&target));

		candidates.remove_up_to_height(&2);
		assert!(!candidates.contains(&target));
		assert!(candidates.backed_candidates_by_block_number_is_empty());
	}
}
