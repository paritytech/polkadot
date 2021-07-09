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

use polkadot_node_subsystem_util::{Fault, runtime, unwrap_non_fatal};
use polkadot_subsystem::SubsystemError;

use crate::LOG_TARGET;
use crate::sender;

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

impl From<sender::Error> for Error {
	fn from(e: sender::Error) -> Self {
		match e.0 {
			Fault::Fatal(f) => Self(Fault::Fatal(Fatal::Sender(f))),
			Fault::Err(nf) => Self(Fault::Err(NonFatal::Sender(nf))),
		}
	}
}

/// Fatal errors of this subsystem.
#[derive(Debug, Error)]
pub enum Fatal {

	/// Receiving subsystem message from overseer failed.
	#[error("Receiving message from overseer failed")]
	SubsystemReceive(#[source] SubsystemError),

	/// Spawning a running task failed.
	#[error("Spawning subsystem task failed")]
	SpawnTask(#[source] SubsystemError),

	/// DisputeSender mpsc receiver exhausted.
	#[error("Erasure chunk requester stream exhausted")]
	SenderExhausted,

	/// Errors coming from runtime::Runtime.
	#[error("Error while accessing runtime information")]
	Runtime(#[from] runtime::Fatal),

	/// Errors coming from DisputeSender
	#[error("Error while accessing runtime information")]
	Sender(#[from] sender::Fatal),
}

/// Non-fatal errors of this subsystem.
#[derive(Debug, Error)]
pub enum NonFatal {
	/// Errors coming from DisputeSender
	#[error("Error while accessing runtime information")]
	Sender(#[from] sender::NonFatal),
}

pub type Result<T> = std::result::Result<T, Error>;

pub type FatalResult<T> = std::result::Result<T, Fatal>;

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them
pub fn log_error(result: Result<()>, ctx: &'static str)
	-> std::result::Result<(), Fatal>
{
	if let Some(error) = unwrap_non_fatal(result.map_err(|e| e.0))? {
		tracing::warn!(target: LOG_TARGET, error = ?error, ctx);
	}
	Ok(())
}
