
//! Weights for `runtime_parachains::Disputes`


#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

/// Weight functions for `runtime_parachains::paras`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> super::WeightInfo for WeightInfo<T> {
	// Storage: Disputes Frozen (r:0 w:1)
	fn force_unfreeze() -> Weight {
		(14_669_000 as Weight)
			// Standard Error: 0
			.saturating_add(T::DbWeight::get().writes(1 as Weight))
	}
}
