// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

pub use sp_consensus_vrf::schnorrkel::{VRFOutput, VRFProof, Randomness};
pub use sp_consensus_babe::Slot;

use polkadot_primitives::v1::{
	CandidateHash, Hash, ValidatorIndex, ValidatorSignature, CoreIndex,
	Header, BlockNumber, CandidateIndex,
};
use parity_scale_codec::{Encode, Decode};
use sp_consensus_babe as babe_primitives;
use sp_application_crypto::Public;

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
	/// The VRF showing the criterion is met.
	pub vrf: (VRFOutput, VRFProof),
}

/// An assignment crt which refers to the candidate under which the assignment is
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

/// A vote of approval on a candidate.
#[derive(Debug, Clone, Encode, Decode)]
pub struct ApprovalVote(pub CandidateHash);

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
	vrf_output: VRFOutput,
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

		let transcript = babe_primitives::make_transcript(
			randomness,
			self.slot,
			epoch_index,
		);

		let inout = self.vrf_output.0.attach_input_hash(&pubkey, transcript)
			.map_err(ApprovalError::SchnorrkelSignature)?;
		Ok(RelayVRFStory(inout.make_bytes(RELAY_VRF_STORY_CONTEXT)))
	}
}

/// Extract the slot number and relay VRF from a header.
///
/// This fails if either there is no BABE `PreRuntime` digest or
/// the digest has type `SecondaryPlain`, which Substrate nodes do
/// not produce or accept anymore.
pub fn babe_unsafe_vrf_info(header: &Header) -> Option<UnsafeVRFOutput> {
	use babe_primitives::digests::{CompatibleDigestItem, PreDigest};

	for digest in &header.digest.logs {
		if let Some(pre) = digest.as_babe_pre_digest() {
			let slot = pre.slot();
			let authority_index = pre.authority_index();

			// exhaustive match to defend against upstream variant changes.
			let vrf_output = match pre {
				PreDigest::Primary(primary) => primary.vrf_output,
				PreDigest::SecondaryVRF(secondary) => secondary.vrf_output,
				PreDigest::SecondaryPlain(_) => return None,
			};

			return Some(UnsafeVRFOutput {
				vrf_output,
				slot,
				authority_index,
			});
		}
	}

	None
}
