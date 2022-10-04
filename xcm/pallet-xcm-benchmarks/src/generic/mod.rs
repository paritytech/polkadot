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
	use xcm::latest::{MultiAssets, MultiLocation, Response};

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config + crate::Config {
		type RuntimeCall: Dispatchable<RuntimeOrigin = Self::RuntimeOrigin>
			+ GetDispatchInfo
			+ From<frame_system::Call<Self>>
			+ Encode;

		///	The response which causes the most runtime weight.
		fn worst_case_response() -> (u64, Response);

		/// The `MultiLocation` used for successful transaction XCMs.
		///
		/// If set to `None`, benchmarks which rely on a `transact_origin` will be skipped.
		fn transact_origin() -> Result<MultiLocation, BenchmarkError>;

		/// A valid `MultiLocation` we can successfully subscribe to.
		///
		/// If set to `None`, benchmarks which rely on a `subscribe_origin` will be skipped.
		fn subscribe_origin() -> Result<MultiLocation, BenchmarkError>;

		/// Return an origin, ticket, and assets that can be trapped and claimed.
		fn claimable_asset() -> Result<(MultiLocation, MultiLocation, MultiAssets), BenchmarkError>;
	}

	#[pallet::pallet]
	pub struct Pallet<T, I = ()>(_);
}
