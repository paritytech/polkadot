// Copyright 2020-2021 Parity Technologies (UK) Ltd.
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

//! XCM `Junctions`/`InteriorMultiLocation` datatype.

use super::{Junction, MultiLocation};
use core::{convert::TryFrom, mem, result};
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

/// Maximum number of `Junction`s that a `Junctions` can contain.
pub(crate) const MAX_JUNCTIONS: usize = 8;

/// Non-parent junctions that can be constructed, up to the length of 8. This specific `Junctions`
/// implementation uses a Rust `enum` in order to make pattern matching easier.
///
/// Parent junctions cannot be constructed with this type. Refer to `MultiLocation` for
/// instructions on constructing parent junctions.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug, TypeInfo)]
pub enum Junctions {
	/// The interpreting consensus system.
	Here,
	/// A relative path comprising 1 junction.
	X1(Junction),
	/// A relative path comprising 2 junctions.
	X2(Junction, Junction),
	/// A relative path comprising 3 junctions.
	X3(Junction, Junction, Junction),
	/// A relative path comprising 4 junctions.
	X4(Junction, Junction, Junction, Junction),
	/// A relative path comprising 5 junctions.
	X5(Junction, Junction, Junction, Junction, Junction),
	/// A relative path comprising 6 junctions.
	X6(Junction, Junction, Junction, Junction, Junction, Junction),
	/// A relative path comprising 7 junctions.
	X7(Junction, Junction, Junction, Junction, Junction, Junction, Junction),
	/// A relative path comprising 8 junctions.
	X8(Junction, Junction, Junction, Junction, Junction, Junction, Junction, Junction),
}

pub struct JunctionsIterator(Junctions);
impl Iterator for JunctionsIterator {
	type Item = Junction;
	fn next(&mut self) -> Option<Junction> {
		self.0.take_first()
	}
}

impl DoubleEndedIterator for JunctionsIterator {
	fn next_back(&mut self) -> Option<Junction> {
		self.0.take_last()
	}
}

pub struct JunctionsRefIterator<'a> {
	junctions: &'a Junctions,
	next: usize,
	back: usize,
}

impl<'a> Iterator for JunctionsRefIterator<'a> {
	type Item = &'a Junction;
	fn next(&mut self) -> Option<&'a Junction> {
		if self.next.saturating_add(self.back) >= self.junctions.len() {
			return None
		}

		let result = self.junctions.at(self.next);
		self.next += 1;
		result
	}
}

impl<'a> DoubleEndedIterator for JunctionsRefIterator<'a> {
	fn next_back(&mut self) -> Option<&'a Junction> {
		let next_back = self.back.saturating_add(1);
		// checked_sub here, because if the result is less than 0, we end iteration
		let index = self.junctions.len().checked_sub(next_back)?;
		if self.next > index {
			return None
		}
		self.back = next_back;

		self.junctions.at(index)
	}
}

impl<'a> IntoIterator for &'a Junctions {
	type Item = &'a Junction;
	type IntoIter = JunctionsRefIterator<'a>;
	fn into_iter(self) -> Self::IntoIter {
		JunctionsRefIterator { junctions: self, next: 0, back: 0 }
	}
}

impl IntoIterator for Junctions {
	type Item = Junction;
	type IntoIter = JunctionsIterator;
	fn into_iter(self) -> Self::IntoIter {
		JunctionsIterator(self)
	}
}

impl Junctions {
	/// Convert `self` into a `MultiLocation` containing 0 parents.
	///
	/// Similar to `Into::into`, except that this method can be used in a const evaluation context.
	pub const fn into_location(self) -> MultiLocation {
		MultiLocation { parents: 0, interior: self }
	}

	/// Convert `self` into a `MultiLocation` containing `n` parents.
	///
	/// Similar to `Self::into`, with the added ability to specify the number of parent junctions.
	pub const fn into_exterior(self, n: u8) -> MultiLocation {
		MultiLocation { parents: n, interior: self }
	}

	/// Returns first junction, or `None` if the location is empty.
	pub fn first(&self) -> Option<&Junction> {
		match &self {
			Junctions::Here => None,
			Junctions::X1(ref a) => Some(a),
			Junctions::X2(ref a, ..) => Some(a),
			Junctions::X3(ref a, ..) => Some(a),
			Junctions::X4(ref a, ..) => Some(a),
			Junctions::X5(ref a, ..) => Some(a),
			Junctions::X6(ref a, ..) => Some(a),
			Junctions::X7(ref a, ..) => Some(a),
			Junctions::X8(ref a, ..) => Some(a),
		}
	}

	/// Returns last junction, or `None` if the location is empty.
	pub fn last(&self) -> Option<&Junction> {
		match &self {
			Junctions::Here => None,
			Junctions::X1(ref a) => Some(a),
			Junctions::X2(.., ref a) => Some(a),
			Junctions::X3(.., ref a) => Some(a),
			Junctions::X4(.., ref a) => Some(a),
			Junctions::X5(.., ref a) => Some(a),
			Junctions::X6(.., ref a) => Some(a),
			Junctions::X7(.., ref a) => Some(a),
			Junctions::X8(.., ref a) => Some(a),
		}
	}

	/// Splits off the first junction, returning the remaining suffix (first item in tuple) and the first element
	/// (second item in tuple) or `None` if it was empty.
	pub fn split_first(self) -> (Junctions, Option<Junction>) {
		match self {
			Junctions::Here => (Junctions::Here, None),
			Junctions::X1(a) => (Junctions::Here, Some(a)),
			Junctions::X2(a, b) => (Junctions::X1(b), Some(a)),
			Junctions::X3(a, b, c) => (Junctions::X2(b, c), Some(a)),
			Junctions::X4(a, b, c, d) => (Junctions::X3(b, c, d), Some(a)),
			Junctions::X5(a, b, c, d, e) => (Junctions::X4(b, c, d, e), Some(a)),
			Junctions::X6(a, b, c, d, e, f) => (Junctions::X5(b, c, d, e, f), Some(a)),
			Junctions::X7(a, b, c, d, e, f, g) => (Junctions::X6(b, c, d, e, f, g), Some(a)),
			Junctions::X8(a, b, c, d, e, f, g, h) => (Junctions::X7(b, c, d, e, f, g, h), Some(a)),
		}
	}

	/// Splits off the last junction, returning the remaining prefix (first item in tuple) and the last element
	/// (second item in tuple) or `None` if it was empty.
	pub fn split_last(self) -> (Junctions, Option<Junction>) {
		match self {
			Junctions::Here => (Junctions::Here, None),
			Junctions::X1(a) => (Junctions::Here, Some(a)),
			Junctions::X2(a, b) => (Junctions::X1(a), Some(b)),
			Junctions::X3(a, b, c) => (Junctions::X2(a, b), Some(c)),
			Junctions::X4(a, b, c, d) => (Junctions::X3(a, b, c), Some(d)),
			Junctions::X5(a, b, c, d, e) => (Junctions::X4(a, b, c, d), Some(e)),
			Junctions::X6(a, b, c, d, e, f) => (Junctions::X5(a, b, c, d, e), Some(f)),
			Junctions::X7(a, b, c, d, e, f, g) => (Junctions::X6(a, b, c, d, e, f), Some(g)),
			Junctions::X8(a, b, c, d, e, f, g, h) => (Junctions::X7(a, b, c, d, e, f, g), Some(h)),
		}
	}

	/// Removes the first element from `self`, returning it (or `None` if it was empty).
	pub fn take_first(&mut self) -> Option<Junction> {
		let mut d = Junctions::Here;
		mem::swap(&mut *self, &mut d);
		let (tail, head) = d.split_first();
		*self = tail;
		head
	}

	/// Removes the last element from `self`, returning it (or `None` if it was empty).
	pub fn take_last(&mut self) -> Option<Junction> {
		let mut d = Junctions::Here;
		mem::swap(&mut *self, &mut d);
		let (head, tail) = d.split_last();
		*self = head;
		tail
	}

	/// Mutates `self` to be appended with `new` or returns an `Err` with `new` if would overflow.
	pub fn push(&mut self, new: impl Into<Junction>) -> result::Result<(), Junction> {
		let new = new.into();
		let mut dummy = Junctions::Here;
		mem::swap(self, &mut dummy);
		match dummy.pushed_with(new) {
			Ok(s) => {
				*self = s;
				Ok(())
			},
			Err((s, j)) => {
				*self = s;
				Err(j)
			},
		}
	}

	/// Mutates `self` to be prepended with `new` or returns an `Err` with `new` if would overflow.
	pub fn push_front(&mut self, new: impl Into<Junction>) -> result::Result<(), Junction> {
		let new = new.into();
		let mut dummy = Junctions::Here;
		mem::swap(self, &mut dummy);
		match dummy.pushed_front_with(new) {
			Ok(s) => {
				*self = s;
				Ok(())
			},
			Err((s, j)) => {
				*self = s;
				Err(j)
			},
		}
	}

	/// Consumes `self` and returns a `Junctions` suffixed with `new`, or an `Err` with the
	/// original value of `self` and `new` in case of overflow.
	pub fn pushed_with(self, new: impl Into<Junction>) -> result::Result<Self, (Self, Junction)> {
		let new = new.into();
		Ok(match self {
			Junctions::Here => Junctions::X1(new),
			Junctions::X1(a) => Junctions::X2(a, new),
			Junctions::X2(a, b) => Junctions::X3(a, b, new),
			Junctions::X3(a, b, c) => Junctions::X4(a, b, c, new),
			Junctions::X4(a, b, c, d) => Junctions::X5(a, b, c, d, new),
			Junctions::X5(a, b, c, d, e) => Junctions::X6(a, b, c, d, e, new),
			Junctions::X6(a, b, c, d, e, f) => Junctions::X7(a, b, c, d, e, f, new),
			Junctions::X7(a, b, c, d, e, f, g) => Junctions::X8(a, b, c, d, e, f, g, new),
			s => Err((s, new))?,
		})
	}

	/// Consumes `self` and returns a `Junctions` prefixed with `new`, or an `Err` with the
	/// original value of `self` and `new` in case of overflow.
	pub fn pushed_front_with(
		self,
		new: impl Into<Junction>,
	) -> result::Result<Self, (Self, Junction)> {
		let new = new.into();
		Ok(match self {
			Junctions::Here => Junctions::X1(new),
			Junctions::X1(a) => Junctions::X2(new, a),
			Junctions::X2(a, b) => Junctions::X3(new, a, b),
			Junctions::X3(a, b, c) => Junctions::X4(new, a, b, c),
			Junctions::X4(a, b, c, d) => Junctions::X5(new, a, b, c, d),
			Junctions::X5(a, b, c, d, e) => Junctions::X6(new, a, b, c, d, e),
			Junctions::X6(a, b, c, d, e, f) => Junctions::X7(new, a, b, c, d, e, f),
			Junctions::X7(a, b, c, d, e, f, g) => Junctions::X8(new, a, b, c, d, e, f, g),
			s => Err((s, new))?,
		})
	}

	/// Mutate `self` so that it is suffixed with `suffix`.
	///
	/// Does not modify `self` and returns `Err` with `suffix` in case of overflow.
	///
	/// # Example
	/// ```rust
	/// # use xcm::v1::{Junctions::*, Junction::*, MultiLocation};
	/// # fn main() {
	/// let mut m = X1(Parachain(21));
	/// assert_eq!(m.append_with(X1(PalletInstance(3))), Ok(()));
	/// assert_eq!(m, X2(Parachain(21), PalletInstance(3)));
	/// # }
	/// ```
	pub fn append_with(&mut self, suffix: impl Into<Junctions>) -> Result<(), Junctions> {
		let suffix = suffix.into();
		if self.len().saturating_add(suffix.len()) > MAX_JUNCTIONS {
			return Err(suffix)
		}
		for j in suffix.into_iter() {
			self.push(j).expect("Already checked the sum of the len()s; qed")
		}
		Ok(())
	}

	/// Returns the number of junctions in `self`.
	pub const fn len(&self) -> usize {
		match &self {
			Junctions::Here => 0,
			Junctions::X1(..) => 1,
			Junctions::X2(..) => 2,
			Junctions::X3(..) => 3,
			Junctions::X4(..) => 4,
			Junctions::X5(..) => 5,
			Junctions::X6(..) => 6,
			Junctions::X7(..) => 7,
			Junctions::X8(..) => 8,
		}
	}

	/// Returns the junction at index `i`, or `None` if the location doesn't contain that many elements.
	pub fn at(&self, i: usize) -> Option<&Junction> {
		Some(match (i, self) {
			(0, Junctions::X1(ref a)) => a,
			(0, Junctions::X2(ref a, ..)) => a,
			(0, Junctions::X3(ref a, ..)) => a,
			(0, Junctions::X4(ref a, ..)) => a,
			(0, Junctions::X5(ref a, ..)) => a,
			(0, Junctions::X6(ref a, ..)) => a,
			(0, Junctions::X7(ref a, ..)) => a,
			(0, Junctions::X8(ref a, ..)) => a,
			(1, Junctions::X2(_, ref a)) => a,
			(1, Junctions::X3(_, ref a, ..)) => a,
			(1, Junctions::X4(_, ref a, ..)) => a,
			(1, Junctions::X5(_, ref a, ..)) => a,
			(1, Junctions::X6(_, ref a, ..)) => a,
			(1, Junctions::X7(_, ref a, ..)) => a,
			(1, Junctions::X8(_, ref a, ..)) => a,
			(2, Junctions::X3(_, _, ref a)) => a,
			(2, Junctions::X4(_, _, ref a, ..)) => a,
			(2, Junctions::X5(_, _, ref a, ..)) => a,
			(2, Junctions::X6(_, _, ref a, ..)) => a,
			(2, Junctions::X7(_, _, ref a, ..)) => a,
			(2, Junctions::X8(_, _, ref a, ..)) => a,
			(3, Junctions::X4(_, _, _, ref a)) => a,
			(3, Junctions::X5(_, _, _, ref a, ..)) => a,
			(3, Junctions::X6(_, _, _, ref a, ..)) => a,
			(3, Junctions::X7(_, _, _, ref a, ..)) => a,
			(3, Junctions::X8(_, _, _, ref a, ..)) => a,
			(4, Junctions::X5(_, _, _, _, ref a)) => a,
			(4, Junctions::X6(_, _, _, _, ref a, ..)) => a,
			(4, Junctions::X7(_, _, _, _, ref a, ..)) => a,
			(4, Junctions::X8(_, _, _, _, ref a, ..)) => a,
			(5, Junctions::X6(_, _, _, _, _, ref a)) => a,
			(5, Junctions::X7(_, _, _, _, _, ref a, ..)) => a,
			(5, Junctions::X8(_, _, _, _, _, ref a, ..)) => a,
			(6, Junctions::X7(_, _, _, _, _, _, ref a)) => a,
			(6, Junctions::X8(_, _, _, _, _, _, ref a, ..)) => a,
			(7, Junctions::X8(_, _, _, _, _, _, _, ref a)) => a,
			_ => return None,
		})
	}

	/// Returns a mutable reference to the junction at index `i`, or `None` if the location doesn't contain that many
	/// elements.
	pub fn at_mut(&mut self, i: usize) -> Option<&mut Junction> {
		Some(match (i, self) {
			(0, Junctions::X1(ref mut a)) => a,
			(0, Junctions::X2(ref mut a, ..)) => a,
			(0, Junctions::X3(ref mut a, ..)) => a,
			(0, Junctions::X4(ref mut a, ..)) => a,
			(0, Junctions::X5(ref mut a, ..)) => a,
			(0, Junctions::X6(ref mut a, ..)) => a,
			(0, Junctions::X7(ref mut a, ..)) => a,
			(0, Junctions::X8(ref mut a, ..)) => a,
			(1, Junctions::X2(_, ref mut a)) => a,
			(1, Junctions::X3(_, ref mut a, ..)) => a,
			(1, Junctions::X4(_, ref mut a, ..)) => a,
			(1, Junctions::X5(_, ref mut a, ..)) => a,
			(1, Junctions::X6(_, ref mut a, ..)) => a,
			(1, Junctions::X7(_, ref mut a, ..)) => a,
			(1, Junctions::X8(_, ref mut a, ..)) => a,
			(2, Junctions::X3(_, _, ref mut a)) => a,
			(2, Junctions::X4(_, _, ref mut a, ..)) => a,
			(2, Junctions::X5(_, _, ref mut a, ..)) => a,
			(2, Junctions::X6(_, _, ref mut a, ..)) => a,
			(2, Junctions::X7(_, _, ref mut a, ..)) => a,
			(2, Junctions::X8(_, _, ref mut a, ..)) => a,
			(3, Junctions::X4(_, _, _, ref mut a)) => a,
			(3, Junctions::X5(_, _, _, ref mut a, ..)) => a,
			(3, Junctions::X6(_, _, _, ref mut a, ..)) => a,
			(3, Junctions::X7(_, _, _, ref mut a, ..)) => a,
			(3, Junctions::X8(_, _, _, ref mut a, ..)) => a,
			(4, Junctions::X5(_, _, _, _, ref mut a)) => a,
			(4, Junctions::X6(_, _, _, _, ref mut a, ..)) => a,
			(4, Junctions::X7(_, _, _, _, ref mut a, ..)) => a,
			(4, Junctions::X8(_, _, _, _, ref mut a, ..)) => a,
			(5, Junctions::X6(_, _, _, _, _, ref mut a)) => a,
			(5, Junctions::X7(_, _, _, _, _, ref mut a, ..)) => a,
			(5, Junctions::X8(_, _, _, _, _, ref mut a, ..)) => a,
			(6, Junctions::X7(_, _, _, _, _, _, ref mut a)) => a,
			(6, Junctions::X8(_, _, _, _, _, _, ref mut a, ..)) => a,
			(7, Junctions::X8(_, _, _, _, _, _, _, ref mut a)) => a,
			_ => return None,
		})
	}

	/// Returns a reference iterator over the junctions.
	pub fn iter(&self) -> JunctionsRefIterator {
		JunctionsRefIterator { junctions: self, next: 0, back: 0 }
	}

	/// Returns a reference iterator over the junctions in reverse.
	#[deprecated(note = "Please use iter().rev()")]
	pub fn iter_rev(&self) -> impl Iterator + '_ {
		self.iter().rev()
	}

	/// Consumes `self` and returns an iterator over the junctions in reverse.
	#[deprecated(note = "Please use into_iter().rev()")]
	pub fn into_iter_rev(self) -> impl Iterator {
		self.into_iter().rev()
	}

	/// Ensures that self begins with `prefix` and that it has a single `Junction` item following.
	/// If so, returns a reference to this `Junction` item.
	///
	/// # Example
	/// ```rust
	/// # use xcm::v1::{Junctions::*, Junction::*};
	/// # fn main() {
	/// let mut m = X3(Parachain(2), PalletInstance(3), OnlyChild);
	/// assert_eq!(m.match_and_split(&X2(Parachain(2), PalletInstance(3))), Some(&OnlyChild));
	/// assert_eq!(m.match_and_split(&X1(Parachain(2))), None);
	/// # }
	/// ```
	pub fn match_and_split(&self, prefix: &Junctions) -> Option<&Junction> {
		if prefix.len() + 1 != self.len() {
			return None
		}
		for i in 0..prefix.len() {
			if prefix.at(i) != self.at(i) {
				return None
			}
		}
		return self.at(prefix.len())
	}
}

impl TryFrom<MultiLocation> for Junctions {
	type Error = ();
	fn try_from(x: MultiLocation) -> result::Result<Self, ()> {
		if x.parents > 0 {
			Err(())
		} else {
			Ok(x.interior)
		}
	}
}

impl<T: Into<Junction>> From<T> for Junctions {
	fn from(x: T) -> Self {
		Self::X1(x.into())
	}
}

impl From<[Junction; 0]> for Junctions {
	fn from(_: [Junction; 0]) -> Self {
		Self::Here
	}
}

impl From<()> for Junctions {
	fn from(_: ()) -> Self {
		Self::Here
	}
}

xcm_procedural::impl_conversion_functions_for_junctions_v3!();

#[test]
fn test_conversion() {
	use super::{Junction::*, Junctions::*, NetworkId::*};
	let x: Junctions = GlobalConsensus(Polkadot).into();
	assert_eq!(x, X1(GlobalConsensus(Polkadot)));
	let x: Junctions = Polkadot.into();
	assert_eq!(x, X1(GlobalConsensus(Polkadot)));
	let x: Junctions = (Polkadot, Kusama).into();
	assert_eq!(x, X2(GlobalConsensus(Polkadot), GlobalConsensus(Kusama)));
}
