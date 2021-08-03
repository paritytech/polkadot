pub use pallet::*;

#[cfg(test)]
mod mock;
pub mod benchmarking;

#[frame_support::pallet]
pub mod pallet {
	use xcm::v0::MultiAsset;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The XCM configurations.
		///
		/// These might affect the execution of XCM messages, such as defining how the
		/// `TransactAsset` is implemented.
		type XcmConfig: xcm_executor::Config;

		/// The type of `fungible` that is being used under the hood.
		///
		/// This is useful for testing and checking.
		type TransactAsset: frame_support::traits::fungible::Mutate<Self::AccountId>;

		/// Give me a fungible asset that your asset transactor is going to accept.
		fn get_multi_asset() -> MultiAsset;
	}

	// transact asset that works with balances and asset
	//
	// transact asset that works with 3 assets

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);
}
