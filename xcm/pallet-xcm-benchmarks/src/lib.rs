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

use codec::Encode;
use frame_support::{
	dispatch::Weight,
	traits::{
		fungible::Inspect as FungibleInspect,
		fungibles::Inspect as FungiblesInspect,
		tokens::{DepositConsequence, WithdrawConsequence},
	},
};
use sp_std::prelude::*;
use xcm::v0::{
	AssetInstance, Error as XcmError, ExecuteXcm, Junction, MultiAsset, MultiLocation, NetworkId,
	Order, Outcome, Xcm,
};
use xcm_executor::Assets;

pub mod fungible;
pub mod fungibles;
pub mod xcm_generic;

#[cfg(test)]
mod mock;

/// A base trait for all individual pallets
pub trait Config {
	/// The XCM configurations.
	///
	/// These might affect the execution of XCM messages, such as defining how the
	/// `TransactAsset` is implemented.
	type XcmConfig: xcm_executor::Config;
}

const SEED: u32 = 0;
const MAX_WEIGHT: Weight = 999_999_999_999;

/// The xcm executor to use for doing stuff.
pub type ExecutorOf<T> = xcm_executor::XcmExecutor<<T as Config>::XcmConfig>;
/// The overarching call type.
pub type OverArchingCallOf<T> = <T as frame_system::Config>::Call;
/// The asset transactor of our executor
pub type AssetTransactorOf<T> = <<T as Config>::XcmConfig as xcm_executor::Config>::AssetTransactor;
/// The call type of executor's config. Should eventually resolve to the same overarching call type.
pub type XcmCallOf<T> = <<T as Config>::XcmConfig as xcm_executor::Config>::Call;

pub fn create_holding(
	fungibles_count: u32,
	fungibles_amount: u128,
	non_fungibles_count: u32,
) -> Assets {
	(0..fungibles_count)
		.map(|i| {
			MultiAsset::ConcreteFungible {
				id: MultiLocation::X1(Junction::GeneralIndex { id: i as u128 }),
				amount: fungibles_amount * i as u128,
			}
			.into()
		})
		.chain((0..non_fungibles_count).map(|i| MultiAsset::ConcreteNonFungible {
			class: MultiLocation::X1(Junction::GeneralIndex { id: i as u128 }),
			instance: asset_instance_from(i),
		}))
		.collect::<Vec<_>>()
		.into()
}

pub fn asset_instance_from(x: u32) -> AssetInstance {
	let bytes = x.encode();
	let mut instance = [0u8; 4];
	instance.copy_from_slice(&bytes);
	AssetInstance::Array4(instance)
}

/// wrapper to execute single order. Can be any hack, for now we just do a noop-xcm with a single
/// order.
pub fn execute_order<T: Config>(
	origin: MultiLocation,
	mut holding: Assets,
	order: Order<XcmCallOf<T>>,
) -> Result<Weight, XcmError> {
	ExecutorOf::<T>::do_execute_effects(&origin, &mut holding, order)
}

/// Execute an xcm.
pub fn execute_xcm<T: Config>(origin: MultiLocation, xcm: Xcm<XcmCallOf<T>>) -> Outcome {
	// TODO: very large weight to ensure all benchmarks execute, sensible?
	ExecutorOf::<T>::execute_xcm(origin, xcm, MAX_WEIGHT)
}

pub fn account<T: frame_system::Config>(index: u32) -> T::AccountId {
	frame_benchmarking::account::<T::AccountId>("account", index, SEED)
}

/// Build a multi-location from an account id.
fn account_id_junction<T: frame_system::Config>(index: u32) -> Junction {
	let account = account::<T>(index);
	let mut encoded = account.encode();
	encoded.resize(32, 0u8);
	let mut id = [0u8; 32];
	id.copy_from_slice(&encoded);
	Junction::AccountId32 { network: NetworkId::Any, id }
}

pub fn account_and_location<T: frame_system::Config>(index: u32) -> (T::AccountId, MultiLocation) {
	let account = account::<T>(index);
	let location = MultiLocation::X1(account_id_junction::<T>(index));

	(account, location)
}

/// Helper struct that converts a `Fungible` to `Fungibles`
///
/// TODO: might not be needed anymore.
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
