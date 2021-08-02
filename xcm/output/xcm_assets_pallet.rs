pub struct XcmAssetsWeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfoSingle for XcmAssetsWeightInfo<T> {
	fn send_xcm() -> Weight {
		10
	}
	fn order_null() -> Weight {
		10
	}
	fn order_deposit_asset() -> Weight {
		10
	}
	fn order_deposit_reserved_asset() -> Weight {
		10
	}
	fn order_exchange_asset() -> Weight {
		10
	}
	fn order_initiate_reserve_withdraw(&self) -> Weight {
		10
	}
	fn order_initiate_teleport() -> Weight {
		10
	}
	fn order_query_holding() -> Weight {
		10
	}
	fn order_buy_execution() -> Weight {
		10
	}
	fn xcm_withdraw_asset() -> Weight {
		10
	}
	fn xcm_reserve_asset_deposit() -> Weight {
		10
	}
	fn xcm_teleport_asset() -> Weight {
		10
	}
	fn xcm_query_response() -> Weight {
		10
	}
	fn xcm_transfer_asset() -> Weight {
		10
	}
	fn xcm_transfer_reserved_asset() -> Weight {
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
