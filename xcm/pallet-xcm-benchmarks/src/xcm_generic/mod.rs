pub use pallet::*;

pub mod benchmarking;
#[cfg(test)]
mod mock;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::{dispatch::Dispatchable, pallet_prelude::Encode, weights::GetDispatchInfo};
	use xcm::latest::Response;

	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config + crate::Config {
		type Call: Dispatchable<Origin = Self::Origin>
			+ GetDispatchInfo
			+ From<frame_system::Call<Self>>
			+ Encode;

		fn worst_case_response() -> (u64, Response);
	}

	#[pallet::pallet]
	pub struct Pallet<T, I = ()>(_);
}
