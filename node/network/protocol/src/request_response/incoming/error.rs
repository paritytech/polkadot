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

//! Error handling related code and Error/Result definitions.

use sc_network::PeerId;

use parity_scale_codec::Error as DecodingError;

#[allow(missing_docs)]
#[fatality::fatality(splitable)]
pub enum Error {
	// Incoming request stream exhausted. Should only happen on shutdown.
	#[fatal]
	#[error("Incoming request channel got closed.")]
	RequestChannelExhausted,

	/// Decoding failed, we were able to change the peer's reputation accordingly.
	#[error("Decoding request failed for peer {0}.")]
	DecodingError(PeerId, #[source] DecodingError),

	/// Decoding failed, but sending reputation change failed.
	#[error("Decoding request failed for peer {0}, and changing reputation failed.")]
	DecodingErrorNoReputationChange(PeerId, #[source] DecodingError),
}

/// General result based on above `Error`.
pub type Result<T> = std::result::Result<T, Error>;
