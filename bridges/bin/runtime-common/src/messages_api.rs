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

//! Helpers for implementing various message-related runtime API mthods.

use crate::messages::{source::FromThisChainMessagePayload, MessageBridge};

use bp_messages::{LaneId, MessageDetails, MessageNonce};
use codec::Decode;
use sp_std::vec::Vec;

/// Implementation of the `To*OutboundLaneApi::message_details`.
pub fn outbound_message_details<Runtime, MessagesPalletInstance, BridgeConfig>(
	lane: LaneId,
	begin: MessageNonce,
	end: MessageNonce,
) -> Vec<MessageDetails<Runtime::OutboundMessageFee>>
where
	Runtime: pallet_bridge_messages::Config<MessagesPalletInstance>,
	MessagesPalletInstance: 'static,
	BridgeConfig: MessageBridge,
{
	(begin..=end)
		.filter_map(|nonce| {
			let message_data =
				pallet_bridge_messages::Pallet::<Runtime, MessagesPalletInstance>::outbound_message_data(lane, nonce)?;
			let decoded_payload =
				FromThisChainMessagePayload::<BridgeConfig>::decode(&mut &message_data.payload[..]).ok()?;
			Some(MessageDetails {
				nonce,
				dispatch_weight: decoded_payload.weight,
				size: message_data.payload.len() as _,
				delivery_and_dispatch_fee: message_data.fee,
				dispatch_fee_payment: decoded_payload.dispatch_fee_payment,
			})
		})
		.collect()
}
