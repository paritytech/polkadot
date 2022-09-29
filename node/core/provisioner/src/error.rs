// Copyright 2017-2022 Parity Technologies (UK) Ltd.
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

///! Error types for provisioner module
use fatality::Nested;
use futures::channel::{mpsc, oneshot};
use polkadot_node_subsystem::errors::{ChainApiError, RuntimeApiError, SubsystemError};
use polkadot_node_subsystem_util as util;
use polkadot_primitives::v2::Hash;

pub type FatalResult<T> = std::result::Result<T, FatalError>;
pub type Result<T> = std::result::Result<T, Error>;

/// Errors in the provisioner.
#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	#[error(transparent)]
	Util(#[from] util::Error),

	#[error("failed to get availability cores")]
	CanceledAvailabilityCores(#[source] oneshot::Canceled),

	#[error("failed to get persisted validation data")]
	CanceledPersistedValidationData(#[source] oneshot::Canceled),

	#[error("failed to get block number")]
	CanceledBlockNumber(#[source] oneshot::Canceled),

	#[error("failed to get backed candidates")]
	CanceledBackedCandidates(#[source] oneshot::Canceled),

	#[error("failed to get votes on dispute")]
	CanceledCandidateVotes(#[source] oneshot::Canceled),

	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Runtime(#[from] RuntimeApiError),

	#[error("failed to send message to ChainAPI")]
	ChainApiMessageSend(#[source] mpsc::SendError),

	#[error("failed to send message to CandidateBacking to get backed candidates")]
	GetBackedCandidatesSend(#[source] mpsc::SendError),

	#[error("Send inherent data timeout.")]
	SendInherentDataTimeout,

	#[error("failed to send return message with Inherents")]
	InherentDataReturnChannel,

	#[error(
		"backed candidate does not correspond to selected candidate; check logic in provisioner"
	)]
	BackedCandidateOrderingProblem,

	#[fatal]
	#[error("Failed to spawn background task")]
	FailedToSpawnBackgroundTask,

	#[error(transparent)]
	SubsystemError(#[from] SubsystemError),
}

/// Used by `get_onchain_disputes` to represent errors related to fetching on-chain disputes from the Runtime
#[allow(dead_code)] // Remove when promoting to stable
#[fatality::fatality]
pub enum GetOnchainDisputesError {
	#[fatal]
	#[error("runtime subsystem is down")]
	Channel,

	#[error("runtime execution error occurred while fetching onchain disputes for parent {1}")]
	Execution(#[source] RuntimeApiError, Hash),

	#[error("runtime doesn't support RuntimeApiRequest::Disputes for parent {1}")]
	NotSupported(#[source] RuntimeApiError, Hash),
}

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
		gum::debug!(target: super::LOG_TARGET, error = ?self);
	}
}
