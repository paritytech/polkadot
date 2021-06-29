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

use crate::cli::bridge::FullBridge;
use crate::cli::{Balance, CliChain, HexBytes, HexLaneId, SourceConnectionParams};
use crate::select_full_bridge;
use codec::{Decode, Encode};
use relay_substrate_client::Chain;
use structopt::StructOpt;

/// Estimate Delivery & Dispatch Fee command.
#[derive(StructOpt, Debug, PartialEq, Eq)]
pub struct EstimateFee {
	/// A bridge instance to encode call for.
	#[structopt(possible_values = &FullBridge::variants(), case_insensitive = true)]
	bridge: FullBridge,
	#[structopt(flatten)]
	source: SourceConnectionParams,
	/// Hex-encoded id of lane that will be delivering the message.
	#[structopt(long, default_value = "00000000")]
	lane: HexLaneId,
	/// Payload to send over the bridge.
	#[structopt(flatten)]
	payload: crate::cli::encode_message::MessagePayload,
}

impl EstimateFee {
	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		let Self {
			source,
			bridge,
			lane,
			payload,
		} = self;

		select_full_bridge!(bridge, {
			let source_client = source.to_client::<Source>().await?;
			let lane = lane.into();
			let payload = Source::encode_message(payload).map_err(|e| anyhow::format_err!("{:?}", e))?;

			let fee: <Source as Chain>::Balance =
				estimate_message_delivery_and_dispatch_fee(&source_client, ESTIMATE_MESSAGE_FEE_METHOD, lane, payload)
					.await?;

			log::info!(target: "bridge", "Fee: {:?}", Balance(fee as _));
			println!("{}", fee);
			Ok(())
		})
	}
}

pub(crate) async fn estimate_message_delivery_and_dispatch_fee<Fee: Decode, C: Chain, P: Encode>(
	client: &relay_substrate_client::Client<C>,
	estimate_fee_method: &str,
	lane: bp_messages::LaneId,
	payload: P,
) -> anyhow::Result<Fee> {
	let encoded_response = client
		.state_call(estimate_fee_method.into(), (lane, payload).encode().into(), None)
		.await?;
	let decoded_response: Option<Fee> =
		Decode::decode(&mut &encoded_response.0[..]).map_err(relay_substrate_client::Error::ResponseParseFailed)?;
	let fee = decoded_response
		.ok_or_else(|| anyhow::format_err!("Unable to decode fee from: {:?}", HexBytes(encoded_response.to_vec())))?;
	Ok(fee)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::cli::encode_call;
	use sp_core::crypto::Ss58Codec;

	#[test]
	fn should_parse_cli_options() {
		// given
		let alice = sp_keyring::AccountKeyring::Alice.to_account_id().to_ss58check();

		// when
		let res = EstimateFee::from_iter(vec![
			"estimate_fee",
			"RialtoToMillau",
			"--source-port",
			"1234",
			"call",
			"--sender",
			&alice,
			"remark",
			"--remark-payload",
			"1234",
		]);

		// then
		assert_eq!(
			res,
			EstimateFee {
				bridge: FullBridge::RialtoToMillau,
				lane: HexLaneId([0, 0, 0, 0]),
				source: SourceConnectionParams {
					source_host: "127.0.0.1".into(),
					source_port: 1234,
					source_secure: false,
				},
				payload: crate::cli::encode_message::MessagePayload::Call {
					sender: alice.parse().unwrap(),
					call: encode_call::Call::Remark {
						remark_payload: Some(HexBytes(vec![0x12, 0x34])),
						remark_size: None,
					}
				}
			}
		);
	}
}
