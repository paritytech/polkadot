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

use thiserror::Error;

use futures::channel::oneshot;

use polkadot_node_subsystem_util::Error as UtilError;
use polkadot_primitives::v1::SessionIndex;
use polkadot_subsystem::{errors::RuntimeApiError, SubsystemError};

/// Errors of this subsystem.
#[derive(Debug, Error)]
pub enum Error {
	#[error("Response channel to obtain QueryChunk failed")]
	QueryChunkResponseChannel(#[source] oneshot::Canceled),

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
}

/// Error that we should handle gracefully by logging it.
#[derive(Debug)]
pub enum NonFatalError {
	/// Some request to utility functions failed.
	/// This can be either `RuntimeRequestCanceled` or `RuntimeApiError`.
	UtilRequest(UtilError),

	/// Runtime API subsystem is down, which means we're shutting down.
	RuntimeRequestCanceled(oneshot::Canceled),

	/// Some request to the runtime failed.
	/// For example if we prune a block we're requesting info about.
	RuntimeRequest(RuntimeApiError),

	/// We tried fetching a session info which was not available.
	NoSuchSession(SessionIndex),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<SubsystemError> for Error {
	fn from(err: SubsystemError) -> Self {
		Self::IncomingMessageChannel(err)
	}
}

/// Receive a response from a runtime request and convert errors.
pub(crate) async fn recv_runtime<V>(
	r: std::result::Result<
		oneshot::Receiver<std::result::Result<V, RuntimeApiError>>,
		UtilError,
	>,
) -> std::result::Result<V, NonFatalError> {
	r.map_err(NonFatalError::UtilRequest)?
		.await
		.map_err(NonFatalError::RuntimeRequestCanceled)?
		.map_err(NonFatalError::RuntimeRequest)
}
