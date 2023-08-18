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

//! Various implementations of `ContainsPair<MultiAsset, MultiLocation>`.

use frame_support::traits::{ContainsPair, Get};
use sp_std::marker::PhantomData;
use xcm::latest::{AssetId::Concrete, MultiAsset, MultiAssetFilter, MultiLocation};

/// Accepts an asset iff it is a native asset.
pub struct NativeAsset;
impl ContainsPair<MultiAsset, MultiLocation> for NativeAsset {
	fn contains(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		log::trace!(target: "xcm::contains", "NativeAsset asset: {:?}, origin: {:?}", asset, origin);
		matches!(asset.id, Concrete(ref id) if id == origin)
	}
}

/// Accepts an asset if it is contained in the given `T`'s `Get` implementation.
pub struct Case<T>(PhantomData<T>);
impl<T: Get<(MultiAssetFilter, MultiLocation)>> ContainsPair<MultiAsset, MultiLocation>
	for Case<T>
{
	fn contains(asset: &MultiAsset, origin: &MultiLocation) -> bool {
		log::trace!(target: "xcm::contains", "Case asset: {:?}, origin: {:?}", asset, origin);
		let (a, o) = T::get();
		a.matches(asset) && &o == origin
	}
}
