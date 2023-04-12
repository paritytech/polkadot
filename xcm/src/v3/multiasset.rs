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

//! Cross-Consensus Message format asset data structures.
//!
//! This encompasses four types for representing assets:
//! - `MultiAsset`: A description of a single asset, either an instance of a non-fungible or some amount of a fungible.
//! - `MultiAssets`: A collection of `MultiAsset`s. These are stored in a `Vec` and sorted with fungibles first.
//! - `Wild`: A single asset wildcard, this can either be "all" assets, or all assets of a specific kind.
//! - `MultiAssetFilter`: A combination of `Wild` and `MultiAssets` designed for efficiently filtering an XCM holding
//!   account.

use super::{InteriorMultiLocation, MultiLocation};
use crate::v2::{
	AssetId as OldAssetId, AssetInstance as OldAssetInstance, Fungibility as OldFungibility,
	MultiAsset as OldMultiAsset, MultiAssetFilter as OldMultiAssetFilter,
	MultiAssets as OldMultiAssets, WildFungibility as OldWildFungibility,
	WildMultiAsset as OldWildMultiAsset,
};
use alloc::{vec, vec::Vec};
use core::{
	cmp::Ordering,
	convert::{TryFrom, TryInto},
};
use parity_scale_codec::{self as codec, Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// A general identifier for an instance of a non-fungible asset class.
#[derive(
	Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum AssetInstance {
	/// Undefined - used if the non-fungible asset class has only one instance.
	Undefined,

	/// A compact index. Technically this could be greater than `u128`, but this implementation supports only
	/// values up to `2**128 - 1`.
	Index(#[codec(compact)] u128),

	/// A 4-byte fixed-length datum.
	Array4([u8; 4]),

	/// An 8-byte fixed-length datum.
	Array8([u8; 8]),

	/// A 16-byte fixed-length datum.
	Array16([u8; 16]),

	/// A 32-byte fixed-length datum.
	Array32([u8; 32]),
}

impl TryFrom<OldAssetInstance> for AssetInstance {
	type Error = ();
	fn try_from(value: OldAssetInstance) -> Result<Self, Self::Error> {
		use OldAssetInstance::*;
		Ok(match value {
			Undefined => Self::Undefined,
			Index(n) => Self::Index(n),
			Array4(n) => Self::Array4(n),
			Array8(n) => Self::Array8(n),
			Array16(n) => Self::Array16(n),
			Array32(n) => Self::Array32(n),
			Blob(_) => return Err(()),
		})
	}
}

impl From<()> for AssetInstance {
	fn from(_: ()) -> Self {
		Self::Undefined
	}
}

impl From<[u8; 4]> for AssetInstance {
	fn from(x: [u8; 4]) -> Self {
		Self::Array4(x)
	}
}

impl From<[u8; 8]> for AssetInstance {
	fn from(x: [u8; 8]) -> Self {
		Self::Array8(x)
	}
}

impl From<[u8; 16]> for AssetInstance {
	fn from(x: [u8; 16]) -> Self {
		Self::Array16(x)
	}
}

impl From<[u8; 32]> for AssetInstance {
	fn from(x: [u8; 32]) -> Self {
		Self::Array32(x)
	}
}

impl From<u8> for AssetInstance {
	fn from(x: u8) -> Self {
		Self::Index(x as u128)
	}
}

impl From<u16> for AssetInstance {
	fn from(x: u16) -> Self {
		Self::Index(x as u128)
	}
}

impl From<u32> for AssetInstance {
	fn from(x: u32) -> Self {
		Self::Index(x as u128)
	}
}

impl From<u64> for AssetInstance {
	fn from(x: u64) -> Self {
		Self::Index(x as u128)
	}
}

impl TryFrom<AssetInstance> for () {
	type Error = ();
	fn try_from(x: AssetInstance) -> Result<Self, ()> {
		match x {
			AssetInstance::Undefined => Ok(()),
			_ => Err(()),
		}
	}
}

impl TryFrom<AssetInstance> for [u8; 4] {
	type Error = ();
	fn try_from(x: AssetInstance) -> Result<Self, ()> {
		match x {
			AssetInstance::Array4(x) => Ok(x),
			_ => Err(()),
		}
	}
}

impl TryFrom<AssetInstance> for [u8; 8] {
	type Error = ();
	fn try_from(x: AssetInstance) -> Result<Self, ()> {
		match x {
			AssetInstance::Array8(x) => Ok(x),
			_ => Err(()),
		}
	}
}

impl TryFrom<AssetInstance> for [u8; 16] {
	type Error = ();
	fn try_from(x: AssetInstance) -> Result<Self, ()> {
		match x {
			AssetInstance::Array16(x) => Ok(x),
			_ => Err(()),
		}
	}
}

impl TryFrom<AssetInstance> for [u8; 32] {
	type Error = ();
	fn try_from(x: AssetInstance) -> Result<Self, ()> {
		match x {
			AssetInstance::Array32(x) => Ok(x),
			_ => Err(()),
		}
	}
}

impl TryFrom<AssetInstance> for u8 {
	type Error = ();
	fn try_from(x: AssetInstance) -> Result<Self, ()> {
		match x {
			AssetInstance::Index(x) => x.try_into().map_err(|_| ()),
			_ => Err(()),
		}
	}
}

impl TryFrom<AssetInstance> for u16 {
	type Error = ();
	fn try_from(x: AssetInstance) -> Result<Self, ()> {
		match x {
			AssetInstance::Index(x) => x.try_into().map_err(|_| ()),
			_ => Err(()),
		}
	}
}

impl TryFrom<AssetInstance> for u32 {
	type Error = ();
	fn try_from(x: AssetInstance) -> Result<Self, ()> {
		match x {
			AssetInstance::Index(x) => x.try_into().map_err(|_| ()),
			_ => Err(()),
		}
	}
}

impl TryFrom<AssetInstance> for u64 {
	type Error = ();
	fn try_from(x: AssetInstance) -> Result<Self, ()> {
		match x {
			AssetInstance::Index(x) => x.try_into().map_err(|_| ()),
			_ => Err(()),
		}
	}
}

impl TryFrom<AssetInstance> for u128 {
	type Error = ();
	fn try_from(x: AssetInstance) -> Result<Self, ()> {
		match x {
			AssetInstance::Index(x) => Ok(x),
			_ => Err(()),
		}
	}
}

/// Classification of whether an asset is fungible or not, along with a mandatory amount or instance.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Debug, Encode, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum Fungibility {
	/// A fungible asset; we record a number of units, as a `u128` in the inner item.
	Fungible(#[codec(compact)] u128),
	/// A non-fungible asset. We record the instance identifier in the inner item. Only one asset
	/// of each instance identifier may ever be in existence at once.
	NonFungible(AssetInstance),
}

#[derive(Decode)]
enum UncheckedFungibility {
	Fungible(#[codec(compact)] u128),
	NonFungible(AssetInstance),
}

impl Decode for Fungibility {
	fn decode<I: codec::Input>(input: &mut I) -> Result<Self, parity_scale_codec::Error> {
		match UncheckedFungibility::decode(input)? {
			UncheckedFungibility::Fungible(a) if a != 0 => Ok(Self::Fungible(a)),
			UncheckedFungibility::NonFungible(i) => Ok(Self::NonFungible(i)),
			UncheckedFungibility::Fungible(_) =>
				Err("Fungible asset of zero amount is not allowed".into()),
		}
	}
}

impl Fungibility {
	pub fn is_kind(&self, w: WildFungibility) -> bool {
		use Fungibility::*;
		use WildFungibility::{Fungible as WildFungible, NonFungible as WildNonFungible};
		matches!((self, w), (Fungible(_), WildFungible) | (NonFungible(_), WildNonFungible))
	}
}

impl From<i32> for Fungibility {
	fn from(amount: i32) -> Fungibility {
		debug_assert_ne!(amount, 0);
		Fungibility::Fungible(amount as u128)
	}
}

impl From<u128> for Fungibility {
	fn from(amount: u128) -> Fungibility {
		debug_assert_ne!(amount, 0);
		Fungibility::Fungible(amount)
	}
}

impl<T: Into<AssetInstance>> From<T> for Fungibility {
	fn from(instance: T) -> Fungibility {
		Fungibility::NonFungible(instance.into())
	}
}

impl TryFrom<OldFungibility> for Fungibility {
	type Error = ();
	fn try_from(value: OldFungibility) -> Result<Self, Self::Error> {
		use OldFungibility::*;
		Ok(match value {
			Fungible(n) => Self::Fungible(n),
			NonFungible(i) => Self::NonFungible(i.try_into()?),
		})
	}
}

/// Classification of whether an asset is fungible or not.
#[derive(
	Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug, Encode, Decode, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum WildFungibility {
	/// The asset is fungible.
	Fungible,
	/// The asset is not fungible.
	NonFungible,
}

impl TryFrom<OldWildFungibility> for WildFungibility {
	type Error = ();
	fn try_from(value: OldWildFungibility) -> Result<Self, Self::Error> {
		use OldWildFungibility::*;
		Ok(match value {
			Fungible => Self::Fungible,
			NonFungible => Self::NonFungible,
		})
	}
}

/// Classification of an asset being concrete or abstract.
#[derive(
	Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug, Encode, Decode, TypeInfo, MaxEncodedLen,
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum AssetId {
	/// A specific location identifying an asset.
	Concrete(MultiLocation),
	/// An abstract location; this is a name which may mean different specific locations on
	/// different chains at different times.
	Abstract([u8; 32]),
}

impl<T: Into<MultiLocation>> From<T> for AssetId {
	fn from(x: T) -> Self {
		Self::Concrete(x.into())
	}
}

impl From<[u8; 32]> for AssetId {
	fn from(x: [u8; 32]) -> Self {
		Self::Abstract(x)
	}
}

impl TryFrom<OldAssetId> for AssetId {
	type Error = ();
	fn try_from(old: OldAssetId) -> Result<Self, ()> {
		use OldAssetId::*;
		Ok(match old {
			Concrete(l) => Self::Concrete(l.try_into()?),
			Abstract(v) if v.len() <= 32 => {
				let mut r = [0u8; 32];
				r[..v.len()].copy_from_slice(&v[..]);
				Self::Abstract(r)
			},
			_ => return Err(()),
		})
	}
}

impl AssetId {
	/// Prepend a `MultiLocation` to a concrete asset, giving it a new root location.
	pub fn prepend_with(&mut self, prepend: &MultiLocation) -> Result<(), ()> {
		if let AssetId::Concrete(ref mut l) = self {
			l.prepend_with(*prepend).map_err(|_| ())?;
		}
		Ok(())
	}

	/// Mutate the asset to represent the same value from the perspective of a new `target`
	/// location. The local chain's location is provided in `context`.
	pub fn reanchor(
		&mut self,
		target: &MultiLocation,
		context: InteriorMultiLocation,
	) -> Result<(), ()> {
		if let AssetId::Concrete(ref mut l) = self {
			l.reanchor(target, context)?;
		}
		Ok(())
	}

	/// Use the value of `self` along with a `fun` fungibility specifier to create the corresponding `MultiAsset` value.
	pub fn into_multiasset(self, fun: Fungibility) -> MultiAsset {
		MultiAsset { fun, id: self }
	}

	/// Use the value of `self` along with a `fun` fungibility specifier to create the corresponding `WildMultiAsset`
	/// wildcard (`AllOf`) value.
	pub fn into_wild(self, fun: WildFungibility) -> WildMultiAsset {
		WildMultiAsset::AllOf { fun, id: self }
	}
}

/// Either an amount of a single fungible asset, or a single well-identified non-fungible asset.
#[derive(Clone, Eq, PartialEq, Debug, Encode, Decode, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct MultiAsset {
	/// The overall asset identity (aka *class*, in the case of a non-fungible).
	pub id: AssetId,
	/// The fungibility of the asset, which contains either the amount (in the case of a fungible
	/// asset) or the *insance ID`, the secondary asset identifier.
	pub fun: Fungibility,
}

impl PartialOrd for MultiAsset {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for MultiAsset {
	fn cmp(&self, other: &Self) -> Ordering {
		match (&self.fun, &other.fun) {
			(Fungibility::Fungible(..), Fungibility::NonFungible(..)) => Ordering::Less,
			(Fungibility::NonFungible(..), Fungibility::Fungible(..)) => Ordering::Greater,
			_ => (&self.id, &self.fun).cmp(&(&other.id, &other.fun)),
		}
	}
}

impl<A: Into<AssetId>, B: Into<Fungibility>> From<(A, B)> for MultiAsset {
	fn from((id, fun): (A, B)) -> MultiAsset {
		MultiAsset { fun: fun.into(), id: id.into() }
	}
}

impl MultiAsset {
	pub fn is_fungible(&self, maybe_id: Option<AssetId>) -> bool {
		use Fungibility::*;
		matches!(self.fun, Fungible(..)) && maybe_id.map_or(true, |i| i == self.id)
	}

	pub fn is_non_fungible(&self, maybe_id: Option<AssetId>) -> bool {
		use Fungibility::*;
		matches!(self.fun, NonFungible(..)) && maybe_id.map_or(true, |i| i == self.id)
	}

	/// Prepend a `MultiLocation` to a concrete asset, giving it a new root location.
	pub fn prepend_with(&mut self, prepend: &MultiLocation) -> Result<(), ()> {
		self.id.prepend_with(prepend)
	}

	/// Mutate the location of the asset identifier if concrete, giving it the same location
	/// relative to a `target` context. The local context is provided as `context`.
	pub fn reanchor(
		&mut self,
		target: &MultiLocation,
		context: InteriorMultiLocation,
	) -> Result<(), ()> {
		self.id.reanchor(target, context)
	}

	/// Mutate the location of the asset identifier if concrete, giving it the same location
	/// relative to a `target` context. The local context is provided as `context`.
	pub fn reanchored(
		mut self,
		target: &MultiLocation,
		context: InteriorMultiLocation,
	) -> Result<Self, ()> {
		self.id.reanchor(target, context)?;
		Ok(self)
	}

	/// Returns true if `self` is a super-set of the given `inner` asset.
	pub fn contains(&self, inner: &MultiAsset) -> bool {
		use Fungibility::*;
		if self.id == inner.id {
			match (&self.fun, &inner.fun) {
				(Fungible(a), Fungible(i)) if a >= i => return true,
				(NonFungible(a), NonFungible(i)) if a == i => return true,
				_ => (),
			}
		}
		false
	}
}

impl TryFrom<OldMultiAsset> for MultiAsset {
	type Error = ();
	fn try_from(old: OldMultiAsset) -> Result<Self, ()> {
		Ok(Self { id: old.id.try_into()?, fun: old.fun.try_into()? })
	}
}

/// A `Vec` of `MultiAsset`s.
///
/// There are a number of invariants which the construction and mutation functions must ensure are
/// maintained:
/// - It may contain no items of duplicate asset class;
/// - All items must be ordered;
/// - The number of items should grow no larger than `MAX_ITEMS_IN_MULTIASSETS`.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Debug, Encode, TypeInfo, Default)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct MultiAssets(Vec<MultiAsset>);

/// Maximum number of items we expect in a single `MultiAssets` value. Note this is not (yet)
/// enforced, and just serves to provide a sensible `max_encoded_len` for `MultiAssets`.
const MAX_ITEMS_IN_MULTIASSETS: usize = 20;

impl MaxEncodedLen for MultiAssets {
	fn max_encoded_len() -> usize {
		MultiAsset::max_encoded_len() * MAX_ITEMS_IN_MULTIASSETS
	}
}

impl Decode for MultiAssets {
	fn decode<I: codec::Input>(input: &mut I) -> Result<Self, parity_scale_codec::Error> {
		Self::from_sorted_and_deduplicated(Vec::<MultiAsset>::decode(input)?)
			.map_err(|()| "Out of order".into())
	}
}

impl TryFrom<OldMultiAssets> for MultiAssets {
	type Error = ();
	fn try_from(old: OldMultiAssets) -> Result<Self, ()> {
		let v = old
			.drain()
			.into_iter()
			.map(MultiAsset::try_from)
			.collect::<Result<Vec<_>, ()>>()?;
		Ok(MultiAssets(v))
	}
}

impl From<Vec<MultiAsset>> for MultiAssets {
	fn from(mut assets: Vec<MultiAsset>) -> Self {
		let mut res = Vec::with_capacity(assets.len());
		if !assets.is_empty() {
			assets.sort();
			let mut iter = assets.into_iter();
			if let Some(first) = iter.next() {
				let last = iter.fold(first, |a, b| -> MultiAsset {
					match (a, b) {
						(
							MultiAsset { fun: Fungibility::Fungible(a_amount), id: a_id },
							MultiAsset { fun: Fungibility::Fungible(b_amount), id: b_id },
						) if a_id == b_id => MultiAsset {
							id: a_id,
							fun: Fungibility::Fungible(a_amount.saturating_add(b_amount)),
						},
						(
							MultiAsset { fun: Fungibility::NonFungible(a_instance), id: a_id },
							MultiAsset { fun: Fungibility::NonFungible(b_instance), id: b_id },
						) if a_id == b_id && a_instance == b_instance =>
							MultiAsset { fun: Fungibility::NonFungible(a_instance), id: a_id },
						(to_push, to_remember) => {
							res.push(to_push);
							to_remember
						},
					}
				});
				res.push(last);
			}
		}
		Self(res)
	}
}

impl<T: Into<MultiAsset>> From<T> for MultiAssets {
	fn from(x: T) -> Self {
		Self(vec![x.into()])
	}
}

impl MultiAssets {
	/// A new (empty) value.
	pub fn new() -> Self {
		Self(Vec::new())
	}

	/// Create a new instance of `MultiAssets` from a `Vec<MultiAsset>` whose contents are sorted and
	/// which contain no duplicates.
	///
	/// Returns `Ok` if the operation succeeds and `Err` if `r` is out of order or had duplicates. If you can't
	/// guarantee that `r` is sorted and deduplicated, then use `From::<Vec<MultiAsset>>::from` which is infallible.
	pub fn from_sorted_and_deduplicated(r: Vec<MultiAsset>) -> Result<Self, ()> {
		if r.is_empty() {
			return Ok(Self(Vec::new()))
		}
		r.iter().skip(1).try_fold(&r[0], |a, b| -> Result<&MultiAsset, ()> {
			if a.id < b.id || a < b && (a.is_non_fungible(None) || b.is_non_fungible(None)) {
				Ok(b)
			} else {
				Err(())
			}
		})?;
		Ok(Self(r))
	}

	/// Create a new instance of `MultiAssets` from a `Vec<MultiAsset>` whose contents are sorted and
	/// which contain no duplicates.
	///
	/// In release mode, this skips any checks to ensure that `r` is correct, making it a negligible-cost operation.
	/// Generally though you should avoid using it unless you have a strict proof that `r` is valid.
	#[cfg(test)]
	pub fn from_sorted_and_deduplicated_skip_checks(r: Vec<MultiAsset>) -> Self {
		Self::from_sorted_and_deduplicated(r).expect("Invalid input r is not sorted/deduped")
	}
	/// Create a new instance of `MultiAssets` from a `Vec<MultiAsset>` whose contents are sorted and
	/// which contain no duplicates.
	///
	/// In release mode, this skips any checks to ensure that `r` is correct, making it a negligible-cost operation.
	/// Generally though you should avoid using it unless you have a strict proof that `r` is valid.
	///
	/// In test mode, this checks anyway and panics on fail.
	#[cfg(not(test))]
	pub fn from_sorted_and_deduplicated_skip_checks(r: Vec<MultiAsset>) -> Self {
		Self(r)
	}

	/// Add some asset onto the list, saturating. This is quite a laborious operation since it maintains the ordering.
	pub fn push(&mut self, a: MultiAsset) {
		for asset in self.0.iter_mut().filter(|x| x.id == a.id) {
			match (&a.fun, &mut asset.fun) {
				(Fungibility::Fungible(amount), Fungibility::Fungible(balance)) => {
					*balance = balance.saturating_add(*amount);
					return
				},
				(Fungibility::NonFungible(inst1), Fungibility::NonFungible(inst2))
					if inst1 == inst2 =>
					return,
				_ => (),
			}
		}
		self.0.push(a);
		self.0.sort();
	}

	/// Returns `true` if this definitely represents no asset.
	pub fn is_none(&self) -> bool {
		self.0.is_empty()
	}

	/// Returns true if `self` is a super-set of the given `inner` asset.
	pub fn contains(&self, inner: &MultiAsset) -> bool {
		self.0.iter().any(|i| i.contains(inner))
	}

	/// Consume `self` and return the inner vec.
	#[deprecated = "Use `into_inner()` instead"]
	pub fn drain(self) -> Vec<MultiAsset> {
		self.0
	}

	/// Consume `self` and return the inner vec.
	pub fn into_inner(self) -> Vec<MultiAsset> {
		self.0
	}

	/// Return a reference to the inner vec.
	pub fn inner(&self) -> &Vec<MultiAsset> {
		&self.0
	}

	/// Return the number of distinct asset instances contained.
	pub fn len(&self) -> usize {
		self.0.len()
	}

	/// Prepend a `MultiLocation` to any concrete asset items, giving it a new root location.
	pub fn prepend_with(&mut self, prefix: &MultiLocation) -> Result<(), ()> {
		self.0.iter_mut().try_for_each(|i| i.prepend_with(prefix))
	}

	/// Mutate the location of the asset identifier if concrete, giving it the same location
	/// relative to a `target` context. The local context is provided as `context`.
	pub fn reanchor(
		&mut self,
		target: &MultiLocation,
		context: InteriorMultiLocation,
	) -> Result<(), ()> {
		self.0.iter_mut().try_for_each(|i| i.reanchor(target, context))
	}

	/// Return a reference to an item at a specific index or `None` if it doesn't exist.
	pub fn get(&self, index: usize) -> Option<&MultiAsset> {
		self.0.get(index)
	}
}

/// A wildcard representing a set of assets.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Debug, Encode, Decode, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum WildMultiAsset {
	/// All assets in Holding.
	All,
	/// All assets in Holding of a given fungibility and ID.
	AllOf { id: AssetId, fun: WildFungibility },
	/// All assets in Holding, up to `u32` individual assets (different instances of non-fungibles
	/// are separate assets).
	AllCounted(#[codec(compact)] u32),
	/// All assets in Holding of a given fungibility and ID up to `count` individual assets
	/// (different instances of non-fungibles are separate assets).
	AllOfCounted {
		id: AssetId,
		fun: WildFungibility,
		#[codec(compact)]
		count: u32,
	},
}

impl TryFrom<OldWildMultiAsset> for WildMultiAsset {
	type Error = ();
	fn try_from(old: OldWildMultiAsset) -> Result<WildMultiAsset, ()> {
		use OldWildMultiAsset::*;
		Ok(match old {
			AllOf { id, fun } => Self::AllOf { id: id.try_into()?, fun: fun.try_into()? },
			All => Self::All,
		})
	}
}

impl TryFrom<(OldWildMultiAsset, u32)> for WildMultiAsset {
	type Error = ();
	fn try_from(old: (OldWildMultiAsset, u32)) -> Result<WildMultiAsset, ()> {
		use OldWildMultiAsset::*;
		let count = old.1;
		Ok(match old.0 {
			AllOf { id, fun } =>
				Self::AllOfCounted { id: id.try_into()?, fun: fun.try_into()?, count },
			All => Self::AllCounted(count),
		})
	}
}

impl WildMultiAsset {
	/// Returns true if `self` is a super-set of the given `inner` asset.
	pub fn contains(&self, inner: &MultiAsset) -> bool {
		use WildMultiAsset::*;
		match self {
			AllOfCounted { count: 0, .. } | AllCounted(0) => false,
			AllOf { fun, id } | AllOfCounted { id, fun, .. } =>
				inner.fun.is_kind(*fun) && &inner.id == id,
			All | AllCounted(_) => true,
		}
	}

	/// Returns true if the wild element of `self` matches `inner`.
	///
	/// Note that for `Counted` variants of wildcards, then it will disregard the count except for
	/// always returning `false` when equal to 0.
	#[deprecated = "Use `contains` instead"]
	pub fn matches(&self, inner: &MultiAsset) -> bool {
		self.contains(inner)
	}

	/// Mutate the asset to represent the same value from the perspective of a new `target`
	/// location. The local chain's location is provided in `context`.
	pub fn reanchor(
		&mut self,
		target: &MultiLocation,
		context: InteriorMultiLocation,
	) -> Result<(), ()> {
		use WildMultiAsset::*;
		match self {
			AllOf { ref mut id, .. } | AllOfCounted { ref mut id, .. } =>
				id.reanchor(target, context),
			All | AllCounted(_) => Ok(()),
		}
	}

	/// Maximum count of assets allowed to match, if any.
	pub fn count(&self) -> Option<u32> {
		use WildMultiAsset::*;
		match self {
			AllOfCounted { count, .. } | AllCounted(count) => Some(*count),
			All | AllOf { .. } => None,
		}
	}

	/// Explicit limit on number of assets allowed to match, if any.
	pub fn limit(&self) -> Option<u32> {
		self.count()
	}

	/// Consume self and return the equivalent version but counted and with the `count` set to the
	/// given parameter.
	pub fn counted(self, count: u32) -> Self {
		use WildMultiAsset::*;
		match self {
			AllOfCounted { fun, id, .. } | AllOf { fun, id } => AllOfCounted { fun, id, count },
			All | AllCounted(_) => AllCounted(count),
		}
	}
}

impl<A: Into<AssetId>, B: Into<WildFungibility>> From<(A, B)> for WildMultiAsset {
	fn from((id, fun): (A, B)) -> WildMultiAsset {
		WildMultiAsset::AllOf { fun: fun.into(), id: id.into() }
	}
}

/// `MultiAsset` collection, defined either by a number of `MultiAssets` or a single wildcard.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Debug, Encode, Decode, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum MultiAssetFilter {
	/// Specify the filter as being everything contained by the given `MultiAssets` inner.
	Definite(MultiAssets),
	/// Specify the filter as the given `WildMultiAsset` wildcard.
	Wild(WildMultiAsset),
}

impl<T: Into<WildMultiAsset>> From<T> for MultiAssetFilter {
	fn from(x: T) -> Self {
		Self::Wild(x.into())
	}
}

impl From<MultiAsset> for MultiAssetFilter {
	fn from(x: MultiAsset) -> Self {
		Self::Definite(vec![x].into())
	}
}

impl From<Vec<MultiAsset>> for MultiAssetFilter {
	fn from(x: Vec<MultiAsset>) -> Self {
		Self::Definite(x.into())
	}
}

impl From<MultiAssets> for MultiAssetFilter {
	fn from(x: MultiAssets) -> Self {
		Self::Definite(x)
	}
}

impl MultiAssetFilter {
	/// Returns true if `inner` would be matched by `self`.
	///
	/// Note that for `Counted` variants of wildcards, then it will disregard the count except for
	/// always returning `false` when equal to 0.
	pub fn matches(&self, inner: &MultiAsset) -> bool {
		match self {
			MultiAssetFilter::Definite(ref assets) => assets.contains(inner),
			MultiAssetFilter::Wild(ref wild) => wild.contains(inner),
		}
	}

	/// Mutate the location of the asset identifier if concrete, giving it the same location
	/// relative to a `target` context. The local context is provided as `context`.
	pub fn reanchor(
		&mut self,
		target: &MultiLocation,
		context: InteriorMultiLocation,
	) -> Result<(), ()> {
		match self {
			MultiAssetFilter::Definite(ref mut assets) => assets.reanchor(target, context),
			MultiAssetFilter::Wild(ref mut wild) => wild.reanchor(target, context),
		}
	}

	/// Maximum count of assets it is possible to match, if known.
	pub fn count(&self) -> Option<u32> {
		use MultiAssetFilter::*;
		match self {
			Definite(x) => Some(x.len() as u32),
			Wild(x) => x.count(),
		}
	}

	/// Explicit limit placed on the number of items, if any.
	pub fn limit(&self) -> Option<u32> {
		use MultiAssetFilter::*;
		match self {
			Definite(_) => None,
			Wild(x) => x.limit(),
		}
	}
}

impl TryFrom<OldMultiAssetFilter> for MultiAssetFilter {
	type Error = ();
	fn try_from(old: OldMultiAssetFilter) -> Result<MultiAssetFilter, ()> {
		Ok(match old {
			OldMultiAssetFilter::Definite(x) => Self::Definite(x.try_into()?),
			OldMultiAssetFilter::Wild(x) => Self::Wild(x.try_into()?),
		})
	}
}

impl TryFrom<(OldMultiAssetFilter, u32)> for MultiAssetFilter {
	type Error = ();
	fn try_from(old: (OldMultiAssetFilter, u32)) -> Result<MultiAssetFilter, ()> {
		let count = old.1;
		Ok(match old.0 {
			OldMultiAssetFilter::Definite(x) if count >= x.len() as u32 =>
				Self::Definite(x.try_into()?),
			OldMultiAssetFilter::Wild(x) => Self::Wild((x, count).try_into()?),
			_ => return Err(()),
		})
	}
}

#[cfg(test)]
mod tests {
	use super::super::prelude::*;

	#[test]
	fn conversion_works() {
		let _: MultiAssets = (Here, 1u128).into();
	}

	#[test]
	fn from_sorted_and_deduplicated_works() {
		use super::*;
		use alloc::vec;

		let empty = vec![];
		let r = MultiAssets::from_sorted_and_deduplicated(empty);
		assert_eq!(r, Ok(MultiAssets(vec![])));

		let dup_fun = vec![(Here, 100).into(), (Here, 10).into()];
		let r = MultiAssets::from_sorted_and_deduplicated(dup_fun);
		assert!(r.is_err());

		let dup_nft = vec![(Here, *b"notgood!").into(), (Here, *b"notgood!").into()];
		let r = MultiAssets::from_sorted_and_deduplicated(dup_nft);
		assert!(r.is_err());

		let good_fun = vec![(Here, 10).into(), (Parent, 10).into()];
		let r = MultiAssets::from_sorted_and_deduplicated(good_fun.clone());
		assert_eq!(r, Ok(MultiAssets(good_fun)));

		let bad_fun = vec![(Parent, 10).into(), (Here, 10).into()];
		let r = MultiAssets::from_sorted_and_deduplicated(bad_fun);
		assert!(r.is_err());

		let good_abstract_fun = vec![(Here, 100).into(), ([0u8; 32], 10).into()];
		let r = MultiAssets::from_sorted_and_deduplicated(good_abstract_fun.clone());
		assert_eq!(r, Ok(MultiAssets(good_abstract_fun)));

		let bad_abstract_fun = vec![([0u8; 32], 10).into(), (Here, 10).into()];
		let r = MultiAssets::from_sorted_and_deduplicated(bad_abstract_fun);
		assert!(r.is_err());

		let good_nft = vec![(Here, ()).into(), (Here, *b"good").into()];
		let r = MultiAssets::from_sorted_and_deduplicated(good_nft.clone());
		assert_eq!(r, Ok(MultiAssets(good_nft)));

		let bad_nft = vec![(Here, *b"bad!").into(), (Here, ()).into()];
		let r = MultiAssets::from_sorted_and_deduplicated(bad_nft);
		assert!(r.is_err());

		let good_abstract_nft = vec![(Here, ()).into(), ([0u8; 32], ()).into()];
		let r = MultiAssets::from_sorted_and_deduplicated(good_abstract_nft.clone());
		assert_eq!(r, Ok(MultiAssets(good_abstract_nft)));

		let bad_abstract_nft = vec![([0u8; 32], ()).into(), (Here, ()).into()];
		let r = MultiAssets::from_sorted_and_deduplicated(bad_abstract_nft);
		assert!(r.is_err());

		let mixed_good = vec![(Here, 10).into(), (Here, *b"good").into()];
		let r = MultiAssets::from_sorted_and_deduplicated(mixed_good.clone());
		assert_eq!(r, Ok(MultiAssets(mixed_good)));

		let mixed_bad = vec![(Here, *b"bad!").into(), (Here, 10).into()];
		let r = MultiAssets::from_sorted_and_deduplicated(mixed_bad);
		assert!(r.is_err());
	}
}
