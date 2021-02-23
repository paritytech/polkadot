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
use polkadot_primitives::v1::Id as ParaId;

pub(crate) fn write_stored_blocks(tx: &mut DBTransaction, range: StoredBlockRange) {
	tx.put_vec(
		DATA_COL,
		&STORED_BLOCKS_KEY[..],
		range.encode(),
	);
}

pub(crate) fn write_blocks_at_height(tx: &mut DBTransaction, height: BlockNumber, blocks: &[Hash]) {
	tx.put_vec(
		DATA_COL,
		&blocks_at_height_key(height)[..],
		blocks.encode(),
	);
}

pub(crate) fn write_block_entry(tx: &mut DBTransaction, block_hash: &Hash, entry: &BlockEntry) {
	tx.put_vec(
		DATA_COL,
		&block_entry_key(block_hash)[..],
		entry.encode(),
	);
}

pub(crate) fn write_candidate_entry(tx: &mut DBTransaction, candidate_hash: &CandidateHash, entry: &CandidateEntry) {
	tx.put_vec(
		DATA_COL,
		&candidate_entry_key(candidate_hash)[..],
		entry.encode(),
	);
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
		slot: Slot::from(1),
		relay_vrf_story: [0u8; 32],
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
	let store = kvdb_memorydb::create(1);

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
				our_assignment: None,
				assignments: Default::default(),
				approved: false,
			})
		].into_iter().collect(),
		approvals: Default::default(),
	};

	let mut tx = DBTransaction::new();

	write_stored_blocks(&mut tx, range.clone());
	write_blocks_at_height(&mut tx, 1, &at_height);
	write_block_entry(&mut tx, &hash_a, &block_entry);
	write_candidate_entry(&mut tx, &candidate_hash, &candidate_entry);

	store.write(tx).unwrap();

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

	let mut tx = DBTransaction::new();
	for key in delete_keys {
		tx.delete(DATA_COL, &key[..]);
	}

	store.write(tx).unwrap();

	assert!(load_stored_blocks(&store).unwrap().is_none());
	assert!(load_blocks_at_height(&store, 1).unwrap().is_empty());
	assert!(load_block_entry(&store, &hash_a).unwrap().is_none());
	assert!(load_candidate_entry(&store, &candidate_hash).unwrap().is_none());
}

#[test]
fn add_block_entry_works() {
	let store = kvdb_memorydb::create(1);

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

#[test]
fn add_block_entry_adds_child() {
	let store = kvdb_memorydb::create(1);

	let parent_hash = Hash::repeat_byte(1);
	let block_hash_a = Hash::repeat_byte(2);
	let block_hash_b = Hash::repeat_byte(69);

	let mut block_entry_a = make_block_entry(
		block_hash_a,
		Vec::new(),
	);

	let block_entry_b = make_block_entry(
		block_hash_b,
		Vec::new(),
	);

	let n_validators = 10;

	add_block_entry(
		&store,
		parent_hash,
		1,
		block_entry_a.clone(),
		n_validators,
		|_| None,
	).unwrap();

	add_block_entry(
		&store,
		block_hash_a,
		2,
		block_entry_b.clone(),
		n_validators,
		|_| None,
	).unwrap();

	block_entry_a.children.push(block_hash_b);

	assert_eq!(load_block_entry(&store, &block_hash_a).unwrap(), Some(block_entry_a));
	assert_eq!(load_block_entry(&store, &block_hash_b).unwrap(), Some(block_entry_b));
}

#[test]
fn canonicalize_works() {
	let store = kvdb_memorydb::create(1);

	//   -> B1 -> C1 -> D1
	// A -> B2 -> C2 -> D2
	//
	// We'll canonicalize C1. Everytning except D1 should disappear.
	//
	// Candidates:
	// Cand1 in B2
	// Cand2 in C2
	// Cand3 in C2 and D1
	// Cand4 in D1
	// Cand5 in D2
	// Only Cand3 and Cand4 should remain after canonicalize.

	let n_validators = 10;

	let mut tx = DBTransaction::new();
	write_stored_blocks(&mut tx, StoredBlockRange(1, 5));
	store.write(tx).unwrap();

	let genesis = Hash::repeat_byte(0);

	let block_hash_a = Hash::repeat_byte(1);
	let block_hash_b1 = Hash::repeat_byte(2);
	let block_hash_b2 = Hash::repeat_byte(3);
	let block_hash_c1 = Hash::repeat_byte(4);
	let block_hash_c2 = Hash::repeat_byte(5);
	let block_hash_d1 = Hash::repeat_byte(6);
	let block_hash_d2 = Hash::repeat_byte(7);

	let cand_hash_1 = CandidateHash(Hash::repeat_byte(10));
	let cand_hash_2 = CandidateHash(Hash::repeat_byte(11));
	let cand_hash_3 = CandidateHash(Hash::repeat_byte(12));
	let cand_hash_4 = CandidateHash(Hash::repeat_byte(13));
	let cand_hash_5 = CandidateHash(Hash::repeat_byte(15));

	let block_entry_a = make_block_entry(block_hash_a, Vec::new());
	let block_entry_b1 = make_block_entry(block_hash_b1, Vec::new());
	let block_entry_b2 = make_block_entry(block_hash_b2, vec![(CoreIndex(0), cand_hash_1)]);
	let block_entry_c1 = make_block_entry(block_hash_c1, Vec::new());
	let block_entry_c2 = make_block_entry(
		block_hash_c2,
		vec![(CoreIndex(0), cand_hash_2), (CoreIndex(1), cand_hash_3)],
	);
	let block_entry_d1 = make_block_entry(
		block_hash_d1,
		vec![(CoreIndex(0), cand_hash_3), (CoreIndex(1), cand_hash_4)],
	);
	let block_entry_d2 = make_block_entry(block_hash_d2, vec![(CoreIndex(0), cand_hash_5)]);


	let candidate_info = {
		let mut candidate_info = HashMap::new();
		candidate_info.insert(cand_hash_1, NewCandidateInfo {
			candidate: make_candidate(1.into(), genesis),
			backing_group: GroupIndex(1),
			our_assignment: None,
		});

		candidate_info.insert(cand_hash_2, NewCandidateInfo {
			candidate: make_candidate(2.into(), block_hash_a),
			backing_group: GroupIndex(2),
			our_assignment: None,
		});

		candidate_info.insert(cand_hash_3, NewCandidateInfo {
			candidate: make_candidate(3.into(), block_hash_a),
			backing_group: GroupIndex(3),
			our_assignment: None,
		});

		candidate_info.insert(cand_hash_4, NewCandidateInfo {
			candidate: make_candidate(4.into(), block_hash_b1),
			backing_group: GroupIndex(4),
			our_assignment: None,
		});

		candidate_info.insert(cand_hash_5, NewCandidateInfo {
			candidate: make_candidate(5.into(), block_hash_c1),
			backing_group: GroupIndex(5),
			our_assignment: None,
		});

		candidate_info
	};

	// now insert all the blocks.
	let blocks = vec![
		(genesis, 1, block_entry_a.clone()),
		(block_hash_a, 2, block_entry_b1.clone()),
		(block_hash_a, 2, block_entry_b2.clone()),
		(block_hash_b1, 3, block_entry_c1.clone()),
		(block_hash_b2, 3, block_entry_c2.clone()),
		(block_hash_c1, 4, block_entry_d1.clone()),
		(block_hash_c2, 4, block_entry_d2.clone()),
	];

	for (parent_hash, number, block_entry) in blocks {
		add_block_entry(
			&store,
			parent_hash,
			number,
			block_entry,
			n_validators,
			|h| candidate_info.get(h).map(|x| x.clone()),
		).unwrap();
	}

	let check_candidates_in_store = |expected: Vec<(CandidateHash, Option<Vec<_>>)>| {
		for (c_hash, in_blocks) in expected {
			let (entry, in_blocks) = match in_blocks {
				None => {
					assert!(load_candidate_entry(&store, &c_hash).unwrap().is_none());
					continue
				}
				Some(i) => (
					load_candidate_entry(&store, &c_hash).unwrap().unwrap(),
					i,
				),
			};

			assert_eq!(entry.block_assignments.len(), in_blocks.len());

			for x in in_blocks {
				assert!(entry.block_assignments.contains_key(&x));
			}
		}
	};

	let check_blocks_in_store = |expected: Vec<(Hash, Option<Vec<_>>)>| {
		for (hash, with_candidates) in expected {
			let (entry, with_candidates) = match with_candidates {
				None => {
					assert!(load_block_entry(&store, &hash).unwrap().is_none());
					continue
				}
				Some(i) => (
					load_block_entry(&store, &hash).unwrap().unwrap(),
					i,
				),
			};

			assert_eq!(entry.candidates.len(), with_candidates.len());

			for x in with_candidates {
				assert!(entry.candidates.iter().position(|&(_, ref c)| c == &x).is_some());
			}
		}
	};

	check_candidates_in_store(vec![
		(cand_hash_1, Some(vec![block_hash_b2])),
		(cand_hash_2, Some(vec![block_hash_c2])),
		(cand_hash_3, Some(vec![block_hash_c2, block_hash_d1])),
		(cand_hash_4, Some(vec![block_hash_d1])),
		(cand_hash_5, Some(vec![block_hash_d2])),
	]);

	check_blocks_in_store(vec![
		(block_hash_a, Some(vec![])),
		(block_hash_b1, Some(vec![])),
		(block_hash_b2, Some(vec![cand_hash_1])),
		(block_hash_c1, Some(vec![])),
		(block_hash_c2, Some(vec![cand_hash_2, cand_hash_3])),
		(block_hash_d1, Some(vec![cand_hash_3, cand_hash_4])),
		(block_hash_d2, Some(vec![cand_hash_5])),
	]);

	canonicalize(&store, 3, block_hash_c1).unwrap();

	assert_eq!(load_stored_blocks(&store).unwrap().unwrap(), StoredBlockRange(4, 5));

	check_candidates_in_store(vec![
		(cand_hash_1, None),
		(cand_hash_2, None),
		(cand_hash_3, Some(vec![block_hash_d1])),
		(cand_hash_4, Some(vec![block_hash_d1])),
		(cand_hash_5, None),
	]);

	check_blocks_in_store(vec![
		(block_hash_a, None),
		(block_hash_b1, None),
		(block_hash_b2, None),
		(block_hash_c1, None),
		(block_hash_c2, None),
		(block_hash_d1, Some(vec![cand_hash_3, cand_hash_4])),
		(block_hash_d2, None),
	]);
}
