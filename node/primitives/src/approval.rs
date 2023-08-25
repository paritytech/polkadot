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

//! Types relevant for approval.

/// A list of primitives introduced in v1.
pub mod v1 {
	use sp_consensus_babe as babe_primitives;
	pub use sp_consensus_babe::{
		Randomness, Slot, VrfOutput, VrfProof, VrfSignature, VrfTranscript,
	};

	use parity_scale_codec::{Decode, Encode};
	use polkadot_primitives::{
		BlockNumber, CandidateHash, CandidateIndex, CoreIndex, Hash, Header, SessionIndex,
		ValidatorIndex, ValidatorSignature,
	};
	use sp_application_crypto::ByteArray;

	/// Validators assigning to check a particular candidate are split up into tranches.
	/// Earlier tranches of validators check first, with later tranches serving as backup.
	pub type DelayTranche = u32;

	/// A static context used to compute the Relay VRF story based on the
	/// VRF output included in the header-chain.
	pub const RELAY_VRF_STORY_CONTEXT: &[u8] = b"A&V RC-VRF";

	/// A static context used for all relay-vrf-modulo VRFs.
	pub const RELAY_VRF_MODULO_CONTEXT: &[u8] = b"A&V MOD";

	/// A static context used for all relay-vrf-modulo VRFs.
	pub const RELAY_VRF_DELAY_CONTEXT: &[u8] = b"A&V DELAY";

	/// A static context used for transcripts indicating assigned availability core.
	pub const ASSIGNED_CORE_CONTEXT: &[u8] = b"A&V ASSIGNED";

	/// A static context associated with producing randomness for a core.
	pub const CORE_RANDOMNESS_CONTEXT: &[u8] = b"A&V CORE";

	/// A static context associated with producing randomness for a tranche.
	pub const TRANCHE_RANDOMNESS_CONTEXT: &[u8] = b"A&V TRANCHE";

	/// random bytes derived from the VRF submitted within the block by the
	/// block author as a credential and used as input to approval assignment criteria.
	#[derive(Debug, Clone, Encode, Decode, PartialEq)]
	pub struct RelayVRFStory(pub [u8; 32]);

	/// Different kinds of input data or criteria that can prove a validator's assignment
	/// to check a particular parachain.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum AssignmentCertKind {
		/// An assignment story based on the VRF that authorized the relay-chain block where the
		/// candidate was included combined with a sample number.
		///
		/// The context used to produce bytes is [`RELAY_VRF_MODULO_CONTEXT`]
		RelayVRFModulo {
			/// The sample number used in this cert.
			sample: u32,
		},
		/// An assignment story based on the VRF that authorized the relay-chain block where the
		/// candidate was included combined with the index of a particular core.
		///
		/// The context is [`RELAY_VRF_DELAY_CONTEXT`]
		RelayVRFDelay {
			/// The core index chosen in this cert.
			core_index: CoreIndex,
		},
	}

	/// A certification of assignment.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub struct AssignmentCert {
		/// The criterion which is claimed to be met by this cert.
		pub kind: AssignmentCertKind,
		/// The VRF signature showing the criterion is met.
		pub vrf: VrfSignature,
	}

	/// An assignment criterion which refers to the candidate under which the assignment is
	/// relevant by block hash.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub struct IndirectAssignmentCert {
		/// A block hash where the candidate appears.
		pub block_hash: Hash,
		/// The validator index.
		pub validator: ValidatorIndex,
		/// The cert itself.
		pub cert: AssignmentCert,
	}

	/// A signed approval vote which references the candidate indirectly via the block.
	///
	/// In practice, we have a look-up from block hash and candidate index to candidate hash,
	/// so this can be transformed into a `SignedApprovalVote`.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub struct IndirectSignedApprovalVote {
		/// A block hash where the candidate appears.
		pub block_hash: Hash,
		/// The index of the candidate in the list of candidates fully included as-of the block.
		pub candidate_index: CandidateIndex,
		/// The validator index.
		pub validator: ValidatorIndex,
		/// The signature by the validator.
		pub signature: ValidatorSignature,
	}

	/// Metadata about a block which is now live in the approval protocol.
	#[derive(Debug)]
	pub struct BlockApprovalMeta {
		/// The hash of the block.
		pub hash: Hash,
		/// The number of the block.
		pub number: BlockNumber,
		/// The hash of the parent block.
		pub parent_hash: Hash,
		/// The candidates included by the block.
		/// Note that these are not the same as the candidates that appear within the block body.
		pub candidates: Vec<CandidateHash>,
		/// The consensus slot of the block.
		pub slot: Slot,
		/// The session of the block.
		pub session: SessionIndex,
	}

	/// Errors that can occur during the approvals protocol.
	#[derive(Debug, thiserror::Error)]
	#[allow(missing_docs)]
	pub enum ApprovalError {
		#[error("Schnorrkel signature error")]
		SchnorrkelSignature(schnorrkel::errors::SignatureError),
		#[error("Authority index {0} out of bounds")]
		AuthorityOutOfBounds(usize),
	}

	/// An unsafe VRF output. Provide BABE Epoch info to create a `RelayVRFStory`.
	pub struct UnsafeVRFOutput {
		vrf_output: VrfOutput,
		slot: Slot,
		authority_index: u32,
	}

	impl UnsafeVRFOutput {
		/// Get the slot.
		pub fn slot(&self) -> Slot {
			self.slot
		}

		/// Compute the randomness associated with this VRF output.
		pub fn compute_randomness(
			self,
			authorities: &[(babe_primitives::AuthorityId, babe_primitives::BabeAuthorityWeight)],
			randomness: &babe_primitives::Randomness,
			epoch_index: u64,
		) -> Result<RelayVRFStory, ApprovalError> {
			let author = match authorities.get(self.authority_index as usize) {
				None => return Err(ApprovalError::AuthorityOutOfBounds(self.authority_index as _)),
				Some(x) => &x.0,
			};

			let pubkey = schnorrkel::PublicKey::from_bytes(author.as_slice())
				.map_err(ApprovalError::SchnorrkelSignature)?;

			let transcript =
				sp_consensus_babe::make_vrf_transcript(randomness, self.slot, epoch_index);

			let inout = self
				.vrf_output
				.0
				.attach_input_hash(&pubkey, transcript.0)
				.map_err(ApprovalError::SchnorrkelSignature)?;
			Ok(RelayVRFStory(inout.make_bytes(super::v1::RELAY_VRF_STORY_CONTEXT)))
		}
	}

	/// Extract the slot number and relay VRF from a header.
	///
	/// This fails if either there is no BABE `PreRuntime` digest or
	/// the digest has type `SecondaryPlain`, which Substrate nodes do
	/// not produce or accept anymore.
	pub fn babe_unsafe_vrf_info(header: &Header) -> Option<UnsafeVRFOutput> {
		use babe_primitives::digests::CompatibleDigestItem;

		for digest in &header.digest.logs {
			if let Some(pre) = digest.as_babe_pre_digest() {
				let slot = pre.slot();
				let authority_index = pre.authority_index();

				return pre.vrf_signature().map(|sig| UnsafeVRFOutput {
					vrf_output: sig.output.clone(),
					slot,
					authority_index,
				})
			}
		}

		None
	}
}

/// A list of primitives introduced by v2.
pub mod v2 {
	use parity_scale_codec::{Decode, Encode};
	pub use sp_consensus_babe::{
		Randomness, Slot, VrfOutput, VrfProof, VrfSignature, VrfTranscript,
	};
	use std::ops::BitOr;

	use bitvec::{prelude::Lsb0, vec::BitVec};
	use polkadot_primitives::{
		CandidateIndex, CoreIndex, Hash, ValidatorIndex, ValidatorSignature,
	};

	/// A static context associated with producing randomness for a core.
	pub const CORE_RANDOMNESS_CONTEXT: &[u8] = b"A&V CORE v2";
	/// A static context associated with producing randomness for v2 multi-core assignments.
	pub const ASSIGNED_CORE_CONTEXT: &[u8] = b"A&V ASSIGNED v2";
	/// A static context used for all relay-vrf-modulo VRFs for v2 multi-core assignments.
	pub const RELAY_VRF_MODULO_CONTEXT: &[u8] = b"A&V MOD v2";
	/// A read-only bitvec wrapper
	#[derive(Clone, Debug, Encode, Decode, Hash, PartialEq, Eq)]
	pub struct Bitfield<T>(BitVec<u8, bitvec::order::Lsb0>, std::marker::PhantomData<T>);

	/// A `read-only`, `non-zero` bitfield.
	/// Each 1 bit identifies a candidate by the bitfield bit index.
	pub type CandidateBitfield = Bitfield<CandidateIndex>;
	/// A bitfield of core assignments.
	pub type CoreBitfield = Bitfield<CoreIndex>;

	/// Errors that can occur when creating and manipulating bitfields.
	#[derive(Debug)]
	pub enum BitfieldError {
		/// All bits are zero.
		NullAssignment,
	}

	/// A bit index in `Bitfield`.
	#[cfg_attr(test, derive(PartialEq, Clone))]
	pub struct BitIndex(pub usize);

	/// Helper trait to convert primitives to `BitIndex`.
	pub trait AsBitIndex {
		/// Returns the index of the corresponding bit in `Bitfield`.
		fn as_bit_index(&self) -> BitIndex;
	}

	impl<T> Bitfield<T> {
		/// Returns the bit value at specified `index`. If `index` is greater than bitfield size,
		/// returns `false`.
		pub fn bit_at(&self, index: BitIndex) -> bool {
			if self.0.len() <= index.0 {
				false
			} else {
				self.0[index.0]
			}
		}

		/// Returns number of bits.
		pub fn len(&self) -> usize {
			self.0.len()
		}

		/// Returns the number of 1 bits.
		pub fn count_ones(&self) -> usize {
			self.0.count_ones()
		}

		/// Returns the index of the first 1 bit.
		pub fn first_one(&self) -> Option<usize> {
			self.0.first_one()
		}

		/// Returns an iterator over inner bits.
		pub fn iter_ones(&self) -> bitvec::slice::IterOnes<u8, bitvec::order::Lsb0> {
			self.0.iter_ones()
		}

		/// For testing purpose, we want a inner mutable ref.
		#[cfg(test)]
		pub fn inner_mut(&mut self) -> &mut BitVec<u8, bitvec::order::Lsb0> {
			&mut self.0
		}

		/// Returns the inner bitfield and consumes `self`.
		pub fn into_inner(self) -> BitVec<u8, bitvec::order::Lsb0> {
			self.0
		}
	}

	impl AsBitIndex for CandidateIndex {
		fn as_bit_index(&self) -> BitIndex {
			BitIndex(*self as usize)
		}
	}

	impl AsBitIndex for CoreIndex {
		fn as_bit_index(&self) -> BitIndex {
			BitIndex(self.0 as usize)
		}
	}

	impl AsBitIndex for usize {
		fn as_bit_index(&self) -> BitIndex {
			BitIndex(*self)
		}
	}

	impl<T> From<T> for Bitfield<T>
	where
		T: AsBitIndex,
	{
		fn from(value: T) -> Self {
			Self(
				{
					let mut bv = bitvec::bitvec![u8, Lsb0; 0; value.as_bit_index().0 + 1];
					bv.set(value.as_bit_index().0, true);
					bv
				},
				Default::default(),
			)
		}
	}

	impl<T> TryFrom<Vec<T>> for Bitfield<T>
	where
		T: Into<Bitfield<T>>,
	{
		type Error = BitfieldError;

		fn try_from(mut value: Vec<T>) -> Result<Self, Self::Error> {
			if value.is_empty() {
				return Err(BitfieldError::NullAssignment)
			}

			let initial_bitfield =
				value.pop().expect("Just checked above it's not empty; qed").into();

			Ok(Self(
				value.into_iter().fold(initial_bitfield.0, |initial_bitfield, element| {
					let mut bitfield: Bitfield<T> = element.into();
					bitfield
						.0
						.resize(std::cmp::max(initial_bitfield.len(), bitfield.0.len()), false);
					bitfield.0.bitor(initial_bitfield)
				}),
				Default::default(),
			))
		}
	}

	/// Certificate is changed compared to `AssignmentCertKind`:
	/// - introduced RelayVRFModuloCompact
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum AssignmentCertKindV2 {
		/// Multiple assignment stories based on the VRF that authorized the relay-chain block
		/// where the candidates were included.
		///
		/// The context is [`v2::RELAY_VRF_MODULO_CONTEXT`]
		#[codec(index = 0)]
		RelayVRFModuloCompact {
			/// A bitfield representing the core indices claimed by this assignment.
			core_bitfield: CoreBitfield,
		},
		/// An assignment story based on the VRF that authorized the relay-chain block where the
		/// candidate was included combined with the index of a particular core.
		///
		/// The context is [`v2::RELAY_VRF_DELAY_CONTEXT`]
		#[codec(index = 1)]
		RelayVRFDelay {
			/// The core index chosen in this cert.
			core_index: CoreIndex,
		},
		/// Deprectated assignment. Soon to be removed.
		///  An assignment story based on the VRF that authorized the relay-chain block where the
		/// candidate was included combined with a sample number.
		///
		/// The context used to produce bytes is [`v1::RELAY_VRF_MODULO_CONTEXT`]
		#[codec(index = 2)]
		RelayVRFModulo {
			/// The sample number used in this cert.
			sample: u32,
		},
	}

	/// A certification of assignment.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub struct AssignmentCertV2 {
		/// The criterion which is claimed to be met by this cert.
		pub kind: AssignmentCertKindV2,
		/// The VRF showing the criterion is met.
		pub vrf: VrfSignature,
	}

	impl From<super::v1::AssignmentCert> for AssignmentCertV2 {
		fn from(cert: super::v1::AssignmentCert) -> Self {
			Self {
				kind: match cert.kind {
					super::v1::AssignmentCertKind::RelayVRFDelay { core_index } =>
						AssignmentCertKindV2::RelayVRFDelay { core_index },
					super::v1::AssignmentCertKind::RelayVRFModulo { sample } =>
						AssignmentCertKindV2::RelayVRFModulo { sample },
				},
				vrf: cert.vrf,
			}
		}
	}

	/// Errors that can occur when trying to convert to/from assignment v1/v2
	#[derive(Debug)]
	pub enum AssignmentConversionError {
		/// Assignment certificate is not supported in v1.
		CertificateNotSupported,
	}

	impl TryFrom<AssignmentCertV2> for super::v1::AssignmentCert {
		type Error = AssignmentConversionError;
		fn try_from(cert: AssignmentCertV2) -> Result<Self, AssignmentConversionError> {
			Ok(Self {
				kind: match cert.kind {
					AssignmentCertKindV2::RelayVRFDelay { core_index } =>
						super::v1::AssignmentCertKind::RelayVRFDelay { core_index },
					AssignmentCertKindV2::RelayVRFModulo { sample } =>
						super::v1::AssignmentCertKind::RelayVRFModulo { sample },
					// Not supported
					_ => return Err(AssignmentConversionError::CertificateNotSupported),
				},
				vrf: cert.vrf,
			})
		}
	}

	/// An assignment criterion which refers to the candidate under which the assignment is
	/// relevant by block hash.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub struct IndirectAssignmentCertV2 {
		/// A block hash where the candidate appears.
		pub block_hash: Hash,
		/// The validator index.
		pub validator: ValidatorIndex,
		/// The cert itself.
		pub cert: AssignmentCertV2,
	}

	impl From<super::v1::IndirectAssignmentCert> for IndirectAssignmentCertV2 {
		fn from(indirect_cert: super::v1::IndirectAssignmentCert) -> Self {
			Self {
				block_hash: indirect_cert.block_hash,
				validator: indirect_cert.validator,
				cert: indirect_cert.cert.into(),
			}
		}
	}

	impl TryFrom<IndirectAssignmentCertV2> for super::v1::IndirectAssignmentCert {
		type Error = AssignmentConversionError;
		fn try_from(
			indirect_cert: IndirectAssignmentCertV2,
		) -> Result<Self, AssignmentConversionError> {
			Ok(Self {
				block_hash: indirect_cert.block_hash,
				validator: indirect_cert.validator,
				cert: indirect_cert.cert.try_into()?,
			})
		}
	}

	impl From<super::v1::IndirectSignedApprovalVote> for IndirectSignedApprovalVoteV2 {
		fn from(value: super::v1::IndirectSignedApprovalVote) -> Self {
			Self {
				block_hash: value.block_hash,
				validator: value.validator,
				candidate_indices: value.candidate_index.into(),
				signature: value.signature,
			}
		}
	}

	/// Errors that can occur when trying to convert to/from approvals v1/v2
	#[derive(Debug)]
	pub enum ApprovalConversionError {
		/// More than one candidate was signed.
		MoreThanOneCandidate(usize),
	}

	impl TryFrom<IndirectSignedApprovalVoteV2> for super::v1::IndirectSignedApprovalVote {
		type Error = ApprovalConversionError;

		fn try_from(value: IndirectSignedApprovalVoteV2) -> Result<Self, Self::Error> {
			if value.candidate_indices.count_ones() != 1 {
				return Err(ApprovalConversionError::MoreThanOneCandidate(
					value.candidate_indices.count_ones(),
				))
			}
			Ok(Self {
				block_hash: value.block_hash,
				validator: value.validator,
				candidate_index: value.candidate_indices.first_one().expect("Qed we checked above")
					as u32,
				signature: value.signature,
			})
		}
	}

	/// A signed approval vote which references the candidate indirectly via the block.
	///
	/// In practice, we have a look-up from block hash and candidate index to candidate hash,
	/// so this can be transformed into a `SignedApprovalVote`.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub struct IndirectSignedApprovalVoteV2 {
		/// A block hash where the candidate appears.
		pub block_hash: Hash,
		/// The index of the candidate in the list of candidates fully included as-of the block.
		pub candidate_indices: CandidateBitfield,
		/// The validator index.
		pub validator: ValidatorIndex,
		/// The signature by the validator.
		pub signature: ValidatorSignature,
	}
}

#[cfg(test)]
mod test {
	use super::v2::{BitIndex, Bitfield};

	use polkadot_primitives::{CandidateIndex, CoreIndex};

	#[test]
	fn test_assignment_bitfield_from_vec() {
		let candidate_indices = vec![1u32, 7, 3, 10, 45, 8, 200, 2];
		let max_index = *candidate_indices.iter().max().unwrap();
		let bitfield = Bitfield::try_from(candidate_indices.clone()).unwrap();
		let candidate_indices =
			candidate_indices.into_iter().map(|i| BitIndex(i as usize)).collect::<Vec<_>>();

		// Test 1 bits.
		for index in candidate_indices.clone() {
			assert!(bitfield.bit_at(index));
		}

		// Test 0 bits.
		for index in 0..max_index {
			if candidate_indices.contains(&BitIndex(index as usize)) {
				continue
			}
			assert!(!bitfield.bit_at(BitIndex(index as usize)));
		}
	}

	#[test]
	fn test_assignment_bitfield_invariant_msb() {
		let core_indices = vec![CoreIndex(1), CoreIndex(3), CoreIndex(10), CoreIndex(20)];
		let mut bitfield = Bitfield::try_from(core_indices.clone()).unwrap();
		assert!(bitfield.inner_mut().pop().unwrap());

		for i in 0..1024 {
			assert!(Bitfield::try_from(CoreIndex(i)).unwrap().inner_mut().pop().unwrap());
			assert!(Bitfield::try_from(i).unwrap().inner_mut().pop().unwrap());
		}
	}

	#[test]
	fn test_assignment_bitfield_basic() {
		let bitfield = Bitfield::try_from(CoreIndex(0)).unwrap();
		assert!(bitfield.bit_at(BitIndex(0)));
		assert!(!bitfield.bit_at(BitIndex(1)));
		assert_eq!(bitfield.len(), 1);

		let mut bitfield = Bitfield::try_from(20 as CandidateIndex).unwrap();
		assert!(bitfield.bit_at(BitIndex(20)));
		assert_eq!(bitfield.inner_mut().count_ones(), 1);
		assert_eq!(bitfield.len(), 21);
	}
}
