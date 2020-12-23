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

//! Tests for the aux-schema of approval voting.

use super::*;
use std::cell::RefCell;
use polkadot_primitives::v1::Id as ParaId;

#[derive(Default)]
struct TestStore {
	inner: RefCell<HashMap<Vec<u8>, Vec<u8>>>,
}

impl AuxStore for TestStore {
	fn insert_aux<'a, 'b: 'a, 'c: 'a, I, D>(&self, insertions: I, deletions: D) -> sp_blockchain::Result<()>
		where I: IntoIterator<Item = &'a (&'c [u8], &'c [u8])>, D: IntoIterator<Item = &'a &'b [u8]>
	{
		let mut store = self.inner.borrow_mut();

		// insertions before deletions.
		for (k, v) in insertions {
			store.insert(k.to_vec(), v.to_vec());
		}

		for k in deletions {
			store.remove(&k[..]);
		}

		Ok(())
	}

	fn get_aux(&self, key: &[u8]) -> sp_blockchain::Result<Option<Vec<u8>>> {
		Ok(self.inner.borrow().get(key).map(|v| v.clone()))
	}
}

impl TestStore {
	fn write_stored_blocks(&self, range: StoredBlockRange) {
		self.inner.borrow_mut().insert(
			STORED_BLOCKS_KEY.to_vec(),
			range.encode(),
		);
	}

	fn write_blocks_at_height(&self, height: BlockNumber, blocks: &[Hash]) {
		self.inner.borrow_mut().insert(
			blocks_at_height_key(height).to_vec(),
			blocks.encode(),
		);
	}

	fn write_block_entry(&self, block_hash: &Hash, entry: &BlockEntry) {
		self.inner.borrow_mut().insert(
			block_entry_key(block_hash).to_vec(),
			entry.encode(),
		);
	}

	fn write_candidate_entry(&self, candidate_hash: &CandidateHash, entry: &CandidateEntry) {
		self.inner.borrow_mut().insert(
			candidate_entry_key(candidate_hash).to_vec(),
			entry.encode(),
		);
	}
}

fn make_bitvec(len: usize) -> BitVec<BitOrderLsb0, u8> {
	bitvec::bitvec![BitOrderLsb0, u8; 0; len]
}

fn make_block_entry(
	block_hash: Hash,
	candidates: Vec<(CoreIndex, CandidateHash)>,
) -> BlockEntry {
	BlockEntry {
		block_hash,
		session: 1,
		slot: 1,
		relay_vrf_story: RelayVRF([0u8; 32]),
		approved_bitfield: make_bitvec(candidates.len()),
		candidates,
		children: Vec::new(),
	}
}

fn make_candidate(para_id: ParaId, relay_parent: Hash) -> CandidateReceipt {
	let mut c = CandidateReceipt::default();

	c.descriptor.para_id = para_id;
	c.descriptor.relay_parent = relay_parent;

	c
}

#[test]
fn read_write() {
	let store = TestStore::default();

	let hash_a = Hash::repeat_byte(1);
	let hash_b = Hash::repeat_byte(2);
	let candidate_hash = CandidateHash(Hash::repeat_byte(3));

	let range = StoredBlockRange(10, 20);
	let at_height = vec![hash_a, hash_b];

	let block_entry = make_block_entry(
		hash_a,
		vec![(CoreIndex(0), candidate_hash)],
	);

	let candidate_entry = CandidateEntry {
		candidate: Default::default(),
		session: 5,
		block_assignments: vec![
			(hash_a, ApprovalEntry {
				tranches: Vec::new(),
				backing_group: GroupIndex(1),
				next_wakeup: 1000,
				our_assignment: None,
				assignments: Default::default(),
				approved: false,
			})
		].into_iter().collect(),
		approvals: Default::default(),
	};

	store.write_stored_blocks(range.clone());
	store.write_blocks_at_height(1, &at_height);
	store.write_block_entry(&hash_a, &block_entry);
	store.write_candidate_entry(&candidate_hash, &candidate_entry);

	assert_eq!(load_stored_blocks(&store).unwrap(), Some(range));
	assert_eq!(load_blocks_at_height(&store, 1).unwrap(), at_height);
	assert_eq!(load_block_entry(&store, &hash_a).unwrap(), Some(block_entry));
	assert_eq!(load_candidate_entry(&store, &candidate_hash).unwrap(), Some(candidate_entry));

	let delete_keys = vec![
		STORED_BLOCKS_KEY.to_vec(),
		blocks_at_height_key(1).to_vec(),
		block_entry_key(&hash_a).to_vec(),
		candidate_entry_key(&candidate_hash).to_vec(),
	];

	let delete_keys: Vec<_> = delete_keys.iter().map(|k| &k[..]).collect();
	store.insert_aux(&[], &delete_keys);

	assert!(load_stored_blocks(&store).unwrap().is_none());
	assert!(load_blocks_at_height(&store, 1).unwrap().is_empty());
	assert!(load_block_entry(&store, &hash_a).unwrap().is_none());
	assert!(load_candidate_entry(&store, &candidate_hash).unwrap().is_none());
}

#[test]
fn add_block_entry_works() {
	let store = TestStore::default();

	let parent_hash = Hash::repeat_byte(1);
	let block_hash_a = Hash::repeat_byte(2);
	let block_hash_b = Hash::repeat_byte(69);

	let candidate_hash_a = CandidateHash(Hash::repeat_byte(3));
	let candidate_hash_b = CandidateHash(Hash::repeat_byte(4));

	let block_entry_a = make_block_entry(
		block_hash_a,
		vec![(CoreIndex(0), candidate_hash_a)],
	);

	let block_entry_b = make_block_entry(
		block_hash_b,
		vec![(CoreIndex(0), candidate_hash_a), (CoreIndex(1), candidate_hash_b)],
	);

	let n_validators = 10;
	let block_number = 10;

	let mut new_candidate_info = HashMap::new();
	new_candidate_info.insert(candidate_hash_a, NewCandidateInfo {
		candidate: make_candidate(1.into(), parent_hash),
		backing_group: GroupIndex(0),
		our_assignment: None,
	});

	add_block_entry(
		&store,
		parent_hash,
		block_number,
		block_entry_a.clone(),
		n_validators,
		|h| new_candidate_info.get(h).map(|x| x.clone()),
	).unwrap();

	new_candidate_info.insert(candidate_hash_b, NewCandidateInfo {
		candidate: make_candidate(2.into(), parent_hash),
		backing_group: GroupIndex(1),
		our_assignment: None,
	});

	add_block_entry(
		&store,
		parent_hash,
		block_number,
		block_entry_b.clone(),
		n_validators,
		|h| new_candidate_info.get(h).map(|x| x.clone()),
	).unwrap();

	assert_eq!(load_block_entry(&store, &block_hash_a).unwrap(), Some(block_entry_a));
	assert_eq!(load_block_entry(&store, &block_hash_b).unwrap(), Some(block_entry_b));

	let candidate_entry_a = load_candidate_entry(&store, &candidate_hash_a).unwrap().unwrap();
	assert_eq!(candidate_entry_a.block_assignments.keys().collect::<Vec<_>>(), vec![&block_hash_a, &block_hash_b]);

	let candidate_entry_b = load_candidate_entry(&store, &candidate_hash_b).unwrap().unwrap();
	assert_eq!(candidate_entry_b.block_assignments.keys().collect::<Vec<_>>(), vec![&block_hash_b]);
}
