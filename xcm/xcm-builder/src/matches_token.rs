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

//! Various implementations for the `MatchesFungible` trait.

use frame_support::traits::Get;
use sp_std::marker::PhantomData;
use xcm::latest::{
	AssetId::{Abstract, Concrete},
	AssetInstance,
	Fungibility::{Fungible, NonFungible},
	MultiAsset, MultiLocation,
};
use xcm_executor::traits::{MatchesFungible, MatchesNonFungible};

/// Converts a `MultiAsset` into balance `B` if it is a concrete fungible with an id equal to that
/// given by `T`'s `Get`.
///
/// # Example
///
/// ```
/// use xcm::latest::{MultiLocation, Parent};
/// use xcm_builder::IsConcrete;
/// use xcm_executor::traits::MatchesFungible;
///
/// frame_support::parameter_types! {
/// 	pub TargetLocation: MultiLocation = Parent.into();
/// }
///
/// # fn main() {
/// let asset = (Parent, 999).into();
/// // match `asset` if it is a concrete asset in `TargetLocation`.
/// assert_eq!(<IsConcrete<TargetLocation> as MatchesFungible<u128>>::matches_fungible(&asset), Some(999));
/// # }
/// ```
pub struct IsConcrete<T>(PhantomData<T>);
impl<T: Get<MultiLocation>, B: TryFrom<u128>> MatchesFungible<B> for IsConcrete<T> {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		match (&a.id, &a.fun) {
			(Concrete(ref id), Fungible(ref amount)) if id == &T::get() =>
				(*amount).try_into().ok(),
			_ => None,
		}
	}
}
impl<T: Get<MultiLocation>, I: TryFrom<AssetInstance>> MatchesNonFungible<I> for IsConcrete<T> {
	fn matches_nonfungible(a: &MultiAsset) -> Option<I> {
		match (&a.id, &a.fun) {
			(Concrete(id), NonFungible(instance)) if id == &T::get() => (*instance).try_into().ok(),
			_ => None,
		}
	}
}

/// Same as [`IsConcrete`] but for a fungible with abstract location.
///
/// # Example
///
/// ```
/// use xcm::latest::prelude::*;
/// use xcm_builder::IsAbstract;
/// use xcm_executor::traits::{MatchesFungible, MatchesNonFungible};
///
/// frame_support::parameter_types! {
/// 	pub TargetLocation: [u8; 32] = [7u8; 32];
/// }
///
/// # fn main() {
/// let asset = ([7u8; 32], 999u128).into();
/// // match `asset` if it is an abstract asset in `TargetLocation`.
/// assert_eq!(<IsAbstract<TargetLocation> as MatchesFungible<u128>>::matches_fungible(&asset), Some(999));
/// let nft = ([7u8; 32], [42u8; 4]).into();
/// assert_eq!(
///     <IsAbstract<TargetLocation> as MatchesNonFungible<[u8; 4]>>::matches_nonfungible(&nft),
///     Some([42u8; 4])
/// );
/// # }
/// ```
pub struct IsAbstract<T>(PhantomData<T>);
impl<T: Get<[u8; 32]>, B: TryFrom<u128>> MatchesFungible<B> for IsAbstract<T> {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		match (&a.id, &a.fun) {
			(Abstract(ref id), Fungible(ref amount)) if id == &T::get() =>
				(*amount).try_into().ok(),
			_ => None,
		}
	}
}
impl<T: Get<[u8; 32]>, B: TryFrom<AssetInstance>> MatchesNonFungible<B> for IsAbstract<T> {
	fn matches_nonfungible(a: &MultiAsset) -> Option<B> {
		match (&a.id, &a.fun) {
			(Abstract(id), NonFungible(instance)) if id == &T::get() => (*instance).try_into().ok(),
			_ => None,
		}
	}
}
