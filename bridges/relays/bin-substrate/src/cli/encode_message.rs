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

use crate::cli::{bridge::FullBridge, AccountId, CliChain, HexBytes};
use crate::select_full_bridge;
use structopt::StructOpt;

/// Generic message payload.
#[derive(StructOpt, Debug, PartialEq, Eq)]
pub enum MessagePayload {
	/// Raw, SCALE-encoded `MessagePayload`.
	Raw {
		/// Hex-encoded SCALE data.
		data: HexBytes,
	},
	/// Construct message to send over the bridge.
	Call {
		/// Message details.
		#[structopt(flatten)]
		call: crate::cli::encode_call::Call,
		/// SS58 encoded Source account that will send the payload.
		#[structopt(long)]
		sender: AccountId,
	},
}

/// A `MessagePayload` to encode.
#[derive(StructOpt)]
pub struct EncodeMessage {
	/// A bridge instance to initalize.
	#[structopt(possible_values = &FullBridge::variants(), case_insensitive = true)]
	bridge: FullBridge,
	#[structopt(flatten)]
	payload: MessagePayload,
}

impl EncodeMessage {
	/// Run the command.
	pub fn encode(self) -> anyhow::Result<HexBytes> {
		select_full_bridge!(self.bridge, {
			let payload = Source::encode_message(self.payload).map_err(|e| anyhow::format_err!("{}", e))?;
			Ok(HexBytes::encode(&payload))
		})
	}

	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		let payload = self.encode()?;
		println!("{:?}", payload);
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_core::crypto::Ss58Codec;

	#[test]
	fn should_encode_raw_message() {
		// given
		let msg = "01000000e88514000000000002d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d003c040130000000000000000000000000";
		let encode_message = EncodeMessage::from_iter(vec!["encode-message", "MillauToRialto", "raw", msg]);

		// when
		let hex = encode_message.encode().unwrap();

		// then
		assert_eq!(format!("{:?}", hex), format!("0x{}", msg));
	}

	#[test]
	fn should_encode_remark_with_size() {
		// given
		let sender = sp_keyring::AccountKeyring::Alice.to_account_id().to_ss58check();
		let encode_message = EncodeMessage::from_iter(vec![
			"encode-message",
			"RialtoToMillau",
			"call",
			"--sender",
			&sender,
			"remark",
			"--remark-size",
			"12",
		]);

		// when
		let hex = encode_message.encode().unwrap();

		// then
		assert_eq!(format!("{:?}", hex), "0x01000000b0d60f000000000002d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d003c040130000000000000000000000000");
	}
}
