pub use pallet::*;

pub mod benchmarking;
#[cfg(test)]
mod mock;

#[frame_support::pallet]
pub mod pallet {
	use crate::MultiAsset;

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config + crate::Config {
		/// The type of `fungibles` that is being used under the hood.
		///
		/// This is useful for testing and checking.
		type TransactAsset: frame_support::traits::fungibles::Mutate<Self::AccountId>;

		/// Give me a `fungibles` multi-asset that your asset transactor is going to accept.
		fn get_multi_asset(id: u32) -> MultiAsset;
	}

	#[pallet::pallet]
	pub struct Pallet<T, I = ()>(_);
}
