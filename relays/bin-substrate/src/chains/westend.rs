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

//! Westend chain specification for CLI.

use crate::cli::{encode_message, CliChain};
use anyhow::anyhow;
use relay_westend_client::Westend;
use sp_version::RuntimeVersion;

impl CliChain for Westend {
	const RUNTIME_VERSION: RuntimeVersion = bp_westend::VERSION;

	type KeyPair = sp_core::sr25519::Pair;
	type MessagePayload = ();

	fn ss58_format() -> u16 {
		sp_core::crypto::Ss58AddressFormat::from(
			sp_core::crypto::Ss58AddressFormatRegistry::SubstrateAccount,
		)
		.into()
	}

	fn encode_message(
		_message: encode_message::MessagePayload,
	) -> anyhow::Result<Self::MessagePayload> {
		Err(anyhow!("Sending messages from Westend is not yet supported."))
	}
}
