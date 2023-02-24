// Copyright Parity Technologies (UK) Ltd.
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

//! Runtime API definition for fungibles.
//!
//! (Initial version: https://github.com/paritytech/cumulus/pull/2180#issuecomment-1442952274)

use crate::ConvertedConcreteId;
use frame_support::RuntimeDebug;
use parity_scale_codec::{Codec, Decode, Encode};
use sp_std::{borrow::Borrow, vec::Vec};
use xcm::latest::{MultiAsset, MultiLocation};
use xcm_executor::traits::{Convert, MatchesFungibles};

/// The possible errors that can happen querying the storage of assets.
#[derive(Eq, PartialEq, Encode, Decode, RuntimeDebug)]
pub enum FungiblesAccessError {
	/// `MultiLocation` to `AssetId`/`ClassId` conversion failed.
	AssetIdConversionFailed,
	/// `u128` amount to currency `Balance` conversion failed.
	AmountToBalanceConversionFailed,
}

sp_api::decl_runtime_apis! {
	/// The API for querying account's balances from runtime.
	pub trait FungiblesApi<AccountId>
	where
		AccountId: Codec,
	{
		/// Returns the list of all [`MultiAsset`] that an `AccountId` has.
		fn query_account_balances(account: AccountId) -> Result<Vec<MultiAsset>, FungiblesAccessError>;
	}
}

/// Converting any [`(AssetId, Balance)`] to [`MultiAsset`]
// TODO: could be replaced by [`Convert<(AssetId, Balance), MultiAsset>`] - see issue https://github.com/paritytech/polkadot/pull/6760
pub trait MultiAssetConverter<AssetId, Balance, ConvertAssetId, ConvertBalance>:
	MatchesFungibles<AssetId, Balance>
where
	AssetId: Clone,
	Balance: Clone,
	ConvertAssetId: Convert<MultiLocation, AssetId>,
	ConvertBalance: Convert<u128, Balance>,
{
	fn convert_ref(
		value: impl Borrow<(AssetId, Balance)>,
	) -> Result<MultiAsset, FungiblesAccessError>;
}

impl<
		AssetId: Clone,
		Balance: Clone,
		ConvertAssetId: Convert<MultiLocation, AssetId>,
		ConvertBalance: Convert<u128, Balance>,
	> MultiAssetConverter<AssetId, Balance, ConvertAssetId, ConvertBalance>
	for ConvertedConcreteId<AssetId, Balance, ConvertAssetId, ConvertBalance>
{
	fn convert_ref(
		value: impl Borrow<(AssetId, Balance)>,
	) -> Result<MultiAsset, FungiblesAccessError> {
		let (asset_id, balance) = value.borrow();
		match ConvertAssetId::reverse_ref(asset_id) {
			Ok(asset_id_as_multilocation) => match ConvertBalance::reverse_ref(balance) {
				Ok(amount) => Ok((asset_id_as_multilocation, amount).into()),
				Err(_) => Err(FungiblesAccessError::AmountToBalanceConversionFailed),
			},
			Err(_) => Err(FungiblesAccessError::AssetIdConversionFailed),
		}
	}
}

/// Helper function to convert collections with [`(AssetId, Balance)`] to [`MultiAsset`]
pub fn convert_asset_balance<'a, AssetId, Balance, ConvertAssetId, ConvertBalance, Converter>(
	items: impl Iterator<Item = &'a (AssetId, Balance)>,
) -> Result<Vec<MultiAsset>, FungiblesAccessError>
where
	AssetId: Clone + 'a,
	Balance: Clone + 'a,
	ConvertAssetId: Convert<MultiLocation, AssetId>,
	ConvertBalance: Convert<u128, Balance>,
	Converter: MultiAssetConverter<AssetId, Balance, ConvertAssetId, ConvertBalance>,
{
	items.map(Converter::convert_ref).collect()
}

/// Helper function to convert [`Balance`] with [`MultiLocation`] to [`MultiAsset`]
pub fn convert_balance<Balance: TryInto<u128>>(
	location: MultiLocation,
	balance: Balance,
) -> Result<MultiAsset, FungiblesAccessError> {
	match balance.try_into() {
		Ok(balance) => Ok((location, balance).into()),
		Err(_) => Err(FungiblesAccessError::AmountToBalanceConversionFailed),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use xcm::latest::prelude::*;
	use xcm_executor::traits::{Identity, JustTry};

	type Converter = ConvertedConcreteId<MultiLocation, u64, Identity, JustTry>;

	#[test]
	fn converted_concrete_id_fungible_multi_asset_conversion_roundtrip_works() {
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

		assert_eq!(Converter::convert_ref((location, amount)), Ok(expected_multi_asset));
	}

	#[test]
	fn convert_asset_balance_works() {
		let data = vec![
			(MultiLocation::new(0, X1(GlobalConsensus(ByGenesis([0; 32])))), 123456_u64),
			(MultiLocation::new(1, X1(GlobalConsensus(ByGenesis([1; 32])))), 654321_u64),
		];

		let expected_data = vec![
			MultiAsset {
				id: Concrete(MultiLocation::new(0, X1(GlobalConsensus(ByGenesis([0; 32]))))),
				fun: Fungible(123456_u128),
			},
			MultiAsset {
				id: Concrete(MultiLocation::new(1, X1(GlobalConsensus(ByGenesis([1; 32]))))),
				fun: Fungible(654321_u128),
			},
		];

		assert_eq!(convert_asset_balance::<_, _, _, _, Converter>(data.iter()), Ok(expected_data));
	}

	#[test]
	fn convert_balance_works() {
		let currency_location = MultiLocation::new(0, X1(GlobalConsensus(ByGenesis([0; 32]))));
		let currency_amount = 123456_u64;

		let expected_data = MultiAsset {
			id: Concrete(MultiLocation::new(0, X1(GlobalConsensus(ByGenesis([0; 32]))))),
			fun: Fungible(123456_u128),
		};

		assert_eq!(convert_balance(currency_location, currency_amount), Ok(expected_data));
	}
}
