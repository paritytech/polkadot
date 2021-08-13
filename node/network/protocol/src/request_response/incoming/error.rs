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

//! Error handling related code and Error/Result definitions.

use sc_network::PeerId;
use thiserror::Error;

use parity_scale_codec::Error as DecodingError;

/// Errors that happen during reception/decoding of incoming requests.
#[derive(Debug, Error, derive_more::From)]
#[error(transparent)]
pub enum Error {
	/// All fatal errors.
	Fatal(Fatal),
	/// All nonfatal/potentially recoverable errors.
	NonFatal(NonFatal),
}

/// Fatal errors when receiving incoming requests.
#[derive(Debug, Error)]
pub enum Fatal {
	/// Incoming request stream exhausted. Should only happen on shutdown.
	#[error("Incoming request channel got closed.")]
	RequestChannelExhausted,
}

/// Non-fatal errors when receiving incoming requests.
#[derive(Debug, Error)]
pub enum NonFatal {
	/// Decoding failed, we were able to change the peer's reputation accordingly.
	#[error("Decoding request failed for peer {0}.")]
	DecodingError(PeerId, #[source] DecodingError),

	/// Decoding failed, but sending reputation change failed.
	#[error("Decoding request failed for peer {0}, and changing reputation failed.")]
	DecodingErrorNoReputationChange(PeerId, #[source] DecodingError),
}

/// General result based on above `Error`.
pub type Result<T> = std::result::Result<T, Error>;
