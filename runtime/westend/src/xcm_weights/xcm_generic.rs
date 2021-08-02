pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo<T> {
	fn send_xcm() -> Weight {
		10
	}
	fn order_null() -> Weight {
		10
	}
	fn xcm_transact() -> Weight {
		10
	}
	fn xcm_hrmp_channel_open_request() -> Weight {
		10
	}
	fn xcm_hrmp_channel_accepted() -> Weight {
		10
	}
	fn xcm_hrmp_channel_closing() -> Weight {
		10
	}
	fn xcm_relayed_from() -> Weight {
		10
	}
}
