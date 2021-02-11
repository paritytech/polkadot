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

use thiserror::Error;

use futures::channel::oneshot;

use polkadot_node_subsystem_util::Error as UtilError;
use polkadot_subsystem::{SubsystemError};

#[derive(Debug, Error)]
pub enum Error {
	#[error("Response channel to obtain StoreChunk failed")]
	StoreChunkResponseChannel(#[source] oneshot::Canceled),

	#[error("Response channel to obtain QueryChunk failed")]
	QueryChunkResponseChannel(#[source] oneshot::Canceled),

	#[error("Response channel to obtain AvailabilityCores failed")]
	QueryAvailabilityResponseChannel(#[source] oneshot::Canceled),

	#[error("Receive channel closed")]
	IncomingMessageChannel(#[source] SubsystemError),

    /// Some request to the runtime in the session cache failed.
	#[error("Session cache runtime request failed")]
	SessionCacheRuntimRequest(#[source] UtilError),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<SubsystemError> for Error {
	fn from(err: SubsystemError) -> Self {
		Self::IncomingMessageChannel(err)
	}
}
