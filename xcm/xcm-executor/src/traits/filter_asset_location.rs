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

use frame_support::traits::ContainsPair;
use xcm::latest::{MultiAsset, MultiLocation};

/// Filters assets/location pairs.
///
/// Can be amalgamated into tuples. If any item returns `true`, it short-circuits, else `false` is
/// returned.
#[deprecated = "Use `frame_support::traits::ContainsPair<MultiAsset, MultiLocation>` instead"]
pub trait FilterAssetLocation {
	/// A filter to distinguish between asset/location pairs.
	fn contains(asset: &MultiAsset, origin: &MultiLocation) -> bool;
}

#[allow(deprecated)]
impl<T: ContainsPair<MultiAsset, MultiLocation>> FilterAssetLocation for T {
	fn contains(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		T::contains(asset, origin)
	}
}
