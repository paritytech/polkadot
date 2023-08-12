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

//! Pallet that serves no other purpose than benchmarking raw messages [`Xcm`].

#![cfg_attr(not(feature = "std"), no_std)]

use codec::Encode;
use frame_benchmarking::{account, BenchmarkError};
use sp_std::prelude::*;
use xcm::latest::prelude::*;
use xcm_executor::{traits::ConvertLocation, Config as XcmConfig};

pub mod fungible;
pub mod generic;

#[cfg(test)]
mod mock;

/// A base trait for all individual pallets
pub trait Config: frame_system::Config {
	/// The XCM configurations.
	///
	/// These might affect the execution of XCM messages, such as defining how the
	/// `TransactAsset` is implemented.
	type XcmConfig: XcmConfig;

	/// A converter between a multi-location to a sovereign account.
	type AccountIdConverter: ConvertLocation<Self::AccountId>;

	/// Does any necessary setup to create a valid destination for XCM messages.
	/// Returns that destination's multi-location to be used in benchmarks.
	fn valid_destination() -> Result<MultiLocation, BenchmarkError>;

	/// Worst case scenario for a holding account in this runtime.
	fn worst_case_holding(depositable_count: u32) -> MultiAssets;
}

const SEED: u32 = 0;

/// The XCM executor to use for doing stuff.
pub type ExecutorOf<T> = xcm_executor::XcmExecutor<<T as Config>::XcmConfig>;
/// The overarching call type.
pub type OverArchingCallOf<T> = <T as frame_system::Config>::RuntimeCall;
/// The asset transactor of our executor
pub type AssetTransactorOf<T> = <<T as Config>::XcmConfig as XcmConfig>::AssetTransactor;
/// The call type of executor's config. Should eventually resolve to the same overarching call type.
pub type XcmCallOf<T> = <<T as Config>::XcmConfig as XcmConfig>::RuntimeCall;

pub fn mock_worst_case_holding(depositable_count: u32, max_assets: u32) -> MultiAssets {
	let fungibles_amount: u128 = 100;
	let holding_fungibles = max_assets / 2 - depositable_count;
	let holding_non_fungibles = holding_fungibles;
	(0..holding_fungibles)
		.map(|i| {
			MultiAsset {
				id: Concrete(GeneralIndex(i as u128).into()),
				fun: Fungible(fungibles_amount * i as u128),
			}
			.into()
		})
		.chain(core::iter::once(MultiAsset { id: Concrete(Here.into()), fun: Fungible(u128::MAX) }))
		.chain((0..holding_non_fungibles).map(|i| MultiAsset {
			id: Concrete(GeneralIndex(i as u128).into()),
			fun: NonFungible(asset_instance_from(i)),
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

pub fn new_executor<T: Config>(origin: MultiLocation) -> ExecutorOf<T> {
	ExecutorOf::<T>::new(origin, [0; 32])
}

/// Build a multi-location from an account id.
fn account_id_junction<T: frame_system::Config>(index: u32) -> Junction {
	let account: T::AccountId = account("account", index, SEED);
	let mut encoded = account.encode();
	encoded.resize(32, 0u8);
	let mut id = [0u8; 32];
	id.copy_from_slice(&encoded);
	Junction::AccountId32 { network: None, id }
}

pub fn account_and_location<T: Config>(index: u32) -> (T::AccountId, MultiLocation) {
	let location: MultiLocation = account_id_junction::<T>(index).into();
	let account = T::AccountIdConverter::convert_location(&location).unwrap();

	(account, location)
}
