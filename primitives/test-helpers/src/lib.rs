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

#![forbid(unused_crate_dependencies)]
#![forbid(unused_extern_crates)]

//! A set of primitive constructors, to aid in crafting meaningful testcase while reducing repetition.
//!
//! Note that `dummy_` prefixed values are meant to be fillers, that should not matter, and will
//! contain randomness based data.
use polkadot_primitives::{v1::{HeadData, ValidationCodeHash, CollatorId, CommittedCandidateReceipt, CandidateReceipt, CandidateDescriptor, Hash, Id as ParaId, ValidationCode}, v1::CandidateCommitments};

use sp_application_crypto::RuntimeAppPublic;

/// Creates a candidate receipt without
pub fn dummy_candidate_receipt<H: AsRef<[u8]>>(relay_parent: H) -> CandidateReceipt<H> {
	CandidateReceipt::<H> {
		commitments_hash: dummy_candidate_commitments(HeadData(vec![1,1,11])).hash(),
		descriptor: dummy_candidate_descriptor(relay_parent),
	}
}

/// Creates a committed candidate receipt without
pub fn dummy_committed_candidate_receipt<H: AsRef<[u8]>>(relay_parent: H) -> CommittedCandidateReceipt<H> {
	CommittedCandidateReceipt::<H> {
		descriptor: dummy_candidate_descriptor::<H>(relay_parent),
		commitments: dummy_candidate_commitments(HeadData(vec![1,1,11])),
	}
}

pub fn dummy_candidate_commitments(head_data: HeadData) -> CandidateCommitments {
	CandidateCommitments {
		head_data,
		upward_messages: vec![],
		new_validation_code: None,
		horizontal_messages: vec![],
		processed_downward_messages: 0,
		hrmp_watermark: 0_u32,
	}
}


/// Create meaningless dummy hash.
pub fn dummy_hash() -> Hash {
	// TODO make this PRNG based
	Hash::zero()
}

pub fn dummy_candidate_descriptor<H: AsRef<[u8]>>(relay_parent: H) -> CandidateDescriptor<H>{
	let collator = sp_keyring::AccountKeyring::Ferdie.public();
	let invalid = Hash::zero();
	let descriptor = make_valid_candidate_descriptor(1.into(), relay_parent, invalid, invalid, invalid, invalid, invalid, collator);
	descriptor
}

pub fn dummy_validation_code() -> ValidationCode {
	ValidationCode(vec![1,2,3])
}

/// Create a new candidate descripter, and applies a valid siganture
/// using the provided `CollatorId` key.
pub fn make_valid_candidate_descriptor<H: AsRef<[u8]>>(
	para_id: ParaId,
	relay_parent: H,
	persisted_validation_data_hash: Hash,
	pov_hash: Hash,
	validation_code_hash: impl Into<ValidationCodeHash>,
	para_head: Hash,
	erasure_root: Hash,
	collator: impl Into<CollatorId>,
) -> CandidateDescriptor<H> {
	let collator: CollatorId = collator.into();
	let validation_code_hash = validation_code_hash.into();
	let payload = polkadot_primitives::v1::collator_signature_payload::<H>(
		&relay_parent,
		&para_id,
		&persisted_validation_data_hash,
		&pov_hash,
		&validation_code_hash,
	);

	let signature = collator.sign(&payload).unwrap();
	let descriptor = CandidateDescriptor {
		para_id,
		relay_parent,
		collator,
		persisted_validation_data_hash,
		pov_hash,
		erasure_root,
		signature,
		para_head,
		validation_code_hash,
	};

	assert!(descriptor.check_collator_signature().is_ok());
	descriptor
}


/// Re-sign after manually modifying a candidate descriptor.
pub fn resign_candidate_descriptor<H: AsRef<[u8]>>(descriptor: &mut CandidateDescriptor<H>) {
	let collator: &CollatorId = &descriptor.collator;
	let payload = polkadot_primitives::v1::collator_signature_payload::<H>(
		&descriptor.relay_parent,
		&descriptor.para_id,
		&descriptor.persisted_validation_data_hash,
		&descriptor.pov_hash,
		&descriptor.validation_code_hash,
	);
	let signature = collator.sign(&payload).unwrap();
	descriptor.signature = signature;
}


/// After manually modifyin the candidate descriptor, resign with a defined collator key.
pub fn resign_candidate_descriptor_with_collator<H: AsRef<[u8]>>(descriptor: &mut CandidateDescriptor<H>, signer: impl Into<CollatorId>) {
	let collator: CollatorId = signer.into();
	let payload = polkadot_primitives::v1::collator_signature_payload::<H>(
		&descriptor.relay_parent,
		&descriptor.para_id,
		&descriptor.persisted_validation_data_hash,
		&descriptor.pov_hash,
		&descriptor.validation_code_hash,
	);
	let signature = collator.sign(&payload).unwrap();
	descriptor.signature = signature;
}
