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

//! Various implementations of `FilterAssetLocation`.

use sp_std::marker::PhantomData;
use xcm::v0::{MultiAsset, MultiLocation};
use frame_support::traits::Get;
use xcm_executor::traits::FilterAssetLocation;

/// Accepts an asset IFF it is a native asset.
pub struct NativeAsset;
impl FilterAssetLocation for NativeAsset {
	fn filter_asset_location(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		matches!(asset, MultiAsset::ConcreteFungible { ref id, .. } if id == origin)
	}
}

/// Accepts an asset if it is contained in the given `T`'s `Get` impl.
pub struct Case<T>(PhantomData<T>);
impl<T: Get<(MultiAsset, MultiLocation)>> FilterAssetLocation for Case<T> {
	fn filter_asset_location(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		let (a, o) = T::get();
		a.contains(asset) && &o == origin
	}
}
