// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Cumulus.

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
//! This encompasses four types for repesenting assets:
//! - `MultiAsset`: A description of a single asset, either an instance of a non-fungible or some amount of a fungible.
//! - `MultiAssets`: A collection of `MultiAsset`s. These are stored in a `Vec` and sorted with fungibles first.
//! - `Wild`: A single asset wildcard, this can either be "all" assets, or all assets of a specific kind.
//! - `MultiAssetFilter`: A combination of `Wild` and `MultiAssets` designed for efficiently filtering an XCM holding
//!   account.

use core::convert::{TryFrom, TryInto};
use alloc::vec::Vec;
use parity_scale_codec::{self as codec, Encode, Decode};
use super::{
	MultiLocation, multi_asset::MultiAsset as OldMultiAsset, VersionedMultiAsset,
	VersionedWildMultiAsset,
};
use core::cmp::Ordering;

/// A general identifier for an instance of a non-fungible asset class.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug)]
pub enum AssetInstance {
	/// Undefined - used if the NFA class has only one instance.
	Undefined,

	/// A compact index. Technically this could be greater than `u128`, but this implementation supports only
	/// values up to `2**128 - 1`.
	Index { #[codec(compact)] id: u128 },

	/// A 4-byte fixed-length datum.
	Array4([u8; 4]),

	/// An 8-byte fixed-length datum.
	Array8([u8; 8]),

	/// A 16-byte fixed-length datum.
	Array16([u8; 16]),

	/// A 32-byte fixed-length datum.
	Array32([u8; 32]),

	/// An arbitrary piece of data. Use only when necessary.
	Blob(Vec<u8>),
}

/// Classification of an asset being concrete or abstract.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum AssetId {
	Concrete(MultiLocation),
	Abstract(Vec<u8>),
}

impl AssetId {
	/// Prepend a `MultiLocation` to a concrete asset, giving it a new root location.
	pub fn reanchor(&mut self, prepend: &MultiLocation) -> Result<(), ()> {
		if let AssetId::Concrete(ref mut l) = self {
			l.prepend_with(prepend.clone()).map_err(|_| ())?;
		}
		Ok(())
	}

	/// Use the value of `self` along with a `fun` fungibility specifier to create the corresponding `MultiAsset` value.
	pub fn into_multiasset(self, fun: Fungibility) -> MultiAsset {
		MultiAsset { fun, id: self }
	}

	/// Use the value of `self` along with a `fun` fungibility specifier to create the corresponding `WildMultiAsset`
	/// definite (`Asset`) value.
	pub fn into_wild(self, fun: Fungibility) -> WildMultiAsset {
		WildMultiAsset::Asset(fun, self)
	}

	/// Use the value of `self` along with a `fun` fungibility specifier to create the corresponding `WildMultiAsset`
	/// wildcard (`AllOf`) value.
	pub fn into_allof(self, fun: WildFungibility) -> WildMultiAsset {
		WildMultiAsset::AllOf(fun, self)
	}
}

/// Classification of whether an asset is fungible or not, along with an mandatory amount or instance.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum Fungibility {
	Fungible(u128),
	NonFungible(AssetInstance),
}

impl Fungibility {
	pub fn is_kind(&self, w: WildFungibility) -> bool {
		use {Fungibility::*, WildFungibility::{Fungible as WildFungible, NonFungible as WildNonFungible}};
		matches!((self, w), (Fungible(_), WildFungible) | (NonFungible(_), WildNonFungible))
	}
}

impl From<u128> for Fungibility {
	fn from(amount: u128) -> Fungibility {
		debug_assert_ne!(amount, 0);
		Fungibility::Fungible(amount)
	}
}

impl From<AssetInstance> for Fungibility {
	fn from(instance: AssetInstance) -> Fungibility {
		Fungibility::NonFungible(instance)
	}
}




#[derive(Clone, Eq, PartialEq, Debug)]
pub struct MultiAsset {
	pub id: AssetId,
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
			(Fungibility::Fungible(..), Fungibility::NonFungible(..)) => return Ordering::Less,
			(Fungibility::NonFungible(..), Fungibility::Fungible(..)) => return Ordering::Greater,
		}
		(&self.id, &self.fun).cmp((&other.id, &other.fun))
	}
}

impl From<(AssetId, Fungibility)> for MultiAsset {
	fn from((id, fun): (AssetId, Fungibility)) -> MultiAsset {
		MultiAsset { fun, id }
	}
}

impl From<(AssetId, u128)> for MultiAsset {
	fn from((id, amount): (AssetId, u128)) -> MultiAsset {
		MultiAsset { fun: amount.into(), id }
	}
}

impl From<(AssetId, AssetInstance)> for MultiAsset {
	fn from((id, instance): (AssetId, AssetInstance)) -> MultiAsset {
		MultiAsset { fun: instance.into(), id }
	}
}

impl From<MultiAsset> for OldMultiAsset {
	fn from(a: MultiAsset) -> Self {
		use {AssetId::*, Fungibility::*, OldMultiAsset::*};
		match (a.fun, a.id) {
			(Fungible(amount), Concrete(id)) => ConcreteFungible { id, amount },
			(Fungible(amount), Abstract(id)) => AbstractFungible { id, amount },
			(NonFungible(instance), Concrete(class)) => ConcreteNonFungible { class, instance },
			(NonFungible(instance), Abstract(class)) => AbstractNonFungible { class, instance },
		}
	}
}

impl TryFrom<OldMultiAsset> for MultiAsset {
	type Error = ();
	fn try_from(a: OldMultiAsset) -> Result<Self, ()> {
		use {AssetId::*, Fungibility::*, OldMultiAsset::*};
		let (fun, id) = match a {
			ConcreteFungible { id, amount } if amount > 0 => (Fungible(amount), Concrete(id)),
			AbstractFungible { id, amount } if amount > 0 => (Fungible(amount), Abstract(id)),
			ConcreteNonFungible { class, instance } => (NonFungible(instance), Concrete(class)),
			AbstractNonFungible { class, instance } => (NonFungible(instance), Abstract(class)),
			_ => return Err(()),
		};
		Ok(MultiAsset { fun, id })
	}
}

impl Encode for MultiAsset {
	fn encode(&self) -> Vec<u8> {
		OldMultiAsset::from(self.clone()).encode()
	}
}

impl Decode for MultiAsset {
	fn decode<I: codec::Input>(input: &mut I) -> Result<Self, parity_scale_codec::Error> {
		OldMultiAsset::decode(input)
			.and_then(|r| TryInto::try_into(r).map_err(|_| "Unsupported wildcard".into()))?
	}
}

impl MultiAsset {
	fn is_fungible(&self, maybe_id: Option<AssetId>) -> bool {
		use Fungibility::*;
		matches!(self.fun, Fungible(..)) && maybe_id.map_or(true, |i| i == self.id)
	}

	fn is_non_fungible(&self, maybe_id: Option<AssetId>) -> bool {
		use Fungibility::*;
		matches!(self.fun, NonFungible(..)) && maybe_id.map_or(true, |i| i == self.id)
	}

	/// Prepend a `MultiLocation` to a concrete asset, giving it a new root location.
	pub fn reanchor(&mut self, prepend: &MultiLocation) -> Result<(), ()> {
		self.id.reanchor(prepend)
	}

	/// Returns true if `self` is a super-set of the given `inner`.
	pub fn contains(&self, inner: &MultiAsset) -> bool {
		use {MultiAsset::*, Fungibility::*};
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



/// A vec of MultiAssets. There may be no duplicate fungible items in here and when decoding, they must be sorted.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Debug, Encode)]
pub struct MultiAssets(Vec<MultiAsset>);

impl Decode for MultiAssets {
	fn decode<I: codec::Input>(input: &mut I) -> Result<Self, parity_scale_codec::Error> {
		let r = Vec::<MultiAsset>::decode(input)?;
		if r.is_empty() { return Ok(Self(Vec::new())) }
		r.iter().skip(1).try_fold(&r[0], |a, b| {
			if a.id < b.id || a < b && (a.is_non_fungible(None) || b.is_non_fungible(None)) {
				Ok(b)
			} else {
				Err("Unsupported wildcard".into())
			}
		})?;
		Ok(Self(r))
	}
}

impl From<MultiAssets> for Vec<OldMultiAsset> {
	fn from(a: MultiAssets) -> Self {
		a.0.into_iter().map(OldMultiAsset::from).collect()
	}
}

impl TryFrom<Vec<OldMultiAsset>> for MultiAssets {
	type Error = ();
	fn try_from(a: Vec<OldMultiAsset>) -> Result<Self, ()> {
		a.0.into_iter().map(MultiAsset::try_from).collect()
	}
}

impl MultiAssets {
	/// A new (empty) value.
	pub fn new() -> Self {
		Self(Vec::new())
	}

	/// Add some asset onto the multiasset list. This is quite a laborious operation since it maintains the ordering.
	pub fn push(&mut self, a: MultiAsset) {
		if let Fungibility::Fungible(ref amount) = a.fun {
			for asset in self.0.iter_mut().filter(|x| x.id == a.id) {
				if let Fungibility::Fungible(ref mut balance) = asset.fun {
					*balance += *amount;
					return
				}
			}
		}
		self.0.push(a);
		self.0.sort();
	}

	/// Returns `true` if this definitely represents no asset.
	pub fn is_none(&self) -> bool {
		self.0.is_empty()
	}

	/// Returns true if `self` is a super-set of the given `inner`.
	pub fn contains(&self, inner: &MultiAsset) -> bool {
		use {MultiAsset::*, Fungibility::*};
		self.0.iter().any(|i| i.contains(inner))
	}

	/// Consume `self` and return the inner vec.
	pub fn drain(self) -> Vec<MultiAsset> {
		self.0
	}

	/// Return a reference to the inner vec.
	pub fn inner(&self) -> &Vec<MultiAsset> {
		&self.0
	}

	/// Prepend a `MultiLocation` to any concrete asset items, giving it a new root location.
	pub fn reanchor(&mut self, prepend: &MultiLocation) -> Result<(), ()> {
		self.0.iter_mut().try_for_each(|i| i.reanchor(prepend))
	}
}





/// Classification of whether an asset is fungible or not, along with an optional amount or instance.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum WildFungibility {
	Fungible,
	NonFungible,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum WildMultiAsset {
	All,
	// TODO: AllOf { fun: WildFungibility, id: AssetId }
	AllOf(WildFungibility, AssetId),
}

impl Encode for WildMultiAsset {
	fn encode(&self) -> Vec<u8> {
		OldMultiAsset::from(self.clone()).encode()
	}
}

impl Decode for WildMultiAsset {
	fn decode<I: codec::Input>(input: &mut I) -> Result<Self, parity_scale_codec::Error> {
		OldMultiAsset::decode(input).map(Into::into)
	}
}

impl From<WildMultiAsset> for OldMultiAsset {
	fn from(a: WildMultiAsset) -> Self {
		use {AssetId::*, Fungibility::*, OldMultiAsset::*, WildMultiAsset::AllOf};
		match a {
			WildMultiAsset::All => All,
			AllOf(WildFungibility::Fungible, Concrete(id)) => AllConcreteFungible { id },
			AllOf(WildFungibility::Fungible, Abstract(id)) => AllAbstractFungible { id },
			AllOf(WildFungibility::NonFungible, Concrete(class)) => AllConcreteNonFungible { class },
			AllOf(WildFungibility::NonFungible, Abstract(class)) => AllAbstractNonFungible { class },
		}
	}
}

impl TryFrom<OldMultiAsset> for WildMultiAsset {
	type Error = ();
	fn try_from(a: OldMultiAsset) -> Result<Self, ()> {
		use {AssetId::*, Fungibility::*, OldMultiAsset::*, WildMultiAsset::AllOf};
		Ok(match a {
			All => WildMultiAsset::All,
			AllConcreteFungible { id } => AllOf(WildFungibility::Fungible, Concrete(id)),
			AllAbstractFungible { id } => AllOf(WildFungibility::Fungible, Abstract(id)),
			AllConcreteNonFungible { class } => AllOf(WildFungibility::NonFungible, Concrete(class)),
			AllAbstractNonFungible { class } => AllOf(WildFungibility::NonFungible, Abstract(class)),
			_ => return Err(()),
		})
	}
}

impl WildMultiAsset {
	/// Returns true if `self` is a super-set of the given `inner`.
	///
	/// Typically, any wildcard is never contained in anything else, and a wildcard can contain any other non-wildcard.
	/// For more details, see the implementation and tests.
	pub fn contains(&self, inner: &MultiAsset) -> bool {
		use WildMultiAsset::*;
		match self {
			AllOf(fun, id) => inner.fun.is_kind(*fun) && inner.id == id,
			All => true,
		}
	}

	/// Prepend a `MultiLocation` to any concrete asset components, giving it a new root location.
	pub fn reanchor(&mut self, prepend: &MultiLocation) -> Result<(), ()> {
		use WildMultiAsset::*;
		match self {
			AllOf(_, ref mut id) => id.prepend_with(prepend.clone()).map_err(|_| ()),
			_ => Ok(()),
		}
	}
}





/// `MultiAsset` collection, either `MultiAssets` or a single wildcard. Note: vectors of wildcards
/// whose encoding is supported in XCM v0 are unsupported in this implementation and will result in a decode error.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum MultiAssetFilter {
	Assets(MultiAssets),
	Wild(WildMultiAsset),
}

impl From<MultiAssetFilter> for Vec<OldMultiAsset> {
	fn from(a: MultiAssetFilter) -> Self {
		use MultiAssetFilter::*;
		match a {
			Assets(assets) => assets.0.into_iter().map(OldMultiAsset::from).collect(),
			Wild(wild) => vec![wild.into()],
		}
	}
}

impl TryFrom<Vec<OldMultiAsset>> for MultiAssetFilter {
	type Error = ();
	fn try_from(old_assets: Vec<OldMultiAsset>) -> Result<Self, ()> {
		use MultiAssetFilter::*;
		if old_assets.is_empty() {
			return Ok(Assets(MultiAssets::new()))
		}
		if let (1, Ok(wild)) = (old_assets.len(), old_assets[0].try_into()) {
			return Ok(Wild(wild))
		}

		old_assets.into_iter().map(MultiAsset::try_from).collect()
	}
}

impl MultiAssetFilter {
	/// Returns `true` if the `MultiAsset` is a wildcard and refers to sets of assets, instead of just one.
	pub fn is_wildcard(&self) -> bool {
		matches!(self, MultiAssetFilter::Wild(..))
	}

	/// Returns `true` if the `MultiAsset` is not a wildcard.
	pub fn is_definite(&self) -> bool {
		!self.is_wildcard()
	}

	/// Returns `true` if this definitely represents no asset.
	pub fn is_none(&self) -> bool {
		matches!(self, MultiAssetFilter::Assets(a) if a.is_none())
	}

	/// Returns true if `self` is a super-set of the given `inner`.
	///
	/// Typically, any wildcard is never contained in anything else, and a wildcard can contain any other non-wildcard.
	/// For more details, see the implementation and tests.
	pub fn contains(&self, inner: &MultiAsset) -> bool {
		match self {
			MultiAssetFilter::Assets(ref assets) => assets.contains(inner),
			MultiAssetFilter::Wild(ref wild) => wild.contains(inner),
		}
	}

	/// Prepend a `MultiLocation` to any concrete asset components, giving it a new root location.
	pub fn reanchor(&mut self, prepend: &MultiLocation) -> Result<(), ()> {
		match self {
			MultiAssetFilter::Assets(ref mut assets) => assets.reanchor(prepend),
			MultiAssetFilter::Wild(ref mut wild) => wild.reanchor(prepend),
		}
	}
}
