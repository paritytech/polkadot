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

use jsonrpsee::core::Error as RpcError;
use relay_utils::MaybeConnectionError;
use sc_rpc_api::system::Health;
use sp_runtime::transaction_validity::TransactionValidityError;
use thiserror::Error;

/// Result type used by Substrate client.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur only when interacting with
/// a Substrate node through RPC.
#[derive(Error, Debug)]
pub enum Error {
	/// IO error.
	#[error("IO error: {0}")]
	Io(#[from] std::io::Error),
	/// An error that can occur when making a request to
	/// an JSON-RPC server.
	#[error("RPC error: {0}")]
	RpcError(#[from] RpcError),
	/// The response from the server could not be SCALE decoded.
	#[error("Response parse failed: {0}")]
	ResponseParseFailed(#[from] codec::Error),
	/// The Substrate bridge pallet has not yet been initialized.
	#[error("The Substrate bridge pallet has not been initialized yet.")]
	UninitializedBridgePallet,
	/// Account does not exist on the chain.
	#[error("Account does not exist on the chain.")]
	AccountDoesNotExist,
	/// Runtime storage is missing mandatory ":code:" entry.
	#[error("Mandatory :code: entry is missing from runtime storage.")]
	MissingMandatoryCodeEntry,
	/// The client we're connected to is not synced, so we can't rely on its state.
	#[error("Substrate client is not synced {0}.")]
	ClientNotSynced(Health),
	/// The bridge pallet is halted and all transactions will be rejected.
	#[error("Bridge pallet is halted.")]
	BridgePalletIsHalted,
	/// An error has happened when we have tried to parse storage proof.
	#[error("Error when parsing storage proof: {0:?}.")]
	StorageProofError(bp_runtime::StorageProofError),
	/// The Substrate transaction is invalid.
	#[error("Substrate transaction is invalid: {0:?}")]
	TransactionInvalid(#[from] TransactionValidityError),
	/// Custom logic error.
	#[error("{0}")]
	Custom(String),
}

impl From<tokio::task::JoinError> for Error {
	fn from(error: tokio::task::JoinError) -> Self {
		Error::Custom(format!("Failed to wait tokio task: {}", error))
	}
}

impl MaybeConnectionError for Error {
	fn is_connection_error(&self) -> bool {
		matches!(
			*self,
			Error::RpcError(RpcError::Transport(_))
				// right now if connection to the ws server is dropped (after it is already established),
				// we're getting this error
				| Error::RpcError(RpcError::Internal(_))
				| Error::RpcError(RpcError::RestartNeeded(_))
				| Error::ClientNotSynced(_),
		)
	}
}
