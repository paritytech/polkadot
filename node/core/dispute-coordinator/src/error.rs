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

use fatality::Nested;
use futures::channel::oneshot;

use polkadot_node_subsystem::{errors::ChainApiError, SubsystemError};
use polkadot_node_subsystem_util::{rolling_session_window::SessionsUnavailable, runtime};

use crate::{db, participation, LOG_TARGET};
use parity_scale_codec::Error as CodecError;

pub type Result<T> = std::result::Result<T, Error>;
pub type FatalResult<T> = std::result::Result<T, FatalError>;
pub type JfyiResult<T> = std::result::Result<T, JfyiError>;

#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	/// We received a legacy `SubystemError::Context` error which is considered fatal.
	#[fatal]
	#[error("SubsystemError::Context error: {0}")]
	SubsystemContext(String),

	/// `ctx.spawn` failed with an error.
	#[fatal]
	#[error("Spawning a task failed: {0}")]
	SpawnFailed(#[source] SubsystemError),

	#[fatal]
	#[error("Participation worker receiver exhausted.")]
	ParticipationWorkerReceiverExhausted,

	/// Receiving subsystem message from overseer failed.
	#[fatal]
	#[error("Receiving message from overseer failed: {0}")]
	SubsystemReceive(#[source] SubsystemError),

	#[fatal]
	#[error("Writing to database failed: {0}")]
	DbWriteFailed(std::io::Error),

	#[fatal]
	#[error("Reading from database failed: {0}")]
	DbReadFailed(db::v1::Error),

	#[fatal]
	#[error("Oneshot for receiving block number from chain API got cancelled")]
	CanceledBlockNumber,

	#[fatal]
	#[error("Retrieving block number from chain API failed with error: {0}")]
	ChainApiBlockNumber(ChainApiError),

	#[fatal]
	#[error(transparent)]
	ChainApiAncestors(ChainApiError),

	#[fatal]
	#[error("Chain API dropped response channel sender")]
	ChainApiSenderDropped,

	#[fatal(forward)]
	#[error("Error while accessing runtime information {0}")]
	Runtime(#[from] runtime::Error),

	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Io(#[from] std::io::Error),

	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),

	#[error("Could not send import confirmation (receiver canceled)")]
	DisputeImportOneshotSend,

	#[error(transparent)]
	Subsystem(#[from] SubsystemError),

	#[error(transparent)]
	Codec(#[from] CodecError),

	/// `RollingSessionWindow` was not able to retrieve `SessionInfo`s.
	#[error("Sessions unavailable in `RollingSessionWindow`: {0}")]
	RollingSessionWindow(#[from] SessionsUnavailable),

	#[error(transparent)]
	QueueError(#[from] participation::QueueError),
}

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them
pub fn log_error(result: Result<()>) -> std::result::Result<(), FatalError> {
	match result.into_nested()? {
		Ok(()) => Ok(()),
		Err(jfyi) => {
			jfyi.log();
			Ok(())
		},
	}
}

impl JfyiError {
	/// Log a `JfyiError`.
	pub fn log(self) {
		match self {
			// don't spam the log with spurious errors
			Self::Runtime(runtime::Error::RuntimeRequestCanceled(_)) | Self::Oneshot(_) => {
				gum::debug!(target: LOG_TARGET, error = ?self)
			},
			// it's worth reporting otherwise
			_ => gum::warn!(target: LOG_TARGET, error = ?self),
		}
	}
}
