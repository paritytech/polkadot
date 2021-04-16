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

//! Utilities for general error handling in Polkadot.


use polkadot_node_subsystem::SubsystemError;

/// Mark error variants as fatal. A fatal error should bring a subsystem down, any other errors
/// should be recoverable.
pub trait IsFatal {
	/// Whether or not a particular error variant should be considered fatal (should bring a
	/// subsystem down).
	///
	fn is_fatal(&self) -> bool;
}

// Escalate fatal errors to `SubsystemError`s.
//
// Fatal errors will be converted to `SubsystemError` and returned, others are passed to `handle`.
pub fn escalate_fatal<A, Err: IsFatal + Send + Sync + std::error::Error + 'static>(
	r: Result<A, Err>,
	handle: impl FnOnce(Err) -> A,
) -> Result<A, SubsystemError> {
	r.or_else(|err| {
		if err.is_fatal() {
			Err(SubsystemError::with_origin("", err))
		} else {
			Ok(handle(err))
		}
	})
}
