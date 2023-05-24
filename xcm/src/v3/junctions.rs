// Copyright (C) Parity Technologies (UK) Ltd.
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

use super::{Junction, MultiLocation, NetworkId};
use alloc::sync::Arc;
use core::{convert::TryFrom, mem, ops::Range, result};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// Maximum number of `Junction`s that a `Junctions` can contain.
pub(crate) const MAX_JUNCTIONS: usize = 8;

/// Non-parent junctions that can be constructed, up to the length of 8. This specific `Junctions`
/// implementation uses a Rust `enum` in order to make pattern matching easier.
///
/// Parent junctions cannot be constructed with this type. Refer to `MultiLocation` for
/// instructions on constructing parent junctions.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug, TypeInfo)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum Junctions {
	/// The interpreting consensus system.
	Here,
	/// A relative path comprising 1 junction.
	X1(Arc<[Junction; 1]>),
	/// A relative path comprising 2 junctions.
	X2(Arc<[Junction; 2]>),
	/// A relative path comprising 3 junctions.
	X3(Arc<[Junction; 3]>),
	/// A relative path comprising 4 junctions.
	X4(Arc<[Junction; 4]>),
	/// A relative path comprising 5 junctions.
	X5(Arc<[Junction; 5]>),
	/// A relative path comprising 6 junctions.
	X6(Arc<[Junction; 6]>),
	/// A relative path comprising 7 junctions.
	X7(Arc<[Junction; 7]>),
	/// A relative path comprising 8 junctions.
	X8(Arc<[Junction; 8]>),
}

// TODO: Remove this once deriving `MaxEncodedLen` on `Arc`s works.
impl MaxEncodedLen for Junctions {
	fn max_encoded_len() -> usize {
		<[Junction; 8]>::max_encoded_len().saturating_add(1)
	}
}

macro_rules! impl_junctions {
	($count:expr, $variant:ident) => {
		impl From<[Junction; $count]> for Junctions {
			fn from(junctions: [Junction; $count]) -> Self {
				Self::$variant(Arc::new(junctions))
			}
		}
		impl PartialEq<[Junction; $count]> for Junctions {
			fn eq(&self, rhs: &[Junction; $count]) -> bool {
				self.as_slice() == rhs
			}
		}
	};
}

impl_junctions!(1, X1);
impl_junctions!(2, X2);
impl_junctions!(3, X3);
impl_junctions!(4, X4);
impl_junctions!(5, X5);
impl_junctions!(6, X6);
impl_junctions!(7, X7);
impl_junctions!(8, X8);

pub struct JunctionsIterator {
	junctions: Junctions,
	range: Range<usize>,
}

impl Iterator for JunctionsIterator {
	type Item = Junction;
	fn next(&mut self) -> Option<Junction> {
		self.junctions.at(self.range.next()?).cloned()
	}
}

impl DoubleEndedIterator for JunctionsIterator {
	fn next_back(&mut self) -> Option<Junction> {
		self.junctions.at(self.range.next_back()?).cloned()
	}
}

pub struct JunctionsRefIterator<'a> {
	junctions: &'a Junctions,
	range: Range<usize>,
}

impl<'a> Iterator for JunctionsRefIterator<'a> {
	type Item = &'a Junction;
	fn next(&mut self) -> Option<&'a Junction> {
		self.junctions.at(self.range.next()?)
	}
}

impl<'a> DoubleEndedIterator for JunctionsRefIterator<'a> {
	fn next_back(&mut self) -> Option<&'a Junction> {
		self.junctions.at(self.range.next_back()?)
	}
}
impl<'a> IntoIterator for &'a Junctions {
	type Item = &'a Junction;
	type IntoIter = JunctionsRefIterator<'a>;
	fn into_iter(self) -> Self::IntoIter {
		JunctionsRefIterator { junctions: self, range: 0..self.len() }
	}
}

impl IntoIterator for Junctions {
	type Item = Junction;
	type IntoIter = JunctionsIterator;
	fn into_iter(self) -> Self::IntoIter {
		JunctionsIterator { range: 0..self.len(), junctions: self }
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
	/// Similar to `Self::into_location`, with the added ability to specify the number of parent junctions.
	pub const fn into_exterior(self, n: u8) -> MultiLocation {
		MultiLocation { parents: n, interior: self }
	}

	/// Casts `self` into a slice containing `Junction`s.
	pub fn as_slice(&self) -> &[Junction] {
		match self {
			Junctions::Here => &[],
			Junctions::X1(ref a) => &a[..],
			Junctions::X2(ref a) => &a[..],
			Junctions::X3(ref a) => &a[..],
			Junctions::X4(ref a) => &a[..],
			Junctions::X5(ref a) => &a[..],
			Junctions::X6(ref a) => &a[..],
			Junctions::X7(ref a) => &a[..],
			Junctions::X8(ref a) => &a[..],
		}
	}

	/// Casts `self` into a mutable slice containing `Junction`s.
	pub fn as_slice_mut(&mut self) -> &mut [Junction] {
		match self {
			Junctions::Here => &mut [],
			Junctions::X1(ref mut a) => &mut Arc::make_mut(a)[..],
			Junctions::X2(ref mut a) => &mut Arc::make_mut(a)[..],
			Junctions::X3(ref mut a) => &mut Arc::make_mut(a)[..],
			Junctions::X4(ref mut a) => &mut Arc::make_mut(a)[..],
			Junctions::X5(ref mut a) => &mut Arc::make_mut(a)[..],
			Junctions::X6(ref mut a) => &mut Arc::make_mut(a)[..],
			Junctions::X7(ref mut a) => &mut Arc::make_mut(a)[..],
			Junctions::X8(ref mut a) => &mut Arc::make_mut(a)[..],
		}
	}

	/// Remove the `NetworkId` value in any `Junction`s.
	pub fn remove_network_id(&mut self) {
		self.for_each_mut(Junction::remove_network_id);
	}

	/// Treating `self` as the universal context, return the location of the local consensus system
	/// from the point of view of the given `target`.
	pub fn invert_target(&self, target: &MultiLocation) -> Result<MultiLocation, ()> {
		let mut itself = self.clone();
		let mut junctions = Self::Here;
		for _ in 0..target.parent_count() {
			junctions = junctions
				.pushed_front_with(itself.take_last().unwrap_or(Junction::OnlyChild))
				.map_err(|_| ())?;
		}
		let parents = target.interior().len() as u8;
		Ok(MultiLocation::new(parents, junctions))
	}

	/// Execute a function `f` on every junction. We use this since we cannot implement a mutable
	/// `Iterator` without unsafe code.
	pub fn for_each_mut(&mut self, x: impl FnMut(&mut Junction)) {
		self.as_slice_mut().iter_mut().for_each(x)
	}

	/// Extract the network ID treating this value as a universal location.
	///
	/// This will return an `Err` if the first item is not a `GlobalConsensus`, which would indicate
	/// that this value is not a universal location.
	pub fn global_consensus(&self) -> Result<NetworkId, ()> {
		if let Some(Junction::GlobalConsensus(network)) = self.first() {
			Ok(*network)
		} else {
			Err(())
		}
	}

	/// Extract the network ID and the interior consensus location, treating this value as a
	/// universal location.
	///
	/// This will return an `Err` if the first item is not a `GlobalConsensus`, which would indicate
	/// that this value is not a universal location.
	pub fn split_global(self) -> Result<(NetworkId, Junctions), ()> {
		match self.split_first() {
			(location, Some(Junction::GlobalConsensus(network))) => Ok((network, location)),
			_ => return Err(()),
		}
	}

	/// Treat `self` as a universal location and the context of `relative`, returning the universal
	/// location of relative.
	///
	/// This will return an error if `relative` has as many (or more) parents than there are
	/// junctions in `self`, implying that relative refers into a different global consensus.
	pub fn within_global(mut self, relative: MultiLocation) -> Result<Self, ()> {
		if self.len() <= relative.parents as usize {
			return Err(())
		}
		for _ in 0..relative.parents {
			self.take_last();
		}
		for j in relative.interior {
			self.push(j).map_err(|_| ())?;
		}
		Ok(self)
	}

	/// Consumes `self` and returns how `viewer` would address it locally.
	pub fn relative_to(mut self, viewer: &Junctions) -> MultiLocation {
		let mut i = 0;
		while match (self.first(), viewer.at(i)) {
			(Some(x), Some(y)) => x == y,
			_ => false,
		} {
			self = self.split_first().0;
			// NOTE: Cannot overflow as loop can only iterate at most `MAX_JUNCTIONS` times.
			i += 1;
		}
		// AUDIT NOTES:
		// - above loop ensures that `i <= viewer.len()`.
		// - `viewer.len()` is at most `MAX_JUNCTIONS`, so won't overflow a `u8`.
		MultiLocation { parents: (viewer.len() - i) as u8, interior: self }
	}

	/// Returns first junction, or `None` if the location is empty.
	pub fn first(&self) -> Option<&Junction> {
		self.as_slice().first()
	}

	/// Returns last junction, or `None` if the location is empty.
	pub fn last(&self) -> Option<&Junction> {
		self.as_slice().last()
	}

	/// Splits off the first junction, returning the remaining suffix (first item in tuple) and the first element
	/// (second item in tuple) or `None` if it was empty.
	pub fn split_first(self) -> (Junctions, Option<Junction>) {
		match self {
			Junctions::Here => (Junctions::Here, None),
			Junctions::X1(xs) => {
				let [a] = *xs;
				(Junctions::Here, Some(a))
			},
			Junctions::X2(xs) => {
				let [a, b] = *xs;
				([b].into(), Some(a))
			},
			Junctions::X3(xs) => {
				let [a, b, c] = *xs;
				([b, c].into(), Some(a))
			},
			Junctions::X4(xs) => {
				let [a, b, c, d] = *xs;
				([b, c, d].into(), Some(a))
			},
			Junctions::X5(xs) => {
				let [a, b, c, d, e] = *xs;
				([b, c, d, e].into(), Some(a))
			},
			Junctions::X6(xs) => {
				let [a, b, c, d, e, f] = *xs;
				([b, c, d, e, f].into(), Some(a))
			},
			Junctions::X7(xs) => {
				let [a, b, c, d, e, f, g] = *xs;
				([b, c, d, e, f, g].into(), Some(a))
			},
			Junctions::X8(xs) => {
				let [a, b, c, d, e, f, g, h] = *xs;
				([b, c, d, e, f, g, h].into(), Some(a))
			},
		}
	}

	/// Splits off the last junction, returning the remaining prefix (first item in tuple) and the last element
	/// (second item in tuple) or `None` if it was empty.
	pub fn split_last(self) -> (Junctions, Option<Junction>) {
		match self {
			Junctions::Here => (Junctions::Here, None),
			Junctions::X1(xs) => {
				let [a] = *xs;
				(Junctions::Here, Some(a))
			},
			Junctions::X2(xs) => {
				let [a, b] = *xs;
				([a].into(), Some(b))
			},
			Junctions::X3(xs) => {
				let [a, b, c] = *xs;
				([a, b].into(), Some(c))
			},
			Junctions::X4(xs) => {
				let [a, b, c, d] = *xs;
				([a, b, c].into(), Some(d))
			},
			Junctions::X5(xs) => {
				let [a, b, c, d, e] = *xs;
				([a, b, c, d].into(), Some(e))
			},
			Junctions::X6(xs) => {
				let [a, b, c, d, e, f] = *xs;
				([a, b, c, d, e].into(), Some(f))
			},
			Junctions::X7(xs) => {
				let [a, b, c, d, e, f, g] = *xs;
				([a, b, c, d, e, f].into(), Some(g))
			},
			Junctions::X8(xs) => {
				let [a, b, c, d, e, f, g, h] = *xs;
				([a, b, c, d, e, f, g].into(), Some(h))
			},
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
			Junctions::Here => [new].into(),
			Junctions::X1(xs) => {
				let [a] = *xs;
				[a, new].into()
			},
			Junctions::X2(xs) => {
				let [a, b] = *xs;
				[a, b, new].into()
			},
			Junctions::X3(xs) => {
				let [a, b, c] = *xs;
				[a, b, c, new].into()
			},
			Junctions::X4(xs) => {
				let [a, b, c, d] = *xs;
				[a, b, c, d, new].into()
			},
			Junctions::X5(xs) => {
				let [a, b, c, d, e] = *xs;
				[a, b, c, d, e, new].into()
			},
			Junctions::X6(xs) => {
				let [a, b, c, d, e, f] = *xs;
				[a, b, c, d, e, f, new].into()
			},
			Junctions::X7(xs) => {
				let [a, b, c, d, e, f, g] = *xs;
				[a, b, c, d, e, f, g, new].into()
			},
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
			Junctions::Here => [new].into(),
			Junctions::X1(xs) => {
				let [a] = *xs;
				[new, a].into()
			},
			Junctions::X2(xs) => {
				let [a, b] = *xs;
				[new, a, b].into()
			},
			Junctions::X3(xs) => {
				let [a, b, c] = *xs;
				[new, a, b, c].into()
			},
			Junctions::X4(xs) => {
				let [a, b, c, d] = *xs;
				[new, a, b, c, d].into()
			},
			Junctions::X5(xs) => {
				let [a, b, c, d, e] = *xs;
				[new, a, b, c, d, e].into()
			},
			Junctions::X6(xs) => {
				let [a, b, c, d, e, f] = *xs;
				[new, a, b, c, d, e, f].into()
			},
			Junctions::X7(xs) => {
				let [a, b, c, d, e, f, g] = *xs;
				[new, a, b, c, d, e, f, g].into()
			},
			s => Err((s, new))?,
		})
	}

	/// Mutate `self` so that it is suffixed with `suffix`.
	///
	/// Does not modify `self` and returns `Err` with `suffix` in case of overflow.
	///
	/// # Example
	/// ```rust
	/// # use xcm::v3::{Junctions, Junction::*, MultiLocation};
	/// # fn main() {
	/// let mut m = Junctions::from([Parachain(21)]);
	/// assert_eq!(m.append_with([PalletInstance(3)]), Ok(()));
	/// assert_eq!(m, [Parachain(21), PalletInstance(3)]);
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
	pub fn len(&self) -> usize {
		self.as_slice().len()
	}

	/// Returns the junction at index `i`, or `None` if the location doesn't contain that many elements.
	pub fn at(&self, i: usize) -> Option<&Junction> {
		self.as_slice().get(i)
	}

	/// Returns a mutable reference to the junction at index `i`, or `None` if the location doesn't contain that many
	/// elements.
	pub fn at_mut(&mut self, i: usize) -> Option<&mut Junction> {
		self.as_slice_mut().get_mut(i)
	}

	/// Returns a reference iterator over the junctions.
	pub fn iter(&self) -> JunctionsRefIterator {
		JunctionsRefIterator { junctions: self, range: 0..self.len() }
	}

	/// Ensures that self begins with `prefix` and that it has a single `Junction` item following.
	/// If so, returns a reference to this `Junction` item.
	///
	/// # Example
	/// ```rust
	/// # use xcm::v3::{Junctions, Junction::*};
	/// # fn main() {
	/// let mut m = Junctions::from([Parachain(2), PalletInstance(3), OnlyChild]);
	/// assert_eq!(m.match_and_split(&[Parachain(2), PalletInstance(3)].into()), Some(&OnlyChild));
	/// assert_eq!(m.match_and_split(&[Parachain(2)].into()), None);
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

	pub fn starts_with(&self, prefix: &Junctions) -> bool {
		prefix.len() <= self.len() && prefix.iter().zip(self.iter()).all(|(x, y)| x == y)
	}
}

impl TryFrom<MultiLocation> for Junctions {
	type Error = MultiLocation;
	fn try_from(x: MultiLocation) -> result::Result<Self, MultiLocation> {
		if x.parents > 0 {
			Err(x)
		} else {
			Ok(x.interior)
		}
	}
}

impl<T: Into<Junction>> From<T> for Junctions {
	fn from(x: T) -> Self {
		[x.into()].into()
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

#[cfg(test)]
mod tests {
	use super::{super::prelude::*, *};

	#[test]
	fn inverting_works() {
		let context: InteriorMultiLocation = (Parachain(1000), PalletInstance(42)).into();
		let target = (Parent, PalletInstance(69)).into();
		let expected = (Parent, PalletInstance(42)).into();
		let inverted = context.invert_target(&target).unwrap();
		assert_eq!(inverted, expected);

		let context: InteriorMultiLocation =
			(Parachain(1000), PalletInstance(42), GeneralIndex(1)).into();
		let target = (Parent, Parent, PalletInstance(69), GeneralIndex(2)).into();
		let expected = (Parent, Parent, PalletInstance(42), GeneralIndex(1)).into();
		let inverted = context.invert_target(&target).unwrap();
		assert_eq!(inverted, expected);
	}

	#[test]
	fn relative_to_works() {
		use NetworkId::*;
		assert_eq!(
			Junctions::from([Polkadot.into()]).relative_to(&Junctions::from([Kusama.into()])),
			(Parent, Polkadot).into()
		);
		let base = Junctions::from([Kusama.into(), Parachain(1), PalletInstance(1)]);

		// Ancestors.
		assert_eq!(Here.relative_to(&base), (Parent, Parent, Parent).into());
		assert_eq!(Junctions::from([Kusama.into()]).relative_to(&base), (Parent, Parent).into());
		assert_eq!(
			Junctions::from([Kusama.into(), Parachain(1)]).relative_to(&base),
			(Parent,).into()
		);
		assert_eq!(
			Junctions::from([Kusama.into(), Parachain(1), PalletInstance(1)]).relative_to(&base),
			Here.into()
		);

		// Ancestors with one child.
		assert_eq!(
			Junctions::from([Polkadot.into()]).relative_to(&base),
			(Parent, Parent, Parent, Polkadot).into()
		);
		assert_eq!(
			Junctions::from([Kusama.into(), Parachain(2)]).relative_to(&base),
			(Parent, Parent, Parachain(2)).into()
		);
		assert_eq!(
			Junctions::from([Kusama.into(), Parachain(1), PalletInstance(2)]).relative_to(&base),
			(Parent, PalletInstance(2)).into()
		);
		assert_eq!(
			Junctions::from([Kusama.into(), Parachain(1), PalletInstance(1), [1u8; 32].into()])
				.relative_to(&base),
			([1u8; 32],).into()
		);

		// Ancestors with grandchildren.
		assert_eq!(
			Junctions::from([Polkadot.into(), Parachain(1)]).relative_to(&base),
			(Parent, Parent, Parent, Polkadot, Parachain(1)).into()
		);
		assert_eq!(
			Junctions::from([Kusama.into(), Parachain(2), PalletInstance(1)]).relative_to(&base),
			(Parent, Parent, Parachain(2), PalletInstance(1)).into()
		);
		assert_eq!(
			Junctions::from([Kusama.into(), Parachain(1), PalletInstance(2), [1u8; 32].into()])
				.relative_to(&base),
			(Parent, PalletInstance(2), [1u8; 32]).into()
		);
		assert_eq!(
			Junctions::from([
				Kusama.into(),
				Parachain(1),
				PalletInstance(1),
				[1u8; 32].into(),
				1u128.into()
			])
			.relative_to(&base),
			([1u8; 32], 1u128).into()
		);
	}

	#[test]
	fn global_consensus_works() {
		use NetworkId::*;
		assert_eq!(Junctions::from([Polkadot.into()]).global_consensus(), Ok(Polkadot));
		assert_eq!(Junctions::from([Kusama.into(), 1u64.into()]).global_consensus(), Ok(Kusama));
		assert_eq!(Here.global_consensus(), Err(()));
		assert_eq!(Junctions::from([1u64.into()]).global_consensus(), Err(()));
		assert_eq!(Junctions::from([1u64.into(), Kusama.into()]).global_consensus(), Err(()));
	}

	#[test]
	fn test_conversion() {
		use super::{Junction::*, NetworkId::*};
		let x: Junctions = GlobalConsensus(Polkadot).into();
		assert_eq!(x, Junctions::from([GlobalConsensus(Polkadot)]));
		let x: Junctions = Polkadot.into();
		assert_eq!(x, Junctions::from([GlobalConsensus(Polkadot)]));
		let x: Junctions = (Polkadot, Kusama).into();
		assert_eq!(x, Junctions::from([GlobalConsensus(Polkadot), GlobalConsensus(Kusama)]));
	}

	#[test]
	fn encode_decode_junctions_works() {
		let original = Junctions::from([
			Polkadot.into(),
			Kusama.into(),
			1u64.into(),
			GlobalConsensus(Polkadot),
			Parachain(123),
			PalletInstance(45),
		]);
		let encoded = original.encode();
		assert_eq!(encoded, &[6, 9, 2, 9, 3, 2, 0, 4, 9, 2, 0, 237, 1, 4, 45]);
		let decoded = Junctions::decode(&mut &encoded[..]).unwrap();
		assert_eq!(decoded, original);
	}
}
