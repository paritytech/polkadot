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

//! Rialto chain specification for CLI.

use crate::cli::{
	bridge,
	encode_call::{self, Call, CliEncodeCall},
	encode_message,
	send_message::{self, DispatchFeePayment},
	CliChain,
};
use anyhow::anyhow;
use bp_message_dispatch::{CallOrigin, MessagePayload};
use codec::Decode;
use frame_support::weights::{DispatchInfo, GetDispatchInfo, Weight};
use relay_rialto_client::Rialto;
use sp_core::storage::StorageKey;
use sp_runtime::FixedU128;
use sp_version::RuntimeVersion;

// Millau/Rialto tokens have no any real value, so the conversion rate we use is always 1:1. But we
// want to test our code that is intended to work with real-value chains. So to keep it close to
// 1:1, we'll be treating Rialto as BTC and Millau as wBTC (only in relayer).

/// The identifier of token, which value is associated with Rialto token value by relayer.
pub(crate) const ASSOCIATED_TOKEN_ID: &str = crate::chains::polkadot::TOKEN_ID;

impl CliEncodeCall for Rialto {
	fn max_extrinsic_size() -> u32 {
		bp_rialto::max_extrinsic_size()
	}

	fn encode_call(call: &Call) -> anyhow::Result<Self::Call> {
		Ok(match call {
			Call::Raw { data } => Decode::decode(&mut &*data.0)?,
			Call::Remark { remark_payload, .. } =>
				rialto_runtime::Call::System(rialto_runtime::SystemCall::remark {
					remark: remark_payload.as_ref().map(|x| x.0.clone()).unwrap_or_default(),
				}),
			Call::Transfer { recipient, amount } =>
				rialto_runtime::Call::Balances(rialto_runtime::BalancesCall::transfer {
					dest: recipient.raw_id().into(),
					value: amount.0,
				}),
			Call::BridgeSendMessage { lane, payload, fee, bridge_instance_index } =>
				match *bridge_instance_index {
					bridge::RIALTO_TO_MILLAU_INDEX => {
						let payload = Decode::decode(&mut &*payload.0)?;
						rialto_runtime::Call::BridgeMillauMessages(
							rialto_runtime::MessagesCall::send_message {
								lane_id: lane.0,
								payload,
								delivery_and_dispatch_fee: fee.0,
							},
						)
					},
					_ => anyhow::bail!(
						"Unsupported target bridge pallet with instance index: {}",
						bridge_instance_index
					),
				},
		})
	}

	fn get_dispatch_info(call: &rialto_runtime::Call) -> anyhow::Result<DispatchInfo> {
		Ok(call.get_dispatch_info())
	}
}

impl CliChain for Rialto {
	const RUNTIME_VERSION: RuntimeVersion = rialto_runtime::VERSION;

	type KeyPair = sp_core::sr25519::Pair;
	type MessagePayload = MessagePayload<
		bp_rialto::AccountId,
		bp_millau::AccountSigner,
		bp_millau::Signature,
		Vec<u8>,
	>;

	fn ss58_format() -> u16 {
		rialto_runtime::SS58Prefix::get() as u16
	}

	fn max_extrinsic_weight() -> Weight {
		bp_rialto::max_extrinsic_weight()
	}

	fn encode_message(
		message: encode_message::MessagePayload,
	) -> anyhow::Result<Self::MessagePayload> {
		match message {
			encode_message::MessagePayload::Raw { data } => MessagePayload::decode(&mut &*data.0)
				.map_err(|e| anyhow!("Failed to decode Rialto's MessagePayload: {:?}", e)),
			encode_message::MessagePayload::Call { mut call, mut sender } => {
				type Source = Rialto;
				type Target = relay_millau_client::Millau;

				sender.enforce_chain::<Source>();
				let spec_version = Target::RUNTIME_VERSION.spec_version;
				let origin = CallOrigin::SourceAccount(sender.raw_id());
				encode_call::preprocess_call::<Source, Target>(
					&mut call,
					bridge::RIALTO_TO_MILLAU_INDEX,
				);
				let call = Target::encode_call(&call)?;
				let weight = call.get_dispatch_info().weight;

				Ok(send_message::message_payload(
					spec_version,
					weight,
					origin,
					&call,
					DispatchFeePayment::AtSourceChain,
				))
			},
		}
	}
}

/// Storage key and initial value of Millau -> Rialto conversion rate.
pub(crate) fn millau_to_rialto_conversion_rate_params() -> (StorageKey, FixedU128) {
	(
		StorageKey(rialto_runtime::millau_messages::MillauToRialtoConversionRate::key().to_vec()),
		rialto_runtime::millau_messages::INITIAL_MILLAU_TO_RIALTO_CONVERSION_RATE,
	)
}
