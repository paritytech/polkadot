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

use fatality::Split;

use polkadot_node_network_protocol::{request_response::incoming, PeerId};
use polkadot_node_subsystem_util::runtime;

use crate::LOG_TARGET;

#[derive(Debug, thiserror::Error, derive_more::From)]
#[error(transparent)]
pub enum Error {
	/// All fatal errors.
	Fatal(Fatal),
	/// All nonfatal/potentially recoverable errors.
	NonFatal(NonFatal),
}

impl From<runtime::Error> for Error {
	fn from(src: runtime::Error) -> Self {
		match src.split() {
			Ok(jfyi) => Self::NonFatal(NonFatal::Runtime(jfyi)),
			Err(fatal) => Self::Fatal(Fatal::Runtime(fatal)),
		}
	}
}

impl From<incoming::Error> for Error {
	fn from(err: incoming::Error) -> Self {
		match err.split() {
			Err(fatal) => Self::Fatal(Fatal::IncomingRequest(fatal)),
			Ok(jfyi) => Self::NonFatal(NonFatal::IncomingRequest(jfyi)),
		}
	}
}

/// Fatal errors of this subsystem.
#[derive(Debug, thiserror::Error)]
pub enum Fatal {
	/// Errors coming from runtime::Runtime.
	#[error("Error while accessing runtime information")]
	Runtime(#[from] runtime::FatalError),

	/// Errors coming from receiving incoming requests.
	#[error("Retrieving next incoming request failed.")]
	IncomingRequest(#[from] incoming::FatalError),
}

/// Non-fatal errors of this subsystem.
#[derive(Debug, thiserror::Error)]
pub enum NonFatal {
	/// Answering request failed.
	#[error("Sending back response to peer {0} failed.")]
	SendResponse(PeerId),

	/// Setting reputation for peer failed.
	#[error("Changing peer's ({0}) reputation failed.")]
	SetPeerReputation(PeerId),

	/// Peer sent us request with invalid signature.
	#[error("Dispute request with invalid signatures, from peer {0}.")]
	InvalidSignature(PeerId),

	/// Import oneshot got canceled.
	#[error("Import of dispute got canceled for peer {0} - import failed for some reason.")]
	ImportCanceled(PeerId),

	/// Non validator tried to participate in dispute.
	#[error("Peer {0} is not a validator.")]
	NotAValidator(PeerId),

	/// Errors coming from runtime::Runtime.
	#[error("Error while accessing runtime information")]
	Runtime(#[from] runtime::JfyiError),

	/// Errors coming from receiving incoming requests.
	#[error("Retrieving next incoming request failed.")]
	IncomingRequest(#[from] incoming::JfyiError),
}

pub type Result<T> = std::result::Result<T, Error>;

pub type NonFatalResult<T> = std::result::Result<T, NonFatal>;

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them.
pub fn log_error(result: Result<()>) -> std::result::Result<(), Fatal> {
	match result {
		Err(Error::Fatal(f)) => Err(f),
		Err(Error::NonFatal(error @ NonFatal::ImportCanceled(_))) => {
			tracing::debug!(target: LOG_TARGET, error = ?error);
			Ok(())
		},
		Err(Error::NonFatal(error)) => {
			tracing::warn!(target: LOG_TARGET, error = ?error);
			Ok(())
		},
		Ok(()) => Ok(()),
	}
}
