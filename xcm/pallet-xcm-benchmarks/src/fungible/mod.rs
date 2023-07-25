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

// Benchmarking for the `AssetTransactor` trait via `Fungible`.

pub use pallet::*;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
#[cfg(test)]
mod mock;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::Get;
	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config + crate::Config {
		/// The type of `fungible` that is being used under the hood.
		///
		/// This is useful for testing and checking.
		type TransactAsset: frame_support::traits::fungible::Mutate<Self::AccountId>;

		/// The account used to check assets being teleported.
		type CheckedAccount: Get<Option<(Self::AccountId, xcm_builder::MintLocation)>>;

		/// A trusted location which we allow teleports from, and the asset we allow to teleport.
		type TrustedTeleporter: Get<Option<(xcm::latest::MultiLocation, xcm::latest::MultiAsset)>>;

		/// A trusted location where reserve assets are stored, and the asset we allow to be
		/// reserves.
		type TrustedReserve: Get<Option<(xcm::latest::MultiLocation, xcm::latest::MultiAsset)>>;

		/// Give me a fungible asset that your asset transactor is going to accept.
		fn get_multi_asset() -> xcm::latest::MultiAsset;
	}

	#[pallet::pallet]
	pub struct Pallet<T, I = ()>(_);
}
