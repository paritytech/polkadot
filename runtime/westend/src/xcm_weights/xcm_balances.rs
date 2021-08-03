use frame_support::dispatch::Weight;

pub struct WeightInfo<T>(sp_std::marker::PhantomData<T>);
impl<T: frame_system::Config> WeightInfo<T> {
	pub fn order_deposit_asset() -> Weight {
		10
	}
	pub fn order_deposit_reserved_asset() -> Weight {
		10
	}
	pub fn order_exchange_asset() -> Weight {
		10
	}
	pub fn order_initiate_reserve_withdraw() -> Weight {
		10
	}
	pub fn order_initiate_teleport() -> Weight {
		10
	}
	pub fn order_query_holding() -> Weight {
		10
	}
	pub fn order_buy_execution() -> Weight {
		10
	}
	pub fn xcm_withdraw_asset() -> Weight {
		10
	}
	pub fn xcm_reserve_asset_deposit() -> Weight {
		10
	}
	pub fn xcm_teleport_asset() -> Weight {
		10
	}
	pub fn xcm_transfer_asset() -> Weight {
		10
	}
	pub fn xcm_transfer_reserved_asset() -> Weight {
		10
	}
}
