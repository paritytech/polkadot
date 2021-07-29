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
// Runtime-generated DecodeLimit::decode_all_with_depth_limit
#![allow(clippy::unnecessary_mut_passed)]

use bp_messages::{LaneId, MessageDetails, MessageNonce, UnrewardedRelayersState};
use frame_support::weights::{WeightToFeeCoefficient, WeightToFeeCoefficients, WeightToFeePolynomial};
use sp_std::prelude::*;
use sp_version::RuntimeVersion;

pub use bp_polkadot_core::*;

/// Rococo Chain
pub type Rococo = PolkadotLike;

/// The target length of a session (how often authorities change) on Westend measured in of number of
/// blocks.
///
/// Note that since this is a target sessions may change before/after this time depending on network
/// conditions.
pub const SESSION_LENGTH: BlockNumber = 10 * time_units::MINUTES;

// NOTE: This needs to be kept up to date with the Rococo runtime found in the Polkadot repo.
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: sp_version::create_runtime_str!("rococo"),
	impl_name: sp_version::create_runtime_str!("parity-rococo-v1.6"),
	authoring_version: 0,
	spec_version: 9004,
	impl_version: 0,
	apis: sp_version::create_apis_vec![[]],
	transaction_version: 0,
};

// NOTE: This needs to be kept up to date with the Rococo runtime found in the Polkadot repo.
pub struct WeightToFee;
impl WeightToFeePolynomial for WeightToFee {
	type Balance = Balance;
	fn polynomial() -> WeightToFeeCoefficients<Balance> {
		const CENTS: Balance = 1_000_000_000_000 / 100;
		let p = CENTS;
		let q = 10 * Balance::from(ExtrinsicBaseWeight::get());
		smallvec::smallvec![WeightToFeeCoefficient {
			degree: 1,
			negative: false,
			coeff_frac: Perbill::from_rational(p % q, q),
			coeff_integer: p / q,
		}]
	}
}

// We use this to get the account on Rococo (target) which is derived from Wococo's (source)
// account.
pub fn derive_account_from_wococo_id(id: bp_runtime::SourceAccount<AccountId>) -> AccountId {
	let encoded_id = bp_runtime::derive_account_id(bp_runtime::WOCOCO_CHAIN_ID, id);
	AccountIdConverter::convert(encoded_id)
}

/// Name of the `RococoFinalityApi::best_finalized` runtime method.
pub const BEST_FINALIZED_ROCOCO_HEADER_METHOD: &str = "RococoFinalityApi_best_finalized";
/// Name of the `RococoFinalityApi::is_known_header` runtime method.
pub const IS_KNOWN_ROCOCO_HEADER_METHOD: &str = "RococoFinalityApi_is_known_header";

/// Name of the `ToRococoOutboundLaneApi::estimate_message_delivery_and_dispatch_fee` runtime method.
pub const TO_ROCOCO_ESTIMATE_MESSAGE_FEE_METHOD: &str =
	"ToRococoOutboundLaneApi_estimate_message_delivery_and_dispatch_fee";
/// Name of the `ToRococoOutboundLaneApi::message_details` runtime method.
pub const TO_ROCOCO_MESSAGE_DETAILS_METHOD: &str = "ToRococoOutboundLaneApi_message_details";
/// Name of the `ToRococoOutboundLaneApi::latest_generated_nonce` runtime method.
pub const TO_ROCOCO_LATEST_GENERATED_NONCE_METHOD: &str = "ToRococoOutboundLaneApi_latest_generated_nonce";
/// Name of the `ToRococoOutboundLaneApi::latest_received_nonce` runtime method.
pub const TO_ROCOCO_LATEST_RECEIVED_NONCE_METHOD: &str = "ToRococoOutboundLaneApi_latest_received_nonce";

/// Name of the `FromRococoInboundLaneApi::latest_received_nonce` runtime method.
pub const FROM_ROCOCO_LATEST_RECEIVED_NONCE_METHOD: &str = "FromRococoInboundLaneApi_latest_received_nonce";
/// Name of the `FromRococoInboundLaneApi::latest_onfirmed_nonce` runtime method.
pub const FROM_ROCOCO_LATEST_CONFIRMED_NONCE_METHOD: &str = "FromRococoInboundLaneApi_latest_confirmed_nonce";
/// Name of the `FromRococoInboundLaneApi::unrewarded_relayers_state` runtime method.
pub const FROM_ROCOCO_UNREWARDED_RELAYERS_STATE: &str = "FromRococoInboundLaneApi_unrewarded_relayers_state";

sp_api::decl_runtime_apis! {
	/// API for querying information about the finalized Rococo headers.
	///
	/// This API is implemented by runtimes that are bridging with the Rococo chain, not the
	/// Rococo runtime itself.
	pub trait RococoFinalityApi {
		/// Returns number and hash of the best finalized header known to the bridge module.
		fn best_finalized() -> (BlockNumber, Hash);
		/// Returns true if the header is known to the runtime.
		fn is_known_header(hash: Hash) -> bool;
	}

	/// Outbound message lane API for messages that are sent to Rococo chain.
	///
	/// This API is implemented by runtimes that are sending messages to Rococo chain, not the
	/// Rococo runtime itself.
	pub trait ToRococoOutboundLaneApi<OutboundMessageFee: Parameter, OutboundPayload: Parameter> {
		/// Estimate message delivery and dispatch fee that needs to be paid by the sender on
		/// this chain.
		///
		/// Returns `None` if message is too expensive to be sent to Rococo from this chain.
		///
		/// Please keep in mind that this method returns the lowest message fee required for message
		/// to be accepted to the lane. It may be good idea to pay a bit over this price to account
		/// future exchange rate changes and guarantee that relayer would deliver your message
		/// to the target chain.
		fn estimate_message_delivery_and_dispatch_fee(
			lane_id: LaneId,
			payload: OutboundPayload,
		) -> Option<OutboundMessageFee>;
		/// Returns dispatch weight, encoded payload size and delivery+dispatch fee of all
		/// messages in given inclusive range.
		///
		/// If some (or all) messages are missing from the storage, they'll also will
		/// be missing from the resulting vector. The vector is ordered by the nonce.
		fn message_details(
			lane: LaneId,
			begin: MessageNonce,
			end: MessageNonce,
		) -> Vec<MessageDetails<OutboundMessageFee>>;
		/// Returns nonce of the latest message, received by bridged chain.
		fn latest_received_nonce(lane: LaneId) -> MessageNonce;
		/// Returns nonce of the latest message, generated by given lane.
		fn latest_generated_nonce(lane: LaneId) -> MessageNonce;
	}

	/// Inbound message lane API for messages sent by Rococo chain.
	///
	/// This API is implemented by runtimes that are receiving messages from Rococo chain, not the
	/// Rococo runtime itself.
	pub trait FromRococoInboundLaneApi {
		/// Returns nonce of the latest message, received by given lane.
		fn latest_received_nonce(lane: LaneId) -> MessageNonce;
		/// Nonce of the latest message that has been confirmed to the bridged chain.
		fn latest_confirmed_nonce(lane: LaneId) -> MessageNonce;
		/// State of the unrewarded relayers set at given lane.
		fn unrewarded_relayers_state(lane: LaneId) -> UnrewardedRelayersState;
	}
}
