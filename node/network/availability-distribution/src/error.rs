// Copyright 2021 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.
//

//! Error handling related code and Error/Result definitions.

use polkadot_node_network_protocol::request_response::outgoing::RequestError;
use thiserror::Error;

use futures::channel::oneshot;

use polkadot_node_subsystem_util::runtime;
use polkadot_subsystem::SubsystemError;

use crate::LOG_TARGET;

#[derive(Debug, Error, derive_more::From)]
#[error(transparent)]
pub enum Error {
	/// All fatal errors.
	Fatal(Fatal),
	/// All nonfatal/potentially recoverable errors.
	NonFatal(NonFatal),
}

impl From<runtime::Error> for Error {
	fn from(o: runtime::Error) -> Self {
		match o {
			runtime::Error::Fatal(f) => Self::Fatal(Fatal::Runtime(f)),
			runtime::Error::NonFatal(f) => Self::NonFatal(NonFatal::Runtime(f)),
		}
	}
}

/// Fatal errors of this subsystem.
#[derive(Debug, Error)]
pub enum Fatal {
	/// Spawning a running task failed.
	#[error("Spawning subsystem task failed")]
	SpawnTask(#[source] SubsystemError),

	/// Requester stream exhausted.
	#[error("Erasure chunk requester stream exhausted")]
	RequesterExhausted,

	#[error("Receive channel closed")]
	IncomingMessageChannel(#[source] SubsystemError),

	/// Errors coming from runtime::Runtime.
	#[error("Error while accessing runtime information")]
	Runtime(#[from] runtime::Fatal),
}

/// Non-fatal errors of this subsystem.
#[derive(Debug, Error)]
pub enum NonFatal {
	/// av-store will drop the sender on any error that happens.
	#[error("Response channel to obtain chunk failed")]
	QueryChunkResponseChannel(#[source] oneshot::Canceled),

	/// av-store will drop the sender on any error that happens.
	#[error("Response channel to obtain available data failed")]
	QueryAvailableDataResponseChannel(#[source] oneshot::Canceled),

	/// We tried accessing a session that was not cached.
	#[error("Session is not cached.")]
	NoSuchCachedSession,

	/// Sending request response failed (Can happen on timeouts for example).
	#[error("Sending a request's response failed.")]
	SendResponse,

	/// Fetching PoV failed with `RequestError`.
	#[error("FetchPoV request error")]
	FetchPoV(#[source] RequestError),

	/// Fetching PoV failed as the received PoV did not match the expected hash.
	#[error("Fetched PoV does not match expected hash")]
	UnexpectedPoV,

	#[error("Remote responded with `NoSuchPoV`")]
	NoSuchPoV,

	/// No validator with the index could be found in current session.
	#[error("Given validator index could not be found")]
	InvalidValidatorIndex,

	/// Errors coming from runtime::Runtime.
	#[error("Error while accessing runtime information")]
	Runtime(#[from] runtime::NonFatal),
}

/// General result type for fatal/nonfatal errors.
pub type Result<T> = std::result::Result<T, Error>;

/// Results which are never fatal.
pub type NonFatalResult<T> = std::result::Result<T, NonFatal>;

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them
pub fn log_error(result: Result<()>, ctx: &'static str) -> std::result::Result<(), Fatal> {
	match result {
		Err(Error::Fatal(f)) => Err(f),
		Err(Error::NonFatal(error)) => {
			tracing::warn!(target: LOG_TARGET, error = ?error, ctx);
			Ok(())
		},
		Ok(()) => Ok(()),
	}
}
