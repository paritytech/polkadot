pub use pallet::*;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;
#[cfg(test)]
mod mock;

#[frame_support::pallet]
pub mod pallet {
	use frame_benchmarking::BenchmarkError;
	use frame_support::{
		dispatch::{Dispatchable, GetDispatchInfo},
		pallet_prelude::Encode,
	};
	use xcm::latest::{Junction, MultiAsset, MultiAssets, MultiLocation, Response};

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

		/// A `Junction` that is one of the `UniversalAliases` configured by the XCM executor.
		///
		/// If set to `Err`, benchmarks which rely on a universal alias will be skipped.
		fn universal_alias() -> Result<Junction, BenchmarkError>;

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
	}

	#[pallet::pallet]
	pub struct Pallet<T, I = ()>(_);
}
