// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Pallet that serves no other purpose than benchmarking raw messages [`Xcm`].

#![cfg_attr(not(feature = "std"), no_std)]

use codec::Codec;
pub use pallet::*;
use xcm::v0::MultiAsset;

#[cfg(test)]
mod mock_fungible;
#[cfg(test)]
mod mock_fungibles;
#[cfg(test)]
mod mock_shared;

mod benchmarking;
pub mod weights;
// pub use weights::*;

#[frame_support::pallet]
pub mod pallet {
	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The XCM configurations.
		///
		/// These might affect the execution of XCM messages, such as defining how the
		/// `TransactAsset` is implemented.
		type XcmConfig: xcm_executor::Config;

		type FungibleTransactAsset: frame_support::traits::fungible::Inspect<Self::AccountId>;
		type FungiblesTransactAsset: frame_support::traits::fungibles::Inspect<Self::AccountId>;
		// type NonFungiblesTransactAsset: frame_support::traits::fungibles::Inspect<Self::AccountId>;
	}

	// transact asset that works with balances and asset
	//
	// transact asset that works with 3 assets

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);
}

// With this, we measure all weights per asset, so NONE of the benchmarks need to have a component
// that is the number of assets, that's pretty pointless. You need to iterate the `Vec<MultiAsset>`
// down the road
enum AssetWeightType {
	Fungible,    // Balances
	Fungibles,   // assets,
	NonFungible, // Uniques
}

trait IdentifyAsset<R> {
	fn identify_asset(asset: MultiAsset) -> R;
}

use frame_support::traits::{
	fungible::Inspect as FungibleInspect,
	fungibles::Inspect as FungiblesInspect,
	tokens::{DepositConsequence, WithdrawConsequence},
};

pub struct AsFungibles<AccountId, AssetId, B>(sp_std::marker::PhantomData<(AccountId, AssetId, B)>);
impl<
		AccountId: sp_runtime::traits::Member + frame_support::dispatch::Parameter,
		AssetId: sp_runtime::traits::Member + frame_support::dispatch::Parameter + Copy,
		B: FungibleInspect<AccountId>,
	> FungiblesInspect<AccountId> for AsFungibles<AccountId, AssetId, B>
{
	type AssetId = AssetId;
	type Balance = B::Balance;

	fn total_issuance(_: Self::AssetId) -> Self::Balance {
		B::total_issuance()
	}
	fn minimum_balance(_: Self::AssetId) -> Self::Balance {
		B::minimum_balance()
	}
	fn balance(_: Self::AssetId, who: &AccountId) -> Self::Balance {
		B::balance(who)
	}
	fn reducible_balance(_: Self::AssetId, who: &AccountId, keep_alive: bool) -> Self::Balance {
		B::reducible_balance(who, keep_alive)
	}
	fn can_deposit(_: Self::AssetId, who: &AccountId, amount: Self::Balance) -> DepositConsequence {
		B::can_deposit(who, amount)
	}

	fn can_withdraw(
		_: Self::AssetId,
		who: &AccountId,
		amount: Self::Balance,
	) -> WithdrawConsequence<Self::Balance> {
		B::can_withdraw(who, amount)
	}
}
