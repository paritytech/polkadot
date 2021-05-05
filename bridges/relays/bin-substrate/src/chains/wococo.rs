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

use crate::cli::{encode_message, CliChain};
use frame_support::weights::Weight;
use relay_wococo_client::Wococo;
use sp_version::RuntimeVersion;

impl CliChain for Wococo {
	const RUNTIME_VERSION: RuntimeVersion = bp_wococo::VERSION;

	type KeyPair = sp_core::sr25519::Pair;
	type MessagePayload = ();

	fn ss58_format() -> u16 {
		42
	}

	fn max_extrinsic_weight() -> Weight {
		0
	}

	fn encode_message(_message: encode_message::MessagePayload) -> Result<Self::MessagePayload, String> {
		Err("Sending messages from Wococo is not yet supported.".into())
	}
}
