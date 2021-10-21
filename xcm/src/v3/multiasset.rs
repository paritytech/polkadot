// Copyright 2020 Parity Technologies (UK) Ltd.
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

use super::MultiLocation;
use crate::v2::{MultiAssetFilter as OldMultiAssetFilter, WildMultiAsset as OldWildMultiAsset};
use alloc::{vec, vec::Vec};
use core::convert::TryFrom;
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

/// These are unchanged from XCM version 2 to version 3.
pub use crate::v2::{
	AssetId, AssetInstance, Fungibility, MultiAsset, MultiAssets, WildFungibility,
};

/// A wildcard representing a set of assets.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Debug, Encode, Decode, TypeInfo)]
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

impl From<OldWildMultiAsset> for WildMultiAsset {
	fn from(old: OldWildMultiAsset) -> WildMultiAsset {
		use OldWildMultiAsset::*;
		match old {
			AllOf { id, fun } => Self::AllOf { id, fun },
			All => Self::All,
		}
	}
}

impl From<(OldWildMultiAsset, u32)> for WildMultiAsset {
	fn from(old: (OldWildMultiAsset, u32)) -> WildMultiAsset {
		use OldWildMultiAsset::*;
		let count = old.1;
		match old.0 {
			AllOf { id, fun } => Self::AllOfCounted { id, fun, count },
			All => Self::AllCounted(count),
		}
	}
}

impl WildMultiAsset {
	/// Returns true if the wild element of `self` matches `inner`.
	///
	/// Note that for `Counted` variants of wildcards, then it will disregard the count except for
	/// always returning `false` when equal to 0.
	pub fn matches(&self, inner: &MultiAsset) -> bool {
		use WildMultiAsset::*;
		match self {
			AllOfCounted { count: 0, .. } | AllCounted(0) => false,
			AllOf { fun, id } | AllOfCounted { id, fun, .. } =>
				inner.fun.is_kind(*fun) && &inner.id == id,
			All | AllCounted(_) => true,
		}
	}

	/// Prepend a `MultiLocation` to any concrete asset components, giving it a new root location.
	pub fn reanchor(&mut self, prepend: &MultiLocation) -> Result<(), ()> {
		use WildMultiAsset::*;
		match self {
			AllOf { ref mut id, .. } | AllOfCounted { ref mut id, .. } =>
				id.reanchor(prepend).map_err(|_| ()),
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
}

impl<A: Into<AssetId>, B: Into<WildFungibility>> From<(A, B)> for WildMultiAsset {
	fn from((id, fun): (A, B)) -> WildMultiAsset {
		WildMultiAsset::AllOf { fun: fun.into(), id: id.into() }
	}
}

/// `MultiAsset` collection, either `MultiAssets` or a single wildcard.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Debug, Encode, Decode, TypeInfo)]
pub enum MultiAssetFilter {
	Definite(MultiAssets),
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
			MultiAssetFilter::Wild(ref wild) => wild.matches(inner),
		}
	}

	/// Prepend a `MultiLocation` to any concrete asset components, giving it a new root location.
	pub fn reanchor(&mut self, prepend: &MultiLocation) -> Result<(), ()> {
		match self {
			MultiAssetFilter::Definite(ref mut assets) => assets.reanchor(prepend),
			MultiAssetFilter::Wild(ref mut wild) => wild.reanchor(prepend),
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

impl From<OldMultiAssetFilter> for MultiAssetFilter {
	fn from(old: OldMultiAssetFilter) -> MultiAssetFilter {
		match old {
			OldMultiAssetFilter::Definite(x) => Self::Definite(x.into()),
			OldMultiAssetFilter::Wild(x) => Self::Wild(x.into()),
		}
	}
}

impl TryFrom<(OldMultiAssetFilter, u32)> for MultiAssetFilter {
	type Error = ();
	fn try_from(old: (OldMultiAssetFilter, u32)) -> Result<MultiAssetFilter, ()> {
		let count = old.1;
		Ok(match old.0 {
			OldMultiAssetFilter::Definite(x) if count >= x.len() as u32 => Self::Definite(x.into()),
			OldMultiAssetFilter::Wild(x) => Self::Wild((x, count).into()),
			_ => return Err(()),
		})
	}
}
