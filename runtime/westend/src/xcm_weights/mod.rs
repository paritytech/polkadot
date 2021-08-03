mod xcm_balances;
mod xcm_generic;

use frame_support::weights::Weight;
use xcm_balances::WeightInfo as XcmBalancesWeight;

use crate::Runtime;
use sp_std::prelude::*;
use xcm::{
	v0::{MultiAsset, MultiLocation, Order, OriginKind, Response, Xcm, XcmWeightInfo},
	DoubleEncoded,
};
use xcm_generic::WeightInfo as XcmGeneric;

pub enum AssetTypes {
	Balances,
	Unknown,
}

impl From<MultiAsset> for AssetTypes {
	fn from(asset: MultiAsset) -> Self {
		match asset {
			MultiAsset::ConcreteFungible { .. } => AssetTypes::Balances,
			_ => AssetTypes::Unknown,
		}
	}
}

trait WeighMultiAssets {
	fn weigh_multi_assets(&self, balances_weight: Weight) -> Weight;
}

impl WeighMultiAssets for Vec<MultiAsset> {
	fn weigh_multi_assets(&self, balances_weight: Weight) -> Weight {
		self.iter()
			.map(|m| <AssetTypes as From<MultiAsset>>::from(m))
			.map(|t| match t {
				AssetTypes::Balances => balances_weight,
				AssetTypes::Unknown => Weight::MAX,
			})
			.sum()
	}
}

pub struct WestendXcmWeight;
impl XcmWeightInfo<()> for WestendXcmWeight {
	fn send_xcm() -> Weight {
		XcmGeneric::<Runtime>::send_xcm()
	}
	fn order_null() -> Weight {
		XcmGeneric::<Runtime>::order_null()
	}
	fn order_deposit_asset(assets: &Vec<MultiAsset>, dest: &MultiLocation) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::order_deposit_asset())
	}
	fn order_deposit_reserved_asset(
		assets: Vec<MultiAsset>,
		dest: MultiLocation,
		effects: Vec<Order<()>>,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::order_deposit_reserved_asset())
	}
	fn order_exchange_asset(give: Vec<MultiAsset>, receive: Vec<MultiAsset>) -> Weight {
		unimplemented!()
	}
	fn order_initiate_reserve_withdraw(
		assets: Vec<MultiAsset>,
		reserve: MultiLocation,
		effects: Vec<Order<()>>,
	) -> Weight {
		unimplemented!()
	}
	fn order_initiate_teleport(
		assets: Vec<MultiAsset>,
		dest: MultiLocation,
		effects: Vec<Order<()>>,
	) -> Weight {
		unimplemented!()
	}
	fn order_query_holding(query_id: u64, dest: MultiLocation, assets: Vec<MultiAsset>) -> Weight {
		unimplemented!()
	}
	fn order_buy_execution(
		fees: MultiAsset,
		weight: u64,
		debt: u64,
		halt_on_error: bool,
		xcm: Vec<Xcm<()>>,
	) -> Weight {
		XcmGeneric::<Runtime>::order_buy_execution()
	}
	fn xcm_withdraw_asset(assets: Vec<MultiAsset>, effects: Vec<Order<()>>) -> Weight {
		unimplemented!()
	}
	fn xcm_reserve_asset_deposit(assets: Vec<MultiAsset>, effects: Vec<Order<()>>) -> Weight {
		unimplemented!()
	}
	// TODO none of these need effects
	fn xcm_teleport_asset(assets: Vec<MultiAsset>, effects: Vec<Order<()>>) -> Weight {
		unimplemented!()
	}
	fn xcm_query_response(query_id: u64, response: Response) -> Weight {
		XcmGeneric::<Runtime>::xcm_query_response()
	}
	fn xcm_transfer_asset(assets: Vec<MultiAsset>, dest: MultiLocation) -> Weight {
		unimplemented!()
	}
	fn xcm_transfer_reserved_asset(assets: Vec<MultiAsset>, dest: MultiLocation) -> Weight {
		unimplemented!()
	}
	fn xcm_transact(
		origin_type: OriginKind,
		require_weight_at_most: u64,
		call: DoubleEncoded<crate::Call>,
	) -> Weight {
		XcmGeneric::<Runtime>::xcm_transact()
	}
	fn xcm_hrmp_channel_open_request(
		sender: u32,
		max_message_size: u32,
		max_capacity: u32,
	) -> Weight {
		XcmGeneric::<Runtime>::xcm_hrmp_channel_open_request()
	}
	fn xcm_hrmp_channel_accepted(recipient: u32) -> Weight {
		XcmGeneric::<Runtime>::xcm_hrmp_channel_accepted()
	}
	fn xcm_hrmp_channel_closing(initiator: u32, sender: u32, recipient: u32) -> Weight {
		XcmGeneric::<Runtime>::xcm_hrmp_channel_accepted()
	}
	fn xcm_relayed_from(who: MultiLocation, message: Box<Xcm<()>>) -> Weight {
		XcmGeneric::<Runtime>::xcm_relayed_from()
	}
}
