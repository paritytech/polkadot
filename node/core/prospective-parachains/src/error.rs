// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Error types.

use futures::channel::oneshot;
use thiserror::Error;

use polkadot_node_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	SubsystemError,
};
use polkadot_node_subsystem_util::{rolling_session_window::SessionsUnavailable, runtime};

use crate::LOG_TARGET;
use fatality::Nested;
use parity_scale_codec::Error as CodecError;

#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	#[fatal]
	#[error("SubsystemError::Context error: {0}")]
	SubsystemContext(String),

	#[fatal]
	#[error("Spawning a task failed: {0}")]
	SpawnFailed(SubsystemError),

	#[fatal]
	#[error("Participation worker receiver exhausted.")]
	ParticipationWorkerReceiverExhausted,

	#[fatal]
	#[error("Receiving message from overseer failed: {0}")]
	SubsystemReceive(#[source] SubsystemError),

	#[error(transparent)]
	RuntimeApi(#[from] RuntimeApiError),

	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Subsystem(SubsystemError),

	#[error("Request to chain API subsystem dropped")]
	ChainApiRequestCanceled(oneshot::Canceled),
}

/// General `Result` type.
pub type Result<R> = std::result::Result<R, Error>;
/// Result for non-fatal only failures.
pub type JfyiErrorResult<T> = std::result::Result<T, JfyiError>;
/// Result for fatal only failures.
pub type FatalResult<T> = std::result::Result<T, FatalError>;

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them
pub fn log_error(result: Result<()>, ctx: &'static str) -> std::result::Result<(), FatalError> {
	match result.into_nested()? {
		Ok(()) => Ok(()),
		Err(jfyi) => {
			tracing::debug!(target: LOG_TARGET, error = ?jfyi, ctx);
			Ok(())
		},
	}
}
