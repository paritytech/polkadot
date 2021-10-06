// this whole file is temp, and will be replaced in the future TODO

use frame_support::dispatch::Weight;

pub struct WeightInfo<T>(sp_std::marker::PhantomData<T>);
impl<T: frame_system::Config> WeightInfo<T> {
	pub fn query_holding() -> Weight {
		1_000_000_000
	}
	pub fn buy_execution() -> Weight {
		1_000_000_000
	}
	pub fn transact() -> Weight {
		1_000_000_000
	}
	pub fn reserve_asset_deposited() -> Weight {
		1_000_000_000
	}
	pub fn hrmp_new_channel_open_request() -> Weight {
		1_000_000_000
	}
	pub fn hrmp_channel_accepted() -> Weight {
		1_000_000_000
	}
	pub fn hrmp_channel_closing() -> Weight {
		1_000_000_000
	}
	pub fn relayed_from() -> Weight {
		1_000_000_000
	}
	pub fn refund_surplus() -> Weight {
		1_000_000_000
	}
	pub fn set_error_handler() -> Weight {
		1_000_000_000
	}
	pub fn set_appendix() -> Weight {
		1_000_000_000
	}
	pub fn clear_error() -> Weight {
		1_000_000_000
	}
	pub fn claim_asset(_assets: &crate::MultiAssets) -> Weight {
		1_000_000_000
	}
	pub fn trap(_code: &u64) -> Weight {
		1_000_000_000
	}

	pub fn subscribe_version() -> Weight {
		1_000_000_000
	}

	pub fn unsubscribe_version() -> Weight {
		1_000_000_000
	}

	pub fn clear_origin() -> Weight {
		1_000_000_000
	}

	pub fn descend_origin(_who: &crate::InteriorMultiLocation) -> Weight {
		1_000_000_000
	}

	pub fn initiate_reserve_withdraw() -> Weight {
		1_000_000_000
	}

	pub fn report_error() -> Weight {
		1_000_000_000
	}
}
