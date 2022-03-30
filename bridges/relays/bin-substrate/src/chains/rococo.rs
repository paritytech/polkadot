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

use anyhow::anyhow;
use bp_message_dispatch::{CallOrigin, MessagePayload};
use bp_runtime::EncodedOrDecodedCall;
use codec::Decode;
use frame_support::weights::{DispatchClass, DispatchInfo, Pays, Weight};
use relay_rococo_client::Rococo;
use sp_version::RuntimeVersion;

use crate::cli::{
	bridge,
	encode_call::{self, Call, CliEncodeCall},
	encode_message,
	send_message::{self, DispatchFeePayment},
	CliChain,
};

/// Weight of the `system::remark` call at Rococo.
///
/// This weight is larger (x2) than actual weight at current Rococo runtime to avoid unsuccessful
/// calls in the future. But since it is used only in tests (and on test chains), this is ok.
pub(crate) const SYSTEM_REMARK_CALL_WEIGHT: Weight = 2 * 1_345_000;

impl CliEncodeCall for Rococo {
	fn encode_call(call: &Call) -> anyhow::Result<EncodedOrDecodedCall<Self::Call>> {
		Ok(match call {
			Call::Raw { data } => EncodedOrDecodedCall::Encoded(data.0.clone()),
			Call::Remark { remark_payload, .. } => relay_rococo_client::runtime::Call::System(
				relay_rococo_client::runtime::SystemCall::remark(
					remark_payload.as_ref().map(|x| x.0.clone()).unwrap_or_default(),
				),
			)
			.into(),
			Call::BridgeSendMessage { lane, payload, fee, bridge_instance_index } =>
				match *bridge_instance_index {
					bridge::ROCOCO_TO_WOCOCO_INDEX => {
						let payload = Decode::decode(&mut &*payload.0)?;
						relay_rococo_client::runtime::Call::BridgeWococoMessages(
							relay_rococo_client::runtime::BridgeWococoMessagesCall::send_message(
								lane.0, payload, fee.0,
							),
						)
						.into()
					},
					_ => anyhow::bail!(
						"Unsupported target bridge pallet with instance index: {}",
						bridge_instance_index
					),
				},
			_ => anyhow::bail!("The call is not supported"),
		})
	}

	fn get_dispatch_info(call: &EncodedOrDecodedCall<Self::Call>) -> anyhow::Result<DispatchInfo> {
		match *call {
			EncodedOrDecodedCall::Decoded(relay_rococo_client::runtime::Call::System(
				relay_rococo_client::runtime::SystemCall::remark(_),
			)) => Ok(DispatchInfo {
				weight: SYSTEM_REMARK_CALL_WEIGHT,
				class: DispatchClass::Normal,
				pays_fee: Pays::Yes,
			}),
			_ => anyhow::bail!("Unsupported Rococo call: {:?}", call),
		}
	}
}

impl CliChain for Rococo {
	const RUNTIME_VERSION: RuntimeVersion = bp_rococo::VERSION;

	type KeyPair = sp_core::sr25519::Pair;
	type MessagePayload = MessagePayload<
		bp_rococo::AccountId,
		bp_wococo::AccountPublic,
		bp_wococo::Signature,
		Vec<u8>,
	>;

	fn ss58_format() -> u16 {
		42
	}

	fn encode_message(
		message: encode_message::MessagePayload,
	) -> anyhow::Result<Self::MessagePayload> {
		match message {
			encode_message::MessagePayload::Raw { data } => MessagePayload::decode(&mut &*data.0)
				.map_err(|e| anyhow!("Failed to decode Rococo's MessagePayload: {:?}", e)),
			encode_message::MessagePayload::Call { mut call, mut sender, dispatch_weight } => {
				type Source = Rococo;
				type Target = relay_wococo_client::Wococo;

				sender.enforce_chain::<Source>();
				let spec_version = Target::RUNTIME_VERSION.spec_version;
				let origin = CallOrigin::SourceAccount(sender.raw_id());
				encode_call::preprocess_call::<Source, Target>(
					&mut call,
					bridge::ROCOCO_TO_WOCOCO_INDEX,
				);
				let call = Target::encode_call(&call)?;
				let dispatch_weight = dispatch_weight.map(Ok).unwrap_or_else(|| {
					Err(anyhow::format_err!(
						"Please specify dispatch weight of the encoded Wococo call"
					))
				})?;

				Ok(send_message::message_payload(
					spec_version,
					dispatch_weight,
					origin,
					&call,
					DispatchFeePayment::AtSourceChain,
				))
			},
		}
	}
}
