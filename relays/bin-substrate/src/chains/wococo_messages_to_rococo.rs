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

//! Wococo-to-Rococo messages sync entrypoint.

use frame_support::weights::Weight;

use messages_relay::relay_strategy::MixStrategy;
use relay_rococo_client::Rococo;
use relay_wococo_client::Wococo;
use substrate_relay_helper::messages_lane::SubstrateMessageLane;

/// Description of Wococo -> Rococo messages bridge.
#[derive(Clone, Debug)]
pub struct WococoMessagesToRococo;
substrate_relay_helper::generate_mocked_receive_message_proof_call_builder!(
	WococoMessagesToRococo,
	WococoMessagesToRococoReceiveMessagesProofCallBuilder,
	relay_rococo_client::runtime::Call::BridgeWococoMessages,
	relay_rococo_client::runtime::BridgeWococoMessagesCall::receive_messages_proof
);
substrate_relay_helper::generate_mocked_receive_message_delivery_proof_call_builder!(
	WococoMessagesToRococo,
	WococoMessagesToRococoReceiveMessagesDeliveryProofCallBuilder,
	relay_wococo_client::runtime::Call::BridgeRococoMessages,
	relay_wococo_client::runtime::BridgeRococoMessagesCall::receive_messages_delivery_proof
);

impl SubstrateMessageLane for WococoMessagesToRococo {
	const SOURCE_TO_TARGET_CONVERSION_RATE_PARAMETER_NAME: Option<&'static str> = None;
	const TARGET_TO_SOURCE_CONVERSION_RATE_PARAMETER_NAME: Option<&'static str> = None;

	const SOURCE_FEE_MULTIPLIER_PARAMETER_NAME: Option<&'static str> = None;
	const TARGET_FEE_MULTIPLIER_PARAMETER_NAME: Option<&'static str> = None;
	const AT_SOURCE_TRANSACTION_PAYMENT_PALLET_NAME: Option<&'static str> = None;
	const AT_TARGET_TRANSACTION_PAYMENT_PALLET_NAME: Option<&'static str> = None;

	type SourceChain = Wococo;
	type TargetChain = Rococo;

	type SourceTransactionSignScheme = Wococo;
	type TargetTransactionSignScheme = Rococo;

	type ReceiveMessagesProofCallBuilder = WococoMessagesToRococoReceiveMessagesProofCallBuilder;
	type ReceiveMessagesDeliveryProofCallBuilder =
		WococoMessagesToRococoReceiveMessagesDeliveryProofCallBuilder;

	type TargetToSourceChainConversionRateUpdateBuilder = ();

	type RelayStrategy = MixStrategy;
}
