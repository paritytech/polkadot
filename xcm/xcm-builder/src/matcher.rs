// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! XCM matcher API, used primarily for writing barrier conditions.

use core::ops::ControlFlow;
use frame_support::traits::ProcessMessageError;
use xcm::latest::{Instruction, MultiLocation};

/// Creates an instruction matcher from an XCM. Since XCM versions differ, we need to make a trait
/// here to unify the interfaces among them.
pub trait CreateMatcher {
	/// The concrete matcher type.
	type Matcher;

	/// Method that creates and returns the matcher type from `Self`.
	fn matcher(self) -> Self::Matcher;
}

impl<'a, Call> CreateMatcher for &'a mut [Instruction<Call>] {
	type Matcher = Matcher<'a, Call>;

	fn matcher(self) -> Self::Matcher {
		let total_inst = self.len();

		Matcher { xcm: self, current_idx: 0, total_inst }
	}
}

/// API that allows to pattern-match against anything that is contained within an XCM.
///
/// The intended usage of the matcher API is to enable the ability to chain successive methods of
/// this trait together, along with the ? operator for the purpose of facilitating the writing,
/// maintenance and auditability of XCM barriers.
///
/// Example:
/// ```rust
/// use frame_support::traits::ProcessMessageError;
/// use xcm::latest::Instruction;
/// use xcm_builder::{CreateMatcher, MatchXcm};
///
/// let mut msg = [Instruction::<()>::ClearOrigin];
/// let res = msg
/// 	.matcher()
/// 	.assert_remaining_insts(1)?
/// 	.match_next_inst(|inst| match inst {
/// 		Instruction::<()>::ClearOrigin => Ok(()),
/// 		_ => Err(ProcessMessageError::BadFormat),
/// 	});
/// assert!(res.is_ok());
///
/// Ok::<(), ProcessMessageError>(())
/// ```
pub trait MatchXcm {
	/// The concrete instruction type. Necessary to specify as it changes between XCM versions.
	type Inst;
	/// The `MultiLocation` type. Necessary to specify as it changes between XCM versions.
	type Loc;
	/// The error type to throw when errors happen during matching.
	type Error;

	/// Returns success if the number of instructions that still have not been iterated over
	/// equals `n`, otherwise returns an error.
	fn assert_remaining_insts(self, n: usize) -> Result<Self, Self::Error>
	where
		Self: Sized;

	/// Accepts a closure `f` that contains an argument signifying the next instruction to be
	/// iterated over. The closure can then be used to check whether the instruction matches a
	/// given condition, and can also be used to mutate the fields of an instruction.
	///
	/// The closure `f` returns success when the instruction passes the condition, otherwise it
	/// returns an error, which will ultimately be returned by this function.
	fn match_next_inst<F>(self, f: F) -> Result<Self, Self::Error>
	where
		Self: Sized,
		F: FnMut(&mut Self::Inst) -> Result<(), Self::Error>;

	/// Attempts to continuously iterate through the instructions while applying `f` to each of
	/// them, until either the last instruction or `cond` returns false.
	///
	/// If `f` returns an error, then iteration halts and the function returns that error.
	/// Otherwise, `f` returns a `ControlFlow` which signifies whether the iteration breaks or
	/// continues.
	fn match_next_inst_while<C, F>(self, cond: C, f: F) -> Result<Self, Self::Error>
	where
		Self: Sized,
		C: Fn(&Self::Inst) -> bool,
		F: FnMut(&mut Self::Inst) -> Result<ControlFlow<()>, Self::Error>;

	/// Iterate instructions forward until `cond` returns false. When there are no more instructions
	/// to be read, an error is returned.
	fn skip_inst_while<C>(self, cond: C) -> Result<Self, Self::Error>
	where
		Self: Sized,
		C: Fn(&Self::Inst) -> bool,
	{
		Self::match_next_inst_while(self, cond, |_| Ok(ControlFlow::Continue(())))
	}
}

/// Struct created from calling `fn matcher()` on a mutable slice of `Instruction`s.
///
/// Implements `MatchXcm` to allow an iterator-like API to match against each `Instruction`
/// contained within the slice, which facilitates the building of XCM barriers.
pub struct Matcher<'a, Call> {
	pub(crate) xcm: &'a mut [Instruction<Call>],
	pub(crate) current_idx: usize,
	pub(crate) total_inst: usize,
}

impl<'a, Call> MatchXcm for Matcher<'a, Call> {
	type Error = ProcessMessageError;
	type Inst = Instruction<Call>;
	type Loc = MultiLocation;

	fn assert_remaining_insts(self, n: usize) -> Result<Self, Self::Error>
	where
		Self: Sized,
	{
		if self.total_inst - self.current_idx != n {
			return Err(ProcessMessageError::BadFormat)
		}

		Ok(self)
	}

	fn match_next_inst<F>(mut self, mut f: F) -> Result<Self, Self::Error>
	where
		Self: Sized,
		F: FnMut(&mut Self::Inst) -> Result<(), Self::Error>,
	{
		if self.current_idx < self.total_inst {
			f(&mut self.xcm[self.current_idx])?;
			self.current_idx += 1;
			Ok(self)
		} else {
			Err(ProcessMessageError::BadFormat)
		}
	}

	fn match_next_inst_while<C, F>(mut self, cond: C, mut f: F) -> Result<Self, Self::Error>
	where
		Self: Sized,
		C: Fn(&Self::Inst) -> bool,
		F: FnMut(&mut Self::Inst) -> Result<ControlFlow<()>, Self::Error>,
	{
		if self.current_idx >= self.total_inst {
			return Err(ProcessMessageError::BadFormat)
		}

		while self.current_idx < self.total_inst && cond(&self.xcm[self.current_idx]) {
			if let ControlFlow::Break(()) = f(&mut self.xcm[self.current_idx])? {
				break
			}
			self.current_idx += 1;
		}

		Ok(self)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::{vec, vec::Vec};
	use xcm::latest::prelude::*;

	#[test]
	fn match_next_inst_while_works() {
		let mut xcm: Vec<Instruction<()>> = vec![ClearOrigin];

		let _ = xcm
			.matcher()
			.match_next_inst_while(|_| true, |_| Ok(ControlFlow::Continue(())))
			.unwrap();
	}
}
