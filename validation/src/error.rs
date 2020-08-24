// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Errors that can occur during the validation process.

/// Error type for validation
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// Client error
	Client(sp_blockchain::Error),
	/// Consensus error
	Consensus(consensus::error::Error),
	/// Unexpected error checking inherents
	#[display(fmt = "Unexpected error while checking inherents: {}", _0)]
	InherentError(inherents::Error),
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Error::Client(ref err) => Some(err),
			Error::Consensus(ref err) => Some(err),
			_ => None,
		}
	}
}
