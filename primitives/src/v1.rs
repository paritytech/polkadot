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
use sp_std::collections::btree_map::BTreeMap;
use parity_scale_codec::{Encode, Decode};
use bitvec::vec::BitVec;

use primitives::RuntimeDebug;
use runtime_primitives::traits::AppVerify;
use inherents::InherentIdentifier;
use sp_arithmetic::traits::{BaseArithmetic, Saturating};
use application_crypto::KeyTypeId;

pub use runtime_primitives::traits::{BlakeTwo256, Hash as HashT};

// Export some core primitives.
pub use polkadot_core_primitives::v1::{
	BlockNumber, Moment, Signature, AccountPublic, AccountId, AccountIndex, ChainId, Hash, Nonce,
	Balance, Header, Block, BlockId, UncheckedExtrinsic, Remark, DownwardMessage,
	InboundDownwardMessage, CandidateHash, InboundHrmpMessage, OutboundHrmpMessage,
};

// Export some polkadot-parachain primitives
pub use polkadot_parachain::primitives::{
	Id, LOWEST_USER_ID, HrmpChannelId, UpwardMessage, HeadData, BlockData, ValidationCode,
};

// Export some basic parachain primitives from v0.
pub use crate::v0::{
	CollatorId, CollatorSignature, PARACHAIN_KEY_TYPE_ID, ValidatorId, ValidatorIndex,
	ValidatorSignature, SigningContext, Signed, ValidityAttestation,
	CompactStatement, SignedStatement, ErasureChunk, EncodeAs,
};

#[cfg(feature = "std")]
use parity_util_mem::{MallocSizeOf, MallocSizeOfOps};

// More exports from v0 for std.
#[cfg(feature = "std")]
pub use crate::v0::{ValidatorPair, CollatorPair};

pub use sp_staking::SessionIndex;
pub use sp_authority_discovery::AuthorityId as AuthorityDiscoveryId;

/// A declarations of storage keys where an external observer can find some interesting data.
pub mod well_known_keys {
	use super::{Id, HrmpChannelId};
	use hex_literal::hex;
	use sp_io::hashing::twox_64;
	use sp_std::prelude::*;
	use parity_scale_codec::Encode as _;

	// A note on generating these magic values below:
	//
	// The `StorageValue`, such as `ACTIVE_CONFIG` was obtained by calling:
	//
	//     <Self as Store>::ActiveConfig::hashed_key()
	//
	// The `StorageMap` values require `prefix`, and for example for `hrmp_egress_channel_index`,
	// it could be obtained like:
	//
	//     <Hrmp as Store>::HrmpEgressChannelsIndex::prefix_hash();
	//

	/// The currently active host configuration.
	///
	/// The storage entry should be accessed as an `AbridgedHostConfiguration` encoded value.
	pub const ACTIVE_CONFIG: &[u8] =
		&hex!["06de3d8a54d27e44a9d5ce189618f22db4b49d95320d9021994c850f25b8e385"];

	/// The upward message dispatch queue for the given para id.
	///
	/// The storage entry stores a tuple of two values:
	///
	/// - `count: u32`, the number of messages currently in the queue for given para,
	/// - `total_size: u32`, the total size of all messages in the queue.
	pub fn relay_dispatch_queue_size(para_id: Id) -> Vec<u8> {
		let prefix = hex!["f5207f03cfdce586301014700e2c2593fad157e461d71fd4c1f936839a5f1f3e"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// The hrmp channel for the given identifier.
	///
	/// The storage entry should be accessed as an `AbridgedHrmpChannel` encoded value.
	pub fn hrmp_channels(channel: HrmpChannelId) -> Vec<u8> {
		let prefix = hex!["6a0da05ca59913bc38a8630590f2627cb6604cff828a6e3f579ca6c59ace013d"];

		channel.using_encoded(|channel: &[u8]| {
			prefix.as_ref()
				.iter()
				.chain(twox_64(channel).iter())
				.chain(channel.iter())
				.cloned()
				.collect()
		})
	}

	/// The list of inbound channels for the given para.
	///
	/// The storage entry stores a `Vec<ParaId>`
	pub fn hrmp_ingress_channel_index(para_id: Id) -> Vec<u8> {
		let prefix = hex!["6a0da05ca59913bc38a8630590f2627c1d3719f5b0b12c7105c073c507445948"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// The list of outbound channels for the given para.
	///
	/// The storage entry stores a `Vec<ParaId>`
	pub fn hrmp_egress_channel_index(para_id: Id) -> Vec<u8> {
		let prefix = hex!["6a0da05ca59913bc38a8630590f2627cf12b746dcf32e843354583c9702cc020"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// The MQC head for the downward message queue of the given para. See more in the `Dmp` module.
	///
	/// The storage entry stores a `Hash`. This is polkadot hash which is at the moment
	/// `blake2b-256`.
	pub fn dmq_mqc_head(para_id: Id) -> Vec<u8> {
		let prefix = hex!["63f78c98723ddc9073523ef3beefda0c4d7fefc408aac59dbfe80a72ac8e3ce5"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}
}

/// Unique identifier for the Inclusion Inherent
pub const INCLUSION_INHERENT_IDENTIFIER: InherentIdentifier = *b"inclusn0";

/// The key type ID for parachain assignment key.
pub const ASSIGNMENT_KEY_TYPE_ID: KeyTypeId = KeyTypeId(*b"asgn");

// The public key of a keypair used by a validator for determining assignments
/// to approve included parachain candidates.
mod assignment_app {
	use application_crypto::{app_crypto, sr25519};
	app_crypto!(sr25519, super::ASSIGNMENT_KEY_TYPE_ID);
}

/// The public key of a keypair used by a validator for determining assignments
/// to approve included parachain candidates.
pub type AssignmentId = assignment_app::Public;

application_crypto::with_pair! {
	/// The full keypair used by a validator for determining assignments to approve included
	/// parachain candidates.
	pub type AssignmentPair = assignment_app::Pair;
}

#[cfg(feature = "std")]
impl MallocSizeOf for AssignmentId {
	fn size_of(&self, _ops: &mut MallocSizeOfOps) -> usize {
		0
	}
	fn constant_size() -> Option<usize> {
		Some(0)
	}
}

/// The index of the candidate in the list of candidates fully included as-of the block.
pub type CandidateIndex = u32;

/// Get a collator signature payload on a relay-parent, block-data combo.
pub fn collator_signature_payload<H: AsRef<[u8]>>(
	relay_parent: &H,
	para_id: &Id,
	persisted_validation_data_hash: &Hash,
	pov_hash: &Hash,
) -> [u8; 100] {
	// 32-byte hash length is protected in a test below.
	let mut payload = [0u8; 100];

	payload[0..32].copy_from_slice(relay_parent.as_ref());
	u32::from(*para_id).using_encoded(|s| payload[32..32 + s.len()].copy_from_slice(s));
	payload[36..68].copy_from_slice(persisted_validation_data_hash.as_ref());
	payload[68..100].copy_from_slice(pov_hash.as_ref());

	payload
}

fn check_collator_signature<H: AsRef<[u8]>>(
	relay_parent: &H,
	para_id: &Id,
	persisted_validation_data_hash: &Hash,
	pov_hash: &Hash,
	collator: &CollatorId,
	signature: &CollatorSignature,
) -> Result<(),()> {
	let payload = collator_signature_payload(
		relay_parent,
		para_id,
		persisted_validation_data_hash,
		pov_hash,
	);

	if signature.verify(&payload[..], collator) {
		Ok(())
	} else {
		Err(())
	}
}

/// A unique descriptor of the candidate receipt.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default, Hash, MallocSizeOf))]
pub struct CandidateDescriptor<H = Hash> {
	/// The ID of the para this is a candidate for.
	pub para_id: Id,
	/// The hash of the relay-chain block this is executed in the context of.
	pub relay_parent: H,
	/// The collator's sr25519 public key.
	pub collator: CollatorId,
	/// The blake2-256 hash of the persisted validation data. This is extra data derived from
	/// relay-chain state which may vary based on bitfields included before the candidate.
	/// Thus it cannot be derived entirely from the relay-parent.
	pub persisted_validation_data_hash: Hash,
	/// The blake2-256 hash of the pov.
	pub pov_hash: Hash,
	/// The root of a block's erasure encoding Merkle tree.
	pub erasure_root: Hash,
	/// Signature on blake2-256 of components of this receipt:
	/// The parachain index, the relay parent, the validation data hash, and the pov_hash.
	pub signature: CollatorSignature,
	/// Hash of the para header that is being generated by this candidate.
	pub para_head: Hash,
}

impl<H: AsRef<[u8]>> CandidateDescriptor<H> {
	/// Check the signature of the collator within this descriptor.
	pub fn check_collator_signature(&self) -> Result<(), ()> {
		check_collator_signature(
			&self.relay_parent,
			&self.para_id,
			&self.persisted_validation_data_hash,
			&self.pov_hash,
			&self.collator,
			&self.signature,
		)
	}
}

/// A candidate-receipt.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default, MallocSizeOf))]
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
	pub fn hash(&self) -> CandidateHash where H: Encode {
		CandidateHash(BlakeTwo256::hash_of(self))
	}
}

/// All data pertaining to the execution of a para candidate.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct FullCandidateReceipt<H = Hash, N = BlockNumber> {
	/// The inner candidate receipt.
	pub inner: CandidateReceipt<H>,
	/// The validation data derived from the relay-chain state at that
	/// point. The hash of the persisted validation data should
	/// match the `persisted_validation_data_hash` in the descriptor
	/// of the receipt.
	pub validation_data: PersistedValidationData<N>,
}

/// A candidate-receipt with commitments directly included.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default, Hash, MallocSizeOf))]
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
	pub fn hash(&self) -> CandidateHash where H: Encode {
		self.to_plain().hash()
	}

	/// Does this committed candidate receipt corrensponds to the given [`CandidateReceipt`]?
	pub fn corresponds_to(&self, receipt: &CandidateReceipt<H>) -> bool where H: PartialEq {
		receipt.descriptor == self.descriptor && receipt.commitments_hash == self.commitments.hash()
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

/// The validation data provides information about how to create the inputs for validation of a candidate.
/// This information is derived from the chain state and will vary from para to para, although some of the
/// fields may be the same for every para.
///
/// Since this data is used to form inputs to the validation function, it needs to be persisted by the
/// availability system to avoid dependence on availability of the relay-chain state.
///
/// Furthermore, the validation data acts as a way to authorize the additional data the collator needs
/// to pass to the validation function. For example, the validation function can check whether the incoming
/// messages (e.g. downward messages) were actually sent by using the data provided in the validation data
/// using so called MQC heads.
///
/// Since the commitments of the validation function are checked by the relay-chain, secondary checkers
/// can rely on the invariant that the relay-chain only includes para-blocks for which these checks have
/// already been done. As such, there is no need for the validation data used to inform validators and
/// collators about the checks the relay-chain will perform to be persisted by the availability system.
///
/// The `PersistedValidationData` should be relatively lightweight primarly because it is constructed
/// during inclusion for each candidate and therefore lies on the critical path of inclusion.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default, MallocSizeOf))]
pub struct PersistedValidationData<N = BlockNumber> {
	/// The parent head-data.
	pub parent_head: HeadData,
	/// The relay-chain block number this is in the context of.
	pub relay_parent_number: N,
	/// The relay-chain block storage root this is in the context of.
	pub relay_parent_storage_root: Hash,
	/// The maximum legal size of a POV block, in bytes.
	pub max_pov_size: u32,
}

impl<N: Encode> PersistedValidationData<N> {
	/// Compute the blake2-256 hash of the persisted validation data.
	pub fn hash(&self) -> Hash {
		BlakeTwo256::hash_of(self)
	}
}

/// Commitments made in a `CandidateReceipt`. Many of these are outputs of validation.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default, Hash, MallocSizeOf))]
pub struct CandidateCommitments<N = BlockNumber> {
	/// Messages destined to be interpreted by the Relay chain itself.
	pub upward_messages: Vec<UpwardMessage>,
	/// Horizontal messages sent by the parachain.
	pub horizontal_messages: Vec<OutboundHrmpMessage<Id>>,
	/// New validation code.
	pub new_validation_code: Option<ValidationCode>,
	/// The head-data produced as a result of execution.
	pub head_data: HeadData,
	/// The number of messages processed from the DMQ.
	pub processed_downward_messages: u32,
	/// The mark which specifies the block number up to which all inbound HRMP messages are processed.
	pub hrmp_watermark: N,
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
#[cfg_attr(feature = "std", derive(Default))]
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

	/// Compute this candidate's hash.
	pub fn hash(&self) -> CandidateHash where H: Clone + Encode {
		self.candidate.hash()
	}

	/// Get this candidate's receipt.
	pub fn receipt(&self) -> CandidateReceipt<H> where H: Clone {
		self.candidate.to_plain()
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
	let hash = backed.candidate.hash();

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
#[cfg_attr(feature = "std", derive(Debug, Hash, MallocSizeOf))]
pub struct CoreIndex(pub u32);

impl From<u32> for CoreIndex {
	fn from(i: u32) -> CoreIndex {
		CoreIndex(i)
	}
}

/// The unique (during session) index of a validator group.
#[derive(Encode, Decode, Default, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Hash, MallocSizeOf))]
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

/// This is the data we keep available for each candidate included in the relay chain.
#[cfg(feature = "std")]
#[derive(Clone, Encode, Decode, PartialEq, Eq, Debug)]
pub struct AvailableData {
	/// The Proof-of-Validation of the candidate.
	pub pov: std::sync::Arc<PoV>,
	/// The persisted validation data needed for secondary checks.
	pub validation_data: PersistedValidationData,
}

/// A helper data-type for tracking validator-group rotations.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug, MallocSizeOf))]
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
	pub fn next_rotation_at(&self) -> N {
		let cycle_once = self.now + self.group_rotation_frequency;
		cycle_once - (
			cycle_once.saturating_sub(self.session_start_block) % self.group_rotation_frequency
		)
	}

	/// Returns the block number of the last rotation before or including the current block. If the
	/// current block is 10 and the rotation frequency is 5, this should return 10.
	pub fn last_rotation_at(&self) -> N {
		self.now - (
			self.now.saturating_sub(self.session_start_block) % self.group_rotation_frequency
		)
	}
}

/// Information about a core which is currently occupied.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, PartialEq, MallocSizeOf))]
pub struct OccupiedCore<H = Hash, N = BlockNumber> {
    // NOTE: this has no ParaId as it can be deduced from the candidate descriptor.

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
	#[cfg_attr(feature = "std", ignore_malloc_size_of = "outside type")]
	pub availability: BitVec<bitvec::order::Lsb0, u8>,
	/// The group assigned to distribute availability pieces of this candidate.
	pub group_responsible: GroupIndex,
	/// The hash of the candidate occupying the core.
	pub candidate_hash: CandidateHash,
	/// The descriptor of the candidate occupying the core.
	pub candidate_descriptor: CandidateDescriptor<H>,
}

impl<H, N> OccupiedCore<H, N> {
	/// Get the Para currently occupying this core.
	pub fn para_id(&self) -> Id {
		self.candidate_descriptor.para_id
	}
}

/// Information about a core which is currently occupied.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, PartialEq, Default, MallocSizeOf))]
pub struct ScheduledCore {
	/// The ID of a para scheduled.
	pub para_id: Id,
	/// The collator required to author the block, if any.
	pub collator: Option<CollatorId>,
}

/// The state of a particular availability core.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, PartialEq, MallocSizeOf))]
pub enum CoreState<H = Hash, N = BlockNumber> {
	/// The core is currently occupied.
	#[codec(index = 0)]
	Occupied(OccupiedCore<H, N>),
	/// The core is currently free, with a para scheduled and given the opportunity
	/// to occupy.
	///
	/// If a particular Collator is required to author this block, that is also present in this
	/// variant.
	#[codec(index = 1)]
	Scheduled(ScheduledCore),
	/// The core is currently free and there is nothing scheduled. This can be the case for parathread
	/// cores when there are no parathread blocks queued. Parachain cores will never be left idle.
	#[codec(index = 2)]
	Free,
}

impl<N> CoreState<N> {
	/// If this core state has a `para_id`, return it.
	pub fn para_id(&self) -> Option<Id> {
		match self {
			Self::Occupied(ref core) => Some(core.para_id()),
			Self::Scheduled(ScheduledCore { para_id, .. }) => Some(*para_id),
			Self::Free => None,
		}
	}

	/// Is this core state `Self::Occupied`?
	pub fn is_occupied(&self) -> bool {
		matches!(self, Self::Occupied(_))
	}
}

/// An assumption being made about the state of an occupied core.
#[derive(Clone, Copy, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Eq, Hash, Debug))]
pub enum OccupiedCoreAssumption {
	/// The candidate occupying the core was made available and included to free the core.
	#[codec(index = 0)]
	Included,
	/// The candidate occupying the core timed out and freed the core without advancing the para.
	#[codec(index = 1)]
	TimedOut,
	/// The core was not occupied to begin with.
	#[codec(index = 2)]
	Free,
}

/// An even concerning a candidate.
#[derive(Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug, MallocSizeOf))]
pub enum CandidateEvent<H = Hash> {
	/// This candidate receipt was backed in the most recent block.
	/// This includes the core index the candidate is now occupying.
	#[codec(index = 0)]
	CandidateBacked(CandidateReceipt<H>, HeadData, CoreIndex, GroupIndex),
	/// This candidate receipt was included and became a parablock at the most recent block.
	/// This includes the core index the candidate was occupying as well as the group responsible
	/// for backing the candidate.
	#[codec(index = 1)]
	CandidateIncluded(CandidateReceipt<H>, HeadData, CoreIndex, GroupIndex),
	/// This candidate receipt was not made available in time and timed out.
	/// This includes the core index the candidate was occupying.
	#[codec(index = 2)]
	CandidateTimedOut(CandidateReceipt<H>, HeadData, CoreIndex),
}

/// Information about validator sets of a session.
#[derive(Clone, Encode, Decode, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq, Default, MallocSizeOf))]
pub struct SessionInfo {
	/// Validators in canonical ordering.
	pub validators: Vec<ValidatorId>,
	/// Validators' authority discovery keys for the session in canonical ordering.
	#[cfg_attr(feature = "std", ignore_malloc_size_of = "outside type")]
	pub discovery_keys: Vec<AuthorityDiscoveryId>,
	/// The assignment keys for validators.
	pub assignment_keys: Vec<AssignmentId>,
	/// Validators in shuffled ordering - these are the validator groups as produced
	/// by the `Scheduler` module for the session and are typically referred to by
	/// `GroupIndex`.
	pub validator_groups: Vec<Vec<ValidatorIndex>>,
	/// The number of availability cores used by the protocol during this session.
	pub n_cores: u32,
	/// The zeroth delay tranche width.
	pub zeroth_delay_tranche_width: u32,
	/// The number of samples we do of relay_vrf_modulo.
	pub relay_vrf_modulo_samples: u32,
	/// The number of delay tranches in total.
	pub n_delay_tranches: u32,
	/// How many slots (BABE / SASSAFRAS) must pass before an assignment is considered a
	/// no-show.
	pub no_show_slots: u32,
	/// The number of validators needed to approve a block.
	pub needed_approvals: u32,
}

sp_api::decl_runtime_apis! {
	/// The API for querying the state of parachains on-chain.
	pub trait ParachainHost<H: Decode = Hash, N: Encode + Decode = BlockNumber> {
		// NOTE: Many runtime API are declared with `#[skip_initialize_block]`. This is because without
		// this attribute before each runtime call, the `initialize_block` runtime API will be called.
		// That in turns will lead to two things:
		//
		// (a) The frame_system module will be initialized to the next block.
		// (b) Initialization sequences for each runtime module (pallet) will be run.
		//
		// (a) is undesirable because the runtime APIs are querying the state against a specific
		// block state. However, due to that initialization the observed block number would be as if
		// it was the next block.
		//
		// We dont want (b) mainly because block initialization can be very heavy. Upgrade enactment,
		// storage migration, and whatever other logic exists in `on_initialize` will be executed
		// if not explicitly opted out with the `#[skip_initialize_block]` attribute.
		//
		// Additionally, some runtime APIs may depend on state that is pruned on the `on_initialize`.
		// At the moment of writing, this is `candidate_events`.

		/// Get the current validators.
		#[skip_initialize_block]
		fn validators() -> Vec<ValidatorId>;

		/// Returns the validator groups and rotation info localized based on the hypothetical child
		///  of a block whose state  this is invoked on. Note that `now` in the `GroupRotationInfo`
		/// should be the successor of the number of the block.
		#[skip_initialize_block]
		fn validator_groups() -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo<N>);

		/// Yields information on all availability cores as relevant to the child block.
		/// Cores are either free or occupied. Free cores can have paras assigned to them.
		#[skip_initialize_block]
		fn availability_cores() -> Vec<CoreState<H, N>>;

		/// Yields the persisted validation data for the given ParaId along with an assumption that
		/// should be used if the para currently occupies a core.
		///
		/// Returns `None` if either the para is not registered or the assumption is `Freed`
		/// and the para already occupies a core.
		#[skip_initialize_block]
		fn persisted_validation_data(para_id: Id, assumption: OccupiedCoreAssumption)
			-> Option<PersistedValidationData<N>>;

		/// Checks if the given validation outputs pass the acceptance criteria.
		#[skip_initialize_block]
		fn check_validation_outputs(para_id: Id, outputs: CandidateCommitments) -> bool;

		/// Returns the session index expected at a child of the block.
		///
		/// This can be used to instantiate a `SigningContext`.
		#[skip_initialize_block]
		fn session_index_for_child() -> SessionIndex;

		/// Get the session info for the given session, if stored.
		#[skip_initialize_block]
		fn session_info(index: SessionIndex) -> Option<SessionInfo>;

		/// Fetch the validation code used by a para, making the given `OccupiedCoreAssumption`.
		///
		/// Returns `None` if either the para is not registered or the assumption is `Freed`
		/// and the para already occupies a core.
		#[skip_initialize_block]
		fn validation_code(para_id: Id, assumption: OccupiedCoreAssumption)
			-> Option<ValidationCode>;

		/// Fetch the historical validation code used by a para for candidates executed in the
		/// context of a given block height in the current chain.
		///
		/// `context_height` may be no greater than the height of the block in whose
		/// state the runtime API is executed.
		#[skip_initialize_block]
		fn historical_validation_code(para_id: Id, context_height: N)
			-> Option<ValidationCode>;

		/// Get the receipt of a candidate pending availability. This returns `Some` for any paras
		/// assigned to occupied cores in `availability_cores` and `None` otherwise.
		#[skip_initialize_block]
		fn candidate_pending_availability(para_id: Id) -> Option<CommittedCandidateReceipt<H>>;

		/// Get a vector of events concerning candidates that occurred within a block.
		#[skip_initialize_block]
		fn candidate_events() -> Vec<CandidateEvent<H>>;

		/// Get all the pending inbound messages in the downward message queue for a para.
		#[skip_initialize_block]
		fn dmq_contents(
			recipient: Id,
		) -> Vec<InboundDownwardMessage<N>>;

		/// Get the contents of all channels addressed to the given recipient. Channels that have no
		/// messages in them are also included.
		#[skip_initialize_block]
		fn inbound_hrmp_channels_contents(recipient: Id) -> BTreeMap<Id, Vec<InboundHrmpMessage<N>>>;
	}
}

/// Custom validity errors used in Polkadot while validating transactions.
#[repr(u8)]
pub enum ValidityError {
	/// The Ethereum signature is invalid.
	InvalidEthereumSignature = 0,
	/// The signer has no claim.
	SignerHasNoClaim = 1,
	/// No permission to execute the call.
	NoPermission = 2,
	/// An invalid statement was made for a claim.
	InvalidStatement = 3,
}

impl From<ValidityError> for u8 {
	fn from(err: ValidityError) -> Self {
		err as u8
	}
}

/// Abridged version of `HostConfiguration` (from the `Configuration` parachains host runtime module)
/// meant to be used by a parachain or PDK such as cumulus.
#[derive(Clone, Encode, Decode, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct AbridgedHostConfiguration {
	/// The maximum validation code size, in bytes.
	pub max_code_size: u32,
	/// The maximum head-data size, in bytes.
	pub max_head_data_size: u32,
	/// Total number of individual messages allowed in the parachain -> relay-chain message queue.
	pub max_upward_queue_count: u32,
	/// Total size of messages allowed in the parachain -> relay-chain message queue before which
	/// no further messages may be added to it. If it exceeds this then the queue may contain only
	/// a single message.
	pub max_upward_queue_size: u32,
	/// The maximum size of an upward message that can be sent by a candidate.
	///
	/// This parameter affects the size upper bound of the `CandidateCommitments`.
	pub max_upward_message_size: u32,
	/// The maximum number of messages that a candidate can contain.
	///
	/// This parameter affects the size upper bound of the `CandidateCommitments`.
	pub max_upward_message_num_per_candidate: u32,
	/// The maximum number of outbound HRMP messages can be sent by a candidate.
	///
	/// This parameter affects the upper bound of size of `CandidateCommitments`.
	pub hrmp_max_message_num_per_candidate: u32,
	/// The minimum frequency at which parachains can update their validation code.
	pub validation_upgrade_frequency: BlockNumber,
	/// The delay, in blocks, before a validation upgrade is applied.
	pub validation_upgrade_delay: BlockNumber,
}

/// Abridged version of `HrmpChannel` (from the `Hrmp` parachains host runtime module) meant to be
/// used by a parachain or PDK such as cumulus.
#[derive(Clone, Encode, Decode, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct AbridgedHrmpChannel {
	/// The maximum number of messages that can be pending in the channel at once.
	pub max_capacity: u32,
	/// The maximum total size of the messages that can be pending in the channel at once.
	pub max_total_size: u32,
	/// The maximum message size that could be put into the channel.
	pub max_message_size: u32,
	/// The current number of messages pending in the channel.
	/// Invariant: should be less or equal to `max_capacity`.s`.
	pub msg_count: u32,
	/// The total size in bytes of all message payloads in the channel.
	/// Invariant: should be less or equal to `max_total_size`.
	pub total_size: u32,
	/// A head of the Message Queue Chain for this channel. Each link in this chain has a form:
	/// `(prev_head, B, H(M))`, where
	/// - `prev_head`: is the previous value of `mqc_head` or zero if none.
	/// - `B`: is the [relay-chain] block number in which a message was appended
	/// - `H(M)`: is the hash of the message being appended.
	/// This value is initialized to a special value that consists of all zeroes which indicates
	/// that no messages were previously added.
	pub mqc_head: Option<Hash>,
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
	}

	#[test]
	fn collator_signature_payload_is_valid() {
		// if this fails, collator signature verification code has to be updated.
		let h = Hash::default();
		assert_eq!(h.as_ref().len(), 32);

		let _payload = collator_signature_payload(
			&Hash::repeat_byte(1),
			&5u32.into(),
			&Hash::repeat_byte(2),
			&Hash::repeat_byte(3),
		);
	}
}
