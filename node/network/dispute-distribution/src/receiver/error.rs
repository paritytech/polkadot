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

use fatality::Nested;

use gum::CandidateHash;
use polkadot_node_network_protocol::{request_response::incoming, PeerId};
use polkadot_node_subsystem_util::runtime;
use polkadot_primitives::AuthorityDiscoveryId;

use crate::LOG_TARGET;

#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	#[fatal(forward)]
	#[error("Error while accessing runtime information")]
	Runtime(#[from] runtime::Error),

	#[fatal(forward)]
	#[error("Retrieving next incoming request failed.")]
	IncomingRequest(#[from] incoming::Error),

	#[error("Sending back response to peers {0:#?} failed.")]
	SendResponses(Vec<PeerId>),

	#[error("Changing peer's ({0}) reputation failed.")]
	SetPeerReputation(PeerId),

	#[error("Dispute request with invalid signatures, from peer {0}.")]
	InvalidSignature(PeerId),

	#[error("Received votes from peer {0} have been completely redundant.")]
	RedundantMessage(PeerId),

	#[error("Import of dispute got canceled for candidate {0} - import failed for some reason.")]
	ImportCanceled(CandidateHash),

	#[error("Peer {0} attempted to participate in dispute and is not a validator.")]
	NotAValidator(PeerId),

	#[error("Force flush for batch that could not be found attempted, candidate hash: {0}")]
	ForceFlushBatchDoesNotExist(CandidateHash),

	// Should never happen in practice:
	#[error("We needed to drop messages, because we reached limit on concurrent batches.")]
	MaxBatchLimitReached,

	#[error("Authority {0} sent messages at a too high rate.")]
	AuthorityFlooding(AuthorityDiscoveryId),
}

pub type Result<T> = std::result::Result<T, Error>;

pub type JfyiResult<T> = std::result::Result<T, JfyiError>;

/// Utility for eating top level errors and log them.
///
/// We basically always want to try and continue on error. This utility function is meant to
/// consume top-level errors by simply logging them.
pub fn log_error(result: Result<()>) -> std::result::Result<(), FatalError> {
	match result.into_nested()? {
		Err(error @ JfyiError::ImportCanceled(_)) => {
			gum::debug!(target: LOG_TARGET, error = ?error);
			Ok(())
		},
		Err(JfyiError::NotAValidator(peer)) => {
			gum::debug!(
				target: LOG_TARGET,
				?peer,
				"Dropping message from peer (unknown authority id)"
			);
			Ok(())
		},
		Err(error) => {
			gum::warn!(target: LOG_TARGET, error = ?error);
			Ok(())
		},
		Ok(()) => Ok(()),
	}
}
