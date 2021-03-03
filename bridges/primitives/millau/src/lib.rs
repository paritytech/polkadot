// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

#![cfg_attr(not(feature = "std"), no_std)]
// RuntimeApi generated functions
#![allow(clippy::too_many_arguments)]
// Runtime-generated DecodeLimit::decode_all_With_depth_limit
#![allow(clippy::unnecessary_mut_passed)]

mod millau_hash;

use bp_message_lane::{LaneId, MessageNonce, UnrewardedRelayersState};
use bp_runtime::Chain;
use frame_support::{
	weights::{constants::WEIGHT_PER_SECOND, DispatchClass, Weight},
	Parameter, RuntimeDebug,
};
use frame_system::limits;
use sp_core::Hasher as HasherT;
use sp_runtime::traits::Convert;
use sp_runtime::{
	traits::{IdentifyAccount, Verify},
	MultiSignature, MultiSigner, Perbill,
};
use sp_std::prelude::*;
use sp_trie::{trie_types::Layout, TrieConfiguration};

#[cfg(feature = "std")]
use serde::{Deserialize, Serialize};

pub use millau_hash::MillauHash;

/// Number of extra bytes (excluding size of storage value itself) of storage proof, built at
/// Millau chain. This mostly depends on number of entries (and their density) in the storage trie.
/// Some reserve is reserved to account future chain growth.
pub const EXTRA_STORAGE_PROOF_SIZE: u32 = 1024;

/// Maximal size (in bytes) of encoded (using `Encode::encode()`) account id.
pub const MAXIMAL_ENCODED_ACCOUNT_ID_SIZE: u32 = 32;

/// Maximum weight of single Millau block.
///
/// This represents 0.5 seconds of compute assuming a target block time of six seconds.
pub const MAXIMUM_BLOCK_WEIGHT: Weight = WEIGHT_PER_SECOND / 2;

/// Represents the average portion of a block's weight that will be used by an
/// `on_initialize()` runtime call.
pub const AVERAGE_ON_INITIALIZE_RATIO: Perbill = Perbill::from_percent(10);

/// Represents the portion of a block that will be used by Normal extrinsics.
pub const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);

/// Maximal number of unrewarded relayer entries at inbound lane.
pub const MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE: MessageNonce = 1024;

/// Maximal number of unconfirmed messages at inbound lane.
pub const MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE: MessageNonce = 1024;

/// Weight of single regular message delivery transaction on Millau chain.
///
/// This value is a result of `pallet_message_lane::Module::receive_messages_proof_weight()` call
/// for the case when single message of `pallet_message_lane::EXPECTED_DEFAULT_MESSAGE_LENGTH` bytes is delivered.
/// The message must have dispatch weight set to zero. The result then must be rounded up to account
/// possible future runtime upgrades.
pub const DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT: Weight = 1_000_000_000;

/// Increase of delivery transaction weight on Millau chain with every additional message byte.
///
/// This value is a result of `pallet_message_lane::WeightInfoExt::storage_proof_size_overhead(1)` call. The
/// result then must be rounded up to account possible future runtime upgrades.
pub const ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT: Weight = 25_000;

/// Maximal weight of single message delivery confirmation transaction on Millau chain.
///
/// This value is a result of `pallet_message_lane::Module::receive_messages_delivery_proof` weight formula computation
/// for the case when single message is confirmed. The result then must be rounded up to account possible future
/// runtime upgrades.
pub const MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT: Weight = 2_000_000_000;

/// The length of a session (how often authorities change) on Millau measured in of number of blocks.
pub const SESSION_LENGTH: BlockNumber = 5 * time_units::MINUTES;

/// Re-export `time_units` to make usage easier.
pub use time_units::*;

/// Human readable time units defined in terms of number of blocks.
pub mod time_units {
	use super::BlockNumber;

	pub const MILLISECS_PER_BLOCK: u64 = 6000;
	pub const SLOT_DURATION: u64 = MILLISECS_PER_BLOCK;

	pub const MINUTES: BlockNumber = 60_000 / (MILLISECS_PER_BLOCK as BlockNumber);
	pub const HOURS: BlockNumber = MINUTES * 60;
	pub const DAYS: BlockNumber = HOURS * 24;
}

/// Block number type used in Millau.
pub type BlockNumber = u64;

/// Hash type used in Millau.
pub type Hash = <BlakeTwoAndKeccak256 as HasherT>::Out;

/// The type of an object that can produce hashes on Millau.
pub type Hasher = BlakeTwoAndKeccak256;

/// The header type used by Millau.
pub type Header = sp_runtime::generic::Header<BlockNumber, Hasher>;

/// Alias to 512-bit hash when used in the context of a transaction signature on the chain.
pub type Signature = MultiSignature;

/// Some way of identifying an account on the chain. We intentionally make it equivalent
/// to the public key of our transaction signing scheme.
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;

/// Public key of the chain account that may be used to verify signatures.
pub type AccountSigner = MultiSigner;

/// Balance of an account.
pub type Balance = u64;

/// Millau chain.
#[derive(RuntimeDebug)]
pub struct Millau;

impl Chain for Millau {
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hasher = Hasher;
	type Header = Header;
}

/// Millau Hasher (Blake2-256 ++ Keccak-256) implementation.
#[derive(PartialEq, Eq, Clone, Copy, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub struct BlakeTwoAndKeccak256;

impl sp_core::Hasher for BlakeTwoAndKeccak256 {
	type Out = MillauHash;
	type StdHasher = hash256_std_hasher::Hash256StdHasher;
	const LENGTH: usize = 64;

	fn hash(s: &[u8]) -> Self::Out {
		let mut combined_hash = MillauHash::default();
		combined_hash.as_mut()[..32].copy_from_slice(&sp_io::hashing::blake2_256(s));
		combined_hash.as_mut()[32..].copy_from_slice(&sp_io::hashing::keccak_256(s));
		combined_hash
	}
}

impl sp_runtime::traits::Hash for BlakeTwoAndKeccak256 {
	type Output = MillauHash;

	fn trie_root(input: Vec<(Vec<u8>, Vec<u8>)>) -> Self::Output {
		Layout::<BlakeTwoAndKeccak256>::trie_root(input)
	}

	fn ordered_trie_root(input: Vec<Vec<u8>>) -> Self::Output {
		Layout::<BlakeTwoAndKeccak256>::ordered_trie_root(input)
	}
}

/// Convert a 256-bit hash into an AccountId.
pub struct AccountIdConverter;

impl sp_runtime::traits::Convert<sp_core::H256, AccountId> for AccountIdConverter {
	fn convert(hash: sp_core::H256) -> AccountId {
		hash.to_fixed_bytes().into()
	}
}

/// We use this to get the account on Millau (target) which is derived from Rialto's (source)
/// account. We do this so we can fund the derived account on Millau at Genesis to it can pay
/// transaction fees.
///
/// The reason we can use the same `AccountId` type for both chains is because they share the same
/// development seed phrase.
///
/// Note that this should only be used for testing.
pub fn derive_account_from_rialto_id(id: bp_runtime::SourceAccount<AccountId>) -> AccountId {
	let encoded_id = bp_runtime::derive_account_id(bp_runtime::RIALTO_BRIDGE_INSTANCE, id);
	AccountIdConverter::convert(encoded_id)
}

frame_support::parameter_types! {
	pub BlockLength: limits::BlockLength =
		limits::BlockLength::max_with_normal_ratio(2 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
	pub BlockWeights: limits::BlockWeights = limits::BlockWeights::builder()
		// Allowance for Normal class
		.for_class(DispatchClass::Normal, |weights| {
			weights.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
		})
		// Allowance for Operational class
		.for_class(DispatchClass::Operational, |weights| {
			weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
			// Extra reserved space for Operational class
			weights.reserved = Some(MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
		})
		// By default Mandatory class is not limited at all.
		// This parameter is used to derive maximal size of a single extrinsic.
		.avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
		.build_or_panic();
}

/// Get the maximum weight (compute time) that a Normal extrinsic on the Millau chain can use.
pub fn max_extrinsic_weight() -> Weight {
	BlockWeights::get()
		.get(DispatchClass::Normal)
		.max_extrinsic
		.unwrap_or(Weight::MAX)
}

/// Get the maximum length in bytes that a Normal extrinsic on the Millau chain requires.
pub fn max_extrinsic_size() -> u32 {
	*BlockLength::get().max.get(DispatchClass::Normal)
}

/// Name of the `MillauHeaderApi::best_block` runtime method.
pub const BEST_MILLAU_BLOCKS_METHOD: &str = "MillauHeaderApi_best_blocks";
/// Name of the `MillauHeaderApi::finalized_block` runtime method.
pub const FINALIZED_MILLAU_BLOCK_METHOD: &str = "MillauHeaderApi_finalized_block";
/// Name of the `MillauHeaderApi::is_known_block` runtime method.
pub const IS_KNOWN_MILLAU_BLOCK_METHOD: &str = "MillauHeaderApi_is_known_block";
/// Name of the `MillauHeaderApi::incomplete_headers` runtime method.
pub const INCOMPLETE_MILLAU_HEADERS_METHOD: &str = "MillauHeaderApi_incomplete_headers";

/// Name of the `ToMillauOutboundLaneApi::estimate_message_delivery_and_dispatch_fee` runtime method.
pub const TO_MILLAU_ESTIMATE_MESSAGE_FEE_METHOD: &str =
	"ToMillauOutboundLaneApi_estimate_message_delivery_and_dispatch_fee";
/// Name of the `ToMillauOutboundLaneApi::messages_dispatch_weight` runtime method.
pub const TO_MILLAU_MESSAGES_DISPATCH_WEIGHT_METHOD: &str = "ToMillauOutboundLaneApi_messages_dispatch_weight";
/// Name of the `ToMillauOutboundLaneApi::latest_received_nonce` runtime method.
pub const TO_MILLAU_LATEST_RECEIVED_NONCE_METHOD: &str = "ToMillauOutboundLaneApi_latest_received_nonce";
/// Name of the `ToMillauOutboundLaneApi::latest_generated_nonce` runtime method.
pub const TO_MILLAU_LATEST_GENERATED_NONCE_METHOD: &str = "ToMillauOutboundLaneApi_latest_generated_nonce";

/// Name of the `FromMillauInboundLaneApi::latest_received_nonce` runtime method.
pub const FROM_MILLAU_LATEST_RECEIVED_NONCE_METHOD: &str = "FromMillauInboundLaneApi_latest_received_nonce";
/// Name of the `FromMillauInboundLaneApi::latest_onfirmed_nonce` runtime method.
pub const FROM_MILLAU_LATEST_CONFIRMED_NONCE_METHOD: &str = "FromMillauInboundLaneApi_latest_confirmed_nonce";
/// Name of the `FromMillauInboundLaneApi::unrewarded_relayers_state` runtime method.
pub const FROM_MILLAU_UNREWARDED_RELAYERS_STATE: &str = "FromMillauInboundLaneApi_unrewarded_relayers_state";

sp_api::decl_runtime_apis! {
	/// API for querying information about Millau headers from the Bridge Pallet instance.
	///
	/// This API is implemented by runtimes that are bridging with Millau chain, not the
	/// Millau runtime itself.
	pub trait MillauHeaderApi {
		/// Returns number and hash of the best blocks known to the bridge module.
		///
		/// Will return multiple headers if there are many headers at the same "best" height.
		///
		/// The caller should only submit an `import_header` transaction that makes
		/// (or leads to making) other header the best one.
		fn best_blocks() -> Vec<(BlockNumber, Hash)>;
		/// Returns number and hash of the best finalized block known to the bridge module.
		fn finalized_block() -> (BlockNumber, Hash);
		/// Returns numbers and hashes of headers that require finality proofs.
		///
		/// An empty response means that there are no headers which currently require a
		/// finality proof.
		fn incomplete_headers() -> Vec<(BlockNumber, Hash)>;
		/// Returns true if the header is known to the runtime.
		fn is_known_block(hash: Hash) -> bool;
		/// Returns true if the header is considered finalized by the runtime.
		fn is_finalized_block(hash: Hash) -> bool;
	}

	/// Outbound message lane API for messages that are sent to Millau chain.
	///
	/// This API is implemented by runtimes that are sending messages to Millau chain, not the
	/// Millau runtime itself.
	pub trait ToMillauOutboundLaneApi<OutboundMessageFee: Parameter, OutboundPayload: Parameter> {
		/// Estimate message delivery and dispatch fee that needs to be paid by the sender on
		/// this chain.
		///
		/// Returns `None` if message is too expensive to be sent to Millau from this chain.
		///
		/// Please keep in mind that this method returns lowest message fee required for message
		/// to be accepted to the lane. It may be good idea to pay a bit over this price to account
		/// future exchange rate changes and guarantee that relayer would deliver your message
		/// to the target chain.
		fn estimate_message_delivery_and_dispatch_fee(
			lane_id: LaneId,
			payload: OutboundPayload,
		) -> Option<OutboundMessageFee>;
		/// Returns total dispatch weight and encoded payload size of all messages in given inclusive range.
		///
		/// If some (or all) messages are missing from the storage, they'll also will
		/// be missing from the resulting vector. The vector is ordered by the nonce.
		fn messages_dispatch_weight(
			lane: LaneId,
			begin: MessageNonce,
			end: MessageNonce,
		) -> Vec<(MessageNonce, Weight, u32)>;
		/// Returns nonce of the latest message, received by bridged chain.
		fn latest_received_nonce(lane: LaneId) -> MessageNonce;
		/// Returns nonce of the latest message, generated by given lane.
		fn latest_generated_nonce(lane: LaneId) -> MessageNonce;
	}

	/// Inbound message lane API for messages sent by Millau chain.
	///
	/// This API is implemented by runtimes that are receiving messages from Millau chain, not the
	/// Millau runtime itself.
	pub trait FromMillauInboundLaneApi {
		/// Returns nonce of the latest message, received by given lane.
		fn latest_received_nonce(lane: LaneId) -> MessageNonce;
		/// Nonce of latest message that has been confirmed to the bridged chain.
		fn latest_confirmed_nonce(lane: LaneId) -> MessageNonce;
		/// State of the unrewarded relayers set at given lane.
		fn unrewarded_relayers_state(lane: LaneId) -> UnrewardedRelayersState;
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_runtime::codec::Encode;

	#[test]
	fn maximal_account_size_does_not_overflow_constant() {
		assert!(
			MAXIMAL_ENCODED_ACCOUNT_ID_SIZE as usize >= AccountId::default().encode().len(),
			"Actual maximal size of encoded AccountId ({}) overflows expected ({})",
			AccountId::default().encode().len(),
			MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
		);
	}
}
