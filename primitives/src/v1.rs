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

//! V1 Primitives.

use sp_std::prelude::*;
use parity_scale_codec::{Encode, Decode};
use bitvec::vec::BitVec;

use primitives::RuntimeDebug;
use runtime_primitives::traits::AppVerify;
use inherents::InherentIdentifier;
use sp_arithmetic::traits::{BaseArithmetic, Saturating, Zero};

pub use runtime_primitives::traits::{BlakeTwo256, Hash as HashT};

// Export some core primitives.
pub use polkadot_core_primitives::v1::{
	BlockNumber, Moment, Signature, AccountPublic, AccountId, AccountIndex,
	ChainId, Hash, Nonce, Balance, Header, Block, BlockId, UncheckedExtrinsic,
	Remark, DownwardMessage,
};

// Export some polkadot-parachain primitives
pub use polkadot_parachain::primitives::{
	Id, ParachainDispatchOrigin, LOWEST_USER_ID, UpwardMessage, HeadData, BlockData,
	ValidationCode,
};

// Export some basic parachain primitives from v0.
pub use crate::v0::{
	CollatorId, CollatorSignature, PARACHAIN_KEY_TYPE_ID, ValidatorId, ValidatorIndex,
	ValidatorSignature, SigningContext, Signed, ValidityAttestation,
	CompactStatement, SignedStatement, ErasureChunk, EncodeAs,
};

// More exports from v0 for std.
#[cfg(feature = "std")]
pub use crate::v0::{ValidatorPair, CollatorPair};

pub use sp_staking::SessionIndex;

/// Unique identifier for the Inclusion Inherent
pub const INCLUSION_INHERENT_IDENTIFIER: InherentIdentifier = *b"inclusn0";

/// Get a collator signature payload on a relay-parent, block-data combo.
pub fn collator_signature_payload<H: AsRef<[u8]>>(
	relay_parent: &H,
	para_id: &Id,
	validation_data_hash: &Hash,
	pov_hash: &Hash,
) -> [u8; 100] {
	// 32-byte hash length is protected in a test below.
	let mut payload = [0u8; 100];

	payload[0..32].copy_from_slice(relay_parent.as_ref());
	u32::from(*para_id).using_encoded(|s| payload[32..32 + s.len()].copy_from_slice(s));
	payload[36..68].copy_from_slice(validation_data_hash.as_ref());
	payload[68..100].copy_from_slice(pov_hash.as_ref());

	payload
}

fn check_collator_signature<H: AsRef<[u8]>>(
	relay_parent: &H,
	para_id: &Id,
	validation_data_hash: &Hash,
	pov_hash: &Hash,
	collator: &CollatorId,
	signature: &CollatorSignature,
) -> Result<(),()> {
	let payload = collator_signature_payload(
		relay_parent,
		para_id,
		validation_data_hash,
		pov_hash,
	);

	if signature.verify(&payload[..], collator) {
		Ok(())
	} else {
		Err(())
	}
}

/// Compute the `validation_data_hash` from global & local validation data.
pub fn validation_data_hash<N: Encode>(
	global: &GlobalValidationData<N>,
	local: &LocalValidationData<N>,
) -> Hash {
	BlakeTwo256::hash_of(&(global, local))
}

/// A unique descriptor of the candidate receipt.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default, Hash))]
pub struct CandidateDescriptor<H = Hash> {
	/// The ID of the para this is a candidate for.
	pub para_id: Id,
	/// The hash of the relay-chain block this is executed in the context of.
	pub relay_parent: H,
	/// The collator's sr25519 public key.
	pub collator: CollatorId,
	/// The blake2-256 hash of the validation data. This is extra data derived from
	/// relay-chain state which may vary based on bitfields included before the candidate.
	/// Thus it cannot be derived entirely from the relay-parent.
	pub validation_data_hash: Hash,
	/// The blake2-256 hash of the pov.
	pub pov_hash: Hash,
	/// Signature on blake2-256 of components of this receipt:
	/// The parachain index, the relay parent, the validation data hash, and the pov_hash.
	pub signature: CollatorSignature,

}

impl<H: AsRef<[u8]>> CandidateDescriptor<H> {
	/// Check the signature of the collator within this descriptor.
	pub fn check_collator_signature(&self) -> Result<(), ()> {
		check_collator_signature(
			&self.relay_parent,
			&self.para_id,
			&self.validation_data_hash,
			&self.pov_hash,
			&self.collator,
			&self.signature,
		)
	}
}

/// A candidate-receipt.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct CandidateReceipt<H = Hash> {
	/// The descriptor of the candidate.
	pub descriptor: CandidateDescriptor<H>,
	/// The hash of the encoded commitments made as a result of candidate execution.
	pub commitments_hash: Hash,
}

impl<H> CandidateReceipt<H> {
	/// Get a reference to the candidate descriptor.
	pub fn descriptor(&self) -> &CandidateDescriptor<H> {
		&self.descriptor
	}

	/// Computes the blake2-256 hash of the receipt.
	pub fn hash(&self) -> Hash where H: Encode {
		BlakeTwo256::hash_of(self)
	}
}

/// All data pertaining to the execution of a para candidate.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct FullCandidateReceipt<H = Hash, N = BlockNumber> {
	/// The inner candidate receipt.
	pub inner: CandidateReceipt<H>,
	/// The global validation schedule.
	pub global_validation: GlobalValidationData<N>,
	/// The local validation data.
	pub local_validation: LocalValidationData<N>,
}

/// A candidate-receipt with commitments directly included.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default, Hash))]
pub struct CommittedCandidateReceipt<H = Hash> {
	/// The descriptor of the candidate.
	pub descriptor: CandidateDescriptor<H>,
	/// The commitments of the candidate receipt.
	pub commitments: CandidateCommitments,
}

impl<H> CommittedCandidateReceipt<H> {
	/// Get a reference to the candidate descriptor.
	pub fn descriptor(&self) -> &CandidateDescriptor<H> {
		&self.descriptor
	}
}

impl<H: Clone> CommittedCandidateReceipt<H> {
	/// Transforms this into a plain CandidateReceipt.
	pub fn to_plain(&self) -> CandidateReceipt<H> {
		CandidateReceipt {
			descriptor: self.descriptor.clone(),
			commitments_hash: self.commitments.hash(),
		}
	}

	/// Computes the hash of the committed candidate receipt.
	///
	/// This computes the canonical hash, not the hash of the directly encoded data.
	/// Thus this is a shortcut for `candidate.to_plain().hash()`.
	pub fn hash(&self) -> Hash where H: Encode {
		self.to_plain().hash()
	}
}

impl PartialOrd for CommittedCandidateReceipt {
	fn partial_cmp(&self, other: &Self) -> Option<sp_std::cmp::Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for CommittedCandidateReceipt {
	fn cmp(&self, other: &Self) -> sp_std::cmp::Ordering {
		// TODO: compare signatures or something more sane
		// https://github.com/paritytech/polkadot/issues/222
		self.descriptor().para_id.cmp(&other.descriptor().para_id)
			.then_with(|| self.commitments.head_data.cmp(&other.commitments.head_data))
	}
}

/// Extra data that is needed along with the other fields in a `CandidateReceipt`
/// to fully validate the candidate. These fields are parachain-specific.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct LocalValidationData<N = BlockNumber> {
	/// The parent head-data.
	pub parent_head: HeadData,
	/// The balance of the parachain at the moment of validation.
	pub balance: Balance,
	/// The blake2-256 hash of the validation code used to execute the candidate.
	pub validation_code_hash: Hash,
	/// Whether the parachain is allowed to upgrade its validation code.
	///
	/// This is `Some` if so, and contains the number of the minimum relay-chain
	/// height at which the upgrade will be applied, if an upgrade is signaled
	/// now.
	///
	/// A parachain should enact its side of the upgrade at the end of the first
	/// parablock executing in the context of a relay-chain block with at least this
	/// height. This may be equal to the current perceived relay-chain block height, in
	/// which case the code upgrade should be applied at the end of the signaling
	/// block.
	pub code_upgrade_allowed: Option<N>,
}

/// Extra data that is needed along with the other fields in a `CandidateReceipt`
/// to fully validate the candidate.
///
/// These are global parameters that apply to all candidates in a block.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct GlobalValidationData<N = BlockNumber> {
	/// The maximum code size permitted, in bytes.
	pub max_code_size: u32,
	/// The maximum head-data size permitted, in bytes.
	pub max_head_data_size: u32,
	/// The relay-chain block number this is in the context of.
	pub block_number: N,
}

/// Commitments made in a `CandidateReceipt`. Many of these are outputs of validation.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default, Hash))]
pub struct CandidateCommitments {
	/// Fees paid from the chain to the relay chain validators.
	pub fees: Balance,
	/// Messages destined to be interpreted by the Relay chain itself.
	pub upward_messages: Vec<UpwardMessage>,
	/// The root of a block's erasure encoding Merkle tree.
	pub erasure_root: Hash,
	/// New validation code.
	pub new_validation_code: Option<ValidationCode>,
	/// The head-data produced as a result of execution.
	pub head_data: HeadData,
}

impl CandidateCommitments {
	/// Compute the blake2-256 hash of the commitments.
	pub fn hash(&self) -> Hash {
		BlakeTwo256::hash_of(self)
	}
}

/// A Proof-of-Validity
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct PoV {
	/// The block witness data.
	pub block_data: BlockData,
}

impl PoV {
	/// Get the blake2-256 hash of the PoV.
	#[cfg(feature = "std")]
	pub fn hash(&self) -> Hash {
		BlakeTwo256::hash_of(self)
	}
}

/// A bitfield concerning availability of backed candidates.
#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
pub struct AvailabilityBitfield(pub BitVec<bitvec::order::Lsb0, u8>);

impl From<BitVec<bitvec::order::Lsb0, u8>> for AvailabilityBitfield {
	fn from(inner: BitVec<bitvec::order::Lsb0, u8>) -> Self {
		AvailabilityBitfield(inner)
	}
}

/// A bitfield signed by a particular validator about the availability of pending candidates.
pub type SignedAvailabilityBitfield = Signed<AvailabilityBitfield>;

/// A set of signed availability bitfields. Should be sorted by validator index, ascending.
pub type SignedAvailabilityBitfields = Vec<SignedAvailabilityBitfield>;

/// A backed (or backable, depending on context) candidate.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub struct BackedCandidate<H = Hash> {
	/// The candidate referred to.
	pub candidate: CommittedCandidateReceipt<H>,
	/// The validity votes themselves, expressed as signatures.
	pub validity_votes: Vec<ValidityAttestation>,
	/// The indices of the validators within the group, expressed as a bitfield.
	pub validator_indices: BitVec<bitvec::order::Lsb0, u8>,
}

impl<H> BackedCandidate<H> {
	/// Get a reference to the descriptor of the para.
	pub fn descriptor(&self) -> &CandidateDescriptor<H> {
		&self.candidate.descriptor
	}
}

/// Verify the backing of the given candidate.
///
/// Provide a lookup from the index of a validator within the group assigned to this para,
/// as opposed to the index of the validator within the overall validator set, as well as
/// the number of validators in the group.
///
/// Also provide the signing context.
///
/// Returns either an error, indicating that one of the signatures was invalid or that the index
/// was out-of-bounds, or the number of signatures checked.
pub fn check_candidate_backing<H: AsRef<[u8]> + Clone + Encode>(
	backed: &BackedCandidate<H>,
	signing_context: &SigningContext<H>,
	group_len: usize,
	validator_lookup: impl Fn(usize) -> Option<ValidatorId>,
) -> Result<usize, ()> {
	if backed.validator_indices.len() != group_len {
		return Err(())
	}

	if backed.validity_votes.len() > group_len {
		return Err(())
	}

	// this is known, even in runtime, to be blake2-256.
	let hash: Hash = backed.candidate.hash();

	let mut signed = 0;
	for ((val_in_group_idx, _), attestation) in backed.validator_indices.iter().enumerate()
		.filter(|(_, signed)| **signed)
		.zip(backed.validity_votes.iter())
	{
		let validator_id = validator_lookup(val_in_group_idx).ok_or(())?;
		let payload = attestation.signed_payload(hash.clone(), signing_context);
		let sig = attestation.signature();

		if sig.verify(&payload[..], &validator_id) {
			signed += 1;
		} else {
			return Err(())
		}
	}

	if signed != backed.validity_votes.len() {
		return Err(())
	}

	Ok(signed)
}

/// The unique (during session) index of a core.
#[derive(Encode, Decode, Default, PartialOrd, Ord, Eq, PartialEq, Clone, Copy)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct CoreIndex(pub u32);

impl From<u32> for CoreIndex {
	fn from(i: u32) -> CoreIndex {
		CoreIndex(i)
	}
}

/// The unique (during session) index of a validator group.
#[derive(Encode, Decode, Default, Clone, Copy)]
#[cfg_attr(feature = "std", derive(Eq, Hash, PartialEq, Debug))]
pub struct GroupIndex(pub u32);

impl From<u32> for GroupIndex {
	fn from(i: u32) -> GroupIndex {
		GroupIndex(i)
	}
}

/// A claim on authoring the next block for a given parathread.
#[derive(Clone, Encode, Decode, Default)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub struct ParathreadClaim(pub Id, pub CollatorId);

/// An entry tracking a claim to ensure it does not pass the maximum number of retries.
#[derive(Clone, Encode, Decode, Default)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub struct ParathreadEntry {
	/// The claim.
	pub claim: ParathreadClaim,
	/// Number of retries.
	pub retries: u32,
}

/// What is occupying a specific availability core.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub enum CoreOccupied {
	/// A parathread.
	Parathread(ParathreadEntry),
	/// A parachain.
	Parachain,
}

/// The assignment type.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub enum AssignmentKind {
	/// A parachain.
	Parachain,
	/// A parathread.
	Parathread(CollatorId, u32),
}

/// How a free core is scheduled to be assigned.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub struct CoreAssignment {
	/// The core that is assigned.
	pub core: CoreIndex,
	/// The unique ID of the para that is assigned to the core.
	pub para_id: Id,
	/// The kind of the assignment.
	pub kind: AssignmentKind,
	/// The index of the validator group assigned to the core.
	pub group_idx: GroupIndex,
}

impl CoreAssignment {
	/// Get the ID of a collator who is required to collate this block.
	pub fn required_collator(&self) -> Option<&CollatorId> {
		match self.kind {
			AssignmentKind::Parachain => None,
			AssignmentKind::Parathread(ref id, _) => Some(id),
		}
	}

	/// Get the `CoreOccupied` from this.
	pub fn to_core_occupied(&self) -> CoreOccupied {
		match self.kind {
			AssignmentKind::Parachain => CoreOccupied::Parachain,
			AssignmentKind::Parathread(ref collator, retries) => CoreOccupied::Parathread(
				ParathreadEntry {
					claim: ParathreadClaim(self.para_id, collator.clone()),
					retries,
				}
			),
		}
	}
}

/// Validation data omitted from most candidate descriptor structs, as it can be derived from the
/// relay-parent.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug, Default))]
pub struct OmittedValidationData {
	/// The global validation schedule.
	pub global_validation: GlobalValidationData,
	/// The local validation data.
	pub local_validation: LocalValidationData,
}

/// This is the data we keep available for each candidate included in the relay chain.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub struct AvailableData {
	/// The Proof-of-Validation of the candidate.
	pub pov: PoV,
	/// The omitted validation data.
	pub omitted_validation: OmittedValidationData,
}

/// A helper data-type for tracking validator-group rotations.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub struct GroupRotationInfo<N = BlockNumber> {
	/// The block number where the session started.
	pub session_start_block: N,
	/// How often groups rotate. 0 means never.
	pub group_rotation_frequency: N,
	/// The current block number.
	pub now: N,
}

impl GroupRotationInfo {
	/// Returns the index of the group needed to validate the core at the given index, assuming
	/// the given number of cores.
	///
	/// `core_index` should be less than `cores`, which is capped at u32::max().
	pub fn group_for_core(&self, core_index: CoreIndex, cores: usize) -> GroupIndex {
		if self.group_rotation_frequency == 0 { return GroupIndex(core_index.0) }
		if cores == 0 { return GroupIndex(0) }

		let cores = sp_std::cmp::min(cores, u32::max_value() as usize);
		let blocks_since_start = self.now.saturating_sub(self.session_start_block);
		let rotations = blocks_since_start / self.group_rotation_frequency;

		let idx = (core_index.0 as usize + rotations as usize) % cores;
		GroupIndex(idx as u32)
	}
}

impl<N: Saturating + BaseArithmetic + Copy> GroupRotationInfo<N> {
	/// Returns the block number of the next rotation after the current block. If the current block
	/// is 10 and the rotation frequency is 5, this should return 15.
	///
	/// If the group rotation frequency is 0, returns 0.
	pub fn next_rotation_at(&self) -> N {
		if self.group_rotation_frequency.is_zero() { return Zero::zero() }

		let cycle_once = self.now + self.group_rotation_frequency;
		cycle_once - (
			cycle_once.saturating_sub(self.session_start_block) % self.group_rotation_frequency
		)
	}

	/// Returns the block number of the last rotation before or including the current block. If the
	/// current block is 10 and the rotation frequency is 5, this should return 10.
	///
	/// If the group rotation frequency is 0, returns 0.
	pub fn last_rotation_at(&self) -> N {
		if self.group_rotation_frequency.is_zero() { return Zero::zero() }
		self.now - (
			self.now.saturating_sub(self.session_start_block) % self.group_rotation_frequency
		)
	}
}

/// Information about a core which is currently occupied.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub struct OccupiedCore<N = BlockNumber> {
	/// The ID of the para occupying the core.
	pub para_id: Id,
	/// If this core is freed by availability, this is the assignment that is next up on this
	/// core, if any. None if there is nothing queued for this core.
	pub next_up_on_available: Option<ScheduledCore>,
	/// The relay-chain block number this began occupying the core at.
	pub occupied_since: N,
	/// The relay-chain block this will time-out at, if any.
	pub time_out_at: N,
	/// If this core is freed by being timed-out, this is the assignment that is next up on this
	/// core. None if there is nothing queued for this core or there is no possibility of timing
	/// out.
	pub next_up_on_time_out: Option<ScheduledCore>,
	/// A bitfield with 1 bit for each validator in the set. `1` bits mean that the corresponding
	/// validators has attested to availability on-chain. A 2/3+ majority of `1` bits means that
	/// this will be available.
	pub availability: BitVec<bitvec::order::Lsb0, u8>,
	/// The group assigned to distribute availability pieces of this candidate.
	pub group_responsible: GroupIndex,
}

/// Information about a core which is currently occupied.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug, Default))]
pub struct ScheduledCore {
	/// The ID of a para scheduled.
	pub para_id: Id,
	/// The collator required to author the block, if any.
	pub collator: Option<CollatorId>,
}

/// The state of a particular availability core.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub enum CoreState<N = BlockNumber> {
	/// The core is currently occupied.
	#[codec(index = "0")]
	Occupied(OccupiedCore<N>),
	/// The core is currently free, with a para scheduled and given the opportunity
	/// to occupy.
	///
	/// If a particular Collator is required to author this block, that is also present in this
	/// variant.
	#[codec(index = "1")]
	Scheduled(ScheduledCore),
	/// The core is currently free and there is nothing scheduled. This can be the case for parathread
	/// cores when there are no parathread blocks queued. Parachain cores will never be left idle.
	#[codec(index = "2")]
	Free,
}

impl<N> CoreState<N> {
	/// If this core state has a `para_id`, return it.
	pub fn para_id(&self) -> Option<Id> {
		match self {
			Self::Occupied(OccupiedCore { para_id, ..}) => Some(*para_id),
			Self::Scheduled(ScheduledCore { para_id, .. }) => Some(*para_id),
			Self::Free => None,
		}
	}
}

/// An assumption being made about the state of an occupied core.
#[derive(Clone, Copy, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub enum OccupiedCoreAssumption {
	/// The candidate occupying the core was made available and included to free the core.
	#[codec(index = "0")]
	Included,
	/// The candidate occupying the core timed out and freed the core without advancing the para.
	#[codec(index = "1")]
	TimedOut,
	/// The core was not occupied to begin with.
	#[codec(index = "2")]
	Free,
}

/// An even concerning a candidate.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub enum CandidateEvent<H = Hash> {
	/// This candidate receipt was backed in the most recent block.
	#[codec(index = "0")]
	CandidateBacked(CandidateReceipt<H>, HeadData),
	/// This candidate receipt was included and became a parablock at the most recent block.
	#[codec(index = "1")]
	CandidateIncluded(CandidateReceipt<H>, HeadData),
	/// This candidate receipt was not made available in time and timed out.
	#[codec(index = "2")]
	CandidateTimedOut(CandidateReceipt<H>, HeadData),
}

sp_api::decl_runtime_apis! {
	/// The API for querying the state of parachains on-chain.
	pub trait ParachainHost<H: Decode = Hash, N: Decode = BlockNumber> {
		/// Get the current validators.
		fn validators() -> Vec<ValidatorId>;

		/// Returns the validator groups and rotation info localized based on the block whose state
		/// this is invoked on. Note that `now` in the `GroupRotationInfo` should be the successor of
		/// the number of the block.
		fn validator_groups() -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo<N>);

		/// Yields information on all availability cores. Cores are either free or occupied. Free
		/// cores can have paras assigned to them.
		fn availability_cores() -> Vec<CoreState<N>>;

		/// Yields the GlobalValidationData. This applies to all para candidates with the
		/// relay-parent equal to the block in which context this is invoked in.
		fn global_validation_data() -> GlobalValidationData<N>;

		/// Yields the LocalValidationData for the given ParaId along with an assumption that
		/// should be used if the para currently occupies a core.
		///
		/// Returns `None` if either the para is not registered or the assumption is `Freed`
		/// and the para already occupies a core.
		fn local_validation_data(para_id: Id, assumption: OccupiedCoreAssumption)
			-> Option<LocalValidationData<N>>;

		/// Returns the session index expected at a child of the block.
		///
		/// This can be used to instantiate a `SigningContext`.
		fn session_index_for_child() -> SessionIndex;

		/// Fetch the validation code used by a para, making the given `OccupiedCoreAssumption`.
		///
		/// Returns `None` if either the para is not registered or the assumption is `Freed`
		/// and the para already occupies a core.
		fn validation_code(para_id: Id, assumption: OccupiedCoreAssumption)
			-> Option<ValidationCode>;

		/// Get the receipt of a candidate pending availability. This returns `Some` for any paras
		/// assigned to occupied cores in `availability_cores` and `None` otherwise.
		fn candidate_pending_availability(para_id: Id) -> Option<CommittedCandidateReceipt<H>>;

		/// Get a vector of events concerning candidates that occurred within a block.
		// NOTE: this needs to skip block initialization as events are wiped within block
		// initialization.
		#[skip_initialize_block]
		fn candidate_events() -> Vec<CandidateEvent<H>>;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn group_rotation_info_calculations() {
		let info = GroupRotationInfo {
			session_start_block: 10u32,
			now: 15,
			group_rotation_frequency: 5,
		};

		assert_eq!(info.next_rotation_at(), 20);
		assert_eq!(info.last_rotation_at(), 15);

		let info = GroupRotationInfo {
			session_start_block: 10u32,
			now: 11,
			group_rotation_frequency: 0,
		};

		assert_eq!(info.next_rotation_at(), 0);
		assert_eq!(info.last_rotation_at(), 0);
	}

	#[test]
	fn collator_signature_payload_is_valid() {
		// if this fails, collator signature verification code has to be updated.
		let h = Hash::default();
		assert_eq!(h.as_ref().len(), 32);

		let _payload = collator_signature_payload(
			&Hash::from([1; 32]),
			&5u32.into(),
			&Hash::from([2; 32]),
			&Hash::from([3; 32]),
		);
	}
}
