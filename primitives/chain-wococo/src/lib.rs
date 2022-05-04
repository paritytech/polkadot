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
use sp_runtime::FixedU128;
use sp_std::prelude::*;

pub use bp_polkadot_core::*;
// Rococo runtime = Wococo runtime
pub use bp_rococo::{WeightToFee, EXISTENTIAL_DEPOSIT, PAY_INBOUND_DISPATCH_FEE_WEIGHT, VERSION};

/// Wococo Chain
pub type Wococo = PolkadotLike;

/// The target length of a session (how often authorities change) on Wococo measured in of number
/// of blocks.
///
/// Note that since this is a target sessions may change before/after this time depending on network
/// conditions.
pub const SESSION_LENGTH: BlockNumber = time_units::MINUTES;

// We use this to get the account on Wococo (target) which is derived from Rococo's (source)
// account.
pub fn derive_account_from_rococo_id(id: bp_runtime::SourceAccount<AccountId>) -> AccountId {
	let encoded_id = bp_runtime::derive_account_id(bp_runtime::ROCOCO_CHAIN_ID, id);
	AccountIdConverter::convert(encoded_id)
}

/// Name of the With-Wococo GRANDPA pallet instance that is deployed at bridged chains.
pub const WITH_WOCOCO_GRANDPA_PALLET_NAME: &str = "BridgeWococoGrandpa";
/// Name of the With-Wococo messages pallet instance that is deployed at bridged chains.
pub const WITH_WOCOCO_MESSAGES_PALLET_NAME: &str = "BridgeWococoMessages";

/// Name of the `WococoFinalityApi::best_finalized` runtime method.
pub const BEST_FINALIZED_WOCOCO_HEADER_METHOD: &str = "WococoFinalityApi_best_finalized";

/// Name of the `ToWococoOutboundLaneApi::estimate_message_delivery_and_dispatch_fee` runtime
/// method.
pub const TO_WOCOCO_ESTIMATE_MESSAGE_FEE_METHOD: &str =
	"ToWococoOutboundLaneApi_estimate_message_delivery_and_dispatch_fee";
/// Name of the `ToWococoOutboundLaneApi::message_details` runtime method.
pub const TO_WOCOCO_MESSAGE_DETAILS_METHOD: &str = "ToWococoOutboundLaneApi_message_details";

sp_api::decl_runtime_apis! {
	/// API for querying information about the finalized Wococo headers.
	///
	/// This API is implemented by runtimes that are bridging with the Wococo chain, not the
	/// Wococo runtime itself.
	pub trait WococoFinalityApi {
		/// Returns number and hash of the best finalized header known to the bridge module.
		fn best_finalized() -> (BlockNumber, Hash);
	}

	/// Outbound message lane API for messages that are sent to Wococo chain.
	///
	/// This API is implemented by runtimes that are sending messages to Wococo chain, not the
	/// Wococo runtime itself.
	pub trait ToWococoOutboundLaneApi<OutboundMessageFee: Parameter, OutboundPayload: Parameter> {
		/// Estimate message delivery and dispatch fee that needs to be paid by the sender on
		/// this chain.
		///
		/// Returns `None` if message is too expensive to be sent to Wococo from this chain.
		///
		/// Please keep in mind that this method returns the lowest message fee required for message
		/// to be accepted to the lane. It may be good idea to pay a bit over this price to account
		/// future exchange rate changes and guarantee that relayer would deliver your message
		/// to the target chain.
		fn estimate_message_delivery_and_dispatch_fee(
			lane_id: LaneId,
			payload: OutboundPayload,
			wococo_to_this_conversion_rate: Option<FixedU128>,
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
