// Copyright 2017 Parity Technologies (UK) Ltd.
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

use rstd::prelude::*;
use rstd::cmp::Ordering;
use parity_scale_codec::{Encode, Decode};
use bitvec::vec::BitVec;
use super::{Hash, Balance, BlockNumber};

#[cfg(feature = "std")]
use serde::{Serialize, Deserialize};

#[cfg(feature = "std")]
use primitives::bytes;
use application_crypto::KeyTypeId;

pub use polkadot_parachain::{
	Id, AccountIdConversion, ParachainDispatchOrigin, UpwardMessage,
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
/// For now we assert that parachain validator set is exactly equivalent to the (Aura) authority set, and
/// so we define it to be the same type as `SessionKey`. In the future it may have different crypto.
pub type ValidatorId = validator_app::Public;

/// Index of the validator is used as a lightweight replacement of the `ValidatorId` when appropriate.
pub type ValidatorIndex = u32;

/// A Parachain validator keypair.
#[cfg(feature = "std")]
pub type ValidatorPair = validator_app::Pair;

 /// Signature with which parachain validators sign blocks.
///
/// For now we assert that parachain validator set is exactly equivalent to the (Aura) authority set, and
/// so we define it to be the same type as `SessionKey`. In the future it may have different crypto.
pub type ValidatorSignature = validator_app::Signature;

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

/// An outgoing message
#[derive(Clone, PartialEq, Eq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
#[cfg_attr(feature = "std", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "std", serde(deny_unknown_fields))]
pub struct OutgoingMessage {
	/// The target parachain.
	pub target: Id,
	/// The message data.
	pub data: Vec<u8>,
}

impl PartialOrd for OutgoingMessage {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.target.cmp(&other.target))
	}
}

impl Ord for OutgoingMessage {
	fn cmp(&self, other: &Self) -> Ordering {
		self.target.cmp(&other.target)
	}
}

/// Extrinsic data for a parachain candidate.
///
/// This is data produced by evaluating the candidate. It contains
/// full records of all outgoing messages to other parachains.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
#[cfg_attr(feature = "std", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "std", serde(deny_unknown_fields))]
pub struct Extrinsic {
	/// The outgoing messages from the execution of the parachain.
	///
	/// This must be sorted in ascending order by parachain ID.
	pub outgoing_messages: Vec<OutgoingMessage>
}

/// Candidate receipt type.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct CandidateReceipt {
	/// The ID of the parachain this is a candidate for.
	pub parachain_index: Id,
	/// The collator's relay-chain account ID
	pub collator: CollatorId,
	/// Signature on blake2-256 of the block data by collator.
	pub signature: CollatorSignature,
	/// The head-data
	pub head_data: HeadData,
	/// Egress queue roots. Must be sorted lexicographically (ascending)
	/// by parachain ID.
	pub egress_queue_roots: Vec<(Id, Hash)>,
	/// Fees paid from the chain to the relay chain validators
	pub fees: Balance,
	/// blake2-256 Hash of block data.
	pub block_data_hash: Hash,
	/// Messages destined to be interpreted by the Relay chain itself.
	pub upward_messages: Vec<UpwardMessage>,
}

impl CandidateReceipt {
	/// Get the blake2_256 hash
	pub fn hash(&self) -> Hash {
		use runtime_primitives::traits::{BlakeTwo256, Hash};
		BlakeTwo256::hash_of(self)
	}

	/// Check integrity vs. provided block data.
	pub fn check_signature(&self) -> Result<(), ()> {
		use runtime_primitives::traits::AppVerify;

		if self.signature.verify(self.block_data_hash.as_ref(), &self.collator) {
			Ok(())
		} else {
			Err(())
		}
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

/// A full collation.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug, Encode, Decode))]
pub struct Collation {
	/// Candidate receipt itself.
	pub receipt: CandidateReceipt,
	/// A proof-of-validation for the receipt.
	pub pov: PoVBlock,
}

/// A Proof-of-Validation block.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug, Encode, Decode))]
pub struct PoVBlock {
	/// Block data.
	pub block_data: BlockData,
	/// Ingress for the parachain.
	pub ingress: ConsolidatedIngress,
}

/// Parachain ingress queue message.
#[derive(PartialEq, Eq, Clone, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Encode, Debug))]
pub struct Message(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

/// All ingress roots at one block.
///
/// This is an ordered vector of other parachain's egress queue roots from a specific block.
/// empty roots are omitted. Each parachain may appear once at most.
#[derive(Default, PartialEq, Eq, Clone, Encode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug, Decode))]
pub struct BlockIngressRoots(pub Vec<(Id, Hash)>);

/// All ingress roots, grouped by block number (ascending). To properly
/// interpret this struct, the user must have knowledge of which fork of the relay
/// chain all block numbers correspond to.
#[derive(Default, PartialEq, Eq, Clone, Encode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug, Decode))]
pub struct StructuredUnroutedIngress(pub Vec<(BlockNumber, BlockIngressRoots)>);

#[cfg(feature = "std")]
impl StructuredUnroutedIngress {
	/// Get the length of all the ingress roots across all blocks.
	pub fn len(&self) -> usize {
		self.0.iter().fold(0, |a, (_, roots)| a + roots.0.len())
	}

	/// Returns an iterator over all ingress roots. The block number indicates
	/// the height at which that root was posted to the relay chain. The parachain ID is the
	/// message sender.
	pub fn iter(&self) -> impl Iterator<Item=(BlockNumber, &Id, &Hash)> {
		self.0.iter().flat_map(|&(n, ref roots)|
			roots.0.iter().map(move |&(ref from, ref root)| (n, from, root))
		)
	}
}

/// Consolidated ingress queue data.
///
/// This is just an ordered vector of other parachains' egress queues,
/// obtained according to the routing rules. The same parachain may appear
/// more than once.
#[derive(Default, PartialEq, Eq, Clone, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Encode, Debug))]
pub struct ConsolidatedIngress(pub Vec<(Id, Vec<Message>)>);

/// Parachain block data.
///
/// contains everything required to validate para-block, may contain block and witness data
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct BlockData(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

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
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct HeadData(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

/// Parachain validation code.
#[derive(PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct ValidationCode(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

/// Activity bit field
#[derive(PartialEq, Eq, Clone, Default, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Activity(#[cfg_attr(feature = "std", serde(with="bytes"))] pub Vec<u8>);

/// Statements which can be made about parachain candidates.
#[derive(Clone, PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum Statement {
	/// Proposal of a parachain candidate.
	#[codec(index = "1")]
	Candidate(CandidateReceipt),
	/// State that a parachain candidate is valid.
	#[codec(index = "2")]
	Valid(Hash),
	/// State a candidate is invalid.
	#[codec(index = "3")]
	Invalid(Hash),
}

/// An either implicit or explicit attestation to the validity of a parachain
/// candidate.
#[derive(Clone, PartialEq, Decode, Encode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum ValidityAttestation {
	/// implicit validity attestation by issuing.
	/// This corresponds to issuance of a `Candidate` statement.
	#[codec(index = "1")]
	Implicit(ValidatorSignature),
	/// An explicit attestation. This corresponds to issuance of a
	/// `Valid` statement.
	#[codec(index = "2")]
	Explicit(ValidatorSignature),
}

/// An attested candidate.
#[derive(Clone, PartialEq, Decode, Encode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct AttestedCandidate {
	/// The candidate data.
	pub candidate: CandidateReceipt,
	/// Validity attestations.
	pub validity_votes: Vec<ValidityAttestation>,
	/// Indices of the corresponding validity votes.
	pub validator_indices: BitVec,
}

impl AttestedCandidate {
	/// Get the candidate.
	pub fn candidate(&self) -> &CandidateReceipt {
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
		use rstd::mem;
		debug_assert!(mem::size_of::<Balance>() >= mem::size_of::<usize>());

		let n_bytes = n_bytes as Balance;
		self.base.saturating_add(n_bytes.saturating_mul(self.per_byte))
	}
}

/// Current Status of a parachain.
#[derive(PartialEq, Eq, Clone, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Status {
	/// The head of the parachain.
	pub head_data: HeadData,
	/// The current balance of the parachain.
	pub balance: Balance,
	/// The fee schedule for messages coming from this parachain.
	pub fee_schedule: FeeSchedule,
}

substrate_client::decl_runtime_apis! {
	/// The API for querying the state of parachains on-chain.
	pub trait ParachainHost {
		/// Get the current validators.
		fn validators() -> Vec<ValidatorId>;
		/// Get the current duty roster.
		fn duty_roster() -> DutyRoster;
		/// Get the currently active parachains.
		fn active_parachains() -> Vec<Id>;
		/// Get the given parachain's status.
		fn parachain_status(id: Id) -> Option<Status>;
		/// Get the given parachain's head code blob.
		fn parachain_code(id: Id) -> Option<Vec<u8>>;
		/// Get all the unrouted ingress roots at the given block that
		/// are targeting the given parachain.
		fn ingress(to: Id) -> Option<StructuredUnroutedIngress>;
	}
}

/// Runtime ID module.
pub mod id {
	use sr_version::ApiId;

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
}
