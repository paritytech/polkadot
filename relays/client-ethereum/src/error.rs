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

//! Ethereum node RPC errors.

use crate::types::U256;

use jsonrpsee_ws_client::types::Error as RpcError;
use relay_utils::MaybeConnectionError;
use thiserror::Error;

/// Result type used by Ethereum client.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur only when interacting with
/// an Ethereum node through RPC.
#[derive(Debug, Error)]
pub enum Error {
	/// IO error.
	#[error("IO error: {0}")]
	Io(#[from] std::io::Error),
	/// An error that can occur when making an HTTP request to
	/// an JSON-RPC client.
	#[error("RPC error: {0}")]
	RpcError(#[from] RpcError),
	/// Failed to parse response.
	#[error("Response parse failed: {0}")]
	ResponseParseFailed(String),
	/// We have received a header with missing fields.
	#[error("Incomplete Ethereum Header Received (missing some of required fields - hash, number, logs_bloom).")]
	IncompleteHeader,
	/// We have received a transaction missing a `raw` field.
	#[error("Incomplete Ethereum Transaction (missing required field - raw).")]
	IncompleteTransaction,
	/// An invalid Substrate block number was received from
	/// an Ethereum node.
	#[error("Received an invalid Substrate block from Ethereum Node.")]
	InvalidSubstrateBlockNumber,
	/// An invalid index has been received from an Ethereum node.
	#[error("Received an invalid incomplete index from Ethereum Node.")]
	InvalidIncompleteIndex,
	/// The client we're connected to is not synced, so we can't rely on its state. Contains
	/// number of unsynced headers.
	#[error("Ethereum client is not synced: syncing {0} headers.")]
	ClientNotSynced(U256),
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
