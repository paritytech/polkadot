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


//! Helper functions and tools to generate mock data useful for testing this subsystem.

use std::sync::Arc;

use sp_keyring::Sr25519Keyring;

use polkadot_erasure_coding::{branches, obtain_chunks_v1 as obtain_chunks};
use polkadot_primitives::v1::{AvailableData, BlockData, CandidateCommitments, CandidateDescriptor,
	CandidateHash, CommittedCandidateReceipt, ErasureChunk, GroupIndex, Hash, HeadData, Id
	as ParaId, OccupiedCore, PersistedValidationData, PoV, SessionInfo,
	ValidatorIndex
};

/// Create dummy session info with two validator groups.
pub fn make_session_info() -> SessionInfo {
		let validators = vec![
			Sr25519Keyring::Ferdie, // <- this node, role: validator
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Eve,
			Sr25519Keyring::One,
		];

		let validator_groups: Vec<Vec<ValidatorIndex>> = [vec![5, 0, 3], vec![1, 6, 2, 4]]
			.iter().map(|g| g.into_iter().map(|v| ValidatorIndex(*v)).collect()).collect();

		SessionInfo {
			discovery_keys: validators.iter().map(|k| k.public().into()).collect(),
			// Not used:
			n_cores: validator_groups.len() as u32,
			validator_groups,
			// Not used values:
			validators: validators.iter().map(|k| k.public().into()).collect(),
			assignment_keys: Vec::new(),
			zeroth_delay_tranche_width: 0,
			relay_vrf_modulo_samples: 0,
			n_delay_tranches: 0,
			no_show_slots: 0,
			needed_approvals: 0,
		}
}

/// Builder for constructing occupied cores.
///
/// Takes all the values we care about and fills the rest with dummy values on `build`.
pub struct OccupiedCoreBuilder {
	pub group_responsible: GroupIndex,
	pub para_id: ParaId,
	pub relay_parent: Hash,
}

impl OccupiedCoreBuilder {
	pub fn build(self) -> (OccupiedCore, (CandidateHash, ErasureChunk)) {
		let pov = PoV {
			block_data: BlockData(vec![45, 46, 47]),
		};
		let pov_hash = pov.hash();
		let (erasure_root, chunk) = get_valid_chunk_data(pov.clone());
		let candidate_receipt = TestCandidateBuilder {
			para_id: self.para_id,
			pov_hash,
			relay_parent: self.relay_parent,
			erasure_root,
			..Default::default()
		}.build();
		let core = OccupiedCore {
			next_up_on_available: None,
			occupied_since: 0,
			time_out_at: 0,
			next_up_on_time_out: None,
			availability: Default::default(),
			group_responsible: self.group_responsible,
			candidate_hash: candidate_receipt.hash(),
			candidate_descriptor: candidate_receipt.descriptor().clone(),
		};
		(core, (candidate_receipt.hash(), chunk))
	}
}

#[derive(Default)]
pub struct TestCandidateBuilder {
	para_id: ParaId,
	head_data: HeadData,
	pov_hash: Hash,
	relay_parent: Hash,
	erasure_root: Hash,
}

impl TestCandidateBuilder {
	pub fn build(self) -> CommittedCandidateReceipt {
		CommittedCandidateReceipt {
			descriptor: CandidateDescriptor {
				para_id: self.para_id,
				pov_hash: self.pov_hash,
				relay_parent: self.relay_parent,
				erasure_root: self.erasure_root,
				..Default::default()
			},
			commitments: CandidateCommitments {
				head_data: self.head_data,
				..Default::default()
			},
		}
	}
}

pub fn get_valid_chunk_data(pov: PoV) -> (Hash, ErasureChunk) {
	let fake_validator_count = 10;
	let persisted = PersistedValidationData {
		parent_head: HeadData(vec![7, 8, 9]),
		relay_parent_number: Default::default(),
		max_pov_size: 1024,
		relay_parent_storage_root: Default::default(),
	};
	let available_data = AvailableData {
		validation_data: persisted, pov: Arc::new(pov),
	};
	let chunks = obtain_chunks(fake_validator_count, &available_data).unwrap();
	let branches = branches(chunks.as_ref());
	let root = branches.root();
	let chunk = branches.enumerate()
			.map(|(index, (proof, chunk))| ErasureChunk {
				chunk: chunk.to_vec(),
				index: ValidatorIndex(index as _),
				proof,
			})
			.next().expect("There really should be 10 chunks.");
	(root, chunk)
}
