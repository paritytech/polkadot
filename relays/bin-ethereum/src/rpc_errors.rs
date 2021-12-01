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

use relay_ethereum_client::Error as EthereumNodeError;
use relay_substrate_client::Error as SubstrateNodeError;
use relay_utils::MaybeConnectionError;
use thiserror::Error;

/// Contains common errors that can occur when
/// interacting with a Substrate or Ethereum node
/// through RPC.
#[derive(Debug, Error)]
pub enum RpcError {
	/// The arguments to the RPC method failed to serialize.
	#[error("RPC arguments serialization failed: {0}")]
	Serialization(#[from] serde_json::Error),
	/// An error occurred when interacting with an Ethereum node.
	#[error("Ethereum node error: {0}")]
	Ethereum(#[from] EthereumNodeError),
	/// An error occurred when interacting with a Substrate node.
	#[error("Substrate node error: {0}")]
	Substrate(#[from] SubstrateNodeError),
	/// Error running relay loop.
	#[error("{0}")]
	SyncLoop(String),
}

impl From<RpcError> for String {
	fn from(err: RpcError) -> Self {
		format!("{}", err)
	}
}

impl From<ethabi::Error> for RpcError {
	fn from(err: ethabi::Error) -> Self {
		Self::Ethereum(EthereumNodeError::ResponseParseFailed(format!("{}", err)))
	}
}

impl MaybeConnectionError for RpcError {
	fn is_connection_error(&self) -> bool {
		match self {
			RpcError::Ethereum(ref error) => error.is_connection_error(),
			RpcError::Substrate(ref error) => error.is_connection_error(),
			_ => false,
		}
	}
}

impl From<codec::Error> for RpcError {
	fn from(err: codec::Error) -> Self {
		Self::Substrate(SubstrateNodeError::ResponseParseFailed(err))
	}
}
