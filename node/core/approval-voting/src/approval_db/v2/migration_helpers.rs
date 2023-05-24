// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Approval DB migration helpers.
use super::{StoredBlockRange, *};
use crate::backend::Backend;
use polkadot_node_primitives::approval::v1::{
	AssignmentCert, AssignmentCertKind, VrfOutput, VrfProof, VrfSignature, RELAY_VRF_MODULO_CONTEXT,
};
use polkadot_node_subsystem_util::database::Database;
use std::{collections::HashSet, sync::Arc};

use ::test_helpers::dummy_candidate_receipt;

fn dummy_assignment_cert(kind: AssignmentCertKind) -> AssignmentCert {
	let ctx = schnorrkel::signing_context(RELAY_VRF_MODULO_CONTEXT);
	let msg = b"test-garbage";
	let mut prng = rand_core::OsRng;
	let keypair = schnorrkel::Keypair::generate_with(&mut prng);
	let (inout, proof, _) = keypair.vrf_sign(ctx.bytes(msg));
	let out = inout.to_output();

	AssignmentCert { kind, vrf: VrfSignature { output: VrfOutput(out), proof: VrfProof(proof) } }
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

fn make_bitvec(len: usize) -> BitVec<u8, BitOrderLsb0> {
	bitvec::bitvec![u8, BitOrderLsb0; 0; len]
}

pub fn dummy_assignment_bitfield() -> CoreBitfield {
	vec![
		CoreIndex(0),
		CoreIndex(1),
		CoreIndex(2),
		CoreIndex(3),
		CoreIndex(4),
		CoreIndex(5),
		CoreIndex(6),
		CoreIndex(7),
	]
	.try_into()
	.expect("If failed, `CoreBitfield` is broken; qed")
}

/// Migrates `OurAssignment`, `CandidateEntry` and `ApprovalEntry` to version 2.
/// Returns on any error.
/// Must only be used in parachains DB migration code - `polkadot-service` crate.
pub fn migrate_approval_db_v1_to_v2(db: Arc<dyn Database>, config: Config) -> Result<()> {
	let mut backend = crate::DbBackend::new(db, config);
	let all_blocks = backend
		.load_all_blocks()
		.map_err(|e| Error::InternalError(e))?
		.iter()
		.filter_map(|block_hash| {
			backend.load_block_entry(block_hash).map_err(|e| Error::InternalError(e)).ok()?
		})
		.collect::<Vec<_>>();

	gum::info!(
		target: crate::LOG_TARGET,
		"Migrating candidate entries on top of {} blocks",
		all_blocks.len()
	);

	let mut overlay = crate::OverlayedBackend::new(&backend);
	let mut counter = 0;
	// Get all candidate entries, approval entries and convert each of them.
	for block in all_blocks {
		for (core_index, candidate_hash) in block.candidates() {
			// Loading the candidate will also perform the conversion to the updated format and return
			// that represantation.
			if let Some(mut candidate_entry) = backend
				.load_candidate_entry_v1(&candidate_hash)
				.map_err(|e| Error::InternalError(e))?
			{
				// Here we patch the core bitfield for all assignments of the candidate.
				for (_, approval_entry) in candidate_entry.block_assignments.iter_mut() {
					if let Some(our_assignment) = approval_entry.our_assignment_mut() {
						// Ensure we are actually patching a dummy bitfield produced by the `load_candidate_entry_v1` code.
						// Cannot happen in practice, but better double check.
						if our_assignment.assignment_bitfield() == &dummy_assignment_bitfield() {
							*our_assignment.assignment_bitfield_mut() = (*core_index).into();
						} else {
							gum::warn!(
								target: crate::LOG_TARGET,
								"Tried to convert an already valid bitfield."
							);
						}
					}
				}
				// Write the updated representation.
				overlay.write_candidate_entry(candidate_entry);
				counter += 1;
			}
		}
	}

	gum::info!(target: crate::LOG_TARGET, "Migrated {} entries", counter);

	// Commit all changes to DB.
	let write_ops = overlay.into_write_ops();
	backend.write(write_ops).unwrap();

	Ok(())
}

// Checks if the migration doesn't leave the DB in an unsane state.
// This function is to be used in tests.
pub fn migrate_approval_db_v1_to_v2_sanity_check(
	db: Arc<dyn Database>,
	config: Config,
	expected_candidates: HashSet<CandidateHash>,
) -> Result<()> {
	let backend = crate::DbBackend::new(db, config);

	let all_blocks = backend
		.load_all_blocks()
		.map_err(|e| Error::InternalError(e))?
		.iter()
		.filter_map(|block_hash| {
			backend.load_block_entry(block_hash).map_err(|e| Error::InternalError(e)).ok()?
		})
		.collect::<Vec<_>>();

	let mut candidates = HashSet::new();

	// Iterate all blocks and approval entries.
	for block in all_blocks {
		for (core_index, candidate_hash) in block.candidates() {
			// Loading the candidate will also perform the conversion to the updated format and return
			// that represantation.
			if let Some(mut candidate_entry) = backend
				.load_candidate_entry(&candidate_hash)
				.map_err(|e| Error::InternalError(e))?
			{
				// We expect that all assignment bitfieds have only one bit set which corresponds to the core_index in the
				// candidates block entry mapping.
				for (_, approval_entry) in candidate_entry.block_assignments.iter_mut() {
					if let Some(our_assignment) = approval_entry.our_assignment_mut() {
						assert_eq!(our_assignment.assignment_bitfield().count_ones(), 1);
						assert_eq!(
							our_assignment.assignment_bitfield().first_one().unwrap(),
							core_index.0 as usize
						);
					}
				}
				candidates.insert(candidate_entry.candidate.hash());
			}
		}
	}

	assert_eq!(candidates, expected_candidates);

	Ok(())
}

// Fills the db with dummy data in v1 scheme.
pub fn migrate_approval_db_v1_to_v2_fill_test_data(
	db: Arc<dyn Database>,
	config: Config,
) -> Result<HashSet<CandidateHash>> {
	let mut backend = crate::DbBackend::new(db.clone(), config);
	let mut overlay_db = crate::OverlayedBackend::new(&backend);
	let mut expected_candidates = HashSet::new();

	const RELAY_BLOCK_COUNT: u32 = 10;

	let range = StoredBlockRange(1, 11);
	overlay_db.write_stored_block_range(range.clone());

	for relay_number in 1..=RELAY_BLOCK_COUNT {
		let relay_hash = Hash::repeat_byte(relay_number as u8);
		let assignment_core_index = CoreIndex(relay_number);
		let candidate = dummy_candidate_receipt(relay_hash);
		let candidate_hash = candidate.hash();

		let at_height = vec![relay_hash];

		let block_entry = make_block_entry(
			relay_hash,
			Default::default(),
			relay_number,
			vec![(assignment_core_index, candidate_hash)],
		);

		let dummy_assignment = crate::approval_db::v1::OurAssignment {
			cert: dummy_assignment_cert(AssignmentCertKind::RelayVRFModulo { sample: 0 }).into(),
			tranche: 0,
			validator_index: ValidatorIndex(0),
			triggered: false,
		};

		let candidate_entry = crate::approval_db::v1::CandidateEntry {
			candidate,
			session: 123,
			block_assignments: vec![(
				relay_hash,
				crate::approval_db::v1::ApprovalEntry {
					tranches: Vec::new(),
					backing_group: GroupIndex(1),
					our_assignment: Some(dummy_assignment),
					our_approval_sig: None,
					assignments: Default::default(),
					approved: false,
				},
			)]
			.into_iter()
			.collect(),
			approvals: Default::default(),
		};

		overlay_db.write_blocks_at_height(relay_number, at_height.clone());
		overlay_db.write_block_entry(block_entry.clone().into());

		expected_candidates.insert(candidate_entry.candidate.hash());
		db.write(write_candidate_entry_v1(candidate_entry, config.clone())).unwrap();
	}

	let write_ops = overlay_db.into_write_ops();
	backend.write(write_ops).unwrap();

	Ok(expected_candidates)
}

// Low level DB helper to write a candidate entry in v1 scheme.
fn write_candidate_entry_v1(
	candidate_entry: crate::approval_db::v1::CandidateEntry,
	config: Config,
) -> DBTransaction {
	let mut tx = DBTransaction::new();
	tx.put_vec(
		config.col_approval_data,
		&candidate_entry_key(&candidate_entry.candidate.hash()),
		candidate_entry.encode(),
	);
	tx
}
