// Copyright (C) Parity Technologies (UK) Ltd.
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

use frame_support::traits::{Contains, Get};
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
			_ => return Err(MatchError::AssetNotHandled),
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
		ConvertClassId: Convert<MultiLocation, ClassId>,
		ConvertInstanceId: Convert<AssetInstance, InstanceId>,
	> MatchesNonFungibles<ClassId, InstanceId>
	for ConvertedConcreteId<ClassId, InstanceId, ConvertClassId, ConvertInstanceId>
{
	fn matches_nonfungibles(a: &MultiAsset) -> result::Result<(ClassId, InstanceId), MatchError> {
		let (instance, class) = match (&a.fun, &a.id) {
			(NonFungible(ref instance), Concrete(ref class)) => (instance, class),
			_ => return Err(MatchError::AssetNotHandled),
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
			_ => return Err(MatchError::AssetNotHandled),
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
			_ => return Err(MatchError::AssetNotHandled),
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

pub struct MatchedConvertedConcreteId<AssetId, Balance, MatchAssetId, ConvertAssetId, ConvertOther>(
	PhantomData<(AssetId, Balance, MatchAssetId, ConvertAssetId, ConvertOther)>,
);
impl<
		AssetId: Clone,
		Balance: Clone,
		MatchAssetId: Contains<MultiLocation>,
		ConvertAssetId: Convert<MultiLocation, AssetId>,
		ConvertBalance: Convert<u128, Balance>,
	> MatchesFungibles<AssetId, Balance>
	for MatchedConvertedConcreteId<AssetId, Balance, MatchAssetId, ConvertAssetId, ConvertBalance>
{
	fn matches_fungibles(a: &MultiAsset) -> result::Result<(AssetId, Balance), MatchError> {
		let (amount, id) = match (&a.fun, &a.id) {
			(Fungible(ref amount), Concrete(ref id)) if MatchAssetId::contains(id) => (amount, id),
			_ => return Err(MatchError::AssetNotHandled),
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
		MatchClassId: Contains<MultiLocation>,
		ConvertClassId: Convert<MultiLocation, ClassId>,
		ConvertInstanceId: Convert<AssetInstance, InstanceId>,
	> MatchesNonFungibles<ClassId, InstanceId>
	for MatchedConvertedConcreteId<ClassId, InstanceId, MatchClassId, ConvertClassId, ConvertInstanceId>
{
	fn matches_nonfungibles(a: &MultiAsset) -> result::Result<(ClassId, InstanceId), MatchError> {
		let (instance, class) = match (&a.fun, &a.id) {
			(NonFungible(ref instance), Concrete(ref class)) if MatchClassId::contains(class) =>
				(instance, class),
			_ => return Err(MatchError::AssetNotHandled),
		};
		let what =
			ConvertClassId::convert_ref(class).map_err(|_| MatchError::AssetIdConversionFailed)?;
		let instance = ConvertInstanceId::convert_ref(instance)
			.map_err(|_| MatchError::InstanceConversionFailed)?;
		Ok((what, instance))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use xcm_executor::traits::JustTry;

	struct OnlyParentZero;
	impl Contains<MultiLocation> for OnlyParentZero {
		fn contains(a: &MultiLocation) -> bool {
			match a {
				MultiLocation { parents: 0, .. } => true,
				_ => false,
			}
		}
	}

	#[test]
	fn matched_converted_concrete_id_for_fungibles_works() {
		type AssetIdForTrustBackedAssets = u32;
		type Balance = u128;
		frame_support::parameter_types! {
			pub TrustBackedAssetsPalletLocation: MultiLocation = PalletInstance(50).into();
		}

		// ConvertedConcreteId cfg
		type Converter = MatchedConvertedConcreteId<
			AssetIdForTrustBackedAssets,
			Balance,
			OnlyParentZero,
			AsPrefixedGeneralIndex<
				TrustBackedAssetsPalletLocation,
				AssetIdForTrustBackedAssets,
				JustTry,
			>,
			JustTry,
		>;
		assert_eq!(
			TrustBackedAssetsPalletLocation::get(),
			MultiLocation { parents: 0, interior: X1(PalletInstance(50)) }
		);

		// err - does not match
		assert_eq!(
			Converter::matches_fungibles(&MultiAsset {
				id: Concrete(MultiLocation::new(1, X2(PalletInstance(50), GeneralIndex(1)))),
				fun: Fungible(12345),
			}),
			Err(MatchError::AssetNotHandled)
		);

		// err - matches, but convert fails
		assert_eq!(
			Converter::matches_fungibles(&MultiAsset {
				id: Concrete(MultiLocation::new(
					0,
					X2(PalletInstance(50), GeneralKey { length: 1, data: [1; 32] })
				)),
				fun: Fungible(12345),
			}),
			Err(MatchError::AssetIdConversionFailed)
		);

		// err - matches, but NonFungible
		assert_eq!(
			Converter::matches_fungibles(&MultiAsset {
				id: Concrete(MultiLocation::new(0, X2(PalletInstance(50), GeneralIndex(1)))),
				fun: NonFungible(Index(54321)),
			}),
			Err(MatchError::AssetNotHandled)
		);

		// ok
		assert_eq!(
			Converter::matches_fungibles(&MultiAsset {
				id: Concrete(MultiLocation::new(0, X2(PalletInstance(50), GeneralIndex(1)))),
				fun: Fungible(12345),
			}),
			Ok((1, 12345))
		);
	}

	#[test]
	fn matched_converted_concrete_id_for_nonfungibles_works() {
		type ClassId = u32;
		type ClassInstanceId = u64;
		frame_support::parameter_types! {
			pub TrustBackedAssetsPalletLocation: MultiLocation = PalletInstance(50).into();
		}

		// ConvertedConcreteId cfg
		struct ClassInstanceIdConverter;
		impl Convert<AssetInstance, ClassInstanceId> for ClassInstanceIdConverter {
			fn convert_ref(value: impl Borrow<AssetInstance>) -> Result<ClassInstanceId, ()> {
				value.borrow().clone().try_into().map_err(|_| ())
			}

			fn reverse_ref(value: impl Borrow<ClassInstanceId>) -> Result<AssetInstance, ()> {
				Ok(AssetInstance::from(value.borrow().clone()))
			}
		}

		type Converter = MatchedConvertedConcreteId<
			ClassId,
			ClassInstanceId,
			OnlyParentZero,
			AsPrefixedGeneralIndex<TrustBackedAssetsPalletLocation, ClassId, JustTry>,
			ClassInstanceIdConverter,
		>;
		assert_eq!(
			TrustBackedAssetsPalletLocation::get(),
			MultiLocation { parents: 0, interior: X1(PalletInstance(50)) }
		);

		// err - does not match
		assert_eq!(
			Converter::matches_nonfungibles(&MultiAsset {
				id: Concrete(MultiLocation::new(1, X2(PalletInstance(50), GeneralIndex(1)))),
				fun: NonFungible(Index(54321)),
			}),
			Err(MatchError::AssetNotHandled)
		);

		// err - matches, but convert fails
		assert_eq!(
			Converter::matches_nonfungibles(&MultiAsset {
				id: Concrete(MultiLocation::new(
					0,
					X2(PalletInstance(50), GeneralKey { length: 1, data: [1; 32] })
				)),
				fun: NonFungible(Index(54321)),
			}),
			Err(MatchError::AssetIdConversionFailed)
		);

		// err - matches, but Fungible vs NonFungible
		assert_eq!(
			Converter::matches_nonfungibles(&MultiAsset {
				id: Concrete(MultiLocation::new(0, X2(PalletInstance(50), GeneralIndex(1)))),
				fun: Fungible(12345),
			}),
			Err(MatchError::AssetNotHandled)
		);

		// ok
		assert_eq!(
			Converter::matches_nonfungibles(&MultiAsset {
				id: Concrete(MultiLocation::new(0, X2(PalletInstance(50), GeneralIndex(1)))),
				fun: NonFungible(Index(54321)),
			}),
			Ok((1, 54321))
		);
	}
}
