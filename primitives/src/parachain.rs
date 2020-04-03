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

//! Polkadot parachain types.

use sp_std::prelude::*;
use sp_std::cmp::Ordering;
use parity_scale_codec::{Encode, Decode};
use bitvec::vec::BitVec;
use super::{Hash, Balance};

#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};

#[cfg(feature = "std")]
use primitives::bytes;
use primitives::RuntimeDebug;
use runtime_primitives::traits::Block as BlockT;
use inherents::InherentIdentifier;
use application_crypto::KeyTypeId;

pub use polkadot_parachain::{
	Id, ParachainDispatchOrigin, LOWEST_USER_ID, UpwardMessage,
};

/// The key type ID for a collator key.
pub const COLLATOR_KEY_TYPE_ID: KeyTypeId = KeyTypeId(*b"coll");

/// An identifier for inherent data that provides new minimally-attested
/// parachain heads.
pub const NEW_HEADS_IDENTIFIER: InherentIdentifier = *b"newheads";

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

/// Index of the validator is used as a lightweight replacement of the `ValidatorId` when appropriate.
pub type ValidatorIndex = u32;

application_crypto::with_pair! {
	/// A Parachain validator keypair.
	pub type ValidatorPair = validator_app::Pair;
}

/// Signature with which parachain validators sign blocks.
///
/// For now we assert that parachain validator set is exactly equivalent to the authority set, and
/// so we define it to be the same type as `SessionKey`. In the future it may have different crypto.
pub type ValidatorSignature = validator_app::Signature;

/// Retriability for a given active para.
#[derive(Clone, Eq, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum Retriable {
	/// Ineligible for retry. This means it's either a parachain that is always scheduled anyway or
	/// has been removed/swapped.
	Never,
	/// Eligible for retry; the associated value is the number of retries that the para already had.
	WithRetries(u32),
}

/// Type determining the active set of parachains in current block.
pub trait ActiveParas {
	/// Return the active set of parachains in current block. This attempts to keep any IDs in the
	/// same place between sequential blocks. It is therefore unordered. The second item in the
	/// tuple is the required collator ID, if any. If `Some`, then it is invalid to include any
	/// other collator's block.
	///
	/// NOTE: The initial implementation simply concatenates the (ordered) set of (permanent)
	/// parachain IDs with the (unordered) set of parathread IDs selected for this block.
	fn active_paras() -> Vec<(Id, Option<(CollatorId, Retriable)>)>;
}

/// Description of how often/when this parachain is scheduled for progression.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub enum Scheduling {
	/// Scheduled every block.
	Always,
	/// Scheduled dynamically (i.e. a parathread).
	Dynamic,
}

/// Information regarding a deployed parachain/thread.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
pub struct Info {
	/// Scheduling info.
	pub scheduling: Scheduling,
}

/// An `Info` value for a standard leased parachain.
pub const PARACHAIN_INFO: Info = Info {
	scheduling: Scheduling::Always,
};

/// Auxilliary for when there's an attempt to swap two parachains/parathreads.
pub trait SwapAux {
	/// Result describing whether it is possible to swap two parachains. Doesn't mutate state.
	fn ensure_can_swap(one: Id, other: Id) -> Result<(), &'static str>;

	/// Updates any needed state/references to enact a logical swap of two parachains. Identity,
	/// code and `head_data` remain equivalent for all parachains/threads, however other properties
	/// such as leases, deposits held and thread/chain nature are swapped.
	///
	/// May only be called on a state that `ensure_can_swap` has previously returned `Ok` for: if this is
	/// not the case, the result is undefined. May only return an error if `ensure_can_swap` also returns
	/// an error.
	fn on_swap(one: Id, other: Id) -> Result<(), &'static str>;
}

/// Identifier for a chain, either one of a number of parachains or the relay chain.
#[derive(Copy, Clone, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum Chain {
	/// The relay chain.
	Relay,
	/// A parachain of the given index.
	Parachain(Id),
}

/// The duty roster specifying what jobs each validator must do.
#[derive(Clone, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Default, Debug))]
pub struct DutyRoster {
	/// Lookup from validator index to chain on which that validator has a duty to validate.
	pub validator_duty: Vec<Chain>,
}

/// Extra data that is needed along with the other fields in a `CandidateReceipt`
/// to fully validate the candidate.
///
/// These are global parameters that apply to all parachain candidates in a block.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct GlobalValidationSchedule {
	/// The maximum code size permitted, in bytes.
	pub max_code_size: u32,
	/// The maximum head-data size permitted, in bytes.
	pub max_head_data_size: u32,
}

/// Extra data that is needed along with the other fields in a `CandidateReceipt`
/// to fully validate the candidate. These fields are parachain-specific.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct LocalValidationData {
	/// The parent head-data.
	pub parent_head: HeadData,
	/// The balance of the parachain at the moment of validation.
	pub balance: Balance,
}

/// Commitments made in a `CandidateReceipt`. Many of these are outputs of validation.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct CandidateCommitments {
	/// Fees paid from the chain to the relay chain validators.
	pub fees: Balance,
	/// Messages destined to be interpreted by the Relay chain itself.
	pub upward_messages: Vec<UpwardMessage>,
	/// The root of a block's erasure encoding Merkle tree.
	pub erasure_root: Hash,
}

/// Get a collator signature payload on a relay-parent, block-data combo.
pub fn collator_signature_payload(
	relay_parent: &Hash,
	parachain_index: &Id,
	pov_block_hash: &Hash,
) -> [u8; 68] {
	// 32-byte hash length is protected in a test below.
	let mut payload = [0u8; 68];

	payload[0..32].copy_from_slice(relay_parent.as_ref());
	u32::from(*parachain_index).using_encoded(|s| payload[32..32 + s.len()].copy_from_slice(s));
	payload[36..68].copy_from_slice(pov_block_hash.as_ref());

	payload
}

fn check_collator_signature(
	relay_parent: &Hash,
	parachain_index: &Id,
	pov_block_hash: &Hash,
	collator: &CollatorId,
	signature: &CollatorSignature,
) -> Result<(),()> {
	use runtime_primitives::traits::AppVerify;

	let payload = collator_signature_payload(relay_parent, parachain_index, pov_block_hash);
	if signature.verify(&payload[..], collator) {
		Ok(())
	} else {
		Err(())
	}
}

/// All data pertaining to the execution of a parachain candidate.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct CandidateReceipt {
	/// The ID of the parachain this is a candidate for.
	pub parachain_index: Id,
	/// The hash of the relay-chain block this should be executed in
	/// the context of.
	pub relay_parent: Hash,
	/// The head-data
	pub head_data: HeadData,
	/// The collator's relay-chain account ID
	pub collator: CollatorId,
	/// Signature on blake2-256 of the block data by collator.
	pub signature: CollatorSignature,
	/// The hash of the PoV-block.
	pub pov_block_hash: Hash,
	/// The global validation schedule.
	pub global_validation: GlobalValidationSchedule,
	/// The local validation data.
	pub local_validation: LocalValidationData,
	/// Commitments made as a result of validation.
	pub commitments: CandidateCommitments,
}

impl CandidateReceipt {
	/// Check integrity vs. provided block data.
	pub fn check_signature(&self) -> Result<(), ()> {
		check_collator_signature(
			&self.relay_parent,
			&self.parachain_index,
			&self.pov_block_hash,
			&self.collator,
			&self.signature,
		)
	}

	/// Abridge this `CandidateReceipt`, splitting it into an `AbridgedCandidateReceipt`
	/// and its omitted component.
	pub fn abridge(self) -> (AbridgedCandidateReceipt, OmittedValidationData) {
		let CandidateReceipt {
			parachain_index,
			relay_parent,
			head_data,
			collator,
			signature,
			pov_block_hash,
			global_validation,
			local_validation,
			commitments,
		} = self;

		let abridged = AbridgedCandidateReceipt {
			parachain_index,
			relay_parent,
			head_data,
			collator,
			signature,
			pov_block_hash,
			commitments,
		};

		let omitted = OmittedValidationData {
			global_validation,
			local_validation,
		};

		(abridged, omitted)
	}
}

impl PartialOrd for CandidateReceipt {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for CandidateReceipt {
	fn cmp(&self, other: &Self) -> Ordering {
		// TODO: compare signatures or something more sane
		// https://github.com/paritytech/polkadot/issues/222
		self.parachain_index.cmp(&other.parachain_index)
			.then_with(|| self.head_data.cmp(&other.head_data))
	}
}

/// All the data which is omitted in an `AbridgedCandidateReceipt`, but that
/// is necessary for validation of the parachain candidate.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct OmittedValidationData {
	/// The global validation schedule.
	pub global_validation: GlobalValidationSchedule,
	/// The local validation data.
	pub local_validation: LocalValidationData,
}

/// An abridged candidate-receipt.
///
/// Much info in a candidate-receipt is duplicated from the relay-chain state.
/// When submitting to the relay-chain, this data should be omitted as it can
/// be re-generated from relay-chain state.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct AbridgedCandidateReceipt {
	/// The ID of the parachain this is a candidate for.
	pub parachain_index: Id,
	/// The hash of the relay-chain block this should be executed in
	/// the context of.
	// NOTE: the fact that the hash includes this value means that code depends
	// on this for deduplication. Removing this field is likely to break things.
	pub relay_parent: Hash,
	/// The head-data
	pub head_data: HeadData,
	/// The collator's relay-chain account ID
	pub collator: CollatorId,
	/// Signature on blake2-256 of the block data by collator.
	pub signature: CollatorSignature,
	/// The hash of the pov-block.
	pub pov_block_hash: Hash,
	/// Commitments made as a result of validation.
	pub commitments: CandidateCommitments,
}

impl AbridgedCandidateReceipt {
	/// Compute the hash of the abridged candidate receipt.
	///
	/// This is often used as the canonical hash of the receipt, rather than
	/// the hash of the full receipt. The reason being that all data in the full
	/// receipt is committed to in the abridged receipt; this receipt references
	/// the relay-chain block in which context it should be executed, which implies
	/// any blockchain state that must be referenced.
	pub fn hash(&self) -> Hash {
		use runtime_primitives::traits::{BlakeTwo256, Hash};
		BlakeTwo256::hash_of(self)
	}

	/// Combine the abridged candidate receipt with the omitted data,
	/// forming a full `CandidateReceipt`.
	pub fn complete(self, omitted: OmittedValidationData) -> CandidateReceipt {
		let AbridgedCandidateReceipt {
			parachain_index,
			relay_parent,
			head_data,
			collator,
			signature,
			pov_block_hash,
			commitments,
		} = self;

		let OmittedValidationData {
			global_validation,
			local_validation,
		} = omitted;

		CandidateReceipt {
			parachain_index,
			relay_parent,
			head_data,
			collator,
			signature,
			pov_block_hash,
			local_validation,
			global_validation,
			commitments,
		}
	}

	/// Clone the relevant portions of the `CandidateReceipt` to form a `CollationInfo`.
	pub fn to_collation_info(&self) -> CollationInfo {
		let AbridgedCandidateReceipt {
			parachain_index,
			relay_parent,
			head_data,
			collator,
			signature,
			pov_block_hash,
			commitments: _commitments,
		} = self;

		CollationInfo {
			parachain_index: *parachain_index,
			relay_parent: *relay_parent,
			head_data: head_data.clone(),
			collator: collator.clone(),
			signature: signature.clone(),
			pov_block_hash: *pov_block_hash,
		}
	}
}


impl PartialOrd for AbridgedCandidateReceipt {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for AbridgedCandidateReceipt {
	fn cmp(&self, other: &Self) -> Ordering {
		// TODO: compare signatures or something more sane
		// https://github.com/paritytech/polkadot/issues/222
		self.parachain_index.cmp(&other.parachain_index)
			.then_with(|| self.head_data.cmp(&other.head_data))
	}
}

/// A collation sent by a collator.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Default))]
pub struct CollationInfo {
	/// The ID of the parachain this is a candidate for.
	pub parachain_index: Id,
	/// The relay-chain block hash this block should execute in the
	/// context of.
	pub relay_parent: Hash,
	/// The collator's relay-chain account ID
	pub collator: CollatorId,
	/// Signature on blake2-256 of the block data by collator.
	pub signature: CollatorSignature,
	/// The head-data
	pub head_data: HeadData,
	/// blake2-256 Hash of the pov-block
	pub pov_block_hash: Hash,
}

impl CollationInfo {
	/// Check integrity vs. a pov-block.
	pub fn check_signature(&self) -> Result<(), ()> {
		check_collator_signature(
			&self.relay_parent,
			&self.parachain_index,
			&self.pov_block_hash,
			&self.collator,
			&self.signature,
		)
	}

	/// Turn this into an `AbridgedCandidateReceipt` by supplying a set of commitments.
	pub fn into_receipt(self, commitments: CandidateCommitments) -> AbridgedCandidateReceipt {
		let CollationInfo {
			parachain_index,
			relay_parent,
			collator,
			signature,
			head_data,
			pov_block_hash,
		} = self;

		AbridgedCandidateReceipt {
			parachain_index,
			relay_parent,
			collator,
			signature,
			head_data,
			pov_block_hash,
			commitments,
		}
	}
}

/// A full collation.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug, Encode, Decode))]
pub struct Collation {
	/// Candidate receipt itself.
	pub info: CollationInfo,
	/// A proof-of-validation for the receipt.
	pub pov: PoVBlock,
}

/// A Proof-of-Validation block.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug, Encode, Decode))]
pub struct PoVBlock {
	/// Block data.
	pub block_data: BlockData,
}

impl PoVBlock {
	/// Compute hash of block data.
	#[cfg(feature = "std")]
	pub fn hash(&self) -> Hash {
		use runtime_primitives::traits::{BlakeTwo256, Hash};
		BlakeTwo256::hash_of(&self)
	}
}

/// The data that is kept available about a particular parachain block.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug, Encode, Decode))]
pub struct AvailableData {
	/// The PoV block.
	pub pov_block: PoVBlock,
	/// Data that is omitted from an abridged candidate receipt
	/// that is necessary for validation.
	pub omitted_validation: OmittedValidationData,
	// In the future, outgoing messages as well.
}

/// Parachain block data.
///
/// Contains everything required to validate para-block, may contain block and witness data.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct BlockData(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

/// A chunk of erasure-encoded block data.
#[derive(PartialEq, Eq, Clone, Encode, Decode, Default)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct ErasureChunk {
	/// The erasure-encoded chunk of data belonging to the candidate block.
	pub chunk: Vec<u8>,
	/// The index of this erasure-encoded chunk of data.
	pub index: u32,
	/// Proof for this chunk's branch in the Merkle tree.
	pub proof: Vec<Vec<u8>>,
}

impl BlockData {
	/// Compute hash of block data.
	#[cfg(feature = "std")]
	pub fn hash(&self) -> Hash {
		use runtime_primitives::traits::{BlakeTwo256, Hash};
		BlakeTwo256::hash(&self.0[..])
	}
}
/// Parachain header raw bytes wrapper type.
#[derive(PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Header(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

/// Parachain head data included in the chain.
#[derive(PartialEq, Eq, Clone, PartialOrd, Ord, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug, Default))]
pub struct HeadData(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

/// Parachain validation code.
#[derive(PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct ValidationCode(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

/// Activity bit field.
#[derive(PartialEq, Eq, Clone, Default, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Activity(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

/// Statements that can be made about parachain candidates. These are the
/// actual values that are signed.
#[derive(Clone, PartialEq, Eq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum Statement {
	/// Proposal of a parachain candidate.
	#[codec(index = "1")]
	Candidate(Hash),
	/// State that a parachain candidate is valid.
	#[codec(index = "2")]
	Valid(Hash),
	/// State that a parachain candidate is invalid.
	#[codec(index = "3")]
	Invalid(Hash),
}

/// An either implicit or explicit attestation to the validity of a parachain
/// candidate.
#[derive(Clone, Eq, PartialEq, Decode, Encode, RuntimeDebug)]
pub enum ValidityAttestation {
	/// Implicit validity attestation by issuing.
	/// This corresponds to issuance of a `Candidate` statement.
	#[codec(index = "1")]
	Implicit(ValidatorSignature),
	/// An explicit attestation. This corresponds to issuance of a
	/// `Valid` statement.
	#[codec(index = "2")]
	Explicit(ValidatorSignature),
}

/// A type returned by runtime with current session index and a parent hash.
#[derive(Clone, Eq, PartialEq, Default, Decode, Encode, RuntimeDebug)]
pub struct SigningContext {
	/// Current session index.
	pub session_index: sp_staking::SessionIndex,
	/// Hash of the parent.
	pub parent_hash: Hash,
}

/// An attested candidate. This is submitted to the relay chain by a block author.
#[derive(Clone, PartialEq, Decode, Encode, RuntimeDebug)]
pub struct AttestedCandidate {
	/// The candidate data. This is abridged, because the omitted data
	/// is already present within the relay chain state.
	pub candidate: AbridgedCandidateReceipt,
	/// Validity attestations.
	pub validity_votes: Vec<ValidityAttestation>,
	/// Indices of the corresponding validity votes.
	pub validator_indices: BitVec<bitvec::order::Lsb0, u8>,
}

impl AttestedCandidate {
	/// Get the candidate.
	pub fn candidate(&self) -> &AbridgedCandidateReceipt {
		&self.candidate
	}

	/// Get the group ID of the candidate.
	pub fn parachain_index(&self) -> Id {
		self.candidate.parachain_index
	}
}

/// A fee schedule for messages. This is a linear function in the number of bytes of a message.
#[derive(PartialEq, Eq, PartialOrd, Hash, Default, Clone, Copy, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct FeeSchedule {
	/// The base fee charged for all messages.
	pub base: Balance,
	/// The per-byte fee charged on top of that.
	pub per_byte: Balance,
}

impl FeeSchedule {
	/// Compute the fee for a message of given size.
	pub fn compute_fee(&self, n_bytes: usize) -> Balance {
		use sp_std::mem;
		debug_assert!(mem::size_of::<Balance>() >= mem::size_of::<usize>());

		let n_bytes = n_bytes as Balance;
		self.base.saturating_add(n_bytes.saturating_mul(self.per_byte))
	}
}

sp_api::decl_runtime_apis! {
	/// The API for querying the state of parachains on-chain.
	#[api_version(3)]
	pub trait ParachainHost {
		/// Get the current validators.
		fn validators() -> Vec<ValidatorId>;
		/// Get the current duty roster.
		fn duty_roster() -> DutyRoster;
		/// Get the currently active parachains.
		fn active_parachains() -> Vec<(Id, Option<(CollatorId, Retriable)>)>;
		/// Get the global validation schedule that all parachains should
		/// be validated under.
		fn global_validation_schedule() -> GlobalValidationSchedule;
		/// Get the local validation data for a particular parachain.
		fn local_validation_data(id: Id) -> Option<LocalValidationData>;
		/// Get the given parachain's head code blob.
		fn parachain_code(id: Id) -> Option<Vec<u8>>;
		/// Extract the abridged head that was set in the extrinsics.
		fn get_heads(extrinsics: Vec<<Block as BlockT>::Extrinsic>)
			-> Option<Vec<AbridgedCandidateReceipt>>;
		/// Get a `SigningContext` with current `SessionIndex` and parent hash.
		fn signing_context() -> SigningContext;
	}
}

/// Runtime ID module.
pub mod id {
	use sp_version::ApiId;

	/// Parachain host runtime API id.
	pub const PARACHAIN_HOST: ApiId = *b"parahost";
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn balance_bigger_than_usize() {
		let zero_b: Balance = 0;
		let zero_u: usize = 0;

		assert!(zero_b.leading_zeros() >= zero_u.leading_zeros());
	}

	#[test]
	fn collator_signature_payload_is_valid() {
		// if this fails, collator signature verification code has to be updated.
		let h = Hash::default();
		assert_eq!(h.as_ref().len(), 32);

		let _payload = collator_signature_payload(
			&[1; 32].into(),
			&5u32.into(),
			&[2; 32].into(),
		);
	}
}
