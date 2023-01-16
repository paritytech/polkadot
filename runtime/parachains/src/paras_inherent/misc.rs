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

use sp_std::{cmp::Ordering, vec::Vec};

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

/// Helper trait until `is_sorted_by` is stabilized.
/// TODO: https://github.com/rust-lang/rust/issues/53485
pub trait IsSortedBy<T> {
	fn is_sorted_by<F>(self, cmp: F) -> bool
	where
		F: FnMut(&T, &T) -> Ordering;
}

impl<'x, T, X> IsSortedBy<T> for X
where
	X: 'x + IntoIterator<Item = &'x T>,
	T: 'x,
{
	fn is_sorted_by<F>(self, mut cmp: F) -> bool
	where
		F: FnMut(&T, &T) -> Ordering,
	{
		let mut iter = self.into_iter();
		let mut previous: &T = if let Some(previous) = iter.next() {
			previous
		} else {
			// empty is always sorted
			return true
		};
		while let Some(cursor) = iter.next() {
			match cmp(&previous, &cursor) {
				Ordering::Greater => return false,
				_ => {
					// ordering is ok
				},
			}
			previous = cursor;
		}
		true
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn is_sorted_simple() {
		let v = vec![1_i32, 2, 3, 1000];
		assert!(IsSortedBy::<i32>::is_sorted_by(v.as_slice(), |a: &i32, b: &i32| { a.cmp(b) }));
		assert!(!IsSortedBy::<i32>::is_sorted_by(&v, |a, b| { b.cmp(a) }));

		let v = vec![8_i32, 8, 8, 8];
		assert!(IsSortedBy::<i32>::is_sorted_by(v.as_slice(), |a: &i32, b: &i32| { a.cmp(b) }));
		assert!(IsSortedBy::<i32>::is_sorted_by(v.as_slice(), |a: &i32, b: &i32| { b.cmp(a) }));
	}

	#[test]
	fn is_not_sorted() {
		let v = vec![7, 1, 3];
		assert!(!IsSortedBy::is_sorted_by(&v, |a, b| { a.cmp(b) }));
		assert!(!IsSortedBy::is_sorted_by(&v, |a, b| { b.cmp(a) }));
	}

	#[test]
	fn empty_is_sorted() {
		let v = Vec::<u8>::new();
		assert!(IsSortedBy::is_sorted_by(&v, |_a, _b| { unreachable!() }));
	}

	#[test]
	fn single_items_is_sorted() {
		let v = vec![7_u8];
		assert!(IsSortedBy::is_sorted_by(&v, |_a, _b| { unreachable!() }));
	}
}
