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

//! Tools to interact with Substrate node using RPC methods.

#![warn(missing_docs)]

mod chain;
mod client;
mod error;
mod rpc;
mod sync_header;

pub mod guard;
pub mod metrics;

use std::time::Duration;

pub use crate::{
	chain::{
		AccountKeyPairOf, BlockWithJustification, CallOf, Chain, ChainWithBalances,
		ChainWithGrandpa, ChainWithMessages, SignParam, TransactionSignScheme, TransactionStatusOf,
		UnsignedTransaction, WeightToFeeOf,
	},
	client::{ChainRuntimeVersion, Client, OpaqueGrandpaAuthoritiesSet, Subscription},
	error::{Error, Result},
	sync_header::SyncHeader,
};
pub use bp_runtime::{
	AccountIdOf, AccountPublicOf, BalanceOf, BlockNumberOf, Chain as ChainBase, HashOf, HeaderOf,
	IndexOf, SignatureOf, TransactionEra, TransactionEraOf,
};

/// Header id used by the chain.
pub type HeaderIdOf<C> = relay_utils::HeaderId<HashOf<C>, BlockNumberOf<C>>;

/// Substrate-over-websocket connection params.
#[derive(Debug, Clone)]
pub struct ConnectionParams {
	/// Websocket server host name.
	pub host: String,
	/// Websocket server TCP port.
	pub port: u16,
	/// Use secure websocket connection.
	pub secure: bool,
	/// Defined chain runtime version
	pub chain_runtime_version: ChainRuntimeVersion,
}

impl Default for ConnectionParams {
	fn default() -> Self {
		ConnectionParams {
			host: "localhost".into(),
			port: 9944,
			secure: false,
			chain_runtime_version: ChainRuntimeVersion::Auto,
		}
	}
}

/// Returns stall timeout for relay loop.
///
/// Relay considers himself stalled if he has submitted transaction to the node, but it has not
/// been mined for this period.
pub fn transaction_stall_timeout(
	mortality_period: Option<u32>,
	average_block_interval: Duration,
	default_stall_timeout: Duration,
) -> Duration {
	// 1 extra block for transaction to reach the pool && 1 for relayer to awake after it is mined
	mortality_period
		.map(|mortality_period| average_block_interval.saturating_mul(mortality_period + 1 + 1))
		.unwrap_or(default_stall_timeout)
}

/// Returns stall timeout for relay loop that submit transactions to two chains.
///
/// Bidirectional relay may have two active transactions. Even if one of them has been spoiled, we
/// can't just restart the loop - the other transaction may still be alive and we'll be submitting
/// duplicate transaction, which may result in funds loss. So we'll be selecting maximal mortality
/// for choosing loop stall timeout.
pub fn bidirectional_transaction_stall_timeout(
	left_mortality_period: Option<u32>,
	right_mortality_period: Option<u32>,
	left_average_block_interval: Duration,
	right_average_block_interval: Duration,
	default_stall_timeout: Duration,
) -> Duration {
	std::cmp::max(
		transaction_stall_timeout(
			left_mortality_period,
			left_average_block_interval,
			default_stall_timeout,
		),
		transaction_stall_timeout(
			right_mortality_period,
			right_average_block_interval,
			default_stall_timeout,
		),
	)
}
