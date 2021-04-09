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

use sp_std::{marker::PhantomData, convert::TryFrom};
use sp_runtime::traits::CheckedConversion;
use xcm::v0::{MultiAsset, MultiLocation};
use frame_support::traits::Get;
use xcm_executor::traits::MatchesFungible;

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
