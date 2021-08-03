use frame_support::dispatch::Weight;

pub struct WeightInfo<T>(sp_std::marker::PhantomData<T>);
impl<T: frame_system::Config> WeightInfo<T> {
	pub fn send_xcm() -> Weight {
		10
	}
	pub fn order_null() -> Weight {
		10
	}
	pub fn xcm_transact() -> Weight {
		10
	}
	pub fn xcm_hrmp_channel_open_request() -> Weight {
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
