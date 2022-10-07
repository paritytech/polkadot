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

use polkadot_node_network_protocol::request_response::incoming;
use polkadot_node_primitives::UncheckedSignedFullStatement;
use polkadot_node_subsystem::errors::SubsystemError;
use polkadot_node_subsystem_util::runtime;

use crate::LOG_TARGET;

/// General result.
pub type Result<T> = std::result::Result<T, Error>;

use fatality::Nested;

#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	#[fatal]
	#[error("Receiving message from overseer failed")]
	SubsystemReceive(#[from] SubsystemError),

	#[fatal(forward)]
	#[error("Retrieving next incoming request failed")]
	IncomingRequest(#[from] incoming::Error),

	#[fatal(forward)]
	#[error("Error while accessing runtime information")]
	Runtime(#[from] runtime::Error),

	#[error("CollationSeconded contained statement with invalid signature")]
	InvalidStatementSignature(UncheckedSignedFullStatement),
}

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them.
pub fn log_error(result: Result<()>, ctx: &'static str) -> std::result::Result<(), FatalError> {
	match result.into_nested()? {
		Ok(()) => Ok(()),
		Err(jfyi) => {
			gum::warn!(target: LOG_TARGET, error = ?jfyi, ctx);
			Ok(())
		},
	}
}
