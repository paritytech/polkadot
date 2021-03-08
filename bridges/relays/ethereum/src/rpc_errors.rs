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

use relay_ethereum_client::Error as EthereumNodeError;
use relay_substrate_client::Error as SubstrateNodeError;
use relay_utils::MaybeConnectionError;

/// Contains common errors that can occur when
/// interacting with a Substrate or Ethereum node
/// through RPC.
#[derive(Debug)]
pub enum RpcError {
	/// The arguments to the RPC method failed to serialize.
	Serialization(serde_json::Error),
	/// An error occured when interacting with an Ethereum node.
	Ethereum(EthereumNodeError),
	/// An error occured when interacting with a Substrate node.
	Substrate(SubstrateNodeError),
}

impl From<RpcError> for String {
	fn from(err: RpcError) -> Self {
		match err {
			RpcError::Serialization(e) => e.to_string(),
			RpcError::Ethereum(e) => e.to_string(),
			RpcError::Substrate(e) => e.to_string(),
		}
	}
}

impl From<serde_json::Error> for RpcError {
	fn from(err: serde_json::Error) -> Self {
		Self::Serialization(err)
	}
}

impl From<EthereumNodeError> for RpcError {
	fn from(err: EthereumNodeError) -> Self {
		Self::Ethereum(err)
	}
}

impl From<SubstrateNodeError> for RpcError {
	fn from(err: SubstrateNodeError) -> Self {
		Self::Substrate(err)
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
