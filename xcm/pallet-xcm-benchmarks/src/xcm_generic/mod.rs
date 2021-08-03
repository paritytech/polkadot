pub use pallet::*;

pub mod benchmarking;
#[cfg(test)]
mod mock;

#[frame_support::pallet]
pub mod pallet {
	#[pallet::config]
	pub trait Config<I: 'static = ()>: frame_system::Config + crate::Config {}

	#[pallet::pallet]
	pub struct Pallet<T, I = ()>(_);
}
