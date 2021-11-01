#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use frame_support::{traits::Get, weights::Weight};
use sp_std::marker::PhantomData;

pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> runtime_parachains::paras_inherent::WeightInfo for WeightInfo<T> {
	fn enter_variable_disputes(_v: u32) -> Weight {
		Weight::MAX
	}
	fn enter_bitfields() -> Weight {
		Weight::MAX
	}
	fn enter_backed_candidates_variable(_v: u32) -> Weight {
		Weight::MAX
	}
}
