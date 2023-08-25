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

pub use pallet::*;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
#[cfg(test)]
mod mock;

#[frame_support::pallet]
pub mod pallet {
	use frame_benchmarking::BenchmarkError;
	use frame_support::{dispatch::GetDispatchInfo, pallet_prelude::Encode};
	use sp_runtime::traits::Dispatchable;
	use xcm::latest::{
		InteriorMultiLocation, Junction, MultiAsset, MultiAssets, MultiLocation, NetworkId,
		Response,
	};

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config + crate::Config {
		type RuntimeCall: Dispatchable<RuntimeOrigin = Self::RuntimeOrigin>
			+ GetDispatchInfo
			+ From<frame_system::Call<Self>>
			+ Encode;

		///	The response which causes the most runtime weight.
		fn worst_case_response() -> (u64, Response);

		/// The pair of asset collections which causes the most runtime weight if demanded to be
		/// exchanged.
		///
		/// The first element in the returned tuple represents the assets that are being exchanged
		/// from, whereas the second element represents the assets that are being exchanged to.
		///
		/// If set to `Err`, benchmarks which rely on an `exchange_asset` will be skipped.
		fn worst_case_asset_exchange() -> Result<(MultiAssets, MultiAssets), BenchmarkError>;

		/// A `(MultiLocation, Junction)` that is one of the `UniversalAliases` configured by the
		/// XCM executor.
		///
		/// If set to `Err`, benchmarks which rely on a universal alias will be skipped.
		fn universal_alias() -> Result<(MultiLocation, Junction), BenchmarkError>;

		/// The `MultiLocation` and `RuntimeCall` used for successful transaction XCMs.
		///
		/// If set to `Err`, benchmarks which rely on a `transact_origin_and_runtime_call` will be
		/// skipped.
		fn transact_origin_and_runtime_call(
		) -> Result<(MultiLocation, <Self as crate::generic::Config<I>>::RuntimeCall), BenchmarkError>;

		/// A valid `MultiLocation` we can successfully subscribe to.
		///
		/// If set to `Err`, benchmarks which rely on a `subscribe_origin` will be skipped.
		fn subscribe_origin() -> Result<MultiLocation, BenchmarkError>;

		/// Return an origin, ticket, and assets that can be trapped and claimed.
		fn claimable_asset() -> Result<(MultiLocation, MultiLocation, MultiAssets), BenchmarkError>;

		/// Return an unlocker, owner and assets that can be locked and unlocked.
		fn unlockable_asset() -> Result<(MultiLocation, MultiLocation, MultiAsset), BenchmarkError>;

		/// A `(MultiLocation, NetworkId, InteriorMultiLocation)` we can successfully export message
		/// to.
		///
		/// If set to `Err`, benchmarks which rely on `export_message` will be skipped.
		fn export_message_origin_and_destination(
		) -> Result<(MultiLocation, NetworkId, InteriorMultiLocation), BenchmarkError>;

		/// A `(MultiLocation, MultiLocation)` that is one of the `Aliasers` configured by the XCM
		/// executor.
		///
		/// If set to `Err`, benchmarks which rely on a universal alias will be skipped.
		fn alias_origin() -> Result<(MultiLocation, MultiLocation), BenchmarkError>;
	}

	#[pallet::pallet]
	pub struct Pallet<T, I = ()>(_);
}
