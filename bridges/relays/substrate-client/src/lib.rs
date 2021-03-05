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

mod chain;
mod client;
mod error;
mod rpc;
mod sync_header;

pub mod guard;
pub mod headers_source;

pub use crate::chain::{BlockWithJustification, Chain, ChainWithBalances, TransactionSignScheme};
pub use crate::client::{Client, JustificationsSubscription, OpaqueGrandpaAuthoritiesSet};
pub use crate::error::{Error, Result};
pub use crate::sync_header::SyncHeader;
pub use bp_runtime::{BlockNumberOf, Chain as ChainBase, HashOf, HeaderOf};

/// Header id used by the chain.
pub type HeaderIdOf<C> = relay_utils::HeaderId<HashOf<C>, BlockNumberOf<C>>;

/// Substrate-over-websocket connection params.
#[derive(Debug, Clone)]
pub struct ConnectionParams {
	/// Websocket server hostname.
	pub host: String,
	/// Websocket server TCP port.
	pub port: u16,
}

impl Default for ConnectionParams {
	fn default() -> Self {
		ConnectionParams {
			host: "localhost".into(),
			port: 9944,
		}
	}
}
