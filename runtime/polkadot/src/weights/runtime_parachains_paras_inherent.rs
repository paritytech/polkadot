#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> runtime_parachains::paras_inherent::WeightInfo for WeightInfo<T> {
	fn enter_backed_dominant(_b: u32) -> Weight {
		Weight::MAX
	}
	fn enter_dispute_dominant(_d: u32) -> Weight {
		Weight::MAX
	}
	fn enter_disputes_only(_d: u32) -> Weight {
		Weight::MAX
	}
	fn enter_backed_only(_b: u32) -> Weight {
		Weight::MAX
	}
}
