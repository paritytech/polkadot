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

use fatality::Nested;
use polkadot_node_network_protocol::request_response::outgoing::RequestError;
use polkadot_primitives::v2::SessionIndex;

use futures::channel::oneshot;

use polkadot_node_subsystem::{ChainApiError, SubsystemError};
use polkadot_node_subsystem_util::runtime;

use crate::LOG_TARGET;

#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	#[fatal]
	#[error("Spawning subsystem task failed: {0}")]
	SpawnTask(#[source] SubsystemError),

	#[fatal]
	#[error("Erasure chunk requester stream exhausted")]
	RequesterExhausted,

	#[fatal]
	#[error("Receive channel closed: {0}")]
	IncomingMessageChannel(#[source] SubsystemError),

	#[fatal(forward)]
	#[error("Error while accessing runtime information: {0}")]
	Runtime(#[from] runtime::Error),

	#[fatal]
	#[error("Oneshot for receiving response from Chain API got cancelled")]
	ChainApiSenderDropped(#[source] oneshot::Canceled),

	#[fatal]
	#[error("Retrieving response from Chain API unexpectedly failed with error: {0}")]
	ChainApi(#[from] ChainApiError),

	// av-store will drop the sender on any error that happens.
	#[error("Response channel to obtain chunk failed")]
	QueryChunkResponseChannel(#[source] oneshot::Canceled),

	// av-store will drop the sender on any error that happens.
	#[error("Response channel to obtain available data failed")]
	QueryAvailableDataResponseChannel(#[source] oneshot::Canceled),

	// We tried accessing a session that was not cached.
	#[error("Session {missing_session} is not cached, cached sessions: {available_sessions:?}.")]
	NoSuchCachedSession { available_sessions: Vec<SessionIndex>, missing_session: SessionIndex },

	// Sending request response failed (Can happen on timeouts for example).
	#[error("Sending a request's response failed.")]
	SendResponse,

	#[error("FetchPoV request error: {0}")]
	FetchPoV(#[source] RequestError),

	#[error("Fetched PoV does not match expected hash")]
	UnexpectedPoV,

	#[error("Remote responded with `NoSuchPoV`")]
	NoSuchPoV,

	#[error("Given validator index could not be found in current session")]
	InvalidValidatorIndex,
}

/// General result abbreviation type alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them
pub fn log_error(result: Result<()>, ctx: &'static str) -> std::result::Result<(), FatalError> {
	match result.into_nested()? {
		Ok(()) => Ok(()),
		Err(jfyi) => {
			match jfyi {
				JfyiError::UnexpectedPoV |
				JfyiError::InvalidValidatorIndex |
				JfyiError::NoSuchCachedSession { .. } |
				JfyiError::QueryAvailableDataResponseChannel(_) |
				JfyiError::QueryChunkResponseChannel(_) => gum::warn!(target: LOG_TARGET, error = %jfyi, ctx),
				JfyiError::FetchPoV(_) |
				JfyiError::SendResponse |
				JfyiError::NoSuchPoV |
				JfyiError::Runtime(_) => gum::debug!(target: LOG_TARGET, error = ?jfyi, ctx),
			}
			Ok(())
		},
	}
}
