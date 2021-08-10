mod xcm_balances;
mod xcm_generic;

use frame_support::weights::Weight;
use xcm_balances::WeightInfo as XcmBalancesWeight;

use crate::Runtime;
use sp_std::prelude::*;
use xcm::{latest::prelude::*, DoubleEncoded};
use xcm_generic::WeightInfo as XcmGeneric;

pub enum AssetTypes {
	Balances,
	Unknown,
}

impl From<&MultiAsset> for AssetTypes {
	fn from(asset: &MultiAsset) -> Self {
		match asset {
			MultiAsset { id: Concrete(MultiLocation { parents: 0, interior: Here }), .. } =>
				AssetTypes::Balances,
			_ => AssetTypes::Unknown,
		}
	}
}

// TODO from Shawn: I dont like this
trait WeighMultiAssets {
	fn weigh_multi_assets(&self, balances_weight: Weight) -> Weight;
}

// TODO wild case
impl WeighMultiAssets for MultiAssetFilter {
	fn weigh_multi_assets(&self, balances_weight: Weight) -> Weight {
		match self {
			Self::Definite(assets) => assets
				.inner()
				.into_iter()
				.map(|m| <AssetTypes as From<&MultiAsset>>::from(m))
				.map(|t| match t {
					AssetTypes::Balances => balances_weight,
					AssetTypes::Unknown => Weight::MAX,
				})
				.fold(0, |acc, x| acc.saturating_add(x)),
			_ => Weight::MAX,
		}
	}
}

impl WeighMultiAssets for MultiAssets {
	fn weigh_multi_assets(&self, balances_weight: Weight) -> Weight {
		self.inner()
			.into_iter()
			.map(|m| <AssetTypes as From<&MultiAsset>>::from(m))
			.map(|t| match t {
				AssetTypes::Balances => balances_weight,
				AssetTypes::Unknown => Weight::MAX,
			})
			.fold(0, |acc, x| acc.saturating_add(x))
	}
}

pub struct WestendXcmWeight;
impl XcmWeightInfo<()> for WestendXcmWeight {
	fn order_noop() -> Weight {
		XcmGeneric::<Runtime>::order_noop()
	}
	fn order_deposit_asset(
		assets: &MultiAssetFilter,
		_max_assets: &u32, // TODO use max assets?
		_dest: &MultiLocation,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::order_deposit_asset())
	}
	fn order_deposit_reserve_asset(
		assets: &MultiAssetFilter,
		_max_assets: &u32, // TODO use max assets?
		_dest: &MultiLocation,
		_effects: &Vec<Order<()>>,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::order_deposit_reserve_asset())
	}
	fn order_exchange_asset(_give: &MultiAssetFilter, _receive: &MultiAssets) -> Weight {
		Weight::MAX // todo fix
	}
	fn order_initiate_reserve_withdraw(
		assets: &MultiAssetFilter,
		_reserve: &MultiLocation,
		_effects: &Vec<Order<()>>,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::order_initiate_reserve_withdraw())
	}
	fn order_initiate_teleport(
		assets: &MultiAssetFilter,
		_dest: &MultiLocation,
		_effects: &Vec<Order<()>>,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::order_initiate_teleport())
	}
	fn order_query_holding(
		_query_id: &u64,
		_dest: &MultiLocation,
		_assets: &MultiAssetFilter,
	) -> Weight {
		XcmGeneric::<Runtime>::order_query_holding()
	}
	fn order_buy_execution(
		_fees: &MultiAsset,
		_weight: &u64,
		_debt: &u64,
		_halt_on_error: &bool,
		_order: &Vec<Order<()>>,
		_xcm: &Vec<Xcm<()>>,
	) -> Weight {
		XcmGeneric::<Runtime>::order_buy_execution()
	}
	fn xcm_withdraw_asset(assets: &MultiAssets, _effects: &Vec<Order<()>>) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::xcm_withdraw_asset())
	}
	fn xcm_reserve_asset_deposited(assets: &MultiAssets, _effects: &Vec<Order<()>>) -> Weight {
		assets.weigh_multi_assets(XcmGeneric::<Runtime>::xcm_reserve_asset_deposited())
	}
	// TODO none of these need effects
	fn xcm_receive_teleported_asset(assets: &MultiAssets, _effects: &Vec<Order<()>>) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::xcm_receive_teleported_asset())
	}
	fn xcm_query_response(_query_id: &u64, _response: &Response) -> Weight {
		XcmGeneric::<Runtime>::xcm_query_response()
	}
	fn xcm_transfer_asset(assets: &MultiAssets, _dest: &MultiLocation) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::xcm_transfer_asset())
	}
	fn xcm_transfer_reserve_asset(
		assets: &MultiAssets,
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
	fn xcm_hrmp_new_channel_open_request(
		_sender: &u32,
		_max_message_size: &u32,
		_max_capacity: &u32,
	) -> Weight {
		XcmGeneric::<Runtime>::xcm_hrmp_new_channel_open_request()
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
