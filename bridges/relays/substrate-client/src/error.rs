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

//! Substrate node RPC errors.

use jsonrpsee::client::RequestError;
use jsonrpsee::transport::ws::WsNewDnsError;
use relay_utils::MaybeConnectionError;
use sc_rpc_api::system::Health;

/// Result type used by Substrate client.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur only when interacting with
/// a Substrate node through RPC.
#[derive(Debug)]
pub enum Error {
	/// Web socket connection error.
	WsConnectionError(WsNewDnsError),
	/// An error that can occur when making a request to
	/// an JSON-RPC server.
	Request(RequestError),
	/// The response from the server could not be SCALE decoded.
	ResponseParseFailed(codec::Error),
	/// The Substrate bridge pallet has not yet been initialized.
	UninitializedBridgePallet,
	/// Account does not exist on the chain.
	AccountDoesNotExist,
	/// The client we're connected to is not synced, so we can't rely on its state.
	ClientNotSynced(Health),
	/// Custom logic error.
	Custom(String),
}

impl From<WsNewDnsError> for Error {
	fn from(error: WsNewDnsError) -> Self {
		Error::WsConnectionError(error)
	}
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
			Error::Request(RequestError::TransportError(_)) | Error::ClientNotSynced(_)
		)
	}
}

impl From<Error> for String {
	fn from(error: Error) -> String {
		error.to_string()
	}
}

impl ToString for Error {
	fn to_string(&self) -> String {
		match self {
			Self::WsConnectionError(e) => e.to_string(),
			Self::Request(e) => e.to_string(),
			Self::ResponseParseFailed(e) => e.to_string(),
			Self::UninitializedBridgePallet => "The Substrate bridge pallet has not been initialized yet.".into(),
			Self::AccountDoesNotExist => "Account does not exist on the chain".into(),
			Self::ClientNotSynced(health) => format!("Substrate client is not synced: {}", health),
			Self::Custom(e) => e.clone(),
		}
	}
}
