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

use bp_messages::{LaneId, MessageDetails, MessageNonce};
use frame_support::weights::{
	Weight, WeightToFeeCoefficient, WeightToFeeCoefficients, WeightToFeePolynomial,
};
use sp_runtime::FixedU128;
use sp_std::prelude::*;
use sp_version::RuntimeVersion;

pub use bp_polkadot_core::*;

/// Rococo Chain
pub type Rococo = PolkadotLike;

/// The target length of a session (how often authorities change) on Rococo measured in of number
/// of blocks.
///
/// Note that since this is a target sessions may change before/after this time depending on network
/// conditions.
pub const SESSION_LENGTH: BlockNumber = time_units::HOURS;

// NOTE: This needs to be kept up to date with the Rococo runtime found in the Polkadot repo.
pub const VERSION: RuntimeVersion = RuntimeVersion {
	spec_name: sp_version::create_runtime_str!("rococo"),
	impl_name: sp_version::create_runtime_str!("parity-rococo-v2.0"),
	authoring_version: 0,
	spec_version: 9180,
	impl_version: 0,
	apis: sp_version::create_apis_vec![[]],
	transaction_version: 0,
	state_version: 0,
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

/// Name of the With-Rococo GRANDPA pallet instance that is deployed at bridged chains.
pub const WITH_ROCOCO_GRANDPA_PALLET_NAME: &str = "BridgeRococoGrandpa";
/// Name of the With-Rococo messages pallet instance that is deployed at bridged chains.
pub const WITH_ROCOCO_MESSAGES_PALLET_NAME: &str = "BridgeRococoMessages";

/// Name of the `RococoFinalityApi::best_finalized` runtime method.
pub const BEST_FINALIZED_ROCOCO_HEADER_METHOD: &str = "RococoFinalityApi_best_finalized";

/// Name of the `ToRococoOutboundLaneApi::estimate_message_delivery_and_dispatch_fee` runtime
/// method.
pub const TO_ROCOCO_ESTIMATE_MESSAGE_FEE_METHOD: &str =
	"ToRococoOutboundLaneApi_estimate_message_delivery_and_dispatch_fee";
/// Name of the `ToRococoOutboundLaneApi::message_details` runtime method.
pub const TO_ROCOCO_MESSAGE_DETAILS_METHOD: &str = "ToRococoOutboundLaneApi_message_details";

/// Existential deposit on Rococo.
pub const EXISTENTIAL_DEPOSIT: Balance = 1_000_000_000_000 / 100;

/// Weight of pay-dispatch-fee operation for inbound messages at Rococo chain.
///
/// This value corresponds to the result of
/// `pallet_bridge_messages::WeightInfoExt::pay_inbound_dispatch_fee_overhead()` call for your
/// chain. Don't put too much reserve there, because it is used to **decrease**
/// `DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT` cost. So putting large reserve would make delivery
/// transactions cheaper.
pub const PAY_INBOUND_DISPATCH_FEE_WEIGHT: Weight = 600_000_000;

sp_api::decl_runtime_apis! {
	/// API for querying information about the finalized Rococo headers.
	///
	/// This API is implemented by runtimes that are bridging with the Rococo chain, not the
	/// Rococo runtime itself.
	pub trait RococoFinalityApi {
		/// Returns number and hash of the best finalized header known to the bridge module.
		fn best_finalized() -> (BlockNumber, Hash);
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
			rococo_to_this_conversion_rate: Option<FixedU128>,
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
	}
}
