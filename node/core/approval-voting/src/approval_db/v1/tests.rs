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

use super::{DbBackend, StoredBlockRange, *};
use crate::{
	backend::{Backend, OverlayedBackend},
	ops::{add_block_entry, canonicalize, force_approve, NewCandidateInfo},
};
use polkadot_node_subsystem_util::database::Database;
use polkadot_primitives::v2::Id as ParaId;
use std::{collections::HashMap, sync::Arc};

use ::test_helpers::{dummy_candidate_receipt, dummy_candidate_receipt_bad_sig, dummy_hash};

const DATA_COL: u32 = 0;
const SESSION_DATA_COL: u32 = 1;

const NUM_COLUMNS: u32 = 2;

const TEST_CONFIG: Config =
	Config { col_approval_data: DATA_COL, col_session_data: SESSION_DATA_COL };

fn make_db() -> (DbBackend, Arc<dyn Database>) {
	let db = kvdb_memorydb::create(NUM_COLUMNS);
	let db = polkadot_node_subsystem_util::database::kvdb_impl::DbAdapter::new(db, &[]);
	let db_writer: Arc<dyn Database> = Arc::new(db);
	(DbBackend::new(db_writer.clone(), TEST_CONFIG), db_writer)
}

fn make_bitvec(len: usize) -> BitVec<u8, BitOrderLsb0> {
	bitvec::bitvec![u8, BitOrderLsb0; 0; len]
}

fn make_block_entry(
	block_hash: Hash,
	parent_hash: Hash,
	block_number: BlockNumber,
	candidates: Vec<(CoreIndex, CandidateHash)>,
) -> BlockEntry {
	BlockEntry {
		block_hash,
		parent_hash,
		block_number,
		session: 1,
		slot: Slot::from(1),
		relay_vrf_story: [0u8; 32],
		approved_bitfield: make_bitvec(candidates.len()),
		candidates,
		children: Vec::new(),
	}
}

fn make_candidate(para_id: ParaId, relay_parent: Hash) -> CandidateReceipt {
	let mut c = dummy_candidate_receipt(dummy_hash());

	c.descriptor.para_id = para_id;
	c.descriptor.relay_parent = relay_parent;

	c
}

#[test]
fn read_write() {
	let (mut db, store) = make_db();

	let hash_a = Hash::repeat_byte(1);
	let hash_b = Hash::repeat_byte(2);
	let candidate_hash = dummy_candidate_receipt_bad_sig(dummy_hash(), None).hash();

	let range = StoredBlockRange(10, 20);
	let at_height = vec![hash_a, hash_b];

	let block_entry =
		make_block_entry(hash_a, Default::default(), 1, vec![(CoreIndex(0), candidate_hash)]);

	let candidate_entry = CandidateEntry {
		candidate: dummy_candidate_receipt_bad_sig(dummy_hash(), None),
		session: 5,
		block_assignments: vec![(
			hash_a,
			ApprovalEntry {
				tranches: Vec::new(),
				backing_group: GroupIndex(1),
				our_assignment: None,
				our_approval_sig: None,
				assignments: Default::default(),
				approved: false,
			},
		)]
		.into_iter()
		.collect(),
		approvals: Default::default(),
	};

	let mut overlay_db = OverlayedBackend::new(&db);
	overlay_db.write_stored_block_range(range.clone());
	overlay_db.write_blocks_at_height(1, at_height.clone());
	overlay_db.write_block_entry(block_entry.clone().into());
	overlay_db.write_candidate_entry(candidate_entry.clone().into());

	let write_ops = overlay_db.into_write_ops();
	db.write(write_ops).unwrap();

	assert_eq!(load_stored_blocks(store.as_ref(), &TEST_CONFIG).unwrap(), Some(range));
	assert_eq!(load_blocks_at_height(store.as_ref(), &TEST_CONFIG, &1).unwrap(), at_height);
	assert_eq!(
		load_block_entry(store.as_ref(), &TEST_CONFIG, &hash_a).unwrap(),
		Some(block_entry.into())
	);
	assert_eq!(
		load_candidate_entry(store.as_ref(), &TEST_CONFIG, &candidate_hash).unwrap(),
		Some(candidate_entry.into()),
	);

	let mut overlay_db = OverlayedBackend::new(&db);
	overlay_db.delete_blocks_at_height(1);
	overlay_db.delete_block_entry(&hash_a);
	overlay_db.delete_candidate_entry(&candidate_hash);
	let write_ops = overlay_db.into_write_ops();
	db.write(write_ops).unwrap();

	assert!(load_blocks_at_height(store.as_ref(), &TEST_CONFIG, &1).unwrap().is_empty());
	assert!(load_block_entry(store.as_ref(), &TEST_CONFIG, &hash_a).unwrap().is_none());
	assert!(load_candidate_entry(store.as_ref(), &TEST_CONFIG, &candidate_hash)
		.unwrap()
		.is_none());
}

#[test]
fn add_block_entry_works() {
	let (mut db, store) = make_db();

	let parent_hash = Hash::repeat_byte(1);
	let block_hash_a = Hash::repeat_byte(2);
	let block_hash_b = Hash::repeat_byte(69);

	let candidate_receipt_a = make_candidate(ParaId::from(1_u32), parent_hash);
	let candidate_receipt_b = make_candidate(ParaId::from(2_u32), parent_hash);

	let candidate_hash_a = candidate_receipt_a.hash();
	let candidate_hash_b = candidate_receipt_b.hash();

	let block_number = 10;

	let block_entry_a = make_block_entry(
		block_hash_a,
		parent_hash,
		block_number,
		vec![(CoreIndex(0), candidate_hash_a)],
	);

	let block_entry_b = make_block_entry(
		block_hash_b,
		parent_hash,
		block_number,
		vec![(CoreIndex(0), candidate_hash_a), (CoreIndex(1), candidate_hash_b)],
	);

	let n_validators = 10;

	let mut new_candidate_info = HashMap::new();
	new_candidate_info
		.insert(candidate_hash_a, NewCandidateInfo::new(candidate_receipt_a, GroupIndex(0), None));

	let mut overlay_db = OverlayedBackend::new(&db);
	add_block_entry(&mut overlay_db, block_entry_a.clone().into(), n_validators, |h| {
		new_candidate_info.get(h).map(|x| x.clone())
	})
	.unwrap();
	let write_ops = overlay_db.into_write_ops();
	db.write(write_ops).unwrap();

	new_candidate_info
		.insert(candidate_hash_b, NewCandidateInfo::new(candidate_receipt_b, GroupIndex(1), None));

	let mut overlay_db = OverlayedBackend::new(&db);
	add_block_entry(&mut overlay_db, block_entry_b.clone().into(), n_validators, |h| {
		new_candidate_info.get(h).map(|x| x.clone())
	})
	.unwrap();
	let write_ops = overlay_db.into_write_ops();
	db.write(write_ops).unwrap();

	assert_eq!(
		load_block_entry(store.as_ref(), &TEST_CONFIG, &block_hash_a).unwrap(),
		Some(block_entry_a.into())
	);
	assert_eq!(
		load_block_entry(store.as_ref(), &TEST_CONFIG, &block_hash_b).unwrap(),
		Some(block_entry_b.into())
	);

	let candidate_entry_a = load_candidate_entry(store.as_ref(), &TEST_CONFIG, &candidate_hash_a)
		.unwrap()
		.unwrap();
	assert_eq!(
		candidate_entry_a.block_assignments.keys().collect::<Vec<_>>(),
		vec![&block_hash_a, &block_hash_b]
	);

	let candidate_entry_b = load_candidate_entry(store.as_ref(), &TEST_CONFIG, &candidate_hash_b)
		.unwrap()
		.unwrap();
	assert_eq!(candidate_entry_b.block_assignments.keys().collect::<Vec<_>>(), vec![&block_hash_b]);
}

#[test]
fn add_block_entry_adds_child() {
	let (mut db, store) = make_db();

	let parent_hash = Hash::repeat_byte(1);
	let block_hash_a = Hash::repeat_byte(2);
	let block_hash_b = Hash::repeat_byte(69);

	let mut block_entry_a = make_block_entry(block_hash_a, parent_hash, 1, Vec::new());

	let block_entry_b = make_block_entry(block_hash_b, block_hash_a, 2, Vec::new());

	let n_validators = 10;

	let mut overlay_db = OverlayedBackend::new(&db);
	add_block_entry(&mut overlay_db, block_entry_a.clone().into(), n_validators, |_| None).unwrap();

	add_block_entry(&mut overlay_db, block_entry_b.clone().into(), n_validators, |_| None).unwrap();

	let write_ops = overlay_db.into_write_ops();
	db.write(write_ops).unwrap();

	block_entry_a.children.push(block_hash_b);

	assert_eq!(
		load_block_entry(store.as_ref(), &TEST_CONFIG, &block_hash_a).unwrap(),
		Some(block_entry_a.into())
	);
	assert_eq!(
		load_block_entry(store.as_ref(), &TEST_CONFIG, &block_hash_b).unwrap(),
		Some(block_entry_b.into())
	);
}

#[test]
fn canonicalize_works() {
	let (mut db, store) = make_db();

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

	let mut overlay_db = OverlayedBackend::new(&db);
	overlay_db.write_stored_block_range(StoredBlockRange(1, 5));
	let write_ops = overlay_db.into_write_ops();
	db.write(write_ops).unwrap();

	let genesis = Hash::repeat_byte(0);

	let block_hash_a = Hash::repeat_byte(1);
	let block_hash_b1 = Hash::repeat_byte(2);
	let block_hash_b2 = Hash::repeat_byte(3);
	let block_hash_c1 = Hash::repeat_byte(4);
	let block_hash_c2 = Hash::repeat_byte(5);
	let block_hash_d1 = Hash::repeat_byte(6);
	let block_hash_d2 = Hash::repeat_byte(7);

	let candidate_receipt_genesis = make_candidate(ParaId::from(1_u32), genesis);
	let candidate_receipt_a = make_candidate(ParaId::from(2_u32), block_hash_a);
	let candidate_receipt_b = make_candidate(ParaId::from(3_u32), block_hash_a);
	let candidate_receipt_b1 = make_candidate(ParaId::from(4_u32), block_hash_b1);
	let candidate_receipt_c1 = make_candidate(ParaId::from(5_u32), block_hash_c1);

	let cand_hash_1 = candidate_receipt_genesis.hash();
	let cand_hash_2 = candidate_receipt_a.hash();
	let cand_hash_3 = candidate_receipt_b.hash();
	let cand_hash_4 = candidate_receipt_b1.hash();
	let cand_hash_5 = candidate_receipt_c1.hash();

	let block_entry_a = make_block_entry(block_hash_a, genesis, 1, Vec::new());
	let block_entry_b1 = make_block_entry(block_hash_b1, block_hash_a, 2, Vec::new());
	let block_entry_b2 =
		make_block_entry(block_hash_b2, block_hash_a, 2, vec![(CoreIndex(0), cand_hash_1)]);
	let block_entry_c1 = make_block_entry(block_hash_c1, block_hash_b1, 3, Vec::new());
	let block_entry_c2 = make_block_entry(
		block_hash_c2,
		block_hash_b2,
		3,
		vec![(CoreIndex(0), cand_hash_2), (CoreIndex(1), cand_hash_3)],
	);
	let block_entry_d1 = make_block_entry(
		block_hash_d1,
		block_hash_c1,
		4,
		vec![(CoreIndex(0), cand_hash_3), (CoreIndex(1), cand_hash_4)],
	);
	let block_entry_d2 =
		make_block_entry(block_hash_d2, block_hash_c2, 4, vec![(CoreIndex(0), cand_hash_5)]);

	let candidate_info = {
		let mut candidate_info = HashMap::new();
		candidate_info.insert(
			cand_hash_1,
			NewCandidateInfo::new(candidate_receipt_genesis, GroupIndex(1), None),
		);

		candidate_info
			.insert(cand_hash_2, NewCandidateInfo::new(candidate_receipt_a, GroupIndex(2), None));

		candidate_info
			.insert(cand_hash_3, NewCandidateInfo::new(candidate_receipt_b, GroupIndex(3), None));

		candidate_info
			.insert(cand_hash_4, NewCandidateInfo::new(candidate_receipt_b1, GroupIndex(4), None));

		candidate_info
			.insert(cand_hash_5, NewCandidateInfo::new(candidate_receipt_c1, GroupIndex(5), None));

		candidate_info
	};

	// now insert all the blocks.
	let blocks = vec![
		block_entry_a.clone(),
		block_entry_b1.clone(),
		block_entry_b2.clone(),
		block_entry_c1.clone(),
		block_entry_c2.clone(),
		block_entry_d1.clone(),
		block_entry_d2.clone(),
	];

	let mut overlay_db = OverlayedBackend::new(&db);
	for block_entry in blocks {
		add_block_entry(&mut overlay_db, block_entry.into(), n_validators, |h| {
			candidate_info.get(h).map(|x| x.clone())
		})
		.unwrap();
	}
	let write_ops = overlay_db.into_write_ops();
	db.write(write_ops).unwrap();

	let check_candidates_in_store = |expected: Vec<(CandidateHash, Option<Vec<_>>)>| {
		for (c_hash, in_blocks) in expected {
			let (entry, in_blocks) = match in_blocks {
				None => {
					assert!(load_candidate_entry(store.as_ref(), &TEST_CONFIG, &c_hash)
						.unwrap()
						.is_none());
					continue
				},
				Some(i) => (
					load_candidate_entry(store.as_ref(), &TEST_CONFIG, &c_hash).unwrap().unwrap(),
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
					assert!(load_block_entry(store.as_ref(), &TEST_CONFIG, &hash)
						.unwrap()
						.is_none());
					continue
				},
				Some(i) =>
					(load_block_entry(store.as_ref(), &TEST_CONFIG, &hash).unwrap().unwrap(), i),
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

	let mut overlay_db = OverlayedBackend::new(&db);
	canonicalize(&mut overlay_db, 3, block_hash_c1).unwrap();
	let write_ops = overlay_db.into_write_ops();
	db.write(write_ops).unwrap();

	assert_eq!(
		load_stored_blocks(store.as_ref(), &TEST_CONFIG).unwrap().unwrap(),
		StoredBlockRange(4, 5)
	);

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

#[test]
fn force_approve_works() {
	let (mut db, store) = make_db();
	let n_validators = 10;

	let mut overlay_db = OverlayedBackend::new(&db);
	overlay_db.write_stored_block_range(StoredBlockRange(1, 4));
	let write_ops = overlay_db.into_write_ops();
	db.write(write_ops).unwrap();

	let candidate_hash = CandidateHash(Hash::repeat_byte(42));
	let single_candidate_vec = vec![(CoreIndex(0), candidate_hash)];
	let candidate_info = {
		let mut candidate_info = HashMap::new();
		candidate_info.insert(
			candidate_hash,
			NewCandidateInfo::new(
				make_candidate(ParaId::from(1_u32), Default::default()),
				GroupIndex(1),
				None,
			),
		);

		candidate_info
	};

	let block_hash_a = Hash::repeat_byte(1); // 1
	let block_hash_b = Hash::repeat_byte(2);
	let block_hash_c = Hash::repeat_byte(3);
	let block_hash_d = Hash::repeat_byte(4); // 4

	let block_entry_a =
		make_block_entry(block_hash_a, Default::default(), 1, single_candidate_vec.clone());
	let block_entry_b =
		make_block_entry(block_hash_b, block_hash_a, 2, single_candidate_vec.clone());
	let block_entry_c =
		make_block_entry(block_hash_c, block_hash_b, 3, single_candidate_vec.clone());
	let block_entry_d =
		make_block_entry(block_hash_d, block_hash_c, 4, single_candidate_vec.clone());

	let blocks = vec![
		block_entry_a.clone(),
		block_entry_b.clone(),
		block_entry_c.clone(),
		block_entry_d.clone(),
	];

	let mut overlay_db = OverlayedBackend::new(&db);
	for block_entry in blocks {
		add_block_entry(&mut overlay_db, block_entry.into(), n_validators, |h| {
			candidate_info.get(h).map(|x| x.clone())
		})
		.unwrap();
	}
	let approved_hashes = force_approve(&mut overlay_db, block_hash_d, 2).unwrap();
	let write_ops = overlay_db.into_write_ops();
	db.write(write_ops).unwrap();

	assert!(load_block_entry(store.as_ref(), &TEST_CONFIG, &block_hash_a,)
		.unwrap()
		.unwrap()
		.approved_bitfield
		.all());
	assert!(load_block_entry(store.as_ref(), &TEST_CONFIG, &block_hash_b,)
		.unwrap()
		.unwrap()
		.approved_bitfield
		.all());
	assert!(load_block_entry(store.as_ref(), &TEST_CONFIG, &block_hash_c,)
		.unwrap()
		.unwrap()
		.approved_bitfield
		.not_any());
	assert!(load_block_entry(store.as_ref(), &TEST_CONFIG, &block_hash_d,)
		.unwrap()
		.unwrap()
		.approved_bitfield
		.not_any());
	assert_eq!(approved_hashes, vec![block_hash_b, block_hash_a]);
}

#[test]
fn load_all_blocks_works() {
	let (mut db, store) = make_db();

	let parent_hash = Hash::repeat_byte(1);
	let block_hash_a = Hash::repeat_byte(2);
	let block_hash_b = Hash::repeat_byte(69);
	let block_hash_c = Hash::repeat_byte(42);

	let block_number = 10;

	let block_entry_a = make_block_entry(block_hash_a, parent_hash, block_number, vec![]);

	let block_entry_b = make_block_entry(block_hash_b, parent_hash, block_number, vec![]);

	let block_entry_c = make_block_entry(block_hash_c, block_hash_a, block_number + 1, vec![]);

	let n_validators = 10;

	let mut overlay_db = OverlayedBackend::new(&db);
	add_block_entry(&mut overlay_db, block_entry_a.clone().into(), n_validators, |_| None).unwrap();

	// add C before B to test sorting.
	add_block_entry(&mut overlay_db, block_entry_c.clone().into(), n_validators, |_| None).unwrap();

	add_block_entry(&mut overlay_db, block_entry_b.clone().into(), n_validators, |_| None).unwrap();

	let write_ops = overlay_db.into_write_ops();
	db.write(write_ops).unwrap();

	assert_eq!(
		load_all_blocks(store.as_ref(), &TEST_CONFIG).unwrap(),
		vec![block_hash_a, block_hash_b, block_hash_c],
	)
}
