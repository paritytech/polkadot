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
use polkadot_primitives::v1::{ValidationCodeHash, CollatorId, CandidateReceipt, CandidateDescriptor, Hash, Id as ParaId};

use sp_application_crypto::RuntimeAppPublic;

/// Creates a candidate receipt without
pub fn dummy_candidate_receipt<H: AsRef<[u8]>>(relay_parent: H) -> CandidateReceipt<H> {
	let collator = sp_keyring::AccountKeyring::Two.public();
	// TODO make this PRNG based
	let invalid = Hash::zero();
	let descriptor = make_candidate_descriptor(1.into(), relay_parent, invalid, invalid, invalid, invalid, invalid, collator);
	CandidateReceipt::<H> {
		commitments_hash: Hash::zero(),
		descriptor,
	}
}

/// Create meaningless dummy hash.
pub fn dummy_hash() -> Hash {
	// TODO make this PRNG based
	Hash::zero()
}

/// Create a new candidate descripter, and applies a valid siganture
/// using the provided `CollatorId` key.
pub fn make_candidate_descriptor<H: AsRef<[u8]>>(
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
