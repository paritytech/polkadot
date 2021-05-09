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

//! Substrate node RPC errors.

use jsonrpsee_ws_client::Error as RpcError;
use relay_utils::MaybeConnectionError;
use sc_rpc_api::system::Health;

/// Result type used by Substrate client.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur only when interacting with
/// a Substrate node through RPC.
#[derive(Debug)]
pub enum Error {
	/// An error that can occur when making a request to
	/// an JSON-RPC server.
	RpcError(RpcError),
	/// The response from the server could not be SCALE decoded.
	ResponseParseFailed(codec::Error),
	/// The Substrate bridge pallet has not yet been initialized.
	UninitializedBridgePallet,
	/// Account does not exist on the chain.
	AccountDoesNotExist,
	/// Runtime storage is missing mandatory ":code:" entry.
	MissingMandatoryCodeEntry,
	/// The client we're connected to is not synced, so we can't rely on its state.
	ClientNotSynced(Health),
	/// An error has happened when we have tried to parse storage proof.
	StorageProofError(bp_runtime::StorageProofError),
	/// Custom logic error.
	Custom(String),
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Self::RpcError(ref e) => Some(e),
			Self::ResponseParseFailed(ref e) => Some(e),
			Self::UninitializedBridgePallet => None,
			Self::AccountDoesNotExist => None,
			Self::MissingMandatoryCodeEntry => None,
			Self::ClientNotSynced(_) => None,
			Self::StorageProofError(_) => None,
			Self::Custom(_) => None,
		}
	}
}

impl From<RpcError> for Error {
	fn from(error: RpcError) -> Self {
		Error::RpcError(error)
	}
}

impl MaybeConnectionError for Error {
	fn is_connection_error(&self) -> bool {
		matches!(
			*self,
			Error::RpcError(RpcError::TransportError(_))
				// right now if connection to the ws server is dropped (after it is already established),
				// we're getting this error
				| Error::RpcError(RpcError::Internal(_))
				| Error::RpcError(RpcError::RestartNeeded(_))
				| Error::ClientNotSynced(_),
		)
	}
}

impl std::fmt::Display for Error {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		let s = match self {
			Self::RpcError(e) => e.to_string(),
			Self::ResponseParseFailed(e) => e.to_string(),
			Self::UninitializedBridgePallet => "The Substrate bridge pallet has not been initialized yet.".into(),
			Self::AccountDoesNotExist => "Account does not exist on the chain".into(),
			Self::MissingMandatoryCodeEntry => "Mandatory :code: entry is missing from runtime storage".into(),
			Self::StorageProofError(e) => format!("Error when parsing storage proof: {:?}", e),
			Self::ClientNotSynced(health) => format!("Substrate client is not synced: {}", health),
			Self::Custom(e) => e.clone(),
		};

		write!(f, "{}", s)
	}
}

impl From<Error> for String {
	fn from(error: Error) -> String {
		error.to_string()
	}
}
