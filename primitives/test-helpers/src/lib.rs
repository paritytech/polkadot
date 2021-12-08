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
use polkadot_primitives::v1::{
	HeadData, ValidationCodeHash, CommittedCandidateReceipt, CandidateReceipt, CandidateDescriptor,
	Hash, Id as ParaId, ValidationCode, CandidateCommitments, CollatorId, CollatorSignature
};
use sp_keyring::Sr25519Keyring;
use sp_application_crypto::sr25519;

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

// pub fn dummy_candidate_receipt_bad_sig<H: AsRef<[u8]>>(relay_parent: H, commitments: Option<Hash>) -> CandidateReceipt<Hash> {
pub fn dummy_candidate_receipt_bad_sig(relay_parent: Hash, commitments: Option<Hash>) -> CandidateReceipt<Hash> {
	CandidateReceipt::<Hash> {
		commitments_hash: if let Some(c) = commitments {
			c
		} else {
			dummy_candidate_commitments(HeadData(vec![1,1,11])).hash()
			// Default::default()
		},
		descriptor: dummy_candidate_descriptor_bad_sig(relay_parent),
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

// pub fn dummy_candidate_descriptor_bad_sig<H: AsRef<[u8]>>(relay_parent: H) -> CandidateDescriptor<Hash> {
pub fn dummy_candidate_descriptor_bad_sig(relay_parent: Hash) -> CandidateDescriptor<Hash> {
	// let invalid = Hash::zero();
	// CandidateDescriptor {
	// 	para_id: 0.into(),
	// 	relay_parent,
	// 	collator: CollatorId::from(sr25519::Public::from_raw([42; 32])),
	// 	persisted_validation_data_hash: invalid,
	// 	pov_hash: invalid,
	// 	erasure_root: invalid,
	// 	signature: CollatorSignature::from(sr25519::Signature([0u8;64])),
	// 	para_head: invalid,
	// 	validation_code_hash: invalid.into(),
	// }

	// CandidateDescriptor::dummy(CollatorId::from(sr25519::Public::from_raw([42; 32])))

	let zeros = Hash::zero();
	// let zeros = Default::default();
	CandidateDescriptor::<Hash> {
		para_id: 0.into(),
		relay_parent,
		collator: CollatorId::from(sr25519::Public::from_raw([42; 32])),
		persisted_validation_data_hash: zeros,
		pov_hash: zeros,
		erasure_root: zeros,
		signature: CollatorSignature::from(sr25519::Signature([0u8;64])),
		para_head: zeros,
		validation_code_hash: ValidationCode(vec![1,2,3]).hash(),
	}
}

pub fn dummy_candidate_descriptor<H: AsRef<[u8]>>(relay_parent: H) -> CandidateDescriptor<H>{
	let collator = sp_keyring::Sr25519Keyring::Ferdie;
	let invalid = Hash::zero();
	let descriptor = make_valid_candidate_descriptor(1.into(), relay_parent, invalid, invalid, invalid, invalid, invalid, collator);
	descriptor
}

pub fn dummy_validation_code() -> ValidationCode {
	ValidationCode(vec![1,2,3])
}

/// Create a new candidate descripter, and applies a valid signature
/// using the provided `CollatorId` key.
pub fn make_valid_candidate_descriptor<H: AsRef<[u8]>>(
	para_id: ParaId,
	relay_parent: H,
	persisted_validation_data_hash: Hash,
	pov_hash: Hash,
	validation_code_hash: impl Into<ValidationCodeHash>,
	para_head: Hash,
	erasure_root: Hash,
	collator: Sr25519Keyring,
) -> CandidateDescriptor<H> {
	let validation_code_hash = validation_code_hash.into();
	let payload = polkadot_primitives::v1::collator_signature_payload::<H>(
		&relay_parent,
		&para_id,
		&persisted_validation_data_hash,
		&pov_hash,
		&validation_code_hash,
	);

	let signature = collator.sign(&payload).into();
	let descriptor = CandidateDescriptor {
		para_id,
		relay_parent,
		collator: collator.public().into(),
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

/// After manually modifyin the candidate descriptor, resign with a defined collator key.
pub fn resign_candidate_descriptor_with_collator<H: AsRef<[u8]>>(descriptor: &mut CandidateDescriptor<H>, collator: Sr25519Keyring) {
	descriptor.collator = collator.public().into();
	let payload = polkadot_primitives::v1::collator_signature_payload::<H>(
		&descriptor.relay_parent,
		&descriptor.para_id,
		&descriptor.persisted_validation_data_hash,
		&descriptor.pov_hash,
		&descriptor.validation_code_hash,
	);
	let signature = collator.sign(&payload).into();
	descriptor.signature = signature;
}
