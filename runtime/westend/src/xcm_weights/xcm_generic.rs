use frame_support::dispatch::Weight;

pub struct WeightInfo<T>(sp_std::marker::PhantomData<T>);
impl<T: frame_system::Config> WeightInfo<T> {
	pub fn order_noop() -> Weight {
		10
	}
	pub fn order_query_holding() -> Weight {
		10
	}
	pub fn order_buy_execution() -> Weight {
		10
	}
	pub fn xcm_transact() -> Weight {
		10
	}
	pub fn xcm_reserve_asset_deposited() -> Weight {
		10
	}
	pub fn xcm_hrmp_new_channel_open_request() -> Weight {
		10
	}
	pub fn xcm_hrmp_channel_accepted() -> Weight {
		10
	}
	pub fn xcm_hrmp_channel_closing() -> Weight {
		10
	}
	pub fn xcm_relayed_from() -> Weight {
		10
	}
	pub fn xcm_query_response() -> Weight {
		10
	}
}
