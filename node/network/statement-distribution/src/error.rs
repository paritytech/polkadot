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

use polkadot_node_network_protocol::PeerId;
use polkadot_primitives::v1::{CandidateHash, Hash};
use polkadot_subsystem::SubsystemError;
use thiserror::Error;

use polkadot_node_subsystem_util::{Fault, runtime, unwrap_non_fatal};

use crate::LOG_TARGET;

/// General result.
pub type Result<T> = std::result::Result<T, Error>;
/// Result for non fatal only failures.
pub type NonFatalResult<T> = std::result::Result<T, NonFatal>;
/// Result for fatal only failures.
pub type FatalResult<T> = std::result::Result<T, Fatal>;

/// Errors for statement distribution.
#[derive(Debug, Error)]
#[error(transparent)]
pub struct Error(pub Fault<NonFatal, Fatal>);

impl From<NonFatal> for Error {
	fn from(e: NonFatal) -> Self {
		Self(Fault::from_non_fatal(e))
	}
}

impl From<Fatal> for Error {
	fn from(f: Fatal) -> Self {
		Self(Fault::from_fatal(f))
	}
}

impl From<runtime::Error> for Error {
	fn from(o: runtime::Error) -> Self {
		Self(Fault::from_other(o))
	}
}

/// Fatal runtime errors.
#[derive(Debug, Error)]
pub enum Fatal {
	/// Requester channel is never closed.
	#[error("Requester receiver stream finished.")]
	RequesterReceiverFinished,

	/// Responder channel is never closed.
	#[error("Responder receiver stream finished.")]
	ResponderReceiverFinished,

	/// Spawning a running task failed.
	#[error("Spawning subsystem task failed")]
	SpawnTask(#[source] SubsystemError),

	/// Receiving subsystem message from overseer failed.
	#[error("Receiving message from overseer failed")]
	SubsystemReceive(#[source] SubsystemError),

	/// Errors coming from runtime::Runtime.
	#[error("Error while accessing runtime information")]
	Runtime(#[from] #[source] runtime::Fatal),
}

/// Errors for fetching of runtime information.
#[derive(Debug, Error)]
pub enum NonFatal {
	/// Errors coming from runtime::Runtime.
	#[error("Error while accessing runtime information")]
	Runtime(#[from] #[source] runtime::NonFatal),

	/// Relay parent was not present in active heads.
	#[error("Relay parent could not be found in active heads")]
	NoSuchHead(Hash),

	/// Peer requested statement data for candidate that was never announced to it.
	#[error("Peer requested data for candidate it never received a notification for")]
	RequestedUnannouncedCandidate(PeerId, CandidateHash),

	/// A large statement status was requested, which could not be found.
	#[error("Statement status does not exist")]
	NoSuchLargeStatementStatus(Hash, CandidateHash),

	/// A fetched large statement was requested, but could not be found.
	#[error("Fetched large statement does not exist")]
	NoSuchFetchedLargeStatement(Hash, CandidateHash),

	/// Responder no longer waits for our data. (Should not happen right now.)
	#[error("Oneshot `GetData` channel closed")]
	ResponderGetDataCanceled,
}

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them.
pub fn log_error(result: Result<()>, ctx: &'static str)
	-> FatalResult<()>
{
	if let Some(error) = unwrap_non_fatal(result.map_err(|e| e.0))? {
		tracing::debug!(target: LOG_TARGET, error = ?error, ctx)
	}
	Ok(())
}
