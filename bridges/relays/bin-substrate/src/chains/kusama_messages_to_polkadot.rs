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

//! Kusama-to-Polkadot messages sync entrypoint.

use frame_support::weights::Weight;

use messages_relay::relay_strategy::MixStrategy;
use relay_kusama_client::Kusama;
use relay_polkadot_client::Polkadot;
use substrate_relay_helper::messages_lane::SubstrateMessageLane;

/// Description of Kusama -> Polkadot messages bridge.
#[derive(Clone, Debug)]
pub struct KusamaMessagesToPolkadot;
substrate_relay_helper::generate_mocked_receive_message_proof_call_builder!(
	KusamaMessagesToPolkadot,
	KusamaMessagesToPolkadotReceiveMessagesProofCallBuilder,
	relay_polkadot_client::runtime::Call::BridgeKusamaMessages,
	relay_polkadot_client::runtime::BridgeKusamaMessagesCall::receive_messages_proof
);
substrate_relay_helper::generate_mocked_receive_message_delivery_proof_call_builder!(
	KusamaMessagesToPolkadot,
	KusamaMessagesToPolkadotReceiveMessagesDeliveryProofCallBuilder,
	relay_kusama_client::runtime::Call::BridgePolkadotMessages,
	relay_kusama_client::runtime::BridgePolkadotMessagesCall::receive_messages_delivery_proof
);
substrate_relay_helper::generate_mocked_update_conversion_rate_call_builder!(
	Kusama,
	KusamaMessagesToPolkadotUpdateConversionRateCallBuilder,
	relay_kusama_client::runtime::Call::BridgePolkadotMessages,
	relay_kusama_client::runtime::BridgePolkadotMessagesCall::update_pallet_parameter,
	relay_kusama_client::runtime::BridgePolkadotMessagesParameter::PolkadotToKusamaConversionRate
);

impl SubstrateMessageLane for KusamaMessagesToPolkadot {
	const SOURCE_TO_TARGET_CONVERSION_RATE_PARAMETER_NAME: Option<&'static str> =
		Some(bp_polkadot::KUSAMA_TO_POLKADOT_CONVERSION_RATE_PARAMETER_NAME);
	const TARGET_TO_SOURCE_CONVERSION_RATE_PARAMETER_NAME: Option<&'static str> =
		Some(bp_kusama::POLKADOT_TO_KUSAMA_CONVERSION_RATE_PARAMETER_NAME);

	const SOURCE_FEE_MULTIPLIER_PARAMETER_NAME: Option<&'static str> =
		Some(bp_polkadot::KUSAMA_FEE_MULTIPLIER_PARAMETER_NAME);
	const TARGET_FEE_MULTIPLIER_PARAMETER_NAME: Option<&'static str> =
		Some(bp_kusama::POLKADOT_FEE_MULTIPLIER_PARAMETER_NAME);

	const AT_SOURCE_TRANSACTION_PAYMENT_PALLET_NAME: Option<&'static str> =
		Some(bp_kusama::TRANSACTION_PAYMENT_PALLET_NAME);
	const AT_TARGET_TRANSACTION_PAYMENT_PALLET_NAME: Option<&'static str> =
		Some(bp_polkadot::TRANSACTION_PAYMENT_PALLET_NAME);

	type SourceChain = Kusama;
	type TargetChain = Polkadot;

	type SourceTransactionSignScheme = Kusama;
	type TargetTransactionSignScheme = Polkadot;

	type ReceiveMessagesProofCallBuilder = KusamaMessagesToPolkadotReceiveMessagesProofCallBuilder;
	type ReceiveMessagesDeliveryProofCallBuilder =
		KusamaMessagesToPolkadotReceiveMessagesDeliveryProofCallBuilder;

	type TargetToSourceChainConversionRateUpdateBuilder =
		KusamaMessagesToPolkadotUpdateConversionRateCallBuilder;

	type RelayStrategy = MixStrategy;
}
