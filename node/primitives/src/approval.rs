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

pub use sp_consensus_vrf::schnorrkel::{VRFOutput, VRFProof};

use polkadot_primitives::v1::{
	CandidateHash, Hash, ValidatorIndex, Signed, ValidatorSignature, CoreIndex,
};
use parity_scale_codec::{Encode, Decode};

/// Validators assigning to check a particular candidate are split up into tranches.
/// Earlier tranches of validators check first, with later tranches serving as backup.
pub type DelayTranche = u32;

/// A static context used for all relay-vrf-modulo VRFs.
pub const RELAY_VRF_MODULO_CONTEXT: &str = "A&V MOD";

/// A static context used for all relay-vrf-delay VRFs.
pub const RELAY_VRF_DELAY_CONTEXT: &str = "A&V TRANCHE";

/// random bytes derived from the VRF submitted within the block by the
/// block author as a credential and used as input to approval assignment criteria.
#[derive(Debug, Clone, Encode, Decode, PartialEq)]
pub struct RelayVRF(pub [u8; 32]);

/// Different kinds of input data or criteria that can prove a validator's assignment
/// to check a particular parachain.
#[derive(Debug, Clone, Encode, Decode)]
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
#[derive(Debug, Clone, Encode, Decode)]
pub struct AssignmentCert {
	/// The criterion which is claimed to be met by this cert.
	pub kind: AssignmentCertKind,
	/// The VRF showing the criterion is met.
	pub vrf: (VRFOutput, VRFProof),
}

/// An assignment crt which refers to the candidate under which the assignment is
/// relevant by block hash.
#[derive(Debug, Clone, Encode, Decode)]
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

/// An approval vote signed by some validator.
pub type SignedApprovalVote = Signed<ApprovalVote>;

/// A signed approval vote which references the candidate indirectly via the block.
///
/// In practice, we have a look-up from block hash and candidate index to candidate hash,
/// so this can be transformed into a `SignedApprovalVote`.
#[derive(Debug, Clone, Encode, Decode)]
pub struct IndirectSignedApprovalVote {
	/// A block hash where the candidate appears.
	pub block_hash: Hash,
	/// The index of the candidate in the list of candidates fully included as-of the block.
	pub candidate_index: u32,
	/// The validator index.
	pub validator: ValidatorIndex,
	/// The signature by the validator.
	pub signature: ValidatorSignature,
}
