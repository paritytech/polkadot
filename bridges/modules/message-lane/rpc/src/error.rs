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

//! Possible errors and results of message-lane RPC calls.

/// Future Result type.
pub type FutureResult<T> = jsonrpc_core::BoxFuture<T>;

/// State RPC errors.
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// When unknown instance id is passed.
	#[display(fmt = "Message lane instance is unknown")]
	UnknownInstance,
	/// Client error.
	#[display(fmt = "Client error: {}", _0)]
	Client(Box<dyn std::error::Error + Send>),
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Error::UnknownInstance => None,
			Error::Client(ref err) => Some(&**err),
		}
	}
}

impl From<Error> for jsonrpc_core::Error {
	fn from(e: Error) -> Self {
		const UNKNOW_INSTANCE_CODE: i64 = 1;

		match e {
			Error::UnknownInstance => jsonrpc_core::Error {
				code: jsonrpc_core::ErrorCode::ServerError(UNKNOW_INSTANCE_CODE),
				message: "Unknown instance passed".into(),
				data: None,
			},
			Error::Client(e) => jsonrpc_core::Error {
				code: jsonrpc_core::ErrorCode::InternalError,
				message: format!("Unknown error occured: {}", e),
				data: Some(format!("{:?}", e).into()),
			},
		}
	}
}
