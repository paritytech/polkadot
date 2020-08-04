// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Error types for the subsystem requests.

/// A description of an error causing the runtime API request to be unservable.
#[derive(Debug, Clone)]
pub struct RuntimeApiError(String);

impl From<String> for RuntimeApiError {
	fn from(s: String) -> Self {
		RuntimeApiError(s)
	}
}

impl core::fmt::Display for RuntimeApiError {
	fn fmt(&self, f: &mut core::fmt::Formatter) -> Result<(), core::fmt::Error> {
		write!(f, "{}", self.0)
	}
}

/// A description of an error causing the chain API request to be unservable.
#[derive(Debug, Clone)]
pub struct ChainApiError {
	msg: String,
}

impl From<&str> for ChainApiError {
	fn from(s: &str) -> Self {
		s.to_owned().into()
	}
}

impl From<String> for ChainApiError {
	fn from(msg: String) -> Self {
		Self { msg }
	}
}

impl core::fmt::Display for ChainApiError {
	fn fmt(&self, f: &mut core::fmt::Formatter) -> Result<(), core::fmt::Error> {
		write!(f, "{}", self.msg)
	}
}
