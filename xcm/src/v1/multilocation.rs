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
use crate::v0::MultiLocation as MultiLocation0;
use core::{
	convert::{TryFrom, TryInto},
	mem, result,
};
use parity_scale_codec::{Decode, Encode};

/// A relative path between state-bearing consensus systems.
///
/// A location in a consensus system is defined as an *isolatable state machine* held within global consensus. The
/// location in question need not have a sophisticated consensus algorithm of its own; a single account within
/// Ethereum, for example, could be considered a location.
///
/// A very-much non-exhaustive list of types of location include:
/// - A (normal, layer-1) block chain, e.g. the Bitcoin mainnet or a parachain.
/// - A layer-0 super-chain, e.g. the Polkadot Relay chain.
/// - A layer-2 smart contract, e.g. an ERC-20 on Ethereum.
/// - A logical functional component of a chain, e.g. a single instance of a pallet on a Frame-based Substrate chain.
/// - An account.
///
/// A `MultiLocation` is a *relative identifier*, meaning that it can only be used to define the relative path
/// between two locations, and cannot generally be used to refer to a location universally. It is comprised of a
/// number of *junctions*, each morphing the previous location, either diving down into one of its internal locations,
/// called a *sub-consensus*, or going up into its parent location.
///
/// The `parents` field of this struct indicates the number of parent junctions that exist at the
/// beginning of this `MultiLocation`. A corollary of such a property is that no parent junctions
/// can be added in the middle or at the end of a `MultiLocation`, thus ensuring well-formedness
/// of each and every `MultiLocation` that can be constructed.
#[derive(Clone, Decode, Encode, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub struct MultiLocation {
	/// The number of parent junctions at the beginning of this `MultiLocation`.
	pub parents: u8,
	/// The interior (i.e. non-parent) junctions that this `MultiLocation` contains.
	pub interior: Junctions,
}

impl Default for MultiLocation {
	fn default() -> Self {
		Self::here()
	}
}

impl MultiLocation {
	/// Creates a new `MultiLocation` with the given number of parents and interior junctions.
	pub fn new(parents: u8, junctions: Junctions) -> MultiLocation {
		MultiLocation { parents, interior: junctions }
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

	/// Whether or not the `MultiLocation` has no parents and has a `Here` interior.
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

	/// Returns boolean indicating whether or not `self` contains only the specified amount of
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
}

/// A unit struct which can be converted into a `MultiLocation` of `parents` value 1.
pub struct Parent;
impl From<Parent> for MultiLocation {
	fn from(_: Parent) -> Self {
		MultiLocation { parents: 1, interior: Junctions::Here }
	}
}

/// A tuple struct which can be converted into a `MultiLocation` of `parents` value 1 with the inner interior.
pub struct ParentThen(Junctions);
impl From<ParentThen> for MultiLocation {
	fn from(x: ParentThen) -> Self {
		MultiLocation { parents: 1, interior: x.0 }
	}
}

/// A unit struct which can be converted into a `MultiLocation` of the inner `parents` value.
pub struct Ancestor(u8);
impl From<Ancestor> for MultiLocation {
	fn from(x: Ancestor) -> Self {
		MultiLocation { parents: x.0, interior: Junctions::Here }
	}
}

/// A unit struct which can be converted into a `MultiLocation` of the inner `parents` value and the inner interior.
pub struct AncestorThen(u8, Junctions);
impl From<AncestorThen> for MultiLocation {
	fn from(x: AncestorThen) -> Self {
		MultiLocation { parents: x.0, interior: x.1 }
	}
}

impl From<Junctions> for MultiLocation {
	fn from(junctions: Junctions) -> Self {
		MultiLocation { parents: 0, interior: junctions }
	}
}

impl From<(u8, Junctions)> for MultiLocation {
	fn from((parents, interior): (u8, Junctions)) -> Self {
		MultiLocation { parents, interior }
	}
}

impl From<Junction> for MultiLocation {
	fn from(x: Junction) -> Self {
		MultiLocation { parents: 0, interior: Junctions::X1(x) }
	}
}

impl From<()> for MultiLocation {
	fn from(_: ()) -> Self {
		MultiLocation { parents: 0, interior: Junctions::Here }
	}
}
impl From<(Junction,)> for MultiLocation {
	fn from(x: (Junction,)) -> Self {
		MultiLocation { parents: 0, interior: Junctions::X1(x.0) }
	}
}
impl From<(Junction, Junction)> for MultiLocation {
	fn from(x: (Junction, Junction)) -> Self {
		MultiLocation { parents: 0, interior: Junctions::X2(x.0, x.1) }
	}
}
impl From<(Junction, Junction, Junction)> for MultiLocation {
	fn from(x: (Junction, Junction, Junction)) -> Self {
		MultiLocation { parents: 0, interior: Junctions::X3(x.0, x.1, x.2) }
	}
}
impl From<(Junction, Junction, Junction, Junction)> for MultiLocation {
	fn from(x: (Junction, Junction, Junction, Junction)) -> Self {
		MultiLocation { parents: 0, interior: Junctions::X4(x.0, x.1, x.2, x.3) }
	}
}
impl From<(Junction, Junction, Junction, Junction, Junction)> for MultiLocation {
	fn from(x: (Junction, Junction, Junction, Junction, Junction)) -> Self {
		MultiLocation { parents: 0, interior: Junctions::X5(x.0, x.1, x.2, x.3, x.4) }
	}
}
impl From<(Junction, Junction, Junction, Junction, Junction, Junction)> for MultiLocation {
	fn from(x: (Junction, Junction, Junction, Junction, Junction, Junction)) -> Self {
		MultiLocation { parents: 0, interior: Junctions::X6(x.0, x.1, x.2, x.3, x.4, x.5) }
	}
}
impl From<(Junction, Junction, Junction, Junction, Junction, Junction, Junction)>
	for MultiLocation
{
	fn from(x: (Junction, Junction, Junction, Junction, Junction, Junction, Junction)) -> Self {
		MultiLocation { parents: 0, interior: Junctions::X7(x.0, x.1, x.2, x.3, x.4, x.5, x.6) }
	}
}
impl From<(Junction, Junction, Junction, Junction, Junction, Junction, Junction, Junction)>
	for MultiLocation
{
	fn from(
		x: (Junction, Junction, Junction, Junction, Junction, Junction, Junction, Junction),
	) -> Self {
		MultiLocation {
			parents: 0,
			interior: Junctions::X8(x.0, x.1, x.2, x.3, x.4, x.5, x.6, x.7),
		}
	}
}

impl From<(u8,)> for MultiLocation {
	fn from((parents,): (u8,)) -> Self {
		MultiLocation { parents, interior: Junctions::Here }
	}
}
impl From<(u8, Junction)> for MultiLocation {
	fn from((parents, j0): (u8, Junction)) -> Self {
		MultiLocation { parents, interior: Junctions::X1(j0) }
	}
}
impl From<(u8, Junction, Junction)> for MultiLocation {
	fn from((parents, j0, j1): (u8, Junction, Junction)) -> Self {
		MultiLocation { parents, interior: Junctions::X2(j0, j1) }
	}
}
impl From<(u8, Junction, Junction, Junction)> for MultiLocation {
	fn from((parents, j0, j1, j2): (u8, Junction, Junction, Junction)) -> Self {
		MultiLocation { parents, interior: Junctions::X3(j0, j1, j2) }
	}
}
impl From<(u8, Junction, Junction, Junction, Junction)> for MultiLocation {
	fn from((parents, j0, j1, j2, j3): (u8, Junction, Junction, Junction, Junction)) -> Self {
		MultiLocation { parents, interior: Junctions::X4(j0, j1, j2, j3) }
	}
}
impl From<(u8, Junction, Junction, Junction, Junction, Junction)> for MultiLocation {
	fn from(
		(parents, j0, j1, j2, j3, j4): (u8, Junction, Junction, Junction, Junction, Junction),
	) -> Self {
		MultiLocation { parents, interior: Junctions::X5(j0, j1, j2, j3, j4) }
	}
}
impl From<(u8, Junction, Junction, Junction, Junction, Junction, Junction)> for MultiLocation {
	fn from(
		(parents, j0, j1, j2, j3, j4, j5): (
			u8,
			Junction,
			Junction,
			Junction,
			Junction,
			Junction,
			Junction,
		),
	) -> Self {
		MultiLocation { parents, interior: Junctions::X6(j0, j1, j2, j3, j4, j5) }
	}
}
impl From<(u8, Junction, Junction, Junction, Junction, Junction, Junction, Junction)>
	for MultiLocation
{
	fn from(
		(parents, j0, j1, j2, j3, j4, j5, j6): (
			u8,
			Junction,
			Junction,
			Junction,
			Junction,
			Junction,
			Junction,
			Junction,
		),
	) -> Self {
		MultiLocation { parents, interior: Junctions::X7(j0, j1, j2, j3, j4, j5, j6) }
	}
}
impl From<(u8, Junction, Junction, Junction, Junction, Junction, Junction, Junction, Junction)>
	for MultiLocation
{
	fn from(
		(parents, j0, j1, j2, j3, j4, j5, j6, j7): (
			u8,
			Junction,
			Junction,
			Junction,
			Junction,
			Junction,
			Junction,
			Junction,
			Junction,
		),
	) -> Self {
		MultiLocation { parents, interior: Junctions::X8(j0, j1, j2, j3, j4, j5, j6, j7) }
	}
}

impl From<[Junction; 0]> for MultiLocation {
	fn from(_: [Junction; 0]) -> Self {
		MultiLocation { parents: 0, interior: Junctions::Here }
	}
}
impl From<[Junction; 1]> for MultiLocation {
	fn from(x: [Junction; 1]) -> Self {
		let [x0] = x;
		MultiLocation { parents: 0, interior: Junctions::X1(x0) }
	}
}
impl From<[Junction; 2]> for MultiLocation {
	fn from(x: [Junction; 2]) -> Self {
		let [x0, x1] = x;
		MultiLocation { parents: 0, interior: Junctions::X2(x0, x1) }
	}
}
impl From<[Junction; 3]> for MultiLocation {
	fn from(x: [Junction; 3]) -> Self {
		let [x0, x1, x2] = x;
		MultiLocation { parents: 0, interior: Junctions::X3(x0, x1, x2) }
	}
}
impl From<[Junction; 4]> for MultiLocation {
	fn from(x: [Junction; 4]) -> Self {
		let [x0, x1, x2, x3] = x;
		MultiLocation { parents: 0, interior: Junctions::X4(x0, x1, x2, x3) }
	}
}
impl From<[Junction; 5]> for MultiLocation {
	fn from(x: [Junction; 5]) -> Self {
		let [x0, x1, x2, x3, x4] = x;
		MultiLocation { parents: 0, interior: Junctions::X5(x0, x1, x2, x3, x4) }
	}
}
impl From<[Junction; 6]> for MultiLocation {
	fn from(x: [Junction; 6]) -> Self {
		let [x0, x1, x2, x3, x4, x5] = x;
		MultiLocation { parents: 0, interior: Junctions::X6(x0, x1, x2, x3, x4, x5) }
	}
}
impl From<[Junction; 7]> for MultiLocation {
	fn from(x: [Junction; 7]) -> Self {
		let [x0, x1, x2, x3, x4, x5, x6] = x;
		MultiLocation { parents: 0, interior: Junctions::X7(x0, x1, x2, x3, x4, x5, x6) }
	}
}
impl From<[Junction; 8]> for MultiLocation {
	fn from(x: [Junction; 8]) -> Self {
		let [x0, x1, x2, x3, x4, x5, x6, x7] = x;
		MultiLocation { parents: 0, interior: Junctions::X8(x0, x1, x2, x3, x4, x5, x6, x7) }
	}
}

/// Maximum number of `Junction`s that a `Junctions` can contain.
const MAX_JUNCTIONS: usize = 8;

/// Non-parent junctions that can be constructed, up to the length of 8. This specific `Junctions`
/// implementation uses a Rust `enum` in order to make pattern matching easier.
///
/// Parent junctions cannot be constructed with this type. Refer to `MultiLocation` for
/// instructions on constructing parent junctions.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug)]
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
	/// # use xcm::latest::{Junctions::*, Junction::*};
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

impl TryFrom<MultiLocation0> for MultiLocation {
	type Error = ();
	fn try_from(old: MultiLocation0) -> result::Result<Self, ()> {
		use Junctions::*;
		match old {
			MultiLocation0::Null => Ok(Here.into()),
			MultiLocation0::X1(j0) if j0.is_parent() => Ok(Parent.into()),
			MultiLocation0::X1(j0) => Ok(X1(j0.try_into()?).into()),
			MultiLocation0::X2(j0, j1) if j0.is_parent() && j1.is_parent() =>
				Ok(MultiLocation::grandparent()),
			MultiLocation0::X2(j0, j1) if j0.is_parent() =>
				Ok(MultiLocation { parents: 1, interior: X1(j1.try_into()?) }),
			MultiLocation0::X2(j0, j1) => Ok(X2(j0.try_into()?, j1.try_into()?).into()),
			MultiLocation0::X3(j0, j1, j2)
				if j0.is_parent() && j1.is_parent() && j2.is_parent() =>
				Ok(MultiLocation::ancestor(3)),
			MultiLocation0::X3(j0, j1, j2) if j0.is_parent() && j1.is_parent() =>
				Ok(MultiLocation { parents: 2, interior: X1(j2.try_into()?) }),
			MultiLocation0::X3(j0, j1, j2) if j0.is_parent() =>
				Ok(MultiLocation { parents: 1, interior: X2(j1.try_into()?, j2.try_into()?) }),
			MultiLocation0::X3(j0, j1, j2) =>
				Ok(X3(j0.try_into()?, j1.try_into()?, j2.try_into()?).into()),
			MultiLocation0::X4(j0, j1, j2, j3)
				if j0.is_parent() && j1.is_parent() && j2.is_parent() && j3.is_parent() =>
				Ok(MultiLocation::ancestor(4)),
			MultiLocation0::X4(j0, j1, j2, j3)
				if j0.is_parent() && j1.is_parent() && j2.is_parent() =>
				Ok(MultiLocation { parents: 3, interior: X1(j3.try_into()?) }),
			MultiLocation0::X4(j0, j1, j2, j3) if j0.is_parent() && j1.is_parent() =>
				Ok(MultiLocation { parents: 2, interior: X2(j2.try_into()?, j3.try_into()?) }),
			MultiLocation0::X4(j0, j1, j2, j3) if j0.is_parent() => Ok(MultiLocation {
				parents: 1,
				interior: X3(j1.try_into()?, j2.try_into()?, j3.try_into()?),
			}),
			MultiLocation0::X4(j0, j1, j2, j3) =>
				Ok(X4(j0.try_into()?, j1.try_into()?, j2.try_into()?, j3.try_into()?).into()),
			MultiLocation0::X5(j0, j1, j2, j3, j4)
				if j0.is_parent() &&
					j1.is_parent() && j2.is_parent() &&
					j3.is_parent() && j4.is_parent() =>
				Ok(MultiLocation::ancestor(5)),
			MultiLocation0::X5(j0, j1, j2, j3, j4)
				if j0.is_parent() && j1.is_parent() && j2.is_parent() && j3.is_parent() =>
				Ok(MultiLocation { parents: 4, interior: X1(j4.try_into()?) }),
			MultiLocation0::X5(j0, j1, j2, j3, j4)
				if j0.is_parent() && j1.is_parent() && j2.is_parent() =>
				Ok(MultiLocation { parents: 3, interior: X2(j3.try_into()?, j4.try_into()?) }),
			MultiLocation0::X5(j0, j1, j2, j3, j4) if j0.is_parent() && j1.is_parent() =>
				Ok(MultiLocation {
					parents: 2,
					interior: X3(j2.try_into()?, j3.try_into()?, j4.try_into()?),
				}),
			MultiLocation0::X5(j0, j1, j2, j3, j4) if j0.is_parent() => Ok(MultiLocation {
				parents: 1,
				interior: X4(j1.try_into()?, j2.try_into()?, j3.try_into()?, j4.try_into()?),
			}),
			MultiLocation0::X5(j0, j1, j2, j3, j4) => Ok(X5(
				j0.try_into()?,
				j1.try_into()?,
				j2.try_into()?,
				j3.try_into()?,
				j4.try_into()?,
			)
			.into()),
			MultiLocation0::X6(j0, j1, j2, j3, j4, j5)
				if j0.is_parent() &&
					j1.is_parent() && j2.is_parent() &&
					j3.is_parent() && j4.is_parent() &&
					j5.is_parent() =>
				Ok(MultiLocation::ancestor(6)),
			MultiLocation0::X6(j0, j1, j2, j3, j4, j5)
				if j0.is_parent() &&
					j1.is_parent() && j2.is_parent() &&
					j3.is_parent() && j4.is_parent() =>
				Ok(MultiLocation { parents: 5, interior: X1(j5.try_into()?) }),
			MultiLocation0::X6(j0, j1, j2, j3, j4, j5)
				if j0.is_parent() && j1.is_parent() && j2.is_parent() && j3.is_parent() =>
				Ok(MultiLocation { parents: 4, interior: X2(j4.try_into()?, j5.try_into()?) }),
			MultiLocation0::X6(j0, j1, j2, j3, j4, j5)
				if j0.is_parent() && j1.is_parent() && j2.is_parent() =>
				Ok(MultiLocation {
					parents: 3,
					interior: X3(j3.try_into()?, j4.try_into()?, j5.try_into()?),
				}),
			MultiLocation0::X6(j0, j1, j2, j3, j4, j5) if j0.is_parent() && j1.is_parent() =>
				Ok(MultiLocation {
					parents: 2,
					interior: X4(j2.try_into()?, j3.try_into()?, j4.try_into()?, j5.try_into()?),
				}),
			MultiLocation0::X6(j0, j1, j2, j3, j4, j5) if j0.is_parent() => Ok(MultiLocation {
				parents: 1,
				interior: X5(
					j1.try_into()?,
					j2.try_into()?,
					j3.try_into()?,
					j4.try_into()?,
					j5.try_into()?,
				),
			}),
			MultiLocation0::X6(j0, j1, j2, j3, j4, j5) => Ok(X6(
				j0.try_into()?,
				j1.try_into()?,
				j2.try_into()?,
				j3.try_into()?,
				j4.try_into()?,
				j5.try_into()?,
			)
			.into()),
			MultiLocation0::X7(j0, j1, j2, j3, j4, j5, j6)
				if j0.is_parent() &&
					j1.is_parent() && j2.is_parent() &&
					j3.is_parent() && j4.is_parent() &&
					j5.is_parent() && j6.is_parent() =>
				Ok(MultiLocation::ancestor(7)),
			MultiLocation0::X7(j0, j1, j2, j3, j4, j5, j6)
				if j0.is_parent() &&
					j1.is_parent() && j2.is_parent() &&
					j3.is_parent() && j4.is_parent() &&
					j5.is_parent() =>
				Ok(MultiLocation { parents: 6, interior: X1(j6.try_into()?) }),
			MultiLocation0::X7(j0, j1, j2, j3, j4, j5, j6)
				if j0.is_parent() &&
					j1.is_parent() && j2.is_parent() &&
					j3.is_parent() && j4.is_parent() =>
				Ok(MultiLocation { parents: 5, interior: X2(j5.try_into()?, j6.try_into()?) }),
			MultiLocation0::X7(j0, j1, j2, j3, j4, j5, j6)
				if j0.is_parent() && j1.is_parent() && j2.is_parent() && j3.is_parent() =>
				Ok(MultiLocation {
					parents: 4,
					interior: X3(j4.try_into()?, j5.try_into()?, j6.try_into()?),
				}),
			MultiLocation0::X7(j0, j1, j2, j3, j4, j5, j6)
				if j0.is_parent() && j1.is_parent() && j2.is_parent() =>
				Ok(MultiLocation {
					parents: 3,
					interior: X4(j3.try_into()?, j4.try_into()?, j5.try_into()?, j6.try_into()?),
				}),
			MultiLocation0::X7(j0, j1, j2, j3, j4, j5, j6) if j0.is_parent() && j1.is_parent() =>
				Ok(MultiLocation {
					parents: 2,
					interior: X5(
						j2.try_into()?,
						j3.try_into()?,
						j4.try_into()?,
						j5.try_into()?,
						j6.try_into()?,
					),
				}),
			MultiLocation0::X7(j0, j1, j2, j3, j4, j5, j6) if j0.is_parent() => Ok(MultiLocation {
				parents: 1,
				interior: X6(
					j1.try_into()?,
					j2.try_into()?,
					j3.try_into()?,
					j4.try_into()?,
					j5.try_into()?,
					j6.try_into()?,
				),
			}),
			MultiLocation0::X7(j0, j1, j2, j3, j4, j5, j6) => Ok(X7(
				j0.try_into()?,
				j1.try_into()?,
				j2.try_into()?,
				j3.try_into()?,
				j4.try_into()?,
				j5.try_into()?,
				j6.try_into()?,
			)
			.into()),
			MultiLocation0::X8(j0, j1, j2, j3, j4, j5, j6, j7)
				if j0.is_parent() &&
					j1.is_parent() && j2.is_parent() &&
					j3.is_parent() && j4.is_parent() &&
					j5.is_parent() && j6.is_parent() &&
					j7.is_parent() =>
				Ok(MultiLocation::ancestor(8)),
			MultiLocation0::X8(j0, j1, j2, j3, j4, j5, j6, j7)
				if j0.is_parent() &&
					j1.is_parent() && j2.is_parent() &&
					j3.is_parent() && j4.is_parent() &&
					j5.is_parent() && j6.is_parent() =>
				Ok(MultiLocation { parents: 7, interior: X1(j7.try_into()?) }),
			MultiLocation0::X8(j0, j1, j2, j3, j4, j5, j6, j7)
				if j0.is_parent() &&
					j1.is_parent() && j2.is_parent() &&
					j3.is_parent() && j4.is_parent() &&
					j5.is_parent() =>
				Ok(MultiLocation { parents: 6, interior: X2(j6.try_into()?, j7.try_into()?) }),
			MultiLocation0::X8(j0, j1, j2, j3, j4, j5, j6, j7)
				if j0.is_parent() &&
					j1.is_parent() && j2.is_parent() &&
					j3.is_parent() && j4.is_parent() =>
				Ok(MultiLocation {
					parents: 5,
					interior: X3(j5.try_into()?, j6.try_into()?, j7.try_into()?),
				}),
			MultiLocation0::X8(j0, j1, j2, j3, j4, j5, j6, j7)
				if j0.is_parent() && j1.is_parent() && j2.is_parent() && j3.is_parent() =>
				Ok(MultiLocation {
					parents: 4,
					interior: X4(j4.try_into()?, j5.try_into()?, j6.try_into()?, j7.try_into()?),
				}),
			MultiLocation0::X8(j0, j1, j2, j3, j4, j5, j6, j7)
				if j0.is_parent() && j1.is_parent() && j2.is_parent() =>
				Ok(MultiLocation {
					parents: 3,
					interior: X5(
						j3.try_into()?,
						j4.try_into()?,
						j5.try_into()?,
						j6.try_into()?,
						j7.try_into()?,
					),
				}),
			MultiLocation0::X8(j0, j1, j2, j3, j4, j5, j6, j7)
				if j0.is_parent() && j1.is_parent() =>
				Ok(MultiLocation {
					parents: 2,
					interior: X6(
						j2.try_into()?,
						j3.try_into()?,
						j4.try_into()?,
						j5.try_into()?,
						j6.try_into()?,
						j7.try_into()?,
					),
				}),
			MultiLocation0::X8(j0, j1, j2, j3, j4, j5, j6, j7) if j0.is_parent() =>
				Ok(MultiLocation {
					parents: 1,
					interior: X7(
						j1.try_into()?,
						j2.try_into()?,
						j3.try_into()?,
						j4.try_into()?,
						j5.try_into()?,
						j6.try_into()?,
						j7.try_into()?,
					),
				}),
			MultiLocation0::X8(j0, j1, j2, j3, j4, j5, j6, j7) => Ok(X8(
				j0.try_into()?,
				j1.try_into()?,
				j2.try_into()?,
				j3.try_into()?,
				j4.try_into()?,
				j5.try_into()?,
				j6.try_into()?,
				j7.try_into()?,
			)
			.into()),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{Junctions::*, MultiLocation};
	use crate::opaque::v1::{Junction::*, NetworkId::Any};
	use parity_scale_codec::{Decode, Encode};

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
}
