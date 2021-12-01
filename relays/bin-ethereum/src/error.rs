// Copyright 2021 Parity Technologies (UK) Ltd.
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

use crate::rpc_errors::RpcError;
use thiserror::Error;

/// Result type used by PoA relay.
pub type Result<T> = std::result::Result<T, Error>;

/// Ethereum PoA relay errors.
#[derive(Error, Debug)]
pub enum Error {
	/// Failed to decode initial header.
	#[error("Error decoding initial header: {0}")]
	DecodeInitialHeader(codec::Error),
	/// RPC error.
	#[error("{0}")]
	Rpc(#[from] RpcError),
	/// Failed to read genesis header.
	#[error("Error reading Substrate genesis header: {0:?}")]
	ReadGenesisHeader(relay_substrate_client::Error),
	/// Failed to read initial GRANDPA authorities.
	#[error("Error reading GRANDPA authorities set: {0:?}")]
	ReadAuthorities(relay_substrate_client::Error),
	/// Failed to deploy bridge contract to Ethereum chain.
	#[error("Error deploying contract: {0:?}")]
	DeployContract(RpcError),
}
