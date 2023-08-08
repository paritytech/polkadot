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

//! Adapters to work with [`frame_support::traits::tokens::nonfungibles_v2`] through XCM.
use crate::{AssetChecking, MintLocation};
use frame_support::{
	ensure,
	traits::{tokens::nonfungibles_v2, Get, Incrementable},
};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_std::{marker::PhantomData, prelude::*, result};
use xcm::latest::prelude::*;
use xcm_executor::traits::{
	ConvertLocation, Error as MatchError, MatchesNonFungibles, TransactAsset,
};

const LOG_TARGET: &str = "xcm::nonfungibles_v2_adapter";
/// Adapter for transferring non-fungible tokens (NFTs) using [`nonfungibles_v2`].
///
/// This adapter facilitates the transfer of NFTs between different locations.
pub struct NonFungiblesV2TransferAdapter<Assets, Matcher, AccountIdConverter, AccountId>(
	PhantomData<(Assets, Matcher, AccountIdConverter, AccountId)>,
);
impl<
		Assets: nonfungibles_v2::Transfer<AccountId>,
		Matcher: MatchesNonFungibles<Assets::CollectionId, Assets::ItemId>,
		AccountIdConverter: ConvertLocation<AccountId>,
		AccountId: Clone, // can't get away without it since Currency is generic over it.
	> TransactAsset for NonFungiblesV2TransferAdapter<Assets, Matcher, AccountIdConverter, AccountId>
{
	fn transfer_asset(
		what: &MultiAsset,
		from: &MultiLocation,
		to: &MultiLocation,
		context: &XcmContext,
	) -> result::Result<xcm_executor::Assets, XcmError> {
		log::trace!(
			target: LOG_TARGET,
			"transfer_asset what: {:?}, from: {:?}, to: {:?}, context: {:?}",
			what,
			from,
			to,
			context,
		);
		// Check we handle this asset.
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		let destination = AccountIdConverter::convert_location(to)
			.ok_or(MatchError::AccountIdConversionFailed)?;
		Assets::transfer(&class, &instance, &destination)
			.map_err(|e| XcmError::FailedToTransactAsset(e.into()))?;
		Ok(what.clone().into())
	}
}

/// Adapter for mutating non-fungible tokens (NFTs) using [`nonfungibles_v2`].
///
/// This adapter provides functions to withdraw, deposit, check in and check out non fungibles.
pub struct NonFungiblesV2MutateAdapter<
	Assets,
	Matcher,
	AccountIdConverter,
	AccountId,
	CheckAsset,
	CheckingAccount,
	ItemConfig,
>(
	PhantomData<(
		Assets,
		Matcher,
		AccountIdConverter,
		AccountId,
		CheckAsset,
		CheckingAccount,
		ItemConfig,
	)>,
)
where
	ItemConfig: Default;

impl<
		Assets: nonfungibles_v2::Mutate<AccountId, ItemConfig>,
		Matcher: MatchesNonFungibles<Assets::CollectionId, Assets::ItemId>,
		AccountIdConverter: ConvertLocation<AccountId>,
		AccountId: Clone + Eq, // can't get away without it since Currency is generic over it.
		CheckAsset: AssetChecking<Assets::CollectionId>,
		CheckingAccount: Get<Option<AccountId>>,
		ItemConfig: Default,
	>
	NonFungiblesV2MutateAdapter<
		Assets,
		Matcher,
		AccountIdConverter,
		AccountId,
		CheckAsset,
		CheckingAccount,
		ItemConfig,
	>
{
	fn can_accrue_checked(class: Assets::CollectionId, instance: Assets::ItemId) -> XcmResult {
		ensure!(Assets::owner(&class, &instance).is_none(), XcmError::NotDepositable);
		Ok(())
	}
	fn can_reduce_checked(class: Assets::CollectionId, instance: Assets::ItemId) -> XcmResult {
		if let Some(checking_account) = CheckingAccount::get() {
			// This is an asset whose teleports we track.
			let owner = Assets::owner(&class, &instance);
			ensure!(owner == Some(checking_account), XcmError::NotWithdrawable);
			ensure!(Assets::can_transfer(&class, &instance), XcmError::NotWithdrawable);
		}
		Ok(())
	}
	fn accrue_checked(class: Assets::CollectionId, instance: Assets::ItemId) {
		if let Some(checking_account) = CheckingAccount::get() {
			let ok = Assets::mint_into(
				&class,
				&instance,
				&checking_account,
				&ItemConfig::default(),
				true,
			)
			.is_ok();
			debug_assert!(ok, "`mint_into` cannot generally fail; qed");
		}
	}
	fn reduce_checked(class: Assets::CollectionId, instance: Assets::ItemId) {
		let ok = Assets::burn(&class, &instance, None).is_ok();
		debug_assert!(ok, "`can_check_in` must have returned `true` immediately prior; qed");
	}
}

impl<
		Assets: nonfungibles_v2::Mutate<AccountId, ItemConfig>,
		Matcher: MatchesNonFungibles<Assets::CollectionId, Assets::ItemId>,
		AccountIdConverter: ConvertLocation<AccountId>,
		AccountId: Clone + Eq, // can't get away without it since Currency is generic over it.
		CheckAsset: AssetChecking<Assets::CollectionId>,
		CheckingAccount: Get<Option<AccountId>>,
		ItemConfig: Default,
	> TransactAsset
	for NonFungiblesV2MutateAdapter<
		Assets,
		Matcher,
		AccountIdConverter,
		AccountId,
		CheckAsset,
		CheckingAccount,
		ItemConfig,
	>
{
	fn can_check_in(_origin: &MultiLocation, what: &MultiAsset, context: &XcmContext) -> XcmResult {
		log::trace!(
			target: LOG_TARGET,
			"can_check_in origin: {:?}, what: {:?}, context: {:?}",
			_origin,
			what,
			context,
		);
		// Check we handle this asset.
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		match CheckAsset::asset_checking(&class) {
			// We track this asset's teleports to ensure no more come in than have gone out.
			Some(MintLocation::Local) => Self::can_reduce_checked(class, instance),
			// We track this asset's teleports to ensure no more go out than have come in.
			Some(MintLocation::NonLocal) => Self::can_accrue_checked(class, instance),
			_ => Ok(()),
		}
	}

	fn check_in(_origin: &MultiLocation, what: &MultiAsset, context: &XcmContext) {
		log::trace!(
			target: LOG_TARGET,
			"check_in origin: {:?}, what: {:?}, context: {:?}",
			_origin,
			what,
			context,
		);
		if let Ok((class, instance)) = Matcher::matches_nonfungibles(what) {
			match CheckAsset::asset_checking(&class) {
				// We track this asset's teleports to ensure no more come in than have gone out.
				Some(MintLocation::Local) => Self::reduce_checked(class, instance),
				// We track this asset's teleports to ensure no more go out than have come in.
				Some(MintLocation::NonLocal) => Self::accrue_checked(class, instance),
				_ => (),
			}
		}
	}

	fn can_check_out(_dest: &MultiLocation, what: &MultiAsset, context: &XcmContext) -> XcmResult {
		log::trace!(
			target: LOG_TARGET,
			"can_check_out dest: {:?}, what: {:?}, context: {:?}",
			_dest,
			what,
			context,
		);
		// Check we handle this asset.
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		match CheckAsset::asset_checking(&class) {
			// We track this asset's teleports to ensure no more come in than have gone out.
			Some(MintLocation::Local) => Self::can_accrue_checked(class, instance),
			// We track this asset's teleports to ensure no more go out than have come in.
			Some(MintLocation::NonLocal) => Self::can_reduce_checked(class, instance),
			_ => Ok(()),
		}
	}

	fn check_out(_dest: &MultiLocation, what: &MultiAsset, context: &XcmContext) {
		log::trace!(
			target: LOG_TARGET,
			"check_out dest: {:?}, what: {:?}, context: {:?}",
			_dest,
			what,
			context,
		);
		if let Ok((class, instance)) = Matcher::matches_nonfungibles(what) {
			match CheckAsset::asset_checking(&class) {
				// We track this asset's teleports to ensure no more come in than have gone out.
				Some(MintLocation::Local) => Self::accrue_checked(class, instance),
				// We track this asset's teleports to ensure no more go out than have come in.
				Some(MintLocation::NonLocal) => Self::reduce_checked(class, instance),
				_ => (),
			}
		}
	}

	fn deposit_asset(what: &MultiAsset, who: &MultiLocation, context: &XcmContext) -> XcmResult {
		log::trace!(
			target: LOG_TARGET,
			"deposit_asset what: {:?}, who: {:?}, context: {:?}",
			what,
			who,
			context,
		);
		// Check we handle this asset.
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		let who = AccountIdConverter::convert_location(who)
			.ok_or(MatchError::AccountIdConversionFailed)?;

		Assets::mint_into(&class, &instance, &who, &ItemConfig::default(), true)
			.map_err(|e| XcmError::FailedToTransactAsset(e.into()))
	}

	fn withdraw_asset(
		what: &MultiAsset,
		who: &MultiLocation,
		maybe_context: Option<&XcmContext>,
	) -> result::Result<xcm_executor::Assets, XcmError> {
		log::trace!(
			target: LOG_TARGET,
			"withdraw_asset what: {:?}, who: {:?}, maybe_context: {:?}",
			what,
			who,
			maybe_context,
		);
		// Check we handle this asset.
		let who = AccountIdConverter::convert_location(who)
			.ok_or(MatchError::AccountIdConversionFailed)?;
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		Assets::burn(&class, &instance, Some(&who))
			.map_err(|e| XcmError::FailedToTransactAsset(e.into()))?;
		Ok(what.clone().into())
	}
}

/// Adapter for handling non-fungible tokens (NFTs) using [`nonfungibles_v2`].
///
/// This adapter combines the functionalities of both the [`NonFungiblesV2TransferAdapter`] and [`NonFungiblesV2MutateAdapter`] adapters,
/// allowing handling NFTs in various scenarios.
/// For detailed information on the functions, refer to [`TransactAsset`].
pub struct NonFungiblesV2Adapter<
	Assets,
	Matcher,
	AccountIdConverter,
	AccountId,
	CheckAsset,
	CheckingAccount,
	ItemConfig,
>(
	PhantomData<(
		Assets,
		Matcher,
		AccountIdConverter,
		AccountId,
		CheckAsset,
		CheckingAccount,
		ItemConfig,
	)>,
)
where
	ItemConfig: Default;
impl<
		Assets: nonfungibles_v2::Mutate<AccountId, ItemConfig> + nonfungibles_v2::Transfer<AccountId>,
		Matcher: MatchesNonFungibles<Assets::CollectionId, Assets::ItemId>,
		AccountIdConverter: ConvertLocation<AccountId>,
		AccountId: Clone + Eq, // can't get away without it since Currency is generic over it.
		CheckAsset: AssetChecking<Assets::CollectionId>,
		CheckingAccount: Get<Option<AccountId>>,
		ItemConfig: Default,
	> TransactAsset
	for NonFungiblesV2Adapter<
		Assets,
		Matcher,
		AccountIdConverter,
		AccountId,
		CheckAsset,
		CheckingAccount,
		ItemConfig,
	>
{
	fn can_check_in(origin: &MultiLocation, what: &MultiAsset, context: &XcmContext) -> XcmResult {
		NonFungiblesV2MutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
			ItemConfig,
		>::can_check_in(origin, what, context)
	}

	fn check_in(origin: &MultiLocation, what: &MultiAsset, context: &XcmContext) {
		NonFungiblesV2MutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
			ItemConfig,
		>::check_in(origin, what, context)
	}

	fn can_check_out(dest: &MultiLocation, what: &MultiAsset, context: &XcmContext) -> XcmResult {
		NonFungiblesV2MutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
			ItemConfig,
		>::can_check_out(dest, what, context)
	}

	fn check_out(dest: &MultiLocation, what: &MultiAsset, context: &XcmContext) {
		NonFungiblesV2MutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
			ItemConfig,
		>::check_out(dest, what, context)
	}

	fn deposit_asset(what: &MultiAsset, who: &MultiLocation, context: &XcmContext) -> XcmResult {
		NonFungiblesV2MutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
			ItemConfig,
		>::deposit_asset(what, who, context)
	}

	fn withdraw_asset(
		what: &MultiAsset,
		who: &MultiLocation,
		maybe_context: Option<&XcmContext>,
	) -> result::Result<xcm_executor::Assets, XcmError> {
		NonFungiblesV2MutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
			ItemConfig,
		>::withdraw_asset(what, who, maybe_context)
	}

	fn transfer_asset(
		what: &MultiAsset,
		from: &MultiLocation,
		to: &MultiLocation,
		context: &XcmContext,
	) -> result::Result<xcm_executor::Assets, XcmError> {
		NonFungiblesV2TransferAdapter::<Assets, Matcher, AccountIdConverter, AccountId>::transfer_asset(
			what, from, to, context,
		)
	}
}

#[derive(
	Copy, Clone, Decode, Encode, Eq, PartialEq, Ord, PartialOrd, Debug, TypeInfo, MaxEncodedLen,
)]
/// Represents a collection ID based on a MultiLocation.
///
/// This structure provides a way to map a MultiLocation to a collection ID,
/// which is useful for describing collections that do not follow an incremental pattern.
pub struct MultiLocationCollectionId(MultiLocation);
impl MultiLocationCollectionId {
	/// Consume `self` and return the inner MultiLocation.
	pub fn into_inner(self) -> MultiLocation {
		self.0
	}

	/// Return a reference to the inner MultiLocation.
	pub fn inner(&self) -> &MultiLocation {
		&self.0
	}
}

impl Incrementable for MultiLocationCollectionId {
	fn increment(&self) -> Option<Self> {
		None
	}

	fn initial_value() -> Option<Self> {
		None
	}
}

impl From<MultiLocation> for MultiLocationCollectionId {
	fn from(value: MultiLocation) -> Self {
		MultiLocationCollectionId(value)
	}
}

impl From<MultiLocationCollectionId> for MultiLocation {
	fn from(value: MultiLocationCollectionId) -> MultiLocation {
		value.into_inner()
	}
}
