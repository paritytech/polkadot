mod pallet_xcm_benchmarks_fungible;
mod pallet_xcm_benchmarks_generic;

use crate::Runtime;
use frame_support::weights::Weight;
use sp_std::prelude::*;
use xcm::{
	latest::{prelude::*, QueryResponseInfo},
	DoubleEncoded,
};

use pallet_xcm_benchmarks_fungible::WeightInfo as XcmBalancesWeight;
use pallet_xcm_benchmarks_generic::WeightInfo as XcmGeneric;

/// Types of asset supported by the westend runtime.
pub enum AssetTypes {
	/// An asset backed by `pallet-balances`.
	Balances,
	/// Unknown asset.
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

trait WeighMultiAssets {
	fn weigh_multi_assets(&self, balances_weight: Weight) -> Weight;
}

// Westend only knows about one asset, the balances pallet.
const MAX_ASSETS: u32 = 1;

impl WeighMultiAssets for MultiAssetFilter {
	fn weigh_multi_assets(&self, balances_weight: Weight) -> Weight {
		match self {
			Self::Definite(assets) => assets
				.inner()
				.into_iter()
				.map(From::from)
				.map(|t| match t {
					AssetTypes::Balances => balances_weight,
					AssetTypes::Unknown => Weight::MAX,
				})
				.fold(0, |acc, x| acc.saturating_add(x)),
			Self::Wild(_) => (MAX_ASSETS as Weight).saturating_mul(balances_weight),
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

pub struct WestendXcmWeight<Call>(core::marker::PhantomData<Call>);
impl<Call> XcmWeightInfo<Call> for WestendXcmWeight<Call> {
	fn withdraw_asset(assets: &MultiAssets) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::withdraw_asset())
	}
	fn reserve_asset_deposited(_assets: &MultiAssets) -> Weight {
		// Westend does not support reserve asset deposits.
		Weight::MAX
	}
	fn receive_teleported_asset(assets: &MultiAssets) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::receive_teleported_asset())
	}
	fn query_response(
		_query_id: &u64,
		_response: &Response,
		_max_weight: &u64,
		_querier: &Option<MultiLocation>,
	) -> Weight {
		XcmGeneric::<Runtime>::query_response()
	}
	fn transfer_asset(assets: &MultiAssets, _dest: &MultiLocation) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::transfer_asset())
	}
	fn transfer_reserve_asset(
		assets: &MultiAssets,
		_dest: &MultiLocation,
		_xcm: &Xcm<()>,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::transfer_reserve_asset())
	}
	fn transact(
		_origin_type: &OriginKind,
		_require_weight_at_most: &u64,
		_call: &DoubleEncoded<Call>,
	) -> Weight {
		XcmGeneric::<Runtime>::transact()
	}
	fn hrmp_new_channel_open_request(
		_sender: &u32,
		_max_message_size: &u32,
		_max_capacity: &u32,
	) -> Weight {
		// XCM Executor does not currently support HRMP channel operations
		Weight::MAX
	}
	fn hrmp_channel_accepted(_recipient: &u32) -> Weight {
		// XCM Executor does not currently support HRMP channel operations
		Weight::MAX
	}
	fn hrmp_channel_closing(_initiator: &u32, _sender: &u32, _recipient: &u32) -> Weight {
		// XCM Executor does not currently support HRMP channel operations
		Weight::MAX
	}
	fn clear_origin() -> Weight {
		XcmGeneric::<Runtime>::clear_origin()
	}
	fn descend_origin(_who: &InteriorMultiLocation) -> Weight {
		XcmGeneric::<Runtime>::descend_origin()
	}
	fn report_error(_query_repsonse_info: &QueryResponseInfo) -> Weight {
		XcmGeneric::<Runtime>::report_error()
	}

	fn deposit_asset(assets: &MultiAssetFilter, _dest: &MultiLocation) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::deposit_asset())
	}
	fn deposit_reserve_asset(
		assets: &MultiAssetFilter,
		_dest: &MultiLocation,
		_xcm: &Xcm<()>,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::deposit_reserve_asset())
	}
	fn exchange_asset(_give: &MultiAssetFilter, _receive: &MultiAssets, _maximal: &bool) -> Weight {
		Weight::MAX // todo fix
	}
	fn initiate_reserve_withdraw(
		assets: &MultiAssetFilter,
		_reserve: &MultiLocation,
		_xcm: &Xcm<()>,
	) -> Weight {
		assets.weigh_multi_assets(XcmGeneric::<Runtime>::initiate_reserve_withdraw())
	}
	fn initiate_teleport(
		assets: &MultiAssetFilter,
		_dest: &MultiLocation,
		_xcm: &Xcm<()>,
	) -> Weight {
		assets.weigh_multi_assets(XcmBalancesWeight::<Runtime>::initiate_teleport())
	}
	fn report_holding(_response_info: &QueryResponseInfo, _assets: &MultiAssetFilter) -> Weight {
		XcmGeneric::<Runtime>::report_holding()
	}
	fn buy_execution(_fees: &MultiAsset, _weight_limit: &WeightLimit) -> Weight {
		XcmGeneric::<Runtime>::buy_execution()
	}
	fn refund_surplus() -> Weight {
		XcmGeneric::<Runtime>::refund_surplus()
	}
	fn set_error_handler(_xcm: &Xcm<Call>) -> Weight {
		XcmGeneric::<Runtime>::set_error_handler()
	}
	fn set_appendix(_xcm: &Xcm<Call>) -> Weight {
		XcmGeneric::<Runtime>::set_appendix()
	}
	fn clear_error() -> Weight {
		XcmGeneric::<Runtime>::clear_error()
	}
	fn claim_asset(_assets: &MultiAssets, _ticket: &MultiLocation) -> Weight {
		XcmGeneric::<Runtime>::claim_asset()
	}
	fn trap(_code: &u64) -> Weight {
		XcmGeneric::<Runtime>::trap()
	}
	fn subscribe_version(_query_id: &QueryId, _max_response_weight: &u64) -> Weight {
		XcmGeneric::<Runtime>::subscribe_version()
	}
	fn unsubscribe_version() -> Weight {
		XcmGeneric::<Runtime>::unsubscribe_version()
	}
	fn burn_asset(assets: &MultiAssets) -> Weight {
		assets.weigh_multi_assets(XcmGeneric::<Runtime>::burn_asset())
	}
	fn expect_asset(assets: &MultiAssets) -> Weight {
		assets.weigh_multi_assets(XcmGeneric::<Runtime>::expect_asset())
	}
	fn expect_origin(_origin: &Option<MultiLocation>) -> Weight {
		XcmGeneric::<Runtime>::expect_origin()
	}
	fn expect_error(_error: &Option<(u32, XcmError)>) -> Weight {
		XcmGeneric::<Runtime>::expect_error()
	}
	fn query_pallet(_module_name: &Vec<u8>, _response_info: &QueryResponseInfo) -> Weight {
		XcmGeneric::<Runtime>::query_pallet()
	}
	fn expect_pallet(
		_index: &u32,
		_name: &Vec<u8>,
		_module_name: &Vec<u8>,
		_crate_major: &u32,
		_min_crate_minor: &u32,
	) -> Weight {
		XcmGeneric::<Runtime>::expect_pallet()
	}
	fn report_transact_status(_response_info: &QueryResponseInfo) -> Weight {
		XcmGeneric::<Runtime>::report_transact_status()
	}
	fn clear_transact_status() -> Weight {
		XcmGeneric::<Runtime>::clear_transact_status()
	}
	fn universal_origin(_: &Junction) -> Weight {
		Weight::MAX // todo fix
	}
	fn export_message(_: &NetworkId, _: &Junctions, _: &Xcm<()>) -> Weight {
		Weight::MAX // todo fix
	}
	fn lock_asset(_: &MultiAsset, _: &MultiLocation) -> Weight {
		Weight::MAX // todo fix
	}
	fn unlock_asset(_: &MultiAsset, _: &MultiLocation) -> Weight {
		Weight::MAX // todo fix
	}
	fn note_unlockable(_: &MultiAsset, _: &MultiLocation) -> Weight {
		Weight::MAX // todo fix
	}
	fn request_unlock(_: &MultiAsset, _: &MultiLocation) -> Weight {
		Weight::MAX // todo fix
	}
	fn set_fees_mode(_: &bool) -> Weight {
		Weight::MAX // todo fix
	}
	fn set_topic(_topic: &[u8; 32]) -> Weight {
		XcmGeneric::<Runtime>::set_topic()
	}
	fn clear_topic() -> Weight {
		XcmGeneric::<Runtime>::clear_topic()
	}
}
