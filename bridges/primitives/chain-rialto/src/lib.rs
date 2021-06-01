// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

use bp_messages::{LaneId, MessageNonce, UnrewardedRelayersState};
use bp_runtime::Chain;
use frame_support::{
	weights::{constants::WEIGHT_PER_SECOND, DispatchClass, Weight},
	Parameter, RuntimeDebug,
};
use frame_system::limits;
use sp_core::Hasher as HasherT;
use sp_runtime::{
	traits::{BlakeTwo256, Convert, IdentifyAccount, Verify},
	MultiSignature, MultiSigner, Perbill,
};
use sp_std::prelude::*;

/// Number of extra bytes (excluding size of storage value itself) of storage proof, built at
/// Rialto chain. This mostly depends on number of entries (and their density) in the storage trie.
/// Some reserve is reserved to account future chain growth.
pub const EXTRA_STORAGE_PROOF_SIZE: u32 = 1024;

/// Number of bytes, included in the signed Rialto transaction apart from the encoded call itself.
///
/// Can be computed by subtracting encoded call size from raw transaction size.
pub const TX_EXTRA_BYTES: u32 = 103;

/// Maximal size (in bytes) of encoded (using `Encode::encode()`) account id.
pub const MAXIMAL_ENCODED_ACCOUNT_ID_SIZE: u32 = 32;

/// Maximal weight of single Rialto block.
///
/// This represents two seconds of compute assuming a target block time of six seconds.
pub const MAXIMUM_BLOCK_WEIGHT: Weight = 2 * WEIGHT_PER_SECOND;

/// Represents the average portion of a block's weight that will be used by an
/// `on_initialize()` runtime call.
pub const AVERAGE_ON_INITIALIZE_RATIO: Perbill = Perbill::from_percent(10);

/// Represents the portion of a block that will be used by Normal extrinsics.
pub const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);

/// Maximal number of unrewarded relayer entries at inbound lane.
pub const MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE: MessageNonce = 128;

/// Maximal number of unconfirmed messages at inbound lane.
pub const MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE: MessageNonce = 128;

/// Weight of single regular message delivery transaction on Rialto chain.
///
/// This value is a result of `pallet_bridge_messages::Pallet::receive_messages_proof_weight()` call
/// for the case when single message of `pallet_bridge_messages::EXPECTED_DEFAULT_MESSAGE_LENGTH` bytes is delivered.
/// The message must have dispatch weight set to zero. The result then must be rounded up to account
/// possible future runtime upgrades.
pub const DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT: Weight = 1_000_000_000;

/// Increase of delivery transaction weight on Rialto chain with every additional message byte.
///
/// This value is a result of `pallet_bridge_messages::WeightInfoExt::storage_proof_size_overhead(1)` call. The
/// result then must be rounded up to account possible future runtime upgrades.
pub const ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT: Weight = 25_000;

/// Maximal weight of single message delivery confirmation transaction on Rialto chain.
///
/// This value is a result of `pallet_bridge_messages::Pallet::receive_messages_delivery_proof` weight formula computation
/// for the case when single message is confirmed. The result then must be rounded up to account possible future
/// runtime upgrades.
pub const MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT: Weight = 2_000_000_000;

/// The target length of a session (how often authorities change) on Rialto measured in of number of
/// blocks.
///
/// Note that since this is a target sessions may change before/after this time depending on network
/// conditions.
pub const SESSION_LENGTH: BlockNumber = 4;

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

/// Block number type used in Rialto.
pub type BlockNumber = u32;

/// Hash type used in Rialto.
pub type Hash = <BlakeTwo256 as HasherT>::Out;

/// The type of an object that can produce hashes on Rialto.
pub type Hasher = BlakeTwo256;

/// The header type used by Rialto.
pub type Header = sp_runtime::generic::Header<BlockNumber, Hasher>;

/// Alias to 512-bit hash when used in the context of a transaction signature on the chain.
pub type Signature = MultiSignature;

/// Some way of identifying an account on the chain. We intentionally make it equivalent
/// to the public key of our transaction signing scheme.
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;

/// Public key of the chain account that may be used to verify signatures.
pub type AccountSigner = MultiSigner;

/// Balance of an account.
pub type Balance = u128;

/// Rialto chain.
#[derive(RuntimeDebug)]
pub struct Rialto;

impl Chain for Rialto {
	type BlockNumber = BlockNumber;
	type Hash = Hash;
	type Hasher = Hasher;
	type Header = Header;
}

/// Convert a 256-bit hash into an AccountId.
pub struct AccountIdConverter;

impl Convert<sp_core::H256, AccountId> for AccountIdConverter {
	fn convert(hash: sp_core::H256) -> AccountId {
		hash.to_fixed_bytes().into()
	}
}

// We use this to get the account on Rialto (target) which is derived from Millau's (source)
// account. We do this so we can fund the derived account on Rialto at Genesis to it can pay
// transaction fees.
//
// The reason we can use the same `AccountId` type for both chains is because they share the same
// development seed phrase.
//
// Note that this should only be used for testing.
pub fn derive_account_from_millau_id(id: bp_runtime::SourceAccount<AccountId>) -> AccountId {
	let encoded_id = bp_runtime::derive_account_id(bp_runtime::MILLAU_BRIDGE_INSTANCE, id);
	AccountIdConverter::convert(encoded_id)
}

frame_support::parameter_types! {
	pub BlockLength: limits::BlockLength =
		limits::BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
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

/// Name of the `RialtoFinalityApi::best_finalized` runtime method.
pub const BEST_FINALIZED_RIALTO_HEADER_METHOD: &str = "RialtoFinalityApi_best_finalized";

/// Name of the `ToRialtoOutboundLaneApi::estimate_message_delivery_and_dispatch_fee` runtime method.
pub const TO_RIALTO_ESTIMATE_MESSAGE_FEE_METHOD: &str =
	"ToRialtoOutboundLaneApi_estimate_message_delivery_and_dispatch_fee";
/// Name of the `ToRialtoOutboundLaneApi::messages_dispatch_weight` runtime method.
pub const TO_RIALTO_MESSAGES_DISPATCH_WEIGHT_METHOD: &str = "ToRialtoOutboundLaneApi_messages_dispatch_weight";
/// Name of the `ToRialtoOutboundLaneApi::latest_generated_nonce` runtime method.
pub const TO_RIALTO_LATEST_GENERATED_NONCE_METHOD: &str = "ToRialtoOutboundLaneApi_latest_generated_nonce";
/// Name of the `ToRialtoOutboundLaneApi::latest_received_nonce` runtime method.
pub const TO_RIALTO_LATEST_RECEIVED_NONCE_METHOD: &str = "ToRialtoOutboundLaneApi_latest_received_nonce";

/// Name of the `FromRialtoInboundLaneApi::latest_received_nonce` runtime method.
pub const FROM_RIALTO_LATEST_RECEIVED_NONCE_METHOD: &str = "FromRialtoInboundLaneApi_latest_received_nonce";
/// Name of the `FromRialtoInboundLaneApi::latest_onfirmed_nonce` runtime method.
pub const FROM_RIALTO_LATEST_CONFIRMED_NONCE_METHOD: &str = "FromRialtoInboundLaneApi_latest_confirmed_nonce";
/// Name of the `FromRialtoInboundLaneApi::unrewarded_relayers_state` runtime method.
pub const FROM_RIALTO_UNREWARDED_RELAYERS_STATE: &str = "FromRialtoInboundLaneApi_unrewarded_relayers_state";

sp_api::decl_runtime_apis! {
	/// API for querying information about the finalized Rialto headers.
	///
	/// This API is implemented by runtimes that are bridging with the Rialto chain, not the
	/// Millau runtime itself.
	pub trait RialtoFinalityApi {
		/// Returns number and hash of the best finalized header known to the bridge module.
		fn best_finalized() -> (BlockNumber, Hash);
		/// Returns true if the header is known to the runtime.
		fn is_known_header(hash: Hash) -> bool;
	}

	/// Outbound message lane API for messages that are sent to Rialto chain.
	///
	/// This API is implemented by runtimes that are sending messages to Rialto chain, not the
	/// Rialto runtime itself.
	pub trait ToRialtoOutboundLaneApi<OutboundMessageFee: Parameter, OutboundPayload: Parameter> {
		/// Estimate message delivery and dispatch fee that needs to be paid by the sender on
		/// this chain.
		///
		/// Returns `None` if message is too expensive to be sent to Rialto from this chain.
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

	/// Inbound message lane API for messages sent by Rialto chain.
	///
	/// This API is implemented by runtimes that are receiving messages from Rialto chain, not the
	/// Rialto runtime itself.
	pub trait FromRialtoInboundLaneApi {
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
