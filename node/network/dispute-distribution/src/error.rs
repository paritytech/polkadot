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

use polkadot_node_subsystem::SubsystemError;
use polkadot_node_subsystem_util::runtime;

use crate::{sender, LOG_TARGET};

use fatality::Nested;

#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	/// Receiving subsystem message from overseer failed.
	#[fatal]
	#[error("Receiving message from overseer failed")]
	SubsystemReceive(#[source] SubsystemError),

	/// Spawning a running task failed.
	#[fatal]
	#[error("Spawning subsystem task failed")]
	SpawnTask(#[source] SubsystemError),

	/// `DisputeSender` mpsc receiver exhausted.
	#[fatal]
	#[error("Erasure chunk requester stream exhausted")]
	SenderExhausted,

	/// Errors coming from `runtime::Runtime`.
	#[fatal(forward)]
	#[error("Error while accessing runtime information")]
	Runtime(#[from] runtime::Error),

	/// Errors coming from `DisputeSender`
	#[fatal(forward)]
	#[error("Error while accessing runtime information")]
	Sender(#[from] sender::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub type FatalResult<T> = std::result::Result<T, FatalError>;

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them
pub fn log_error(result: Result<()>, ctx: &'static str) -> std::result::Result<(), FatalError> {
	match result.into_nested()? {
		Err(jfyi) => {
			gum::warn!(target: LOG_TARGET, error = ?jfyi, ctx);
			Ok(())
		},
		Ok(()) => Ok(()),
	}
}
