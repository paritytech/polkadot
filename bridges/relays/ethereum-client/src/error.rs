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

//! Ethereum node RPC errors.

use crate::types::U256;

use jsonrpsee::client::RequestError;
use relay_utils::MaybeConnectionError;

/// Result type used by Ethereum client.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur only when interacting with
/// an Ethereum node through RPC.
#[derive(Debug)]
pub enum Error {
	/// An error that can occur when making an HTTP request to
	/// an JSON-RPC client.
	Request(RequestError),
	/// Failed to parse response.
	ResponseParseFailed(String),
	/// We have received a header with missing fields.
	IncompleteHeader,
	/// We have received a transaction missing a `raw` field.
	IncompleteTransaction,
	/// An invalid Substrate block number was received from
	/// an Ethereum node.
	InvalidSubstrateBlockNumber,
	/// An invalid index has been received from an Ethereum node.
	InvalidIncompleteIndex,
	/// The client we're connected to is not synced, so we can't rely on its state. Contains
	/// number of unsynced headers.
	ClientNotSynced(U256),
}

impl From<RequestError> for Error {
	fn from(error: RequestError) -> Self {
		Error::Request(error)
	}
}

impl MaybeConnectionError for Error {
	fn is_connection_error(&self) -> bool {
		matches!(
			*self,
			Error::Request(RequestError::TransportError(_)) | Error::ClientNotSynced(_),
		)
	}
}

impl ToString for Error {
	fn to_string(&self) -> String {
		match self {
			Self::Request(e) => e.to_string(),
			Self::ResponseParseFailed(e) => e.to_string(),
			Self::IncompleteHeader => {
				"Incomplete Ethereum Header Received (missing some of required fields - hash, number, logs_bloom)"
					.to_string()
			}
			Self::IncompleteTransaction => "Incomplete Ethereum Transaction (missing required field - raw)".to_string(),
			Self::InvalidSubstrateBlockNumber => "Received an invalid Substrate block from Ethereum Node".to_string(),
			Self::InvalidIncompleteIndex => "Received an invalid incomplete index from Ethereum Node".to_string(),
			Self::ClientNotSynced(missing_headers) => {
				format!("Ethereum client is not synced: syncing {} headers", missing_headers)
			}
		}
	}
}
