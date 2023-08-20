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
//

//! Error handling related code and Error/Result definitions.

use polkadot_node_network_protocol::PeerId;
use polkadot_node_subsystem::{RuntimeApiError, SubsystemError};
use polkadot_node_subsystem_util::{
	backing_implicit_view::FetchError as ImplicitViewFetchError, runtime,
};
use polkadot_primitives::{CandidateHash, Hash, Id as ParaId};

use futures::channel::oneshot;

use crate::LOG_TARGET;

/// General result.
pub type Result<T> = std::result::Result<T, Error>;
/// Result for non-fatal only failures.
pub type JfyiErrorResult<T> = std::result::Result<T, JfyiError>;
/// Result for fatal only failures.
pub type FatalResult<T> = std::result::Result<T, FatalError>;

use fatality::Nested;

#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	#[fatal]
	#[error("Requester receiver stream finished")]
	RequesterReceiverFinished,

	#[fatal]
	#[error("Responder receiver stream finished")]
	ResponderReceiverFinished,

	#[fatal]
	#[error("Spawning subsystem task failed")]
	SpawnTask(#[source] SubsystemError),

	#[fatal]
	#[error("Receiving message from overseer failed")]
	SubsystemReceive(#[source] SubsystemError),

	#[fatal(forward)]
	#[error("Error while accessing runtime information")]
	Runtime(#[from] runtime::Error),

	#[error("RuntimeAPISubsystem channel closed before receipt")]
	RuntimeApiUnavailable(#[source] oneshot::Canceled),

	#[error("Fetching persisted validation data for para {0:?}, {1:?}")]
	FetchPersistedValidationData(ParaId, RuntimeApiError),

	#[error("Fetching session index failed {0:?}")]
	FetchSessionIndex(RuntimeApiError),

	#[error("Fetching session info failed {0:?}")]
	FetchSessionInfo(RuntimeApiError),

	#[error("Fetching availability cores failed {0:?}")]
	FetchAvailabilityCores(RuntimeApiError),

	#[error("Fetching validator groups failed {0:?}")]
	FetchValidatorGroups(RuntimeApiError),

	#[error("Attempted to share statement when not a validator or not assigned")]
	InvalidShare,

	#[error("Relay parent could not be found in active heads")]
	NoSuchHead(Hash),

	#[error("Message from not connected peer")]
	NoSuchPeer(PeerId),

	#[error("Peer requested data for candidate it never received a notification for (malicious?)")]
	RequestedUnannouncedCandidate(PeerId, CandidateHash),

	// A large statement status was requested, which could not be found.
	#[error("Statement status does not exist")]
	NoSuchLargeStatementStatus(Hash, CandidateHash),

	// A fetched large statement was requested, but could not be found.
	#[error("Fetched large statement does not exist")]
	NoSuchFetchedLargeStatement(Hash, CandidateHash),

	// Responder no longer waits for our data. (Should not happen right now.)
	#[error("Oneshot `GetData` channel closed")]
	ResponderGetDataCanceled,

	// Failed to activate leaf due to a fetch error.
	#[error("Implicit view failure while activating leaf")]
	ActivateLeafFailure(ImplicitViewFetchError),
}

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them.
pub fn log_error(
	result: Result<()>,
	ctx: &'static str,
	warn_freq: &mut gum::Freq,
) -> std::result::Result<(), FatalError> {
	match result.into_nested()? {
		Err(jfyi) => {
			match jfyi {
				JfyiError::RequestedUnannouncedCandidate(_, _) =>
					gum::warn!(target: LOG_TARGET, error = %jfyi, ctx),
				_ =>
					gum::warn_if_frequent!(freq: warn_freq, max_rate: gum::Times::PerHour(100), target: LOG_TARGET, error = %jfyi, ctx),
			}
			Ok(())
		},
		Ok(()) => Ok(()),
	}
}
