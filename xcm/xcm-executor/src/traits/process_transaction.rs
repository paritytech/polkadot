// Copyright Parity Technologies (UK) Ltd.
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

use xcm::latest::prelude::*;
/// Provides mechanisms for transactional processing of XCM instructions.
///
/// This trait defines the behavior required to process XCM instructions in a transactional
/// manner. Implementers of this trait can ensure that XCM instructions are executed
/// atomically, meaning they either fully succeed or fully fail without any partial effects.
///
/// Implementers of this trait can also choose to not process XCM instructions transactionally.
/// This is useful for cases where the implementer is not able to provide transactional guarantees.
/// In this case the `IS_TRANSACTIONAL` constant should be set to `false`.
/// The `()` type implements this trait in a non-transactional manner.
pub trait ProcessTransaction {
	/// Whether the processor (i.e. the type implementing this trait) is transactional.
	const IS_TRANSACTIONAL: bool;
	/// Processes an XCM instruction encapsulated within the provided closure. Responsible for
	/// processing an XCM instruction transactionally. If the closure returns an error, any
	/// changes made during its execution should be rolled back. In the case where the
	/// implementer is not able to provide transactional guarantees, the closure should be
	/// executed as is.
	/// # Parameters
	/// - `f`: A closure that encapsulates the XCM instruction to be processed. It should
	///   return a `Result` indicating the success or failure of the instruction.
	///
	/// # Returns
	/// - A `Result` indicating the overall success or failure of the transactional process.
	fn process<F>(f: F) -> Result<(), XcmError>
	where
		F: FnOnce() -> Result<(), XcmError>;
}

impl ProcessTransaction for () {
	const IS_TRANSACTIONAL: bool = false;
	fn process<F>(f: F) -> Result<(), XcmError>
	where
		F: FnOnce() -> Result<(), XcmError>,
	{
		f()
	}
}
