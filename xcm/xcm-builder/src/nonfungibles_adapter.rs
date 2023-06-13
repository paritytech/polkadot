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

use crate::{AssetChecking, MintLocation};
use frame_support::{
	ensure,
	traits::{tokens::nonfungibles, Get},
};
use sp_std::{marker::PhantomData, prelude::*, result};
use xcm::latest::prelude::*;
use xcm_executor::traits::{
	ConvertLocation, Error as MatchError, MatchesNonFungibles, TransactAsset,
};

const LOG_TARGET: &str = "xcm::nonfungibles_adapter";

pub struct NonFungiblesTransferAdapter<Assets, Matcher, AccountIdConverter, AccountId>(
	PhantomData<(Assets, Matcher, AccountIdConverter, AccountId)>,
);
impl<
		Assets: nonfungibles::Transfer<AccountId>,
		Matcher: MatchesNonFungibles<Assets::CollectionId, Assets::ItemId>,
		AccountIdConverter: ConvertLocation<AccountId>,
		AccountId: Clone, // can't get away without it since Currency is generic over it.
	> TransactAsset for NonFungiblesTransferAdapter<Assets, Matcher, AccountIdConverter, AccountId>
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

pub struct NonFungiblesMutateAdapter<
	Assets,
	Matcher,
	AccountIdConverter,
	AccountId,
	CheckAsset,
	CheckingAccount,
>(PhantomData<(Assets, Matcher, AccountIdConverter, AccountId, CheckAsset, CheckingAccount)>);

impl<
		Assets: nonfungibles::Mutate<AccountId>,
		Matcher: MatchesNonFungibles<Assets::CollectionId, Assets::ItemId>,
		AccountIdConverter: ConvertLocation<AccountId>,
		AccountId: Clone + Eq, // can't get away without it since Currency is generic over it.
		CheckAsset: AssetChecking<Assets::CollectionId>,
		CheckingAccount: Get<Option<AccountId>>,
	>
	NonFungiblesMutateAdapter<
		Assets,
		Matcher,
		AccountIdConverter,
		AccountId,
		CheckAsset,
		CheckingAccount,
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
			let ok = Assets::mint_into(&class, &instance, &checking_account).is_ok();
			debug_assert!(ok, "`mint_into` cannot generally fail; qed");
		}
	}
	fn reduce_checked(class: Assets::CollectionId, instance: Assets::ItemId) {
		let ok = Assets::burn(&class, &instance, None).is_ok();
		debug_assert!(ok, "`can_check_in` must have returned `true` immediately prior; qed");
	}
}

impl<
		Assets: nonfungibles::Mutate<AccountId>,
		Matcher: MatchesNonFungibles<Assets::CollectionId, Assets::ItemId>,
		AccountIdConverter: ConvertLocation<AccountId>,
		AccountId: Clone + Eq, // can't get away without it since Currency is generic over it.
		CheckAsset: AssetChecking<Assets::CollectionId>,
		CheckingAccount: Get<Option<AccountId>>,
	> TransactAsset
	for NonFungiblesMutateAdapter<
		Assets,
		Matcher,
		AccountIdConverter,
		AccountId,
		CheckAsset,
		CheckingAccount,
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
		Assets::mint_into(&class, &instance, &who)
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

pub struct NonFungiblesAdapter<
	Assets,
	Matcher,
	AccountIdConverter,
	AccountId,
	CheckAsset,
	CheckingAccount,
>(PhantomData<(Assets, Matcher, AccountIdConverter, AccountId, CheckAsset, CheckingAccount)>);
impl<
		Assets: nonfungibles::Mutate<AccountId> + nonfungibles::Transfer<AccountId>,
		Matcher: MatchesNonFungibles<Assets::CollectionId, Assets::ItemId>,
		AccountIdConverter: ConvertLocation<AccountId>,
		AccountId: Clone + Eq, // can't get away without it since Currency is generic over it.
		CheckAsset: AssetChecking<Assets::CollectionId>,
		CheckingAccount: Get<Option<AccountId>>,
	> TransactAsset
	for NonFungiblesAdapter<Assets, Matcher, AccountIdConverter, AccountId, CheckAsset, CheckingAccount>
{
	fn can_check_in(origin: &MultiLocation, what: &MultiAsset, context: &XcmContext) -> XcmResult {
		NonFungiblesMutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
		>::can_check_in(origin, what, context)
	}

	fn check_in(origin: &MultiLocation, what: &MultiAsset, context: &XcmContext) {
		NonFungiblesMutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
		>::check_in(origin, what, context)
	}

	fn can_check_out(dest: &MultiLocation, what: &MultiAsset, context: &XcmContext) -> XcmResult {
		NonFungiblesMutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
		>::can_check_out(dest, what, context)
	}

	fn check_out(dest: &MultiLocation, what: &MultiAsset, context: &XcmContext) {
		NonFungiblesMutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
		>::check_out(dest, what, context)
	}

	fn deposit_asset(what: &MultiAsset, who: &MultiLocation, context: &XcmContext) -> XcmResult {
		NonFungiblesMutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
		>::deposit_asset(what, who, context)
	}

	fn withdraw_asset(
		what: &MultiAsset,
		who: &MultiLocation,
		maybe_context: Option<&XcmContext>,
	) -> result::Result<xcm_executor::Assets, XcmError> {
		NonFungiblesMutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
		>::withdraw_asset(what, who, maybe_context)
	}

	fn transfer_asset(
		what: &MultiAsset,
		from: &MultiLocation,
		to: &MultiLocation,
		context: &XcmContext,
	) -> result::Result<xcm_executor::Assets, XcmError> {
		NonFungiblesTransferAdapter::<Assets, Matcher, AccountIdConverter, AccountId>::transfer_asset(
			what, from, to, context,
		)
	}
}
