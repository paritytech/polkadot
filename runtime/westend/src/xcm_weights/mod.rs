mod xcm_balances;
mod xcm_generic;

use frame_support::weights::Weight;
use xcm_balances::WeightInfo as XcmBalancesWeight;

use crate::{Runtime, WndLocation};
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

impl From<&MultiAsset> for AssetTypes {
	fn from(asset: &MultiAsset) -> Self {
		let wnd_location = WndLocation::get();
		match asset {
			MultiAsset::ConcreteFungible { id: wnd_location, .. } => AssetTypes::Balances,
			_ => AssetTypes::Unknown,
		}
	}
}

// TODO from Shawn: I dont like this
trait WeighMultiAssets {
	fn weigh_multi_assets(&self, balances_weight: Weight) -> Weight;
}

impl WeighMultiAssets for Vec<MultiAsset> {
	fn weigh_multi_assets(&self, balances_weight: Weight) -> Weight {
		self.into_iter()
			.map(|m| <AssetTypes as From<&MultiAsset>>::from(m))
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
	fn order_deposit_asset(assets: &Vec<MultiAsset>, _dest: &MultiLocation) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::order_deposit_asset())
	}
	fn order_deposit_reserved_asset(
		assets: &Vec<MultiAsset>,
		_dest: &MultiLocation,
		_effects: &Vec<Order<()>>,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::order_deposit_reserved_asset())
	}
	fn order_exchange_asset(_give: &Vec<MultiAsset>, _receive: &Vec<MultiAsset>) -> Weight {
		Weight::MAX // todo fix
	}
	fn order_initiate_reserve_withdraw(
		assets: &Vec<MultiAsset>,
		_reserve: &MultiLocation,
		_effects: &Vec<Order<()>>,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::order_initiate_reserve_withdraw())
	}
	fn order_initiate_teleport(
		assets: &Vec<MultiAsset>,
		_dest: &MultiLocation,
		_effects: &Vec<Order<()>>,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::order_initiate_teleport())
	}
	fn order_query_holding(
		_query_id: &u64,
		_dest: &MultiLocation,
		_assets: &Vec<MultiAsset>,
	) -> Weight {
		XcmGeneric::<Runtime>::order_query_holding()
	}
	fn order_buy_execution(
		_fees: &MultiAsset,
		_weight: &u64,
		_debt: &u64,
		_halt_on_error: &bool,
		_xcm: &Vec<Xcm<()>>,
	) -> Weight {
		XcmGeneric::<Runtime>::order_buy_execution()
	}
	fn xcm_withdraw_asset(assets: &Vec<MultiAsset>, _effects: &Vec<Order<()>>) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::xcm_withdraw_asset())
	}
	fn xcm_reserve_asset_deposit(assets: &Vec<MultiAsset>, _effects: &Vec<Order<()>>) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::xcm_reserve_asset_deposit())
	}
	// TODO none of these need effects
	fn xcm_teleport_asset(assets: &Vec<MultiAsset>, _effects: &Vec<Order<()>>) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::xcm_teleport_asset())
	}
	fn xcm_query_response(_query_id: &u64, _response: &Response) -> Weight {
		XcmGeneric::<Runtime>::xcm_query_response()
	}
	fn xcm_transfer_asset(assets: &Vec<MultiAsset>, _dest: &MultiLocation) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::xcm_transfer_asset())
	}
	fn xcm_transfer_reserve_asset(
		assets: &Vec<MultiAsset>,
		_dest: &MultiLocation,
		_effects: &Vec<Order<()>>,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::xcm_transfer_reserve_asset())
	}
	fn xcm_transact(
		_origin_type: &OriginKind,
		_require_weight_at_most: &u64,
		_call: &DoubleEncoded<()>,
	) -> Weight {
		XcmGeneric::<Runtime>::xcm_transact()
	}
	fn xcm_hrmp_channel_open_request(
		_sender: &u32,
		_max_message_size: &u32,
		_max_capacity: &u32,
	) -> Weight {
		XcmGeneric::<Runtime>::xcm_hrmp_channel_open_request()
	}
	fn xcm_hrmp_channel_accepted(_recipient: &u32) -> Weight {
		XcmGeneric::<Runtime>::xcm_hrmp_channel_accepted()
	}
	fn xcm_hrmp_channel_closing(_initiator: &u32, _sender: &u32, _recipient: &u32) -> Weight {
		XcmGeneric::<Runtime>::xcm_hrmp_channel_closing()
	}
	fn xcm_relayed_from(_who: &MultiLocation, _message: &Box<Xcm<()>>) -> Weight {
		XcmGeneric::<Runtime>::xcm_relayed_from()
	}
}
