// Copyright (C) Parity Technologies (UK) Ltd.
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
use futures::channel::{mpsc, oneshot};

use polkadot_node_subsystem::{
	messages::{StoreAvailableDataError, ValidationFailed},
	RuntimeApiError, SubsystemError,
};
use polkadot_node_subsystem_util::{runtime, Error as UtilError};
use polkadot_primitives::{BackedCandidate, ValidationCodeHash};

use crate::LOG_TARGET;

pub type Result<T> = std::result::Result<T, Error>;
pub type FatalResult<T> = std::result::Result<T, FatalError>;

/// Errors that can occur in candidate backing.
#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	#[fatal]
	#[error("Failed to spawn background task")]
	FailedToSpawnBackgroundTask,

	#[fatal(forward)]
	#[error("Error while accessing runtime information")]
	Runtime(#[from] runtime::Error),

	#[fatal]
	#[error(transparent)]
	BackgroundValidationMpsc(#[from] mpsc::SendError),

	#[error("Candidate is not found")]
	CandidateNotFound,

	#[error("Signature is invalid")]
	InvalidSignature,

	#[error("Failed to send candidates {0:?}")]
	Send(Vec<BackedCandidate>),

	#[error("FetchPoV failed")]
	FetchPoV,

	#[error("Fetching validation code by hash failed {0:?}, {1:?}")]
	FetchValidationCode(ValidationCodeHash, RuntimeApiError),

	#[error("Fetching Runtime API version failed {0:?}")]
	FetchRuntimeApiVersion(RuntimeApiError),

	#[error("No validation code {0:?}")]
	NoValidationCode(ValidationCodeHash),

	#[error("Candidate rejected by prospective parachains subsystem")]
	RejectedByProspectiveParachains,

	#[error("ValidateFromExhaustive channel closed before receipt")]
	ValidateFromExhaustive(#[source] oneshot::Canceled),

	#[error("StoreAvailableData channel closed before receipt")]
	StoreAvailableDataChannel(#[source] oneshot::Canceled),

	#[error("RuntimeAPISubsystem channel closed before receipt")]
	RuntimeApiUnavailable(#[source] oneshot::Canceled),

	#[error("a channel was closed before receipt in try_join!")]
	JoinMultiple(#[source] oneshot::Canceled),

	#[error("Obtaining erasure chunks failed")]
	ObtainErasureChunks(#[from] erasure_coding::Error),

	#[error(transparent)]
	ValidationFailed(#[from] ValidationFailed),

	#[error(transparent)]
	UtilError(#[from] UtilError),

	#[error(transparent)]
	SubsystemError(#[from] SubsystemError),

	#[fatal]
	#[error(transparent)]
	OverseerExited(SubsystemError),

	#[error("Availability store error")]
	StoreAvailableData(#[source] StoreAvailableDataError),
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
		gum::debug!(target: LOG_TARGET, error = ?self);
	}
}
