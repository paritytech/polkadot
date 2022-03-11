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

use bp_messages::{LaneId, MessageDetails, MessageNonce, UnrewardedRelayersState};
use frame_support::weights::{
	WeightToFeeCoefficient, WeightToFeeCoefficients, WeightToFeePolynomial,
};
use sp_std::prelude::*;
use sp_version::RuntimeVersion;

pub use bp_polkadot_core::*;

/// Polkadot Chain
pub type Polkadot = PolkadotLike;

// NOTE: This needs to be kept up to date with the Polkadot runtime found in the Polkadot repo.
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: sp_version::create_runtime_str!("polkadot"),
	impl_name: sp_version::create_runtime_str!("parity-polkadot"),
	authoring_version: 0,
	spec_version: 9100,
	impl_version: 0,
	apis: sp_version::create_apis_vec![[]],
	transaction_version: 7,
};

// NOTE: This needs to be kept up to date with the Polkadot runtime found in the Polkadot repo.
pub struct WeightToFee;
impl WeightToFeePolynomial for WeightToFee {
	type Balance = Balance;
	fn polynomial() -> WeightToFeeCoefficients<Self::Balance> {
		const CENTS: Balance = 10_000_000_000 / 100;
		// in Polkadot, extrinsic base weight (smallest non-zero weight) is mapped to 1/10 CENT:
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

// We use this to get the account on Polkadot (target) which is derived from Kusama's (source)
// account.
pub fn derive_account_from_kusama_id(id: bp_runtime::SourceAccount<AccountId>) -> AccountId {
	let encoded_id = bp_runtime::derive_account_id(bp_runtime::KUSAMA_CHAIN_ID, id);
	AccountIdConverter::convert(encoded_id)
}

/// Per-byte fee for Polkadot transactions.
pub const TRANSACTION_BYTE_FEE: Balance = 10 * 10_000_000_000 / 100 / 1_000;

/// Existential deposit on Polkadot.
pub const EXISTENTIAL_DEPOSIT: Balance = 10_000_000_000;

/// The target length of a session (how often authorities change) on Polkadot measured in of number
/// of blocks.
///
/// Note that since this is a target sessions may change before/after this time depending on network
/// conditions.
pub const SESSION_LENGTH: BlockNumber = 4 * time_units::HOURS;

/// Name of the With-Kusama messages pallet instance in the Polkadot runtime.
pub const WITH_KUSAMA_MESSAGES_PALLET_NAME: &str = "BridgeKusamaMessages";

/// Name of the KSM->DOT conversion rate stored in the Polkadot runtime.
pub const KUSAMA_TO_POLKADOT_CONVERSION_RATE_PARAMETER_NAME: &str =
	"KusamaToPolkadotConversionRate";

/// Name of the `PolkadotFinalityApi::best_finalized` runtime method.
pub const BEST_FINALIZED_POLKADOT_HEADER_METHOD: &str = "PolkadotFinalityApi_best_finalized";
/// Name of the `PolkadotFinalityApi::is_known_header` runtime method.
pub const IS_KNOWN_POLKADOT_HEADER_METHOD: &str = "PolkadotFinalityApi_is_known_header";

/// Name of the `ToPolkadotOutboundLaneApi::estimate_message_delivery_and_dispatch_fee` runtime
/// method.
pub const TO_POLKADOT_ESTIMATE_MESSAGE_FEE_METHOD: &str =
	"ToPolkadotOutboundLaneApi_estimate_message_delivery_and_dispatch_fee";
/// Name of the `ToPolkadotOutboundLaneApi::message_details` runtime method.
pub const TO_POLKADOT_MESSAGE_DETAILS_METHOD: &str = "ToPolkadotOutboundLaneApi_message_details";
/// Name of the `ToPolkadotOutboundLaneApi::latest_generated_nonce` runtime method.
pub const TO_POLKADOT_LATEST_GENERATED_NONCE_METHOD: &str =
	"ToPolkadotOutboundLaneApi_latest_generated_nonce";
/// Name of the `ToPolkadotOutboundLaneApi::latest_received_nonce` runtime method.
pub const TO_POLKADOT_LATEST_RECEIVED_NONCE_METHOD: &str =
	"ToPolkadotOutboundLaneApi_latest_received_nonce";

/// Name of the `FromPolkadotInboundLaneApi::latest_received_nonce` runtime method.
pub const FROM_POLKADOT_LATEST_RECEIVED_NONCE_METHOD: &str =
	"FromPolkadotInboundLaneApi_latest_received_nonce";
/// Name of the `FromPolkadotInboundLaneApi::latest_onfirmed_nonce` runtime method.
pub const FROM_POLKADOT_LATEST_CONFIRMED_NONCE_METHOD: &str =
	"FromPolkadotInboundLaneApi_latest_confirmed_nonce";
/// Name of the `FromPolkadotInboundLaneApi::unrewarded_relayers_state` runtime method.
pub const FROM_POLKADOT_UNREWARDED_RELAYERS_STATE: &str =
	"FromPolkadotInboundLaneApi_unrewarded_relayers_state";

sp_api::decl_runtime_apis! {
	/// API for querying information about the finalized Polkadot headers.
	///
	/// This API is implemented by runtimes that are bridging with the Polkadot chain, not the
	/// Polkadot runtime itself.
	pub trait PolkadotFinalityApi {
		/// Returns number and hash of the best finalized header known to the bridge module.
		fn best_finalized() -> (BlockNumber, Hash);
		/// Returns true if the header is known to the runtime.
		fn is_known_header(hash: Hash) -> bool;
	}

	/// Outbound message lane API for messages that are sent to Polkadot chain.
	///
	/// This API is implemented by runtimes that are sending messages to Polkadot chain, not the
	/// Polkadot runtime itself.
	pub trait ToPolkadotOutboundLaneApi<OutboundMessageFee: Parameter, OutboundPayload: Parameter> {
		/// Estimate message delivery and dispatch fee that needs to be paid by the sender on
		/// this chain.
		///
		/// Returns `None` if message is too expensive to be sent to Polkadot from this chain.
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

	/// Inbound message lane API for messages sent by Polkadot chain.
	///
	/// This API is implemented by runtimes that are receiving messages from Polkadot chain, not the
	/// Polkadot runtime itself.
	pub trait FromPolkadotInboundLaneApi {
		/// Returns nonce of the latest message, received by given lane.
		fn latest_received_nonce(lane: LaneId) -> MessageNonce;
		/// Nonce of the latest message that has been confirmed to the bridged chain.
		fn latest_confirmed_nonce(lane: LaneId) -> MessageNonce;
		/// State of the unrewarded relayers set at given lane.
		fn unrewarded_relayers_state(lane: LaneId) -> UnrewardedRelayersState;
	}
}
