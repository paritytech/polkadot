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

//! Millau-to-Rialto messages sync entrypoint.

use messages_relay::relay_strategy::MixStrategy;
use relay_millau_client::Millau;
use relay_rialto_client::Rialto;
use substrate_relay_helper::messages_lane::{
	DirectReceiveMessagesDeliveryProofCallBuilder, DirectReceiveMessagesProofCallBuilder,
	SubstrateMessageLane,
};

/// Description of Millau -> Rialto messages bridge.
#[derive(Clone, Debug)]
pub struct MillauMessagesToRialto;
substrate_relay_helper::generate_direct_update_conversion_rate_call_builder!(
	Millau,
	MillauMessagesToRialtoUpdateConversionRateCallBuilder,
	millau_runtime::Runtime,
	millau_runtime::WithRialtoMessagesInstance,
	millau_runtime::rialto_messages::MillauToRialtoMessagesParameter::RialtoToMillauConversionRate
);

impl SubstrateMessageLane for MillauMessagesToRialto {
	const SOURCE_TO_TARGET_CONVERSION_RATE_PARAMETER_NAME: Option<&'static str> =
		Some(bp_rialto::MILLAU_TO_RIALTO_CONVERSION_RATE_PARAMETER_NAME);
	const TARGET_TO_SOURCE_CONVERSION_RATE_PARAMETER_NAME: Option<&'static str> =
		Some(bp_millau::RIALTO_TO_MILLAU_CONVERSION_RATE_PARAMETER_NAME);

	const SOURCE_FEE_MULTIPLIER_PARAMETER_NAME: Option<&'static str> = None;
	const TARGET_FEE_MULTIPLIER_PARAMETER_NAME: Option<&'static str> = None;
	const AT_SOURCE_TRANSACTION_PAYMENT_PALLET_NAME: Option<&'static str> = None;
	const AT_TARGET_TRANSACTION_PAYMENT_PALLET_NAME: Option<&'static str> = None;

	type SourceChain = Millau;
	type TargetChain = Rialto;

	type SourceTransactionSignScheme = Millau;
	type TargetTransactionSignScheme = Rialto;

	type ReceiveMessagesProofCallBuilder = DirectReceiveMessagesProofCallBuilder<
		Self,
		rialto_runtime::Runtime,
		rialto_runtime::WithMillauMessagesInstance,
	>;
	type ReceiveMessagesDeliveryProofCallBuilder = DirectReceiveMessagesDeliveryProofCallBuilder<
		Self,
		millau_runtime::Runtime,
		millau_runtime::WithRialtoMessagesInstance,
	>;

	type TargetToSourceChainConversionRateUpdateBuilder =
		MillauMessagesToRialtoUpdateConversionRateCallBuilder;

	type RelayStrategy = MixStrategy;
}
