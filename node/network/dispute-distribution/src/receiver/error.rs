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

use polkadot_node_network_protocol::PeerId;
use polkadot_node_network_protocol::request_response::request::ReceiveError;
use polkadot_node_subsystem_util::{Fault, runtime, unwrap_non_fatal};

use crate::LOG_TARGET;

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

/// Fatal errors of this subsystem.
#[derive(Debug, Error)]
pub enum Fatal {
	/// Request channel returned `None`. Likely a system shutdown.
	#[error("Request channel stream finished.")]
	RequestChannelFinished,

	/// Errors coming from runtime::Runtime.
	#[error("Error while accessing runtime information")]
	Runtime(#[from] runtime::Fatal),
}

/// Non-fatal errors of this subsystem.
#[derive(Debug, Error)]
pub enum NonFatal {
	/// Answering request failed.
	#[error("Sending back response to peer {0} failed.")]
	SendResponse(PeerId),

	/// Getting request from raw request failed.
	#[error("Decoding request failed.")]
	FromRawRequest(#[source] ReceiveError),

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
	Runtime(#[from] runtime::NonFatal),
}

pub type Result<T> = std::result::Result<T, Error>;

pub type FatalResult<T> = std::result::Result<T, Fatal>;
pub type NonFatalResult<T> = std::result::Result<T, NonFatal>;

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them
pub fn log_error(result: Result<()>)
	-> std::result::Result<(), Fatal>
{
	if let Some(error) = unwrap_non_fatal(result.map_err(|e| e.0))? {
		tracing::warn!(target: LOG_TARGET, error = ?error);
	}
	Ok(())
}
