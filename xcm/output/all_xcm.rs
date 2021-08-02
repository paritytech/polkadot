use crate::{XcmAssetsWeightInfo, XcmBalancesWeightInfo};

pub enum AssetTypes {
	Assets,
	Balances,
}

impl GetWeightInfoSingle for AssetTypes {
	fn xcm_withdraw_asset(&self) -> Weight {
		10
	}
}

impl From<MultiAsset> for AssetTypes {
	fn from(_: MultiAsset)
}

pub struct FinalWeightWhatever;
impl XcmWeightInfo for FinalWeightWhatever {
	fn xcm_withdraw_asset(assets: Vec<MultiAsset>, effects: Vec<Order<()>) -> Weight {
		assets.into_iter().map(|a| a.into::<AssetType>()).map(|t|
			match t {
				AsstType::Assets => XcmAssetsWeightInfo::xcm_withdraw_asset(),
				AssetType::Balances => XcmBalancesWeightInfo::xcm_withdraw_asset()
			}
			.sum()
	}

}
