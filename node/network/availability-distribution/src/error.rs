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

use polkadot_node_network_protocol::request_response::request::RequestError;
use thiserror::Error;

use futures::channel::oneshot;

use polkadot_node_subsystem_util::Error as UtilError;
use polkadot_primitives::v1::SessionIndex;
use polkadot_subsystem::{errors::RuntimeApiError, SubsystemError};

use crate::LOG_TARGET;

/// Errors of this subsystem.
#[derive(Debug, Error)]
pub enum Error {
	#[error("Response channel to obtain chunk failed")]
	QueryChunkResponseChannel(#[source] oneshot::Canceled),

	#[error("Response channel to obtain available data failed")]
	QueryAvailableDataResponseChannel(#[source] oneshot::Canceled),

	#[error("Receive channel closed")]
	IncomingMessageChannel(#[source] SubsystemError),

	/// Spawning a running task failed.
	#[error("Spawning subsystem task failed")]
	SpawnTask(#[source] SubsystemError),

	/// We tried accessing a session that was not cached.
	#[error("Session is not cached.")]
	NoSuchCachedSession,

	/// We tried reporting bad validators, although we are not a validator ourselves.
	#[error("Not a validator.")]
	NotAValidator,

	/// Requester stream exhausted.
	#[error("Erasure chunk requester stream exhausted")]
	RequesterExhausted,

	/// Sending response failed.
	#[error("Sending a request's response failed.")]
	SendResponse,

	/// Some request to utility functions failed.
	/// This can be either `RuntimeRequestCanceled` or `RuntimeApiError`.
	#[error("Utility request failed")]
	UtilRequest(UtilError),

	/// Runtime API subsystem is down, which means we're shutting down.
	#[error("Runtime request canceled")]
	RuntimeRequestCanceled(oneshot::Canceled),

	/// Some request to the runtime failed.
	/// For example if we prune a block we're requesting info about.
	#[error("Runtime API error")]
	RuntimeRequest(RuntimeApiError),

	/// We tried fetching a session info which was not available.
	#[error("There was no session with the given index")]
	NoSuchSession(SessionIndex),

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
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<SubsystemError> for Error {
	fn from(err: SubsystemError) -> Self {
		Self::IncomingMessageChannel(err)
	}
}

/// Receive a response from a runtime request and convert errors.
pub(crate) async fn recv_runtime<V>(
	r: oneshot::Receiver<std::result::Result<V, RuntimeApiError>>,
) -> std::result::Result<V, Error> {
	r.await
		.map_err(Error::RuntimeRequestCanceled)?
		.map_err(Error::RuntimeRequest)
}


/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them
pub fn log_error(result: Result<()>, ctx: &'static str) {
	if let Err(error) = result {
		tracing::warn!(target: LOG_TARGET, error = ?error, ctx);
	}
}
