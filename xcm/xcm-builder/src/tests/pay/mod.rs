mod pay;
mod salary;

/// Type representing both a location and an asset that is held at that location.
/// The id of the held asset is relative to the location where it is being held.
#[derive(Encode, Decode, Clone, Copy, PartialEq, Eq)]
pub struct AssetKind {
	destination: MultiLocation,
	asset_id: AssetId,
}

pub struct LocatableAssetKindConverter;
impl sp_runtime::traits::Convert<AssetKind, LocatableAssetId> for LocatableAssetKindConverter {
	fn convert(value: AssetKind) -> LocatableAssetId {
		LocatableAssetId { asset_id: value.asset_id, location: value.destination }
	}
}

/// Simple converter to turn a u64 into a MultiLocation using the AccountIndex64 junction and no parents
pub struct AliasesIntoAccountIndex64;
impl<'a> sp_runtime::traits::Convert<&'a AccountId, MultiLocation> for AliasesIntoAccountIndex64 {
	fn convert(who: &AccountId) -> MultiLocation {
		AccountIndex64 { network: None, index: who.clone().into() }.into()
	}
}
