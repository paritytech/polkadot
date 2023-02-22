// Copyright 2022 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Adapters to work with `frame_support::traits::tokens::fungibles` through XCM.

use frame_support::traits::Get;
use sp_std::{borrow::Borrow, marker::PhantomData, prelude::*, result};
use xcm::latest::prelude::*;
use xcm_executor::traits::{Convert, Error as MatchError, MatchesFungibles, MatchesNonFungibles};

/// Converter struct implementing `AssetIdConversion` converting a numeric asset ID (must be `TryFrom/TryInto<u128>`) into
/// a `GeneralIndex` junction, prefixed by some `MultiLocation` value. The `MultiLocation` value will typically be a
/// `PalletInstance` junction.
pub struct AsPrefixedGeneralIndex<Prefix, AssetId, ConvertAssetId>(
	PhantomData<(Prefix, AssetId, ConvertAssetId)>,
);
impl<Prefix: Get<MultiLocation>, AssetId: Clone, ConvertAssetId: Convert<u128, AssetId>>
	Convert<MultiLocation, AssetId> for AsPrefixedGeneralIndex<Prefix, AssetId, ConvertAssetId>
{
	fn convert_ref(id: impl Borrow<MultiLocation>) -> result::Result<AssetId, ()> {
		let prefix = Prefix::get();
		let id = id.borrow();
		if prefix.parent_count() != id.parent_count() ||
			prefix
				.interior()
				.iter()
				.enumerate()
				.any(|(index, junction)| id.interior().at(index) != Some(junction))
		{
			return Err(())
		}
		match id.interior().at(prefix.interior().len()) {
			Some(Junction::GeneralIndex(id)) => ConvertAssetId::convert_ref(id),
			_ => Err(()),
		}
	}
	fn reverse_ref(what: impl Borrow<AssetId>) -> result::Result<MultiLocation, ()> {
		let mut location = Prefix::get();
		let id = ConvertAssetId::reverse_ref(what)?;
		location.push_interior(Junction::GeneralIndex(id)).map_err(|_| ())?;
		Ok(location)
	}
}

pub struct ConvertedConcreteId<AssetId, Balance, ConvertAssetId, ConvertOther>(
	PhantomData<(AssetId, Balance, ConvertAssetId, ConvertOther)>,
);
impl<
		AssetId: Clone,
		Balance: Clone,
		ConvertAssetId: Convert<MultiLocation, AssetId>,
		ConvertBalance: Convert<u128, Balance>,
	> MatchesFungibles<AssetId, Balance>
	for ConvertedConcreteId<AssetId, Balance, ConvertAssetId, ConvertBalance>
{
	fn matches_fungibles(a: &MultiAsset) -> result::Result<(AssetId, Balance), MatchError> {
		let (amount, id) = match (&a.fun, &a.id) {
			(Fungible(ref amount), Concrete(ref id)) => (amount, id),
			_ => return Err(MatchError::AssetNotFound),
		};
		let what =
			ConvertAssetId::convert_ref(id).map_err(|_| MatchError::AssetIdConversionFailed)?;
		let amount = ConvertBalance::convert_ref(amount)
			.map_err(|_| MatchError::AmountToBalanceConversionFailed)?;
		Ok((what, amount))
	}
}

impl<
		AssetId: Clone,
		Balance: Clone,
		ConvertAssetId: Convert<MultiLocation, AssetId>,
		ConvertBalance: Convert<u128, Balance>,
	> Convert<(AssetId, Balance), MultiAsset>
	for ConvertedConcreteId<AssetId, Balance, ConvertAssetId, ConvertBalance>
{
	fn convert_ref(value: impl Borrow<(AssetId, Balance)>) -> Result<MultiAsset, ()> {
		let (asset_id, balance) = value.borrow();
		match ConvertAssetId::reverse_ref(asset_id) {
			Ok(asset_id_as_multilocation) => match ConvertBalance::reverse_ref(balance) {
				Ok(amount) => Ok((asset_id_as_multilocation, amount).into()),
				Err(_) => Err(()),
			},
			Err(_) => Err(()),
		}
	}

	fn reverse_ref(value: impl Borrow<MultiAsset>) -> Result<(AssetId, Balance), ()> {
		<Self as MatchesFungibles<AssetId, Balance>>::matches_fungibles(value.borrow())
			.map_err(|_| ())
	}
}

impl<
		ClassId: Clone,
		InstanceId: Clone,
		ConvertClassId: Convert<MultiLocation, ClassId>,
		ConvertInstanceId: Convert<AssetInstance, InstanceId>,
	> MatchesNonFungibles<ClassId, InstanceId>
	for ConvertedConcreteId<ClassId, InstanceId, ConvertClassId, ConvertInstanceId>
{
	fn matches_nonfungibles(a: &MultiAsset) -> result::Result<(ClassId, InstanceId), MatchError> {
		let (instance, class) = match (&a.fun, &a.id) {
			(NonFungible(ref instance), Concrete(ref class)) => (instance, class),
			_ => return Err(MatchError::AssetNotFound),
		};
		let what =
			ConvertClassId::convert_ref(class).map_err(|_| MatchError::AssetIdConversionFailed)?;
		let instance = ConvertInstanceId::convert_ref(instance)
			.map_err(|_| MatchError::InstanceConversionFailed)?;
		Ok((what, instance))
	}
}

pub struct ConvertedAbstractId<AssetId, Balance, ConvertAssetId, ConvertOther>(
	PhantomData<(AssetId, Balance, ConvertAssetId, ConvertOther)>,
);
impl<
		AssetId: Clone,
		Balance: Clone,
		ConvertAssetId: Convert<[u8; 32], AssetId>,
		ConvertBalance: Convert<u128, Balance>,
	> MatchesFungibles<AssetId, Balance>
	for ConvertedAbstractId<AssetId, Balance, ConvertAssetId, ConvertBalance>
{
	fn matches_fungibles(a: &MultiAsset) -> result::Result<(AssetId, Balance), MatchError> {
		let (amount, id) = match (&a.fun, &a.id) {
			(Fungible(ref amount), Abstract(ref id)) => (amount, id),
			_ => return Err(MatchError::AssetNotFound),
		};
		let what =
			ConvertAssetId::convert_ref(id).map_err(|_| MatchError::AssetIdConversionFailed)?;
		let amount = ConvertBalance::convert_ref(amount)
			.map_err(|_| MatchError::AmountToBalanceConversionFailed)?;
		Ok((what, amount))
	}
}
impl<
		ClassId: Clone,
		InstanceId: Clone,
		ConvertClassId: Convert<[u8; 32], ClassId>,
		ConvertInstanceId: Convert<AssetInstance, InstanceId>,
	> MatchesNonFungibles<ClassId, InstanceId>
	for ConvertedAbstractId<ClassId, InstanceId, ConvertClassId, ConvertInstanceId>
{
	fn matches_nonfungibles(a: &MultiAsset) -> result::Result<(ClassId, InstanceId), MatchError> {
		let (instance, class) = match (&a.fun, &a.id) {
			(NonFungible(ref instance), Abstract(ref class)) => (instance, class),
			_ => return Err(MatchError::AssetNotFound),
		};
		let what =
			ConvertClassId::convert_ref(class).map_err(|_| MatchError::AssetIdConversionFailed)?;
		let instance = ConvertInstanceId::convert_ref(instance)
			.map_err(|_| MatchError::InstanceConversionFailed)?;
		Ok((what, instance))
	}
}

#[deprecated = "Use `ConvertedConcreteId` instead"]
pub type ConvertedConcreteAssetId<A, B, C, O> = ConvertedConcreteId<A, B, C, O>;
#[deprecated = "Use `ConvertedAbstractId` instead"]
pub type ConvertedAbstractAssetId<A, B, C, O> = ConvertedAbstractId<A, B, C, O>;

#[cfg(test)]
mod tests {
	use super::*;

	use xcm_executor::traits::{Identity, JustTry};

	#[test]
	fn converted_concrete_id_fungible_multi_asset_conversion_roundtrip_works() {
		type Converter = ConvertedConcreteId<MultiLocation, u64, Identity, JustTry>;

		let location = MultiLocation::new(0, X1(GlobalConsensus(ByGenesis([0; 32]))));
		let amount = 123456_u64;
		let expected_multi_asset = MultiAsset {
			id: Concrete(MultiLocation::new(0, X1(GlobalConsensus(ByGenesis([0; 32]))))),
			fun: Fungible(123456_u128),
		};

		assert_eq!(
			Converter::matches_fungibles(&expected_multi_asset).map_err(|_| ()),
			Ok((location, amount))
		);

		assert_eq!(Converter::reverse_ref(&expected_multi_asset), Ok((location, amount)));
		assert_eq!(Converter::reverse(expected_multi_asset.clone()), Ok((location, amount)));
		assert_eq!(Converter::convert_ref((location, amount)), Ok(expected_multi_asset.clone()));
		assert_eq!(Converter::convert((location, amount)), Ok(expected_multi_asset));
	}
}
