mod pallet_balances;
mod xcm_generic;

use pallet_balances::WeightInfo as PalletBalances;
use xcm_generic::WeightInfo as XcmGeneric;

pub enum AssetTypes {
	Balances,
}

impl From<MultiAsset> for AssetTypes {
	fn from(asset: MultiAsset) -> Self {
		match self {
			_ => Balances,
		}
	}
}

pub struct WestendXcmWeight;
impl XcmWeightInfo for WestendXcmWeight {
	fn send_xcm() -> Weight {
		XcmGeneric::send_xcm()
	}
	fn order_null() -> Weight {
		XcmGeneric::order_null()
	}
	fn order_deposit_asset(assets: Vec<MultiAsset>, dest: MultiLocation) -> Weight {
		assets.iter().map(|asset| asset.into()).map(|asset_type| match asset_type {
			AssetType::Balances => PalletBalances::order_deposit_asset(),
		})
	}
	fn order_deposit_reserved_asset(
		assets: Vec<MultiAsset>,
		dest: MultiLocation,
		effects: Vec<Order<Call>>,
	) -> Weight {
		assets.iter().map(|asset| asset.into()).map(|asset_type| match asset_type {
			AssetType::Balances => PalletBalances::order_deposit_reserved_asset(),
		})
	}
	fn order_exchange_asset(give: Vec<MultiAsset>, receive: Vec<MultiAsset>) -> Weight {
		assets.iter().map(|asset| asset.into()).map(|asset_type| match asset_type {
			AssetType::Balances => PalletBalances::order_exchange_asset(),
		})
	}
	fn order_initiate_reserve_withdraw(
		assets: Vec<MultiAsset>,
		reserve: MultiLocation,
		effects: Vec<Order<Call>>,
	) -> Weight {
		assets.iter().map(|asset| asset.into()).map(|asset_type| match asset_type {
			AssetType::Balances => PalletBalances::order_initiate_reserve_withdraw(),
		})
	}
	fn order_initiate_teleport(
		assets: Vec<MultiAsset>,
		dest: MultiLocation,
		effects: Vec<Order<()>>,
	) -> Weight {
		assets.iter().map(|asset| asset.into()).map(|asset_type| match asset_type {
			AssetType::Balances => PalletBalances::order_initiate_teleport(),
		})
	}
	fn order_query_holding(query_id: u64, dest: MultiLocation, assets: Vec<MultiAsset>) -> Weight {
		assets.iter().map(|asset| asset.into()).map(|asset_type| match asset_type {
			AssetType::Balances => PalletBalances::order_query_holding(),
		})
	}
	fn order_buy_execution(
		fees: MultiAsset,
		weight: u64,
		debt: u64,
		halt_on_error: bool,
		xcm: Vec<Xcm<Call>>,
	) -> Weight {
		XcmGeneric::order_buy_execution
	}
	fn xcm_withdraw_asset(assets: Vec<MultiAsset>, effects: Vec<Order<Call>>) -> Weight {
		assets.iter().map(|asset| asset.into()).map(|asset_type| match asset_type {
			AssetType::Balances => PalletBalances::xcm_withdraw_asset(),
		})
	}
	fn xcm_reserve_asset_deposit(assets: Vec<MultiAsset>, effects: Vec<Order<Call>>) -> Weight {
		assets.iter().map(|asset| asset.into()).map(|asset_type| match asset_type {
			AssetType::Balances => PalletBalances::xcm_reserve_asset_deposit(),
		})
	}
	// TODO none of these need effects
	fn xcm_teleport_asset(assets: Vec<MultiAsset>, effects: Vec<Order<Call>>) -> Weight {
		assets.iter().map(|asset| asset.into()).map(|asset_type| match asset_type {
			AssetType::Balances => PalletBalances::xcm_teleport_asset(),
		})
	}
	fn xcm_query_response(query_id: u64, response: Response) -> Weight {
		XcmGeneric::xcm_query_response()
	}
	fn xcm_transfer_asset(assets: Vec<MultiAsset>, dest: MultiLocation) -> Weight {
		assets.iter().map(|asset| asset.into()).map(|asset_type| match asset_type {
			AssetType::Balances => PalletBalances::xcm_transfer_asset(),
		})
	}
	fn xcm_transfer_reserved_asset(assets: Vec<MultiAsset>, dest: MultiLocation) -> Weight {
		assets.iter().map(|asset| asset.into()).map(|asset_type| match asset_type {
			AssetType::Balances => PalletBalances::xcm_transfer_reserved_asset(),
		})
	}
	// TODO: Maybe remove call
	fn xcm_transact(
		origin_type: OriginKind,
		require_weight_at_most: u64,
		call: DoubleEncoded<Call>,
	) -> Weight {
		XcmGeneric::xcm_transact()
	}
	fn xcm_hrmp_channel_open_request(
		sender: u32,
		max_message_size: u32,
		max_capacity: u32,
	) -> Weight {
		XcmGeneric::xcm_hrmp_channel_open_request()
	}
	fn xcm_hrmp_channel_accepted(recipient: u32) -> Weight {
		XcmGeneric::xcm_hrmp_channel_accepted()
	}
	fn xcm_hrmp_channel_closing(initiator: u32, sender: u32, recipient: u32) -> Weight {
		XcmGeneric::xcm_hrmp_channel_accepted()
	}
	fn xcm_relayed_from(who: MultiLocation, message: alloc::boxed::Box<Xcm<Call>>) -> Weight {
		XcmGeneric::xcm_relayed_from()
	}
}
