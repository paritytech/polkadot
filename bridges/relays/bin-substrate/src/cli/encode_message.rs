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

use crate::cli::{ExplicitOrMaximal, HexBytes};
use bp_messages::LaneId;
use bp_runtime::EncodedOrDecodedCall;
use relay_substrate_client::Chain;
use structopt::StructOpt;

/// All possible messages that may be delivered to generic Substrate chain.
///
/// Note this enum may be used in the context of both Source (as part of `encode-call`)
/// and Target chain (as part of `encode-message/send-message`).
#[derive(StructOpt, Debug, PartialEq, Eq)]
pub enum Message {
	/// Raw bytes for the message.
	Raw {
		/// Raw message bytes.
		data: HexBytes,
	},
	/// Message with given size.
	Sized {
		/// Sized of the message.
		size: ExplicitOrMaximal<u32>,
	},
}

/// Raw, SCALE-encoded message payload used in expected deployment.
pub type RawMessage = Vec<u8>;

pub trait CliEncodeMessage: Chain {
	/// Encode a send message call.
	fn encode_send_message_call(
		lane: LaneId,
		message: RawMessage,
		fee: Self::Balance,
		bridge_instance_index: u8,
	) -> anyhow::Result<EncodedOrDecodedCall<Self::Call>>;
}

/// Encode message payload passed through CLI flags.
pub(crate) fn encode_message<Source: Chain, Target: Chain>(
	message: &Message,
) -> anyhow::Result<RawMessage> {
	Ok(match message {
		Message::Raw { ref data } => data.0.clone(),
		Message::Sized { ref size } => match *size {
			ExplicitOrMaximal::Explicit(size) => vec![42; size as usize],
			ExplicitOrMaximal::Maximal => {
				let maximal_size = compute_maximal_message_size(
					Source::max_extrinsic_size(),
					Target::max_extrinsic_size(),
				);
				vec![42; maximal_size as usize]
			},
		},
	})
}

/// Compute maximal message size, given max extrinsic size at source and target chains.
pub(crate) fn compute_maximal_message_size(
	maximal_source_extrinsic_size: u32,
	maximal_target_extrinsic_size: u32,
) -> u32 {
	// assume that both signed extensions and other arguments fit 1KB
	let service_tx_bytes_on_source_chain = 1024;
	let maximal_source_extrinsic_size =
		maximal_source_extrinsic_size - service_tx_bytes_on_source_chain;
	let maximal_message_size =
		bridge_runtime_common::messages::target::maximal_incoming_message_size(
			maximal_target_extrinsic_size,
		);
	let maximal_message_size = if maximal_message_size > maximal_source_extrinsic_size {
		maximal_source_extrinsic_size
	} else {
		maximal_message_size
	};

	maximal_message_size
}
