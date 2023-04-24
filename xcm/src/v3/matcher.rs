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

use super::{Instruction, MultiLocation};
use crate::{CreateMatcher, MatchXcm};
use core::ops::ControlFlow;

impl<'a, Call> CreateMatcher for &'a mut [Instruction<Call>] {
	type Matcher = Matcher<'a, Call>;

	fn matcher(self) -> Self::Matcher {
		let total_inst = self.len();

		Matcher { xcm: self, current_idx: 0, total_inst }
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
	type Error = ();
	type Inst = Instruction<Call>;
	type Loc = MultiLocation;

	fn assert_remaining_insts(self, n: usize) -> Result<Self, Self::Error>
	where
		Self: Sized,
	{
		if self.total_inst - self.current_idx != n {
			return Err(())
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
			Err(())
		}
	}

	fn match_next_inst_while<C, F>(mut self, cond: C, mut f: F) -> Result<Self, Self::Error>
	where
		Self: Sized,
		C: Fn(&Self::Inst) -> bool,
		F: FnMut(&mut Self::Inst) -> Result<ControlFlow<()>, Self::Error>,
	{
		if self.current_idx >= self.total_inst {
			return Err(())
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
	use crate::v3::prelude::*;
	use alloc::{vec, vec::Vec};

	#[test]
	fn match_next_inst_while_works() {
		let mut xcm: Vec<Instruction<()>> = vec![ClearOrigin];

		let _ = xcm
			.matcher()
			.match_next_inst_while(|_| true, |_| Ok(ControlFlow::Continue(())))
			.unwrap();
	}
}
