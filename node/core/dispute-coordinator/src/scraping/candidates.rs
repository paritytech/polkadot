use polkadot_primitives::v2::CandidateHash;
use std::collections::HashMap;

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
mod tests {
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
