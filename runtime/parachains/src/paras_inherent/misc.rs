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

use sp_std::vec::Vec;

/// A helper trait to allow calling retain while getting access
/// to the index of the item in the `vec`.
pub trait IndexedRetain<T> {
	/// Retains only the elements specified by the predicate.
	///
	/// In other words, remove all elements `e` residing at
	/// index `i` such that `f(i, &e)` returns `false`. This method
	/// operates in place, visiting each element exactly once in the
	/// original order, and preserves the order of the retained elements.
	fn indexed_retain(&mut self, f: impl FnMut(usize, &T) -> bool);
}

impl<T> IndexedRetain<T> for Vec<T> {
	fn indexed_retain(&mut self, mut f: impl FnMut(usize, &T) -> bool) {
		let mut idx = 0_usize;
		self.retain(move |item| {
			let ret = f(idx, item);
			idx += 1_usize;
			ret
		})
	}
}
