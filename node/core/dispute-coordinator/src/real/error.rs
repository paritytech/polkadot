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

use futures::channel::oneshot;
use thiserror::Error;

use polkadot_node_subsystem::{
	errors::{ChainApiError, RuntimeApiError}, SubsystemError,
};
use polkadot_node_subsystem_util::runtime;

use crate::real::CodecError;
use crate::real::LOG_TARGET;
use super::db;

/// Errors for this subsystem.
#[derive(Debug, Error, derive_more::From)]
#[error(transparent)]
pub enum Error {
	/// All fatal errors.
	Fatal(Fatal),
	/// All nonfatal/potentially recoverable errors.
	NonFatal(NonFatal),
}

/// General `Result` type for dispute coordinator.
pub type Result<R> = std::result::Result<R, Error>;
/// Result type with only fatal errors.
pub type FatalResult<R> = std::result::Result<R, Fatal>;

impl From<runtime::Error> for Error {
	fn from(o: runtime::Error) -> Self {
		match o {
			runtime::Error::Fatal(f) => Self::Fatal(Fatal::Runtime(f)),
			runtime::Error::NonFatal(f) => Self::NonFatal(NonFatal::Runtime(f)),
		}
	}
}

impl From<SubsystemError> for Error {
	fn from(o: SubsystemError) -> Self {
		match o {
			SubsystemError::Context(msg) => Self::Fatal(Fatal::SubsystemContext(msg)),
			_ => Self::NonFatal(NonFatal::Subsystem(o)),
		}
	}
}


/// Fatal errors of this subsystem.
#[derive(Debug, Error)]
pub enum Fatal {
	/// Errors coming from runtime::Runtime.
	#[error("Error while accessing runtime information {0}")]
	Runtime(#[from] runtime::Fatal),

	/// We received a legacy SubystemError::Context error which is considered fatal.
	#[error("SubsystemError::Context error: {0}")]
	SubsystemContext(String)
}

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum NonFatal {
	#[error(transparent)]
	RuntimeApi(#[from] RuntimeApiError),

	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Io(#[from] std::io::Error),

	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),

	#[error("Dispute import confirmation send failed.")]
	DisputeImportOneshotSend,

	#[error(transparent)]
	Subsystem(SubsystemError),

	#[error(transparent)]
	Codec(#[from] CodecError),

	/// Errors coming from runtime::Runtime.
	#[error("Error while accessing runtime information: {0}")]
	Runtime(#[from] runtime::NonFatal),
}

impl From<db::v1::Error> for Error {
	fn from(err: db::v1::Error) -> Self {
		match err {
			db::v1::Error::Io(io) => Self::NonFatal(NonFatal::Io(io)),
			db::v1::Error::Codec(e) => Self::NonFatal(NonFatal::Codec(e)),
		}
	}
}

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them
pub fn log_error(result: Result<()>) -> std::result::Result<(), Fatal> {
	match result {
		Err(Error::Fatal(f)) => Err(f),
		Err(Error::NonFatal(error)) => {
			match error {
				// don't spam the log with spurious errors
				NonFatal::RuntimeApi(_) | NonFatal::Oneshot(_) =>
					tracing::debug!(target: LOG_TARGET, ?error),
				// it's worth reporting otherwise
				_ => tracing::warn!(target: LOG_TARGET, ?error),
			}
			Ok(())
		},
		Ok(()) => Ok(()),
	}
}
