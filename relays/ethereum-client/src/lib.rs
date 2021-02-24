// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Tools to interact with (Open) Ethereum node using RPC methods.

#![warn(missing_docs)]

mod client;
mod error;
mod rpc;
mod sign;

pub use crate::client::Client;
pub use crate::error::{Error, Result};
pub use crate::sign::{sign_and_submit_transaction, SigningParams};

pub mod types;

/// Ethereum connection params.
#[derive(Debug, Clone)]
pub struct ConnectionParams {
	/// Ethereum RPC host.
	pub host: String,
	/// Ethereum RPC port.
	pub port: u16,
}

impl Default for ConnectionParams {
	fn default() -> Self {
		ConnectionParams {
			host: "localhost".into(),
			port: 8545,
		}
	}
}
