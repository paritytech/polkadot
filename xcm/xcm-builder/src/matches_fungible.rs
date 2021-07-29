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

//! Various implementations for the `MatchesFungible` trait.

use sp_std::{marker::PhantomData, convert::TryFrom};
use sp_runtime::traits::CheckedConversion;
use xcm::v0::{MultiAsset, MultiLocation};
use frame_support::traits::Get;
use xcm_executor::traits::MatchesFungible;

/// Converts a `MultiAsset` into balance `B` if it is a concrete fungible with an id equal to that
/// given by `T`'s `Get`.
///
/// # Example
///
/// ```
/// use xcm::v0::{MultiAsset, MultiLocation, Junction};
/// use xcm_builder::IsConcrete;
/// use xcm_executor::traits::MatchesFungible;
///
/// frame_support::parameter_types! {
/// 	pub TargetLocation: MultiLocation = MultiLocation::X1(Junction::Parent);
/// }
///
/// # fn main() {
/// let id = MultiLocation::X1(Junction::Parent);
/// let asset = MultiAsset::ConcreteFungible { id, amount: 999u128 };
/// // match `asset` if it is a concrete asset in `TargetLocation`.
/// assert_eq!(<IsConcrete<TargetLocation> as MatchesFungible<u128>>::matches_fungible(&asset), Some(999));
/// # }
/// ```
pub struct IsConcrete<T>(PhantomData<T>);
impl<T: Get<MultiLocation>, B: TryFrom<u128>> MatchesFungible<B> for IsConcrete<T> {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		match a {
			MultiAsset::ConcreteFungible { id, amount } if id == &T::get() =>
				CheckedConversion::checked_from(*amount),
			_ => None,
		}
	}
}

/// Same as [`IsConcrete`] but for a fungible with abstract location.
///
/// # Example
///
/// ```
/// use xcm::v0::{MultiAsset};
/// use xcm_builder::IsAbstract;
/// use xcm_executor::traits::MatchesFungible;
///
/// frame_support::parameter_types! {
/// 	pub TargetLocation: &'static [u8] = &[7u8];
/// }
///
/// # fn main() {
/// let asset = MultiAsset::AbstractFungible { id: vec![7u8], amount: 999u128 };
/// // match `asset` if it is a concrete asset in `TargetLocation`.
/// assert_eq!(<IsAbstract<TargetLocation> as MatchesFungible<u128>>::matches_fungible(&asset), Some(999));
/// # }
/// ```
pub struct IsAbstract<T>(PhantomData<T>);
impl<T: Get<&'static [u8]>, B: TryFrom<u128>> MatchesFungible<B> for IsAbstract<T> {
	fn matches_fungible(a: &MultiAsset) -> Option<B> {
		match a {
			MultiAsset::AbstractFungible { id, amount } if &id[..] == T::get() =>
				CheckedConversion::checked_from(*amount),
			_ => None,
		}
	}
}
