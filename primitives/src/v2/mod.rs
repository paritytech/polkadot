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

//! `V1` Primitives.

use bitvec::vec::BitVec;
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;
use sp_std::{
	marker::PhantomData,
	prelude::*,
	slice::{Iter, IterMut},
	vec::IntoIter,
};

use application_crypto::KeyTypeId;
use inherents::InherentIdentifier;
use primitives::RuntimeDebug;
use runtime_primitives::traits::{AppVerify, Header as HeaderT};
use sp_arithmetic::traits::{BaseArithmetic, Saturating};

pub use runtime_primitives::traits::{BlakeTwo256, Hash as HashT};

// Export some core primitives.
pub use polkadot_core_primitives::v2::{
	AccountId, AccountIndex, AccountPublic, Balance, Block, BlockId, BlockNumber, CandidateHash,
	ChainId, DownwardMessage, Hash, Header, InboundDownwardMessage, InboundHrmpMessage, Moment,
	Nonce, OutboundHrmpMessage, Remark, Signature, UncheckedExtrinsic,
};

// Export some polkadot-parachain primitives
pub use polkadot_parachain::primitives::{
	HeadData, HrmpChannelId, Id, UpwardMessage, ValidationCode, ValidationCodeHash,
	LOWEST_PUBLIC_ID, LOWEST_USER_ID,
};

#[cfg(feature = "std")]
use serde::{Deserialize, Serialize};

pub use sp_authority_discovery::AuthorityId as AuthorityDiscoveryId;
pub use sp_consensus_slots::Slot;
pub use sp_staking::SessionIndex;

/// Signed data.
mod signed;
pub use signed::{EncodeAs, Signed, UncheckedSigned};

mod metrics;
pub use metrics::{
	metric_definitions, RuntimeMetricLabel, RuntimeMetricLabelValue, RuntimeMetricLabelValues,
	RuntimeMetricLabels, RuntimeMetricOp, RuntimeMetricUpdate,
};

/// The key type ID for a collator key.
pub const COLLATOR_KEY_TYPE_ID: KeyTypeId = KeyTypeId(*b"coll");

mod collator_app {
	use application_crypto::{app_crypto, sr25519};
	app_crypto!(sr25519, super::COLLATOR_KEY_TYPE_ID);
}

/// Identity that collators use.
pub type CollatorId = collator_app::Public;

/// A Parachain collator keypair.
#[cfg(feature = "std")]
pub type CollatorPair = collator_app::Pair;

/// Signature on candidate's block data by a collator.
pub type CollatorSignature = collator_app::Signature;

/// The key type ID for a parachain validator key.
pub const PARACHAIN_KEY_TYPE_ID: KeyTypeId = KeyTypeId(*b"para");

mod validator_app {
	use application_crypto::{app_crypto, sr25519};
	app_crypto!(sr25519, super::PARACHAIN_KEY_TYPE_ID);
}

/// Identity that parachain validators use when signing validation messages.
///
/// For now we assert that parachain validator set is exactly equivalent to the authority set, and
/// so we define it to be the same type as `SessionKey`. In the future it may have different crypto.
pub type ValidatorId = validator_app::Public;

/// Trait required for type specific indices e.g. `ValidatorIndex` and `GroupIndex`
pub trait TypeIndex {
	/// Returns the index associated to this value.
	fn type_index(&self) -> usize;
}

/// Index of the validator is used as a lightweight replacement of the `ValidatorId` when appropriate.
#[derive(Eq, Ord, PartialEq, PartialOrd, Copy, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Hash))]
pub struct ValidatorIndex(pub u32);

// We should really get https://github.com/paritytech/polkadot/issues/2403 going ..
impl From<u32> for ValidatorIndex {
	fn from(n: u32) -> Self {
		ValidatorIndex(n)
	}
}

impl TypeIndex for ValidatorIndex {
	fn type_index(&self) -> usize {
		self.0 as usize
	}
}

application_crypto::with_pair! {
	/// A Parachain validator keypair.
	pub type ValidatorPair = validator_app::Pair;
}

/// Signature with which parachain validators sign blocks.
///
/// For now we assert that parachain validator set is exactly equivalent to the authority set, and
/// so we define it to be the same type as `SessionKey`. In the future it may have different crypto.
pub type ValidatorSignature = validator_app::Signature;

/// A declarations of storage keys where an external observer can find some interesting data.
pub mod well_known_keys {
	use super::{HrmpChannelId, Id};
	use hex_literal::hex;
	use parity_scale_codec::Encode as _;
	use sp_io::hashing::twox_64;
	use sp_std::prelude::*;

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

	/// The current epoch index.
	///
	/// The storage item should be access as a `u64` encoded value.
	pub const EPOCH_INDEX: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087f38316cbf8fa0da822a20ac1c55bf1be3"];

	/// The current relay chain block randomness
	///
	/// The storage item should be accessed as a `schnorrkel::Randomness` encoded value.
	pub const CURRENT_BLOCK_RANDOMNESS: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087fd077dfdb8adb10f78f10a5df8742c545"];

	/// The randomness for one epoch ago
	///
	/// The storage item should be accessed as a `schnorrkel::Randomness` encoded value.
	pub const ONE_EPOCH_AGO_RANDOMNESS: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087f7ce678799d3eff024253b90e84927cc6"];

	/// The randomness for two epochs ago
	///
	/// The storage item should be accessed as a `schnorrkel::Randomness` encoded value.
	pub const TWO_EPOCHS_AGO_RANDOMNESS: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087f7a414cb008e0e61e46722aa60abdd672"];

	/// The current slot number.
	///
	/// The storage entry should be accessed as a `Slot` encoded value.
	pub const CURRENT_SLOT: &[u8] =
		&hex!["1cb6f36e027abb2091cfb5110ab5087f06155b3cd9a8c9e5e9a23fd5dc13a5ed"];

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
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// The HRMP channel for the given identifier.
	///
	/// The storage entry should be accessed as an `AbridgedHrmpChannel` encoded value.
	pub fn hrmp_channels(channel: HrmpChannelId) -> Vec<u8> {
		let prefix = hex!["6a0da05ca59913bc38a8630590f2627cb6604cff828a6e3f579ca6c59ace013d"];

		channel.using_encoded(|channel: &[u8]| {
			prefix
				.as_ref()
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
			prefix
				.as_ref()
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
			prefix
				.as_ref()
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
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// The signal that indicates whether the parachain should go-ahead with the proposed validation
	/// code upgrade.
	///
	/// The storage entry stores a value of `UpgradeGoAhead` type.
	pub fn upgrade_go_ahead_signal(para_id: Id) -> Vec<u8> {
		let prefix = hex!["cd710b30bd2eab0352ddcc26417aa1949e94c040f5e73d9b7addd6cb603d15d3"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}

	/// The signal that indicates whether the parachain is disallowed to signal an upgrade at this
	/// relay-parent.
	///
	/// The storage entry stores a value of `UpgradeRestriction` type.
	pub fn upgrade_restriction_signal(para_id: Id) -> Vec<u8> {
		let prefix = hex!["cd710b30bd2eab0352ddcc26417aa194f27bbb460270642b5bcaf032ea04d56a"];

		para_id.using_encoded(|para_id: &[u8]| {
			prefix
				.as_ref()
				.iter()
				.chain(twox_64(para_id).iter())
				.chain(para_id.iter())
				.cloned()
				.collect()
		})
	}
}

/// Unique identifier for the Parachains Inherent
pub const PARACHAINS_INHERENT_IDENTIFIER: InherentIdentifier = *b"parachn0";

/// The key type ID for parachain assignment key.
pub const ASSIGNMENT_KEY_TYPE_ID: KeyTypeId = KeyTypeId(*b"asgn");

/// Maximum compressed code size we support right now.
/// At the moment we have runtime upgrade on chain, which restricts scalability severely. If we want
/// to have bigger values, we should fix that first.
///
/// Used for:
/// * initial genesis for the Parachains configuration
/// * checking updates to this stored runtime configuration do not exceed this limit
/// * when detecting a code decompression bomb in the client
// NOTE: This value is used in the runtime so be careful when changing it.
pub const MAX_CODE_SIZE: u32 = 3 * 1024 * 1024;

/// Maximum head data size we support right now.
///
/// Used for:
/// * initial genesis for the Parachains configuration
/// * checking updates to this stored runtime configuration do not exceed this limit
// NOTE: This value is used in the runtime so be careful when changing it.
pub const MAX_HEAD_DATA_SIZE: u32 = 1 * 1024 * 1024;

/// Maximum PoV size we support right now.
///
/// Used for:
/// * initial genesis for the Parachains configuration
/// * checking updates to this stored runtime configuration do not exceed this limit
/// * when detecting a PoV decompression bomb in the client
// NOTE: This value is used in the runtime so be careful when changing it.
pub const MAX_POV_SIZE: u32 = 5 * 1024 * 1024;

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

/// The index of the candidate in the list of candidates fully included as-of the block.
pub type CandidateIndex = u32;

/// Get a collator signature payload on a relay-parent, block-data combo.
pub fn collator_signature_payload<H: AsRef<[u8]>>(
	relay_parent: &H,
	para_id: &Id,
	persisted_validation_data_hash: &Hash,
	pov_hash: &Hash,
	validation_code_hash: &ValidationCodeHash,
) -> [u8; 132] {
	// 32-byte hash length is protected in a test below.
	let mut payload = [0u8; 132];

	payload[0..32].copy_from_slice(relay_parent.as_ref());
	u32::from(*para_id).using_encoded(|s| payload[32..32 + s.len()].copy_from_slice(s));
	payload[36..68].copy_from_slice(persisted_validation_data_hash.as_ref());
	payload[68..100].copy_from_slice(pov_hash.as_ref());
	payload[100..132].copy_from_slice(validation_code_hash.as_ref());

	payload
}

fn check_collator_signature<H: AsRef<[u8]>>(
	relay_parent: &H,
	para_id: &Id,
	persisted_validation_data_hash: &Hash,
	pov_hash: &Hash,
	validation_code_hash: &ValidationCodeHash,
	collator: &CollatorId,
	signature: &CollatorSignature,
) -> Result<(), ()> {
	let payload = collator_signature_payload(
		relay_parent,
		para_id,
		persisted_validation_data_hash,
		pov_hash,
		validation_code_hash,
	);

	if signature.verify(&payload[..], collator) {
		Ok(())
	} else {
		Err(())
	}
}

/// A unique descriptor of the candidate receipt.
#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Hash))]
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
	/// The blake2-256 hash of the PoV.
	pub pov_hash: Hash,
	/// The root of a block's erasure encoding Merkle tree.
	pub erasure_root: Hash,
	/// Signature on blake2-256 of components of this receipt:
	/// The parachain index, the relay parent, the validation data hash, and the `pov_hash`.
	pub signature: CollatorSignature,
	/// Hash of the para header that is being generated by this candidate.
	pub para_head: Hash,
	/// The blake2-256 hash of the validation code bytes.
	pub validation_code_hash: ValidationCodeHash,
}

impl<H: AsRef<[u8]>> CandidateDescriptor<H> {
	/// Check the signature of the collator within this descriptor.
	pub fn check_collator_signature(&self) -> Result<(), ()> {
		check_collator_signature(
			&self.relay_parent,
			&self.para_id,
			&self.persisted_validation_data_hash,
			&self.pov_hash,
			&self.validation_code_hash,
			&self.collator,
			&self.signature,
		)
	}
}

/// A candidate-receipt.
#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
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
	pub fn hash(&self) -> CandidateHash
	where
		H: Encode,
	{
		CandidateHash(BlakeTwo256::hash_of(self))
	}
}

/// All data pertaining to the execution of a para candidate.
#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
pub struct FullCandidateReceipt<H = Hash, N = BlockNumber> {
	/// The inner candidate receipt.
	pub inner: CandidateReceipt<H>,
	/// The validation data derived from the relay-chain state at that
	/// point. The hash of the persisted validation data should
	/// match the `persisted_validation_data_hash` in the descriptor
	/// of the receipt.
	pub validation_data: PersistedValidationData<H, N>,
}

/// A candidate-receipt with commitments directly included.
#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Hash))]
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
	/// Transforms this into a plain `CandidateReceipt`.
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
	pub fn hash(&self) -> CandidateHash
	where
		H: Encode,
	{
		self.to_plain().hash()
	}

	/// Does this committed candidate receipt corresponds to the given [`CandidateReceipt`]?
	pub fn corresponds_to(&self, receipt: &CandidateReceipt<H>) -> bool
	where
		H: PartialEq,
	{
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
		self.descriptor()
			.para_id
			.cmp(&other.descriptor().para_id)
			.then_with(|| self.commitments.head_data.cmp(&other.commitments.head_data))
	}
}

/// The validation data provides information about how to create the inputs for validation of a candidate.
/// This information is derived from the chain state and will vary from para to para, although some
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
/// The `PersistedValidationData` should be relatively lightweight primarily because it is constructed
/// during inclusion for each candidate and therefore lies on the critical path of inclusion.
#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Default))]
pub struct PersistedValidationData<H = Hash, N = BlockNumber> {
	/// The parent head-data.
	pub parent_head: HeadData,
	/// The relay-chain block number this is in the context of.
	pub relay_parent_number: N,
	/// The relay-chain block storage root this is in the context of.
	pub relay_parent_storage_root: H,
	/// The maximum legal size of a POV block, in bytes.
	pub max_pov_size: u32,
}

impl<H: Encode, N: Encode> PersistedValidationData<H, N> {
	/// Compute the blake2-256 hash of the persisted validation data.
	pub fn hash(&self) -> Hash {
		BlakeTwo256::hash_of(self)
	}
}

/// Commitments made in a `CandidateReceipt`. Many of these are outputs of validation.
#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Hash, Default))]
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

/// A bitfield concerning availability of backed candidates.
///
/// Every bit refers to an availability core index.
#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
pub struct AvailabilityBitfield(pub BitVec<u8, bitvec::order::Lsb0>);

impl From<BitVec<u8, bitvec::order::Lsb0>> for AvailabilityBitfield {
	fn from(inner: BitVec<u8, bitvec::order::Lsb0>) -> Self {
		AvailabilityBitfield(inner)
	}
}

/// A signed compact statement, suitable to be sent to the chain.
pub type SignedStatement = Signed<CompactStatement>;
/// A signed compact statement, with signature not yet checked.
pub type UncheckedSignedStatement = UncheckedSigned<CompactStatement>;

/// A bitfield signed by a particular validator about the availability of pending candidates.
pub type SignedAvailabilityBitfield = Signed<AvailabilityBitfield>;
/// A signed bitfield with signature not yet checked.
pub type UncheckedSignedAvailabilityBitfield = UncheckedSigned<AvailabilityBitfield>;

/// A set of signed availability bitfields. Should be sorted by validator index, ascending.
pub type SignedAvailabilityBitfields = Vec<SignedAvailabilityBitfield>;
/// A set of unchecked signed availability bitfields. Should be sorted by validator index, ascending.
pub type UncheckedSignedAvailabilityBitfields = Vec<UncheckedSignedAvailabilityBitfield>;

/// A backed (or backable, depending on context) candidate.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub struct BackedCandidate<H = Hash> {
	/// The candidate referred to.
	pub candidate: CommittedCandidateReceipt<H>,
	/// The validity votes themselves, expressed as signatures.
	pub validity_votes: Vec<ValidityAttestation>,
	/// The indices of the validators within the group, expressed as a bitfield.
	pub validator_indices: BitVec<u8, bitvec::order::Lsb0>,
}

impl<H> BackedCandidate<H> {
	/// Get a reference to the descriptor of the para.
	pub fn descriptor(&self) -> &CandidateDescriptor<H> {
		&self.candidate.descriptor
	}

	/// Compute this candidate's hash.
	pub fn hash(&self) -> CandidateHash
	where
		H: Clone + Encode,
	{
		self.candidate.hash()
	}

	/// Get this candidate's receipt.
	pub fn receipt(&self) -> CandidateReceipt<H>
	where
		H: Clone,
	{
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
	for ((val_in_group_idx, _), attestation) in backed
		.validator_indices
		.iter()
		.enumerate()
		.filter(|(_, signed)| **signed)
		.zip(backed.validity_votes.iter())
	{
		let validator_id = validator_lookup(val_in_group_idx).ok_or(())?;
		let payload = attestation.signed_payload(hash, signing_context);
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
#[derive(
	Encode, Decode, Default, PartialOrd, Ord, Eq, PartialEq, Clone, Copy, TypeInfo, RuntimeDebug,
)]
#[cfg_attr(feature = "std", derive(Hash))]
pub struct CoreIndex(pub u32);

impl From<u32> for CoreIndex {
	fn from(i: u32) -> CoreIndex {
		CoreIndex(i)
	}
}

impl TypeIndex for CoreIndex {
	fn type_index(&self) -> usize {
		self.0 as usize
	}
}

/// The unique (during session) index of a validator group.
#[derive(Encode, Decode, Default, Clone, Copy, Debug, PartialEq, Eq, TypeInfo)]
#[cfg_attr(feature = "std", derive(Hash))]
pub struct GroupIndex(pub u32);

impl From<u32> for GroupIndex {
	fn from(i: u32) -> GroupIndex {
		GroupIndex(i)
	}
}

impl TypeIndex for GroupIndex {
	fn type_index(&self) -> usize {
		self.0 as usize
	}
}

/// A claim on authoring the next block for a given parathread.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct ParathreadClaim(pub Id, pub CollatorId);

/// An entry tracking a claim to ensure it does not pass the maximum number of retries.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct ParathreadEntry {
	/// The claim.
	pub claim: ParathreadClaim,
	/// Number of retries.
	pub retries: u32,
}

/// What is occupying a specific availability core.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub enum CoreOccupied {
	/// A parathread.
	Parathread(ParathreadEntry),
	/// A parachain.
	Parachain,
}

/// A helper data-type for tracking validator-group rotations.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
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
	/// `core_index` should be less than `cores`, which is capped at `u32::max()`.
	pub fn group_for_core(&self, core_index: CoreIndex, cores: usize) -> GroupIndex {
		if self.group_rotation_frequency == 0 {
			return GroupIndex(core_index.0)
		}
		if cores == 0 {
			return GroupIndex(0)
		}

		let cores = sp_std::cmp::min(cores, u32::MAX as usize);
		let blocks_since_start = self.now.saturating_sub(self.session_start_block);
		let rotations = blocks_since_start / self.group_rotation_frequency;

		// g = c + r mod cores

		let idx = (core_index.0 as usize + rotations as usize) % cores;
		GroupIndex(idx as u32)
	}

	/// Returns the index of the group assigned to the given core. This does no checking or
	/// whether the group index is in-bounds.
	///
	/// `core_index` should be less than `cores`, which is capped at `u32::max()`.
	pub fn core_for_group(&self, group_index: GroupIndex, cores: usize) -> CoreIndex {
		if self.group_rotation_frequency == 0 {
			return CoreIndex(group_index.0)
		}
		if cores == 0 {
			return CoreIndex(0)
		}

		let cores = sp_std::cmp::min(cores, u32::MAX as usize);
		let blocks_since_start = self.now.saturating_sub(self.session_start_block);
		let rotations = blocks_since_start / self.group_rotation_frequency;
		let rotations = rotations % cores as u32;

		// g = c + r mod cores
		// c = g - r mod cores
		// x = x + cores mod cores
		// c = (g + cores) - r mod cores

		let idx = (group_index.0 as usize + cores - rotations as usize) % cores;
		CoreIndex(idx as u32)
	}

	/// Create a new `GroupRotationInfo` with one further rotation applied.
	pub fn bump_rotation(&self) -> Self {
		GroupRotationInfo {
			session_start_block: self.session_start_block,
			group_rotation_frequency: self.group_rotation_frequency,
			now: self.next_rotation_at(),
		}
	}
}

impl<N: Saturating + BaseArithmetic + Copy> GroupRotationInfo<N> {
	/// Returns the block number of the next rotation after the current block. If the current block
	/// is 10 and the rotation frequency is 5, this should return 15.
	pub fn next_rotation_at(&self) -> N {
		let cycle_once = self.now + self.group_rotation_frequency;
		cycle_once -
			(cycle_once.saturating_sub(self.session_start_block) % self.group_rotation_frequency)
	}

	/// Returns the block number of the last rotation before or including the current block. If the
	/// current block is 10 and the rotation frequency is 5, this should return 10.
	pub fn last_rotation_at(&self) -> N {
		self.now -
			(self.now.saturating_sub(self.session_start_block) % self.group_rotation_frequency)
	}
}

/// Information about a core which is currently occupied.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
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
	pub availability: BitVec<u8, bitvec::order::Lsb0>,
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
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct ScheduledCore {
	/// The ID of a para scheduled.
	pub para_id: Id,
	/// The collator required to author the block, if any.
	pub collator: Option<CollatorId>,
}

/// The state of a particular availability core.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
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
#[derive(Clone, Copy, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq, Eq, Hash))]
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
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
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

/// Scraped runtime backing votes and resolved disputes.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct ScrapedOnChainVotes<H: Encode + Decode = Hash> {
	/// The session in which the block was included.
	pub session: SessionIndex,
	/// Set of backing validators for each candidate, represented by its candidate
	/// receipt.
	pub backing_validators_per_candidate:
		Vec<(CandidateReceipt<H>, Vec<(ValidatorIndex, ValidityAttestation)>)>,
	/// On-chain-recorded set of disputes.
	/// Note that the above `backing_validators` are
	/// unrelated to the backers of the disputes candidates.
	pub disputes: MultiDisputeStatementSet,
}

/// A vote of approval on a candidate.
#[derive(Clone, RuntimeDebug)]
pub struct ApprovalVote(pub CandidateHash);

impl ApprovalVote {
	/// Yields the signing payload for this approval vote.
	pub fn signing_payload(&self, session_index: SessionIndex) -> Vec<u8> {
		const MAGIC: [u8; 4] = *b"APPR";

		(MAGIC, &self.0, session_index).encode()
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
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
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
	/// The minimum period, in blocks, between which parachains can update their validation code.
	pub validation_upgrade_cooldown: BlockNumber,
	/// The delay, in blocks, before a validation upgrade is applied.
	pub validation_upgrade_delay: BlockNumber,
}

/// Abridged version of `HrmpChannel` (from the `Hrmp` parachains host runtime module) meant to be
/// used by a parachain or PDK such as cumulus.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
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

/// A possible upgrade restriction that prevents a parachain from performing an upgrade.
#[derive(Copy, Clone, Encode, Decode, PartialEq, RuntimeDebug, TypeInfo)]
pub enum UpgradeRestriction {
	/// There is an upgrade restriction and there are no details about its specifics nor how long
	/// it could last.
	#[codec(index = 0)]
	Present,
}

/// A struct that the relay-chain communicates to a parachain indicating what course of action the
/// parachain should take in the coordinated parachain validation code upgrade process.
///
/// This data type appears in the last step of the upgrade process. After the parachain observes it
/// and reacts to it the upgrade process concludes.
#[derive(Copy, Clone, Encode, Decode, PartialEq, RuntimeDebug, TypeInfo)]
pub enum UpgradeGoAhead {
	/// Abort the upgrade process. There is something wrong with the validation code previously
	/// submitted by the parachain. This variant can also be used to prevent upgrades by the governance
	/// should an emergency emerge.
	///
	/// The expected reaction on this variant is that the parachain will admit this message and
	/// remove all the data about the pending upgrade. Depending on the nature of the problem (to
	/// be examined offchain for now), it can try to send another validation code or just retry later.
	#[codec(index = 0)]
	Abort,
	/// Apply the pending code change. The parablock that is built on a relay-parent that is descendant
	/// of the relay-parent where the parachain observed this signal must use the upgraded validation
	/// code.
	#[codec(index = 1)]
	GoAhead,
}

/// Consensus engine id for polkadot v1 consensus engine.
pub const POLKADOT_ENGINE_ID: runtime_primitives::ConsensusEngineId = *b"POL1";

/// A consensus log item for polkadot validation. To be used with [`POLKADOT_ENGINE_ID`].
#[derive(Decode, Encode, Clone, PartialEq, Eq)]
pub enum ConsensusLog {
	/// A parachain or parathread upgraded its code.
	#[codec(index = 1)]
	ParaUpgradeCode(Id, ValidationCodeHash),
	/// A parachain or parathread scheduled a code upgrade.
	#[codec(index = 2)]
	ParaScheduleUpgradeCode(Id, ValidationCodeHash, BlockNumber),
	/// Governance requests to auto-approve every candidate included up to the given block
	/// number in the current chain, inclusive.
	#[codec(index = 3)]
	ForceApprove(BlockNumber),
	/// A signal to revert the block number in the same chain as the
	/// header this digest is part of and all of its descendants.
	///
	/// It is a no-op for a block to contain a revert digest targeting
	/// its own number or a higher number.
	///
	/// In practice, these are issued when on-chain logic has detected an
	/// invalid parachain block within its own chain, due to a dispute.
	#[codec(index = 4)]
	Revert(BlockNumber),
}

impl ConsensusLog {
	/// Attempt to convert a reference to a generic digest item into a consensus log.
	pub fn from_digest_item(
		digest_item: &runtime_primitives::DigestItem,
	) -> Result<Option<Self>, parity_scale_codec::Error> {
		match digest_item {
			runtime_primitives::DigestItem::Consensus(id, encoded) if id == &POLKADOT_ENGINE_ID =>
				Ok(Some(Self::decode(&mut &encoded[..])?)),
			_ => Ok(None),
		}
	}
}

impl From<ConsensusLog> for runtime_primitives::DigestItem {
	fn from(c: ConsensusLog) -> runtime_primitives::DigestItem {
		Self::Consensus(POLKADOT_ENGINE_ID, c.encode())
	}
}

/// A statement about a candidate, to be used within the dispute resolution process.
///
/// Statements are either in favor of the candidate's validity or against it.
#[derive(Encode, Decode, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub enum DisputeStatement {
	/// A valid statement, of the given kind.
	#[codec(index = 0)]
	Valid(ValidDisputeStatementKind),
	/// An invalid statement, of the given kind.
	#[codec(index = 1)]
	Invalid(InvalidDisputeStatementKind),
}

impl DisputeStatement {
	/// Get the payload data for this type of dispute statement.
	pub fn payload_data(&self, candidate_hash: CandidateHash, session: SessionIndex) -> Vec<u8> {
		match *self {
			DisputeStatement::Valid(ValidDisputeStatementKind::Explicit) =>
				ExplicitDisputeStatement { valid: true, candidate_hash, session }.signing_payload(),
			DisputeStatement::Valid(ValidDisputeStatementKind::BackingSeconded(
				inclusion_parent,
			)) => CompactStatement::Seconded(candidate_hash).signing_payload(&SigningContext {
				session_index: session,
				parent_hash: inclusion_parent,
			}),
			DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(inclusion_parent)) =>
				CompactStatement::Valid(candidate_hash).signing_payload(&SigningContext {
					session_index: session,
					parent_hash: inclusion_parent,
				}),
			DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking) =>
				ApprovalVote(candidate_hash).signing_payload(session),
			DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit) =>
				ExplicitDisputeStatement { valid: false, candidate_hash, session }.signing_payload(),
		}
	}

	/// Check the signature on a dispute statement.
	pub fn check_signature(
		&self,
		validator_public: &ValidatorId,
		candidate_hash: CandidateHash,
		session: SessionIndex,
		validator_signature: &ValidatorSignature,
	) -> Result<(), ()> {
		let payload = self.payload_data(candidate_hash, session);

		if validator_signature.verify(&payload[..], &validator_public) {
			Ok(())
		} else {
			Err(())
		}
	}

	/// Whether the statement indicates validity.
	pub fn indicates_validity(&self) -> bool {
		match *self {
			DisputeStatement::Valid(_) => true,
			DisputeStatement::Invalid(_) => false,
		}
	}

	/// Whether the statement indicates invalidity.
	pub fn indicates_invalidity(&self) -> bool {
		match *self {
			DisputeStatement::Valid(_) => false,
			DisputeStatement::Invalid(_) => true,
		}
	}

	/// Statement is backing statement.
	pub fn is_backing(&self) -> bool {
		match *self {
			Self::Valid(ValidDisputeStatementKind::BackingSeconded(_)) |
			Self::Valid(ValidDisputeStatementKind::BackingValid(_)) => true,
			Self::Valid(ValidDisputeStatementKind::Explicit) |
			Self::Valid(ValidDisputeStatementKind::ApprovalChecking) |
			Self::Invalid(_) => false,
		}
	}
}

/// Different kinds of statements of validity on  a candidate.
#[derive(Encode, Decode, Copy, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub enum ValidDisputeStatementKind {
	/// An explicit statement issued as part of a dispute.
	#[codec(index = 0)]
	Explicit,
	/// A seconded statement on a candidate from the backing phase.
	#[codec(index = 1)]
	BackingSeconded(Hash),
	/// A valid statement on a candidate from the backing phase.
	#[codec(index = 2)]
	BackingValid(Hash),
	/// An approval vote from the approval checking phase.
	#[codec(index = 3)]
	ApprovalChecking,
}

/// Different kinds of statements of invalidity on a candidate.
#[derive(Encode, Decode, Copy, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub enum InvalidDisputeStatementKind {
	/// An explicit statement issued as part of a dispute.
	#[codec(index = 0)]
	Explicit,
}

/// An explicit statement on a candidate issued as part of a dispute.
#[derive(Clone, PartialEq, RuntimeDebug)]
pub struct ExplicitDisputeStatement {
	/// Whether the candidate is valid
	pub valid: bool,
	/// The candidate hash.
	pub candidate_hash: CandidateHash,
	/// The session index of the candidate.
	pub session: SessionIndex,
}

impl ExplicitDisputeStatement {
	/// Produce the payload used for signing this type of statement.
	pub fn signing_payload(&self) -> Vec<u8> {
		const MAGIC: [u8; 4] = *b"DISP";

		(MAGIC, self.valid, self.candidate_hash, self.session).encode()
	}
}

/// A set of statements about a specific candidate.
#[derive(Encode, Decode, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub struct DisputeStatementSet {
	/// The candidate referenced by this set.
	pub candidate_hash: CandidateHash,
	/// The session index of the candidate.
	pub session: SessionIndex,
	/// Statements about the candidate.
	pub statements: Vec<(DisputeStatement, ValidatorIndex, ValidatorSignature)>,
}

impl From<CheckedDisputeStatementSet> for DisputeStatementSet {
	fn from(other: CheckedDisputeStatementSet) -> Self {
		other.0
	}
}

impl AsRef<DisputeStatementSet> for DisputeStatementSet {
	fn as_ref(&self) -> &DisputeStatementSet {
		&self
	}
}

/// A set of dispute statements.
pub type MultiDisputeStatementSet = Vec<DisputeStatementSet>;

/// A _checked_ set of dispute statements.
#[derive(Clone, PartialEq, RuntimeDebug)]
pub struct CheckedDisputeStatementSet(DisputeStatementSet);

impl AsRef<DisputeStatementSet> for CheckedDisputeStatementSet {
	fn as_ref(&self) -> &DisputeStatementSet {
		&self.0
	}
}

impl core::cmp::PartialEq<DisputeStatementSet> for CheckedDisputeStatementSet {
	fn eq(&self, other: &DisputeStatementSet) -> bool {
		self.0.eq(other)
	}
}

impl CheckedDisputeStatementSet {
	/// Convert from an unchecked, the verification of correctness of the `unchecked` statement set
	/// _must_ be done before calling this function!
	pub fn unchecked_from_unchecked(unchecked: DisputeStatementSet) -> Self {
		Self(unchecked)
	}
}

/// A set of _checked_ dispute statements.
pub type CheckedMultiDisputeStatementSet = Vec<CheckedDisputeStatementSet>;

/// The entire state of a dispute.
#[derive(Encode, Decode, Clone, RuntimeDebug, PartialEq, TypeInfo)]
pub struct DisputeState<N = BlockNumber> {
	/// A bitfield indicating all validators for the candidate.
	pub validators_for: BitVec<u8, bitvec::order::Lsb0>, // one bit per validator.
	/// A bitfield indicating all validators against the candidate.
	pub validators_against: BitVec<u8, bitvec::order::Lsb0>, // one bit per validator.
	/// The block number at which the dispute started on-chain.
	pub start: N,
	/// The block number at which the dispute concluded on-chain.
	pub concluded_at: Option<N>,
}

/// Parachains inherent-data passed into the runtime by a block author
#[derive(Encode, Decode, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub struct InherentData<HDR: HeaderT = Header> {
	/// Signed bitfields by validators about availability.
	pub bitfields: UncheckedSignedAvailabilityBitfields,
	/// Backed candidates for inclusion in the block.
	pub backed_candidates: Vec<BackedCandidate<HDR::Hash>>,
	/// Sets of dispute votes for inclusion,
	pub disputes: MultiDisputeStatementSet,
	/// The parent block header. Used for checking state proofs.
	pub parent_header: HDR,
}

/// An either implicit or explicit attestation to the validity of a parachain
/// candidate.
#[derive(Clone, Eq, PartialEq, Decode, Encode, RuntimeDebug, TypeInfo)]
pub enum ValidityAttestation {
	/// Implicit validity attestation by issuing.
	/// This corresponds to issuance of a `Candidate` statement.
	#[codec(index = 1)]
	Implicit(ValidatorSignature),
	/// An explicit attestation. This corresponds to issuance of a
	/// `Valid` statement.
	#[codec(index = 2)]
	Explicit(ValidatorSignature),
}

impl ValidityAttestation {
	/// Produce the underlying signed payload of the attestation, given the hash of the candidate,
	/// which should be known in context.
	pub fn to_compact_statement(&self, candidate_hash: CandidateHash) -> CompactStatement {
		// Explicit and implicit map directly from
		// `ValidityVote::Valid` and `ValidityVote::Issued`, and hence there is a
		// `1:1` relationshow which enables the conversion.
		match *self {
			ValidityAttestation::Implicit(_) => CompactStatement::Seconded(candidate_hash),
			ValidityAttestation::Explicit(_) => CompactStatement::Valid(candidate_hash),
		}
	}

	/// Get a reference to the signature.
	pub fn signature(&self) -> &ValidatorSignature {
		match *self {
			ValidityAttestation::Implicit(ref sig) => sig,
			ValidityAttestation::Explicit(ref sig) => sig,
		}
	}

	/// Produce the underlying signed payload of the attestation, given the hash of the candidate,
	/// which should be known in context.
	pub fn signed_payload<H: Encode>(
		&self,
		candidate_hash: CandidateHash,
		signing_context: &SigningContext<H>,
	) -> Vec<u8> {
		match *self {
			ValidityAttestation::Implicit(_) =>
				(CompactStatement::Seconded(candidate_hash), signing_context).encode(),
			ValidityAttestation::Explicit(_) =>
				(CompactStatement::Valid(candidate_hash), signing_context).encode(),
		}
	}
}

/// A type returned by runtime with current session index and a parent hash.
#[derive(Clone, Eq, PartialEq, Default, Decode, Encode, RuntimeDebug)]
pub struct SigningContext<H = Hash> {
	/// Current session index.
	pub session_index: sp_staking::SessionIndex,
	/// Hash of the parent.
	pub parent_hash: H,
}

const BACKING_STATEMENT_MAGIC: [u8; 4] = *b"BKNG";

/// Statements that can be made about parachain candidates. These are the
/// actual values that are signed.
#[derive(Clone, PartialEq, Eq, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Hash))]
pub enum CompactStatement {
	/// Proposal of a parachain candidate.
	Seconded(CandidateHash),
	/// State that a parachain candidate is valid.
	Valid(CandidateHash),
}

impl CompactStatement {
	/// Yields the payload used for validator signatures on this kind
	/// of statement.
	pub fn signing_payload(&self, context: &SigningContext) -> Vec<u8> {
		(self, context).encode()
	}
}

// Inner helper for codec on `CompactStatement`.
#[derive(Encode, Decode, TypeInfo)]
enum CompactStatementInner {
	#[codec(index = 1)]
	Seconded(CandidateHash),
	#[codec(index = 2)]
	Valid(CandidateHash),
}

impl From<CompactStatement> for CompactStatementInner {
	fn from(s: CompactStatement) -> Self {
		match s {
			CompactStatement::Seconded(h) => CompactStatementInner::Seconded(h),
			CompactStatement::Valid(h) => CompactStatementInner::Valid(h),
		}
	}
}

impl parity_scale_codec::Encode for CompactStatement {
	fn size_hint(&self) -> usize {
		// magic + discriminant + payload
		4 + 1 + 32
	}

	fn encode_to<T: parity_scale_codec::Output + ?Sized>(&self, dest: &mut T) {
		dest.write(&BACKING_STATEMENT_MAGIC);
		CompactStatementInner::from(self.clone()).encode_to(dest)
	}
}

impl parity_scale_codec::Decode for CompactStatement {
	fn decode<I: parity_scale_codec::Input>(
		input: &mut I,
	) -> Result<Self, parity_scale_codec::Error> {
		let maybe_magic = <[u8; 4]>::decode(input)?;
		if maybe_magic != BACKING_STATEMENT_MAGIC {
			return Err(parity_scale_codec::Error::from("invalid magic string"))
		}

		Ok(match CompactStatementInner::decode(input)? {
			CompactStatementInner::Seconded(h) => CompactStatement::Seconded(h),
			CompactStatementInner::Valid(h) => CompactStatement::Valid(h),
		})
	}
}

impl CompactStatement {
	/// Get the underlying candidate hash this references.
	pub fn candidate_hash(&self) -> &CandidateHash {
		match *self {
			CompactStatement::Seconded(ref h) | CompactStatement::Valid(ref h) => h,
		}
	}
}

/// `IndexedVec` struct indexed by type specific indices.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct IndexedVec<K, V>(Vec<V>, PhantomData<fn(K) -> K>);

impl<K, V> Default for IndexedVec<K, V> {
	fn default() -> Self {
		Self(vec![], PhantomData)
	}
}

impl<K, V> From<Vec<V>> for IndexedVec<K, V> {
	fn from(validators: Vec<V>) -> Self {
		Self(validators, PhantomData)
	}
}

impl<K, V> FromIterator<V> for IndexedVec<K, V> {
	fn from_iter<T: IntoIterator<Item = V>>(iter: T) -> Self {
		Self(Vec::from_iter(iter), PhantomData)
	}
}

impl<K, V> IndexedVec<K, V>
where
	V: Clone,
{
	/// Returns a reference to an element indexed using `K`.
	pub fn get(&self, index: K) -> Option<&V>
	where
		K: TypeIndex,
	{
		self.0.get(index.type_index())
	}

	/// Returns number of elements in vector.
	pub fn len(&self) -> usize {
		self.0.len()
	}

	/// Returns contained vector.
	pub fn to_vec(&self) -> Vec<V> {
		self.0.clone()
	}

	/// Returns an iterator over the underlying vector.
	pub fn iter(&self) -> Iter<'_, V> {
		self.0.iter()
	}

	/// Returns a mutable iterator over the underlying vector.
	pub fn iter_mut(&mut self) -> IterMut<'_, V> {
		self.0.iter_mut()
	}

	/// Creates a consuming iterator.
	pub fn into_iter(self) -> IntoIter<V> {
		self.0.into_iter()
	}

	/// Returns true if the underlying container is empty.
	pub fn is_empty(&self) -> bool {
		self.0.is_empty()
	}
}

/// The maximum number of validators `f` which may safely be faulty.
///
/// The total number of validators is `n = 3f + e` where `e in { 1, 2, 3 }`.
pub fn byzantine_threshold(n: usize) -> usize {
	n.saturating_sub(1) / 3
}

/// The supermajority threshold of validators which represents a subset
/// guaranteed to have at least f+1 honest validators.
pub fn supermajority_threshold(n: usize) -> usize {
	n - byzantine_threshold(n)
}

/// Information about validator sets of a session.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct SessionInfo {
	/****** New in v2 *******/
	/// All the validators actively participating in parachain consensus.
	/// Indices are into the broader validator set.
	pub active_validator_indices: Vec<ValidatorIndex>,
	/// A secure random seed for the session, gathered from BABE.
	pub random_seed: [u8; 32],
	/// The amount of sessions to keep for disputes.
	pub dispute_period: SessionIndex,

	/****** Old fields ******/
	/// Validators in canonical ordering.
	///
	/// NOTE: There might be more authorities in the current session, than `validators` participating
	/// in parachain consensus. See
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148).
	///
	/// `SessionInfo::validators` will be limited to to `max_validators` when set.
	pub validators: IndexedVec<ValidatorIndex, ValidatorId>,
	/// Validators' authority discovery keys for the session in canonical ordering.
	///
	/// NOTE: The first `validators.len()` entries will match the corresponding validators in
	/// `validators`, afterwards any remaining authorities can be found. This is any authorities not
	/// participating in parachain consensus - see
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148)
	pub discovery_keys: Vec<AuthorityDiscoveryId>,
	/// The assignment keys for validators.
	///
	/// NOTE: There might be more authorities in the current session, than validators participating
	/// in parachain consensus. See
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148).
	///
	/// Therefore:
	/// ```ignore
	///		assignment_keys.len() == validators.len() && validators.len() <= discovery_keys.len()
	///	```
	pub assignment_keys: Vec<AssignmentId>,
	/// Validators in shuffled ordering - these are the validator groups as produced
	/// by the `Scheduler` module for the session and are typically referred to by
	/// `GroupIndex`.
	pub validator_groups: IndexedVec<GroupIndex, Vec<ValidatorIndex>>,
	/// The number of availability cores used by the protocol during this session.
	pub n_cores: u32,
	/// The zeroth delay tranche width.
	pub zeroth_delay_tranche_width: u32,
	/// The number of samples we do of `relay_vrf_modulo`.
	pub relay_vrf_modulo_samples: u32,
	/// The number of delay tranches in total.
	pub n_delay_tranches: u32,
	/// How many slots (BABE / SASSAFRAS) must pass before an assignment is considered a
	/// no-show.
	pub no_show_slots: u32,
	/// The number of validators needed to approve a block.
	pub needed_approvals: u32,
}

/// A statement from the specified validator whether the given validation code passes PVF
/// pre-checking or not anchored to the given session index.
#[derive(Encode, Decode, Clone, PartialEq, RuntimeDebug, TypeInfo)]
pub struct PvfCheckStatement {
	/// `true` if the subject passed pre-checking and `false` otherwise.
	pub accept: bool,
	/// The validation code hash that was checked.
	pub subject: ValidationCodeHash,
	/// The index of a session during which this statement is considered valid.
	pub session_index: SessionIndex,
	/// The index of the validator from which this statement originates.
	pub validator_index: ValidatorIndex,
}

impl PvfCheckStatement {
	/// Produce the payload used for signing this type of statement.
	///
	/// It is expected that it will be signed by the validator at `validator_index` in the
	/// `session_index`.
	pub fn signing_payload(&self) -> Vec<u8> {
		const MAGIC: [u8; 4] = *b"VCPC"; // for "validation code pre-checking"
		(MAGIC, self.accept, self.subject, self.session_index, self.validator_index).encode()
	}
}

/// Old, v1-style info about session info. Only needed for limited
/// backwards-compatibility.
#[derive(Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct OldV1SessionInfo {
	/// Validators in canonical ordering.
	///
	/// NOTE: There might be more authorities in the current session, than `validators` participating
	/// in parachain consensus. See
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148).
	///
	/// `SessionInfo::validators` will be limited to to `max_validators` when set.
	pub validators: IndexedVec<ValidatorIndex, ValidatorId>,
	/// Validators' authority discovery keys for the session in canonical ordering.
	///
	/// NOTE: The first `validators.len()` entries will match the corresponding validators in
	/// `validators`, afterwards any remaining authorities can be found. This is any authorities not
	/// participating in parachain consensus - see
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148)
	pub discovery_keys: Vec<AuthorityDiscoveryId>,
	/// The assignment keys for validators.
	///
	/// NOTE: There might be more authorities in the current session, than validators participating
	/// in parachain consensus. See
	/// [`max_validators`](https://github.com/paritytech/polkadot/blob/a52dca2be7840b23c19c153cf7e110b1e3e475f8/runtime/parachains/src/configuration.rs#L148).
	///
	/// Therefore:
	/// ```ignore
	///		assignment_keys.len() == validators.len() && validators.len() <= discovery_keys.len()
	///	```
	pub assignment_keys: Vec<AssignmentId>,
	/// Validators in shuffled ordering - these are the validator groups as produced
	/// by the `Scheduler` module for the session and are typically referred to by
	/// `GroupIndex`.
	pub validator_groups: IndexedVec<GroupIndex, Vec<ValidatorIndex>>,
	/// The number of availability cores used by the protocol during this session.
	pub n_cores: u32,
	/// The zeroth delay tranche width.
	pub zeroth_delay_tranche_width: u32,
	/// The number of samples we do of `relay_vrf_modulo`.
	pub relay_vrf_modulo_samples: u32,
	/// The number of delay tranches in total.
	pub n_delay_tranches: u32,
	/// How many slots (BABE / SASSAFRAS) must pass before an assignment is considered a
	/// no-show.
	pub no_show_slots: u32,
	/// The number of validators needed to approve a block.
	pub needed_approvals: u32,
}

impl From<OldV1SessionInfo> for SessionInfo {
	fn from(old: OldV1SessionInfo) -> SessionInfo {
		SessionInfo {
			// new fields
			active_validator_indices: Vec::new(),
			random_seed: [0u8; 32],
			dispute_period: 6,
			// old fields
			validators: old.validators,
			discovery_keys: old.discovery_keys,
			assignment_keys: old.assignment_keys,
			validator_groups: old.validator_groups,
			n_cores: old.n_cores,
			zeroth_delay_tranche_width: old.zeroth_delay_tranche_width,
			relay_vrf_modulo_samples: old.relay_vrf_modulo_samples,
			n_delay_tranches: old.n_delay_tranches,
			no_show_slots: old.no_show_slots,
			needed_approvals: old.needed_approvals,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn group_rotation_info_calculations() {
		let info =
			GroupRotationInfo { session_start_block: 10u32, now: 15, group_rotation_frequency: 5 };

		assert_eq!(info.next_rotation_at(), 20);
		assert_eq!(info.last_rotation_at(), 15);
	}

	#[test]
	fn group_for_core_is_core_for_group() {
		for cores in 1..=256 {
			for rotations in 0..(cores * 2) {
				let info = GroupRotationInfo {
					session_start_block: 0u32,
					now: rotations,
					group_rotation_frequency: 1,
				};

				for core in 0..cores {
					let group = info.group_for_core(CoreIndex(core), cores as usize);
					assert_eq!(info.core_for_group(group, cores as usize).0, core);
				}
			}
		}
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
			&Hash::repeat_byte(4).into(),
		);
	}

	#[test]
	fn test_byzantine_threshold() {
		assert_eq!(byzantine_threshold(0), 0);
		assert_eq!(byzantine_threshold(1), 0);
		assert_eq!(byzantine_threshold(2), 0);
		assert_eq!(byzantine_threshold(3), 0);
		assert_eq!(byzantine_threshold(4), 1);
		assert_eq!(byzantine_threshold(5), 1);
		assert_eq!(byzantine_threshold(6), 1);
		assert_eq!(byzantine_threshold(7), 2);
	}

	#[test]
	fn test_supermajority_threshold() {
		assert_eq!(supermajority_threshold(0), 0);
		assert_eq!(supermajority_threshold(1), 1);
		assert_eq!(supermajority_threshold(2), 2);
		assert_eq!(supermajority_threshold(3), 3);
		assert_eq!(supermajority_threshold(4), 3);
		assert_eq!(supermajority_threshold(5), 4);
		assert_eq!(supermajority_threshold(6), 5);
		assert_eq!(supermajority_threshold(7), 5);
	}

	#[test]
	fn balance_bigger_than_usize() {
		let zero_b: Balance = 0;
		let zero_u: usize = 0;

		assert!(zero_b.leading_zeros() >= zero_u.leading_zeros());
	}
}
