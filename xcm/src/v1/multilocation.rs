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

//! Cross-Consensus Message format data structures.

use super::Junction;
use core::{mem, result};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// A relative path between state-bearing consensus systems.
///
/// A location in a consensus system is defined as an *isolatable state machine* held within global
/// consensus. The location in question need not have a sophisticated consensus algorithm of its
/// own; a single account within Ethereum, for example, could be considered a location.
///
/// A very-much non-exhaustive list of types of location include:
/// - A (normal, layer-1) block chain, e.g. the Bitcoin mainnet or a parachain.
/// - A layer-0 super-chain, e.g. the Polkadot Relay chain.
/// - A layer-2 smart contract, e.g. an ERC-20 on Ethereum.
/// - A logical functional component of a chain, e.g. a single instance of a pallet on a Frame-based
///   Substrate chain.
/// - An account.
///
/// A `MultiLocation` is a *relative identifier*, meaning that it can only be used to define the
/// relative path between two locations, and cannot generally be used to refer to a location
/// universally. It is comprised of an integer number of parents specifying the number of times to
/// "escape" upwards into the containing consensus system and then a number of *junctions*, each
/// diving down and specifying some interior portion of state (which may be considered a
/// "sub-consensus" system).
///
/// This specific `MultiLocation` implementation uses a `Junctions` datatype which is a Rust `enum`
/// in order to make pattern matching easier. There are occasions where it is important to ensure
/// that a value is strictly an interior location, in those cases, `Junctions` may be used.
///
/// The `MultiLocation` value of `Null` simply refers to the interpreting consensus system.
#[derive(Clone, Decode, Encode, Eq, PartialEq, Ord, PartialOrd, Debug, TypeInfo, MaxEncodedLen)]
pub struct MultiLocation {
	/// The number of parent junctions at the beginning of this `MultiLocation`.
	pub parents: u8,
	/// The interior (i.e. non-parent) junctions that this `MultiLocation` contains.
	pub interior: Junctions,
}

impl Default for MultiLocation {
	fn default() -> Self {
		Self { parents: 0, interior: Junctions::Here }
	}
}

/// A relative location which is constrained to be an interior location of the context.
///
/// See also `MultiLocation`.
pub type InteriorMultiLocation = Junctions;

impl MultiLocation {
	/// Creates a new `MultiLocation` with the given number of parents and interior junctions.
	pub fn new(parents: u8, junctions: Junctions) -> MultiLocation {
		MultiLocation { parents, interior: junctions }
	}

	/// Consume `self` and return the equivalent `VersionedMultiLocation` value.
	pub fn versioned(self) -> crate::VersionedMultiLocation {
		self.into()
	}

	/// Creates a new `MultiLocation` with 0 parents and a `Here` interior.
	///
	/// The resulting `MultiLocation` can be interpreted as the "current consensus system".
	pub const fn here() -> MultiLocation {
		MultiLocation { parents: 0, interior: Junctions::Here }
	}

	/// Creates a new `MultiLocation` which evaluates to the parent context.
	pub const fn parent() -> MultiLocation {
		MultiLocation { parents: 1, interior: Junctions::Here }
	}

	/// Creates a new `MultiLocation` which evaluates to the grand parent context.
	pub const fn grandparent() -> MultiLocation {
		MultiLocation { parents: 2, interior: Junctions::Here }
	}

	/// Creates a new `MultiLocation` with `parents` and an empty (`Here`) interior.
	pub const fn ancestor(parents: u8) -> MultiLocation {
		MultiLocation { parents, interior: Junctions::Here }
	}

	/// Whether the `MultiLocation` has no parents and has a `Here` interior.
	pub const fn is_here(&self) -> bool {
		self.parents == 0 && self.interior.len() == 0
	}

	/// Return a reference to the interior field.
	pub fn interior(&self) -> &Junctions {
		&self.interior
	}

	/// Return a mutable reference to the interior field.
	pub fn interior_mut(&mut self) -> &mut Junctions {
		&mut self.interior
	}

	/// Returns the number of `Parent` junctions at the beginning of `self`.
	pub const fn parent_count(&self) -> u8 {
		self.parents
	}

	/// Returns boolean indicating whether `self` contains only the specified amount of
	/// parents and no interior junctions.
	pub const fn contains_parents_only(&self, count: u8) -> bool {
		matches!(self.interior, Junctions::Here) && self.parents == count
	}

	/// Returns the number of parents and junctions in `self`.
	pub const fn len(&self) -> usize {
		self.parent_count() as usize + self.interior.len()
	}

	/// Returns the first interior junction, or `None` if the location is empty or contains only
	/// parents.
	pub fn first_interior(&self) -> Option<&Junction> {
		self.interior.first()
	}

	/// Returns last junction, or `None` if the location is empty or contains only parents.
	pub fn last(&self) -> Option<&Junction> {
		self.interior.last()
	}

	/// Splits off the first interior junction, returning the remaining suffix (first item in tuple)
	/// and the first element (second item in tuple) or `None` if it was empty.
	pub fn split_first_interior(self) -> (MultiLocation, Option<Junction>) {
		let MultiLocation { parents, interior: junctions } = self;
		let (suffix, first) = junctions.split_first();
		let multilocation = MultiLocation { parents, interior: suffix };
		(multilocation, first)
	}

	/// Splits off the last interior junction, returning the remaining prefix (first item in tuple)
	/// and the last element (second item in tuple) or `None` if it was empty or if `self` only
	/// contains parents.
	pub fn split_last_interior(self) -> (MultiLocation, Option<Junction>) {
		let MultiLocation { parents, interior: junctions } = self;
		let (prefix, last) = junctions.split_last();
		let multilocation = MultiLocation { parents, interior: prefix };
		(multilocation, last)
	}

	/// Mutates `self`, suffixing its interior junctions with `new`. Returns `Err` with `new` in
	/// case of overflow.
	pub fn push_interior(&mut self, new: Junction) -> result::Result<(), Junction> {
		self.interior.push(new)
	}

	/// Mutates `self`, prefixing its interior junctions with `new`. Returns `Err` with `new` in
	/// case of overflow.
	pub fn push_front_interior(&mut self, new: Junction) -> result::Result<(), Junction> {
		self.interior.push_front(new)
	}

	/// Consumes `self` and returns a `MultiLocation` suffixed with `new`, or an `Err` with theoriginal value of
	/// `self` in case of overflow.
	pub fn pushed_with_interior(self, new: Junction) -> result::Result<Self, (Self, Junction)> {
		match self.interior.pushed_with(new) {
			Ok(i) => Ok(MultiLocation { interior: i, parents: self.parents }),
			Err((i, j)) => Err((MultiLocation { interior: i, parents: self.parents }, j)),
		}
	}

	/// Consumes `self` and returns a `MultiLocation` prefixed with `new`, or an `Err` with the original value of
	/// `self` in case of overflow.
	pub fn pushed_front_with_interior(
		self,
		new: Junction,
	) -> result::Result<Self, (Self, Junction)> {
		match self.interior.pushed_front_with(new) {
			Ok(i) => Ok(MultiLocation { interior: i, parents: self.parents }),
			Err((i, j)) => Err((MultiLocation { interior: i, parents: self.parents }, j)),
		}
	}

	/// Returns the junction at index `i`, or `None` if the location is a parent or if the location
	/// does not contain that many elements.
	pub fn at(&self, i: usize) -> Option<&Junction> {
		let num_parents = self.parents as usize;
		if i < num_parents {
			return None
		}
		self.interior.at(i - num_parents)
	}

	/// Returns a mutable reference to the junction at index `i`, or `None` if the location is a
	/// parent or if it doesn't contain that many elements.
	pub fn at_mut(&mut self, i: usize) -> Option<&mut Junction> {
		let num_parents = self.parents as usize;
		if i < num_parents {
			return None
		}
		self.interior.at_mut(i - num_parents)
	}

	/// Decrements the parent count by 1.
	pub fn dec_parent(&mut self) {
		self.parents = self.parents.saturating_sub(1);
	}

	/// Removes the first interior junction from `self`, returning it
	/// (or `None` if it was empty or if `self` contains only parents).
	pub fn take_first_interior(&mut self) -> Option<Junction> {
		self.interior.take_first()
	}

	/// Removes the last element from `interior`, returning it (or `None` if it was empty or if
	/// `self` only contains parents).
	pub fn take_last(&mut self) -> Option<Junction> {
		self.interior.take_last()
	}

	/// Ensures that `self` has the same number of parents as `prefix`, its junctions begins with
	/// the junctions of `prefix` and that it has a single `Junction` item following.
	/// If so, returns a reference to this `Junction` item.
	///
	/// # Example
	/// ```rust
	/// # use xcm::v1::{Junctions::*, Junction::*, MultiLocation};
	/// # fn main() {
	/// let mut m = MultiLocation::new(1, X2(PalletInstance(3), OnlyChild));
	/// assert_eq!(
	///     m.match_and_split(&MultiLocation::new(1, X1(PalletInstance(3)))),
	///     Some(&OnlyChild),
	/// );
	/// assert_eq!(m.match_and_split(&MultiLocation::new(1, Here)), None);
	/// # }
	/// ```
	pub fn match_and_split(&self, prefix: &MultiLocation) -> Option<&Junction> {
		if self.parents != prefix.parents {
			return None
		}
		self.interior.match_and_split(&prefix.interior)
	}

	/// Returns whether `self` has the same number of parents as `prefix` and its junctions begins
	/// with the junctions of `prefix`.
	///
	/// # Example
	/// ```rust
	/// # use xcm::v1::{Junctions::*, Junction::*, MultiLocation};
	/// let m = MultiLocation::new(1, X3(PalletInstance(3), OnlyChild, OnlyChild));
	/// assert!(m.starts_with(&MultiLocation::new(1, X1(PalletInstance(3)))));
	/// assert!(!m.starts_with(&MultiLocation::new(1, X1(GeneralIndex(99)))));
	/// assert!(!m.starts_with(&MultiLocation::new(0, X1(PalletInstance(3)))));
	/// ```
	pub fn starts_with(&self, prefix: &MultiLocation) -> bool {
		if self.parents != prefix.parents {
			return false
		}
		self.interior.starts_with(&prefix.interior)
	}

	/// Mutate `self` so that it is suffixed with `suffix`.
	///
	/// Does not modify `self` and returns `Err` with `suffix` in case of overflow.
	///
	/// # Example
	/// ```rust
	/// # use xcm::v1::{Junctions::*, Junction::*, MultiLocation};
	/// # fn main() {
	/// let mut m = MultiLocation::new(1, X1(Parachain(21)));
	/// assert_eq!(m.append_with(X1(PalletInstance(3))), Ok(()));
	/// assert_eq!(m, MultiLocation::new(1, X2(Parachain(21), PalletInstance(3))));
	/// # }
	/// ```
	pub fn append_with(&mut self, suffix: Junctions) -> Result<(), Junctions> {
		if self.interior.len().saturating_add(suffix.len()) > MAX_JUNCTIONS {
			return Err(suffix)
		}
		for j in suffix.into_iter() {
			self.interior.push(j).expect("Already checked the sum of the len()s; qed")
		}
		Ok(())
	}

	/// Mutate `self` so that it is prefixed with `prefix`.
	///
	/// Does not modify `self` and returns `Err` with `prefix` in case of overflow.
	///
	/// # Example
	/// ```rust
	/// # use xcm::v1::{Junctions::*, Junction::*, MultiLocation};
	/// # fn main() {
	/// let mut m = MultiLocation::new(2, X1(PalletInstance(3)));
	/// assert_eq!(m.prepend_with(MultiLocation::new(1, X2(Parachain(21), OnlyChild))), Ok(()));
	/// assert_eq!(m, MultiLocation::new(1, X1(PalletInstance(3))));
	/// # }
	/// ```
	pub fn prepend_with(&mut self, mut prefix: MultiLocation) -> Result<(), MultiLocation> {
		//     prefix     self (suffix)
		// P .. P I .. I  p .. p i .. i
		let prepend_interior = prefix.interior.len().saturating_sub(self.parents as usize);
		let final_interior = self.interior.len().saturating_add(prepend_interior);
		if final_interior > MAX_JUNCTIONS {
			return Err(prefix)
		}
		let suffix_parents = (self.parents as usize).saturating_sub(prefix.interior.len());
		let final_parents = (prefix.parents as usize).saturating_add(suffix_parents);
		if final_parents > 255 {
			return Err(prefix)
		}

		// cancel out the final item on the prefix interior for one of the suffix's parents.
		while self.parents > 0 && prefix.take_last().is_some() {
			self.dec_parent();
		}

		// now we have either removed all suffix's parents or prefix interior.
		// this means we can combine the prefix's and suffix's remaining parents/interior since
		// we know that with at least one empty, the overall order will be respected:
		//     prefix     self (suffix)
		// P .. P   (I)   p .. p i .. i => P + p .. (no I) i
		//  -- or --
		// P .. P I .. I    (p)  i .. i => P (no p) .. I + i

		self.parents = self.parents.saturating_add(prefix.parents);
		for j in prefix.interior.into_iter().rev() {
			self.push_front_interior(j)
				.expect("final_interior no greater than MAX_JUNCTIONS; qed");
		}
		Ok(())
	}

	/// Mutate `self` so that it represents the same location from the point of view of `target`.
	/// The context of `self` is provided as `ancestry`.
	///
	/// Does not modify `self` in case of overflow.
	pub fn reanchor(&mut self, target: &MultiLocation, ancestry: &MultiLocation) -> Result<(), ()> {
		// TODO: https://github.com/paritytech/polkadot/issues/4489 Optimize this.

		// 1. Use our `ancestry` to figure out how the `target` would address us.
		let inverted_target = ancestry.inverted(target)?;

		// 2. Prepend `inverted_target` to `self` to get self's location from the perspective of
		// `target`.
		self.prepend_with(inverted_target).map_err(|_| ())?;

		// 3. Given that we know some of `target` ancestry, ensure that any parents in `self` are
		// strictly needed.
		self.simplify(target.interior());

		Ok(())
	}

	/// Treating `self` as a context, determine how it would be referenced by a `target` location.
	pub fn inverted(&self, target: &MultiLocation) -> Result<MultiLocation, ()> {
		use Junction::OnlyChild;
		let mut ancestry = self.clone();
		let mut junctions = Junctions::Here;
		for _ in 0..target.parent_count() {
			junctions = junctions
				.pushed_front_with(ancestry.interior.take_last().unwrap_or(OnlyChild))
				.map_err(|_| ())?;
		}
		let parents = target.interior().len() as u8;
		Ok(MultiLocation::new(parents, junctions))
	}

	/// Remove any unneeded parents/junctions in `self` based on the given context it will be
	/// interpreted in.
	pub fn simplify(&mut self, context: &Junctions) {
		if context.len() < self.parents as usize {
			// Not enough context
			return
		}
		while self.parents > 0 {
			let maybe = context.at(context.len() - (self.parents as usize));
			match (self.interior.first(), maybe) {
				(Some(i), Some(j)) if i == j => {
					self.interior.take_first();
					self.parents -= 1;
				},
				_ => break,
			}
		}
	}
}

/// A unit struct which can be converted into a `MultiLocation` of `parents` value 1.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Parent;
impl From<Parent> for MultiLocation {
	fn from(_: Parent) -> Self {
		MultiLocation { parents: 1, interior: Junctions::Here }
	}
}

/// A tuple struct which can be converted into a `MultiLocation` of `parents` value 1 with the inner interior.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ParentThen(Junctions);
impl From<ParentThen> for MultiLocation {
	fn from(ParentThen(interior): ParentThen) -> Self {
		MultiLocation { parents: 1, interior }
	}
}

/// A unit struct which can be converted into a `MultiLocation` of the inner `parents` value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Ancestor(u8);
impl From<Ancestor> for MultiLocation {
	fn from(Ancestor(parents): Ancestor) -> Self {
		MultiLocation { parents, interior: Junctions::Here }
	}
}

/// A unit struct which can be converted into a `MultiLocation` of the inner `parents` value and the inner interior.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct AncestorThen(u8, Junctions);
impl From<AncestorThen> for MultiLocation {
	fn from(AncestorThen(parents, interior): AncestorThen) -> Self {
		MultiLocation { parents, interior }
	}
}

xcm_procedural::impl_conversion_functions_for_multilocation_v1!();

/// Maximum number of `Junction`s that a `Junctions` can contain.
const MAX_JUNCTIONS: usize = 8;

/// Non-parent junctions that can be constructed, up to the length of 8. This specific `Junctions`
/// implementation uses a Rust `enum` in order to make pattern matching easier.
///
/// Parent junctions cannot be constructed with this type. Refer to `MultiLocation` for
/// instructions on constructing parent junctions.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
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
	pub const fn into(self) -> MultiLocation {
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
	pub fn push(&mut self, new: Junction) -> result::Result<(), Junction> {
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
	pub fn push_front(&mut self, new: Junction) -> result::Result<(), Junction> {
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
	pub fn pushed_with(self, new: Junction) -> result::Result<Self, (Self, Junction)> {
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
	pub fn pushed_front_with(self, new: Junction) -> result::Result<Self, (Self, Junction)> {
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
		if prefix.len() + 1 != self.len() || !self.starts_with(prefix) {
			return None
		}
		self.at(prefix.len())
	}

	/// Returns whether `self` begins with or is equal to `prefix`.
	///
	/// # Example
	/// ```rust
	/// # use xcm::v1::{Junctions::*, Junction::*};
	/// let mut j = X3(Parachain(2), PalletInstance(3), OnlyChild);
	/// assert!(j.starts_with(&X2(Parachain(2), PalletInstance(3))));
	/// assert!(j.starts_with(&j));
	/// assert!(j.starts_with(&X1(Parachain(2))));
	/// assert!(!j.starts_with(&X1(Parachain(999))));
	/// assert!(!j.starts_with(&X4(Parachain(2), PalletInstance(3), OnlyChild, OnlyChild)));
	/// ```
	pub fn starts_with(&self, prefix: &Junctions) -> bool {
		if self.len() < prefix.len() {
			return false
		}
		prefix.iter().zip(self.iter()).all(|(l, r)| l == r)
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

#[cfg(test)]
mod tests {
	use super::{Ancestor, AncestorThen, Junctions::*, MultiLocation, Parent, ParentThen};
	use crate::opaque::v1::{Junction::*, NetworkId::*};
	use parity_scale_codec::{Decode, Encode};

	#[test]
	fn inverted_works() {
		let ancestry: MultiLocation = (Parachain(1000), PalletInstance(42)).into();
		let target = (Parent, PalletInstance(69)).into();
		let expected = (Parent, PalletInstance(42)).into();
		let inverted = ancestry.inverted(&target).unwrap();
		assert_eq!(inverted, expected);

		let ancestry: MultiLocation = (Parachain(1000), PalletInstance(42), GeneralIndex(1)).into();
		let target = (Parent, Parent, PalletInstance(69), GeneralIndex(2)).into();
		let expected = (Parent, Parent, PalletInstance(42), GeneralIndex(1)).into();
		let inverted = ancestry.inverted(&target).unwrap();
		assert_eq!(inverted, expected);
	}

	#[test]
	fn simplify_basic_works() {
		let mut location: MultiLocation =
			(Parent, Parent, Parachain(1000), PalletInstance(42), GeneralIndex(69)).into();
		let context = X2(Parachain(1000), PalletInstance(42));
		let expected = GeneralIndex(69).into();
		location.simplify(&context);
		assert_eq!(location, expected);

		let mut location: MultiLocation = (Parent, PalletInstance(42), GeneralIndex(69)).into();
		let context = X1(PalletInstance(42));
		let expected = GeneralIndex(69).into();
		location.simplify(&context);
		assert_eq!(location, expected);

		let mut location: MultiLocation = (Parent, PalletInstance(42), GeneralIndex(69)).into();
		let context = X2(Parachain(1000), PalletInstance(42));
		let expected = GeneralIndex(69).into();
		location.simplify(&context);
		assert_eq!(location, expected);

		let mut location: MultiLocation =
			(Parent, Parent, Parachain(1000), PalletInstance(42), GeneralIndex(69)).into();
		let context = X3(OnlyChild, Parachain(1000), PalletInstance(42));
		let expected = GeneralIndex(69).into();
		location.simplify(&context);
		assert_eq!(location, expected);
	}

	#[test]
	fn simplify_incompatible_location_fails() {
		let mut location: MultiLocation =
			(Parent, Parent, Parachain(1000), PalletInstance(42), GeneralIndex(69)).into();
		let context = X3(Parachain(1000), PalletInstance(42), GeneralIndex(42));
		let expected =
			(Parent, Parent, Parachain(1000), PalletInstance(42), GeneralIndex(69)).into();
		location.simplify(&context);
		assert_eq!(location, expected);

		let mut location: MultiLocation =
			(Parent, Parent, Parachain(1000), PalletInstance(42), GeneralIndex(69)).into();
		let context = X1(Parachain(1000));
		let expected =
			(Parent, Parent, Parachain(1000), PalletInstance(42), GeneralIndex(69)).into();
		location.simplify(&context);
		assert_eq!(location, expected);
	}

	#[test]
	fn reanchor_works() {
		let mut id: MultiLocation = (Parent, Parachain(1000), GeneralIndex(42)).into();
		let ancestry = Parachain(2000).into();
		let target = (Parent, Parachain(1000)).into();
		let expected = GeneralIndex(42).into();
		id.reanchor(&target, &ancestry).unwrap();
		assert_eq!(id, expected);
	}

	#[test]
	fn encode_and_decode_works() {
		let m = MultiLocation {
			parents: 1,
			interior: X2(Parachain(42), AccountIndex64 { network: Any, index: 23 }),
		};
		let encoded = m.encode();
		assert_eq!(encoded, [1, 2, 0, 168, 2, 0, 92].to_vec());
		let decoded = MultiLocation::decode(&mut &encoded[..]);
		assert_eq!(decoded, Ok(m));
	}

	#[test]
	fn match_and_split_works() {
		let m = MultiLocation {
			parents: 1,
			interior: X2(Parachain(42), AccountIndex64 { network: Any, index: 23 }),
		};
		assert_eq!(m.match_and_split(&MultiLocation { parents: 1, interior: Here }), None);
		assert_eq!(
			m.match_and_split(&MultiLocation { parents: 1, interior: X1(Parachain(42)) }),
			Some(&AccountIndex64 { network: Any, index: 23 })
		);
		assert_eq!(m.match_and_split(&m), None);
	}

	#[test]
	fn starts_with_works() {
		let full: MultiLocation =
			(Parent, Parachain(1000), AccountId32 { network: Any, id: [0; 32] }).into();
		let identity: MultiLocation = full.clone();
		let prefix: MultiLocation = (Parent, Parachain(1000)).into();
		let wrong_parachain: MultiLocation = (Parent, Parachain(1001)).into();
		let wrong_account: MultiLocation =
			(Parent, Parachain(1000), AccountId32 { network: Any, id: [1; 32] }).into();
		let no_parents: MultiLocation = (Parachain(1000)).into();
		let too_many_parents: MultiLocation = (Parent, Parent, Parachain(1000)).into();

		assert!(full.starts_with(&identity));
		assert!(full.starts_with(&prefix));
		assert!(!full.starts_with(&wrong_parachain));
		assert!(!full.starts_with(&wrong_account));
		assert!(!full.starts_with(&no_parents));
		assert!(!full.starts_with(&too_many_parents));
	}

	#[test]
	fn append_with_works() {
		let acc = AccountIndex64 { network: Any, index: 23 };
		let mut m = MultiLocation { parents: 1, interior: X1(Parachain(42)) };
		assert_eq!(m.append_with(X2(PalletInstance(3), acc.clone())), Ok(()));
		assert_eq!(
			m,
			MultiLocation {
				parents: 1,
				interior: X3(Parachain(42), PalletInstance(3), acc.clone())
			}
		);

		// cannot append to create overly long multilocation
		let acc = AccountIndex64 { network: Any, index: 23 };
		let m = MultiLocation {
			parents: 254,
			interior: X5(Parachain(42), OnlyChild, OnlyChild, OnlyChild, OnlyChild),
		};
		let suffix = X4(PalletInstance(3), acc.clone(), OnlyChild, OnlyChild);
		assert_eq!(m.clone().append_with(suffix.clone()), Err(suffix));
	}

	#[test]
	fn prepend_with_works() {
		let mut m = MultiLocation {
			parents: 1,
			interior: X2(Parachain(42), AccountIndex64 { network: Any, index: 23 }),
		};
		assert_eq!(m.prepend_with(MultiLocation { parents: 1, interior: X1(OnlyChild) }), Ok(()));
		assert_eq!(
			m,
			MultiLocation {
				parents: 1,
				interior: X2(Parachain(42), AccountIndex64 { network: Any, index: 23 })
			}
		);

		// cannot prepend to create overly long multilocation
		let mut m = MultiLocation { parents: 254, interior: X1(Parachain(42)) };
		let prefix = MultiLocation { parents: 2, interior: Here };
		assert_eq!(m.prepend_with(prefix.clone()), Err(prefix));

		let prefix = MultiLocation { parents: 1, interior: Here };
		assert_eq!(m.prepend_with(prefix), Ok(()));
		assert_eq!(m, MultiLocation { parents: 255, interior: X1(Parachain(42)) });
	}

	#[test]
	fn double_ended_ref_iteration_works() {
		let m = X3(Parachain(1000), Parachain(3), PalletInstance(5));
		let mut iter = m.iter();

		let first = iter.next().unwrap();
		assert_eq!(first, &Parachain(1000));
		let third = iter.next_back().unwrap();
		assert_eq!(third, &PalletInstance(5));
		let second = iter.next_back().unwrap();
		assert_eq!(iter.next(), None);
		assert_eq!(iter.next_back(), None);
		assert_eq!(second, &Parachain(3));

		let res = Here
			.pushed_with(first.clone())
			.unwrap()
			.pushed_with(second.clone())
			.unwrap()
			.pushed_with(third.clone())
			.unwrap();
		assert_eq!(m, res);

		// make sure there's no funny business with the 0 indexing
		let m = Here;
		let mut iter = m.iter();

		assert_eq!(iter.next(), None);
		assert_eq!(iter.next_back(), None);
	}

	#[test]
	fn conversion_from_other_types_works() {
		use crate::v0;
		fn takes_multilocation<Arg: Into<MultiLocation>>(_arg: Arg) {}

		takes_multilocation(Parent);
		takes_multilocation(Here);
		takes_multilocation(X1(Parachain(42)));
		takes_multilocation((255, PalletInstance(8)));
		takes_multilocation((Ancestor(5), Parachain(1), PalletInstance(3)));
		takes_multilocation((Ancestor(2), Here));
		takes_multilocation(AncestorThen(
			3,
			X2(Parachain(43), AccountIndex64 { network: Any, index: 155 }),
		));
		takes_multilocation((Parent, AccountId32 { network: Any, id: [0; 32] }));
		takes_multilocation((Parent, Here));
		takes_multilocation(ParentThen(X1(Parachain(75))));
		takes_multilocation([Parachain(100), PalletInstance(3)]);

		assert_eq!(v0::MultiLocation::Null.try_into(), Ok(MultiLocation::here()));
		assert_eq!(
			v0::MultiLocation::X1(v0::Junction::Parent).try_into(),
			Ok(MultiLocation::parent())
		);
		assert_eq!(
			v0::MultiLocation::X2(v0::Junction::Parachain(88), v0::Junction::Parent).try_into(),
			Ok(MultiLocation::here()),
		);
		assert_eq!(
			v0::MultiLocation::X3(
				v0::Junction::Parent,
				v0::Junction::Parent,
				v0::Junction::GeneralKey(b"foo".to_vec().try_into().unwrap()),
			)
			.try_into(),
			Ok(MultiLocation {
				parents: 2,
				interior: X1(GeneralKey(b"foo".to_vec().try_into().unwrap()))
			}),
		);
	}
}
