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

use codec::Decode;
use frame_support::weights::{DispatchClass, DispatchInfo, Pays, Weight};
use relay_kusama_client::Kusama;
use sp_core::storage::StorageKey;
use sp_runtime::{FixedPointNumber, FixedU128};
use sp_version::RuntimeVersion;

use crate::cli::{
	bridge,
	encode_call::{Call, CliEncodeCall},
	encode_message, CliChain,
};

/// Weight of the `system::remark` call at Kusama.
///
/// This weight is larger (x2) than actual weight at current Kusama runtime to avoid unsuccessful
/// calls in the future. But since it is used only in tests (and on test chains), this is ok.
pub(crate) const SYSTEM_REMARK_CALL_WEIGHT: Weight = 2 * 1_345_000;

/// Id of Kusama token that is used to fetch token price.
pub(crate) const TOKEN_ID: &str = "kusama";

impl CliEncodeCall for Kusama {
	fn max_extrinsic_size() -> u32 {
		bp_kusama::max_extrinsic_size()
	}

	fn encode_call(call: &Call) -> anyhow::Result<Self::Call> {
		Ok(match call {
			Call::Remark { remark_payload, .. } => relay_kusama_client::runtime::Call::System(
				relay_kusama_client::runtime::SystemCall::remark(
					remark_payload.as_ref().map(|x| x.0.clone()).unwrap_or_default(),
				),
			),
			Call::BridgeSendMessage { lane, payload, fee, bridge_instance_index } =>
				match *bridge_instance_index {
					bridge::KUSAMA_TO_POLKADOT_INDEX => {
						let payload = Decode::decode(&mut &*payload.0)?;
						relay_kusama_client::runtime::Call::BridgePolkadotMessages(
							relay_kusama_client::runtime::BridgePolkadotMessagesCall::send_message(
								lane.0, payload, fee.0,
							),
						)
					},
					_ => anyhow::bail!(
						"Unsupported target bridge pallet with instance index: {}",
						bridge_instance_index
					),
				},
			_ => anyhow::bail!("Unsupported Kusama call: {:?}", call),
		})
	}

	fn get_dispatch_info(
		call: &relay_kusama_client::runtime::Call,
	) -> anyhow::Result<DispatchInfo> {
		match *call {
			relay_kusama_client::runtime::Call::System(
				relay_kusama_client::runtime::SystemCall::remark(_),
			) => Ok(DispatchInfo {
				weight: crate::chains::kusama::SYSTEM_REMARK_CALL_WEIGHT,
				class: DispatchClass::Normal,
				pays_fee: Pays::Yes,
			}),
			_ => anyhow::bail!("Unsupported Kusama call: {:?}", call),
		}
	}
}

impl CliChain for Kusama {
	const RUNTIME_VERSION: RuntimeVersion = bp_kusama::VERSION;

	type KeyPair = sp_core::sr25519::Pair;
	type MessagePayload = ();

	fn ss58_format() -> u16 {
		42
	}

	fn max_extrinsic_weight() -> Weight {
		bp_kusama::max_extrinsic_weight()
	}

	fn encode_message(
		_message: encode_message::MessagePayload,
	) -> anyhow::Result<Self::MessagePayload> {
		anyhow::bail!("Sending messages from Kusama is not yet supported.")
	}
}

/// Storage key and initial value of Polkadot -> Kusama conversion rate.
pub(crate) fn polkadot_to_kusama_conversion_rate_params() -> (StorageKey, FixedU128) {
	(
		bp_runtime::storage_parameter_key(
			bp_kusama::POLKADOT_TO_KUSAMA_CONVERSION_RATE_PARAMETER_NAME,
		),
		// starting relay before this parameter will be set to some value may cause troubles
		FixedU128::from_inner(FixedU128::DIV),
	)
}
