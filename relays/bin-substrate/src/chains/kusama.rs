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

use bp_messages::LaneId;
use bp_runtime::EncodedOrDecodedCall;
use relay_kusama_client::Kusama;
use relay_substrate_client::BalanceOf;
use sp_version::RuntimeVersion;

use crate::cli::{
	bridge,
	encode_message::{CliEncodeMessage, RawMessage},
	CliChain,
};

impl CliEncodeMessage for Kusama {
	fn encode_send_message_call(
		lane: LaneId,
		payload: RawMessage,
		fee: BalanceOf<Self>,
		bridge_instance_index: u8,
	) -> anyhow::Result<EncodedOrDecodedCall<Self::Call>> {
		Ok(match bridge_instance_index {
			bridge::KUSAMA_TO_POLKADOT_INDEX =>
				relay_kusama_client::runtime::Call::BridgePolkadotMessages(
					relay_kusama_client::runtime::BridgePolkadotMessagesCall::send_message(
						lane, payload, fee,
					),
				)
				.into(),
			_ => anyhow::bail!(
				"Unsupported target bridge pallet with instance index: {}",
				bridge_instance_index
			),
		})
	}
}

impl CliChain for Kusama {
	const RUNTIME_VERSION: RuntimeVersion = bp_kusama::VERSION;

	type KeyPair = sp_core::sr25519::Pair;
	type MessagePayload = Vec<u8>;

	fn ss58_format() -> u16 {
		sp_core::crypto::Ss58AddressFormat::from(
			sp_core::crypto::Ss58AddressFormatRegistry::KusamaAccount,
		)
		.into()
	}
}
