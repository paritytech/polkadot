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
use polkadot_primitives::v2::{
	CandidateCommitments, CandidateDescriptor, CandidateReceipt, CollatorId, CollatorSignature,
	CommittedCandidateReceipt, Hash, HeadData, Id as ParaId, ValidationCode, ValidationCodeHash,
	ValidatorId,
};
pub use rand;
use sp_application_crypto::sr25519;
use sp_keyring::Sr25519Keyring;
use sp_runtime::generic::Digest;

/// Creates a candidate receipt with filler data.
pub fn dummy_candidate_receipt<H: AsRef<[u8]>>(relay_parent: H) -> CandidateReceipt<H> {
	CandidateReceipt::<H> {
		commitments_hash: dummy_candidate_commitments(dummy_head_data()).hash(),
		descriptor: dummy_candidate_descriptor(relay_parent),
	}
}

/// Creates a committed candidate receipt with filler data.
pub fn dummy_committed_candidate_receipt<H: AsRef<[u8]>>(
	relay_parent: H,
) -> CommittedCandidateReceipt<H> {
	CommittedCandidateReceipt::<H> {
		descriptor: dummy_candidate_descriptor::<H>(relay_parent),
		commitments: dummy_candidate_commitments(dummy_head_data()),
	}
}

/// Create a candidate receipt with a bogus signature and filler data. Optionally set the commitment
/// hash with the `commitments` arg.
pub fn dummy_candidate_receipt_bad_sig(
	relay_parent: Hash,
	commitments: impl Into<Option<Hash>>,
) -> CandidateReceipt<Hash> {
	let commitments_hash = if let Some(commitments) = commitments.into() {
		commitments
	} else {
		dummy_candidate_commitments(dummy_head_data()).hash()
	};
	CandidateReceipt::<Hash> {
		commitments_hash,
		descriptor: dummy_candidate_descriptor_bad_sig(relay_parent),
	}
}

/// Create candidate commitments with filler data.
pub fn dummy_candidate_commitments(head_data: impl Into<Option<HeadData>>) -> CandidateCommitments {
	CandidateCommitments {
		head_data: head_data.into().unwrap_or(dummy_head_data()),
		upward_messages: vec![],
		new_validation_code: None,
		horizontal_messages: vec![],
		processed_downward_messages: 0,
		hrmp_watermark: 0_u32,
	}
}

/// Create meaningless dummy hash.
pub fn dummy_hash() -> Hash {
	Hash::zero()
}

/// Create meaningless dummy digest.
pub fn dummy_digest() -> Digest {
	Digest::default()
}

/// Create a candidate descriptor with a bogus signature and filler data.
pub fn dummy_candidate_descriptor_bad_sig(relay_parent: Hash) -> CandidateDescriptor<Hash> {
	let zeros = Hash::zero();
	CandidateDescriptor::<Hash> {
		para_id: 0.into(),
		relay_parent,
		collator: dummy_collator(),
		persisted_validation_data_hash: zeros,
		pov_hash: zeros,
		erasure_root: zeros,
		signature: dummy_collator_signature(),
		para_head: zeros,
		validation_code_hash: dummy_validation_code().hash(),
	}
}

/// Create a candidate descriptor with filler data.
pub fn dummy_candidate_descriptor<H: AsRef<[u8]>>(relay_parent: H) -> CandidateDescriptor<H> {
	let collator = sp_keyring::Sr25519Keyring::Ferdie;
	let invalid = Hash::zero();
	let descriptor = make_valid_candidate_descriptor(
		1.into(),
		relay_parent,
		invalid,
		invalid,
		invalid,
		invalid,
		invalid,
		collator,
	);
	descriptor
}

/// Create meaningless validation code.
pub fn dummy_validation_code() -> ValidationCode {
	ValidationCode(vec![1, 2, 3])
}

/// Create meaningless head data.
pub fn dummy_head_data() -> HeadData {
	HeadData(vec![])
}

/// Create a meaningless collator id.
pub fn dummy_collator() -> CollatorId {
	CollatorId::from(sr25519::Public::from_raw([0; 32]))
}

/// Create a meaningless validator id.
pub fn dummy_validator() -> ValidatorId {
	ValidatorId::from(sr25519::Public::from_raw([0; 32]))
}

/// Create a meaningless collator signature.
pub fn dummy_collator_signature() -> CollatorSignature {
	CollatorSignature::from(sr25519::Signature([0u8; 64]))
}

/// Create a new candidate descriptor, and apply a valid signature
/// using the provided `collator` key.
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
	let payload = polkadot_primitives::v2::collator_signature_payload::<H>(
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

/// After manually modifying the candidate descriptor, resign with a defined collator key.
pub fn resign_candidate_descriptor_with_collator<H: AsRef<[u8]>>(
	descriptor: &mut CandidateDescriptor<H>,
	collator: Sr25519Keyring,
) {
	descriptor.collator = collator.public().into();
	let payload = polkadot_primitives::v2::collator_signature_payload::<H>(
		&descriptor.relay_parent,
		&descriptor.para_id,
		&descriptor.persisted_validation_data_hash,
		&descriptor.pov_hash,
		&descriptor.validation_code_hash,
	);
	let signature = collator.sign(&payload).into();
	descriptor.signature = signature;
}

/// Builder for `CandidateReceipt`.
pub struct TestCandidateBuilder {
	pub para_id: ParaId,
	pub pov_hash: Hash,
	pub relay_parent: Hash,
	pub commitments_hash: Hash,
}

impl std::default::Default for TestCandidateBuilder {
	fn default() -> Self {
		let zeros = Hash::zero();
		Self { para_id: 0.into(), pov_hash: zeros, relay_parent: zeros, commitments_hash: zeros }
	}
}

impl TestCandidateBuilder {
	/// Build a `CandidateReceipt`.
	pub fn build(self) -> CandidateReceipt {
		let mut descriptor = dummy_candidate_descriptor(self.relay_parent);
		descriptor.para_id = self.para_id;
		descriptor.pov_hash = self.pov_hash;
		CandidateReceipt { descriptor, commitments_hash: self.commitments_hash }
	}
}

/// A special `Rng` that always returns zero for testing something that implied
/// to be random but should not be random in the tests
pub struct AlwaysZeroRng;

impl Default for AlwaysZeroRng {
	fn default() -> Self {
		Self {}
	}
}
impl rand::RngCore for AlwaysZeroRng {
	fn next_u32(&mut self) -> u32 {
		0_u32
	}

	fn next_u64(&mut self) -> u64 {
		0_u64
	}

	fn fill_bytes(&mut self, dest: &mut [u8]) {
		for element in dest.iter_mut() {
			*element = 0_u8;
		}
	}

	fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
		self.fill_bytes(dest);
		Ok(())
	}
}

pub fn dummy_signature() -> polkadot_primitives::v2::ValidatorSignature {
	sp_core::crypto::UncheckedFrom::unchecked_from([1u8; 64])
}
