pub use pallet::*;

pub mod benchmarking;
#[cfg(test)]
mod mock;
// #[cfg(test)]
// mod mock2;
// TODO: make this instanciable.
#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::Get;
	use xcm::latest::{MultiAsset, MultiLocation};

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config + crate::Config {
		/// The type of `fungible` that is being used under the hood.
		///
		/// This is useful for testing and checking.
		type TransactAsset: frame_support::traits::fungible::Mutate<Self::AccountId>;

		/// Maybe I can get this in some better way?
		type CheckedAccount: Get<Option<Self::AccountId>>;

		/// A valid destination location which can be used in benchmarks.
		type ValidDestination: Get<MultiLocation>;

		/// Give me a fungible asset that your asset transactor is going to accept.
		fn get_multi_asset() -> xcm::latest::MultiAsset;
	}

	#[pallet::pallet]
	pub struct Pallet<T, I = ()>(_);
}
