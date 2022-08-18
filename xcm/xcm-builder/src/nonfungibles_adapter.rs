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

use frame_support::{
	ensure,
	traits::{tokens::nonfungibles, Contains, Get},
};
use sp_std::{marker::PhantomData, prelude::*, result};
use xcm::latest::prelude::*;
use xcm_executor::traits::{Convert, Error as MatchError, MatchesNonFungibles, TransactAsset};

pub struct NonFungiblesTransferAdapter<Assets, Matcher, AccountIdConverter, AccountId>(
	PhantomData<(Assets, Matcher, AccountIdConverter, AccountId)>,
);
impl<
		Assets: nonfungibles::Transfer<AccountId>,
		Matcher: MatchesNonFungibles<Assets::ItemId, Assets::CollectionId>,
		AccountIdConverter: Convert<MultiLocation, AccountId>,
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
			target: "xcm::non_fungibles_adapter",
			"transfer_asset what: {:?}, from: {:?}, to: {:?}, context: {:?}",
			what, from, to, context,
		);
		// Check we handle this asset.
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		let destination = AccountIdConverter::convert_ref(to)
			.map_err(|()| MatchError::AccountIdConversionFailed)?;
		Assets::transfer(&instance, &class, &destination)
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
		Matcher: MatchesNonFungibles<Assets::ItemId, Assets::CollectionId>,
		AccountIdConverter: Convert<MultiLocation, AccountId>,
		AccountId: Clone + Eq, // can't get away without it since Currency is generic over it.
		CheckAsset: Contains<Assets::ItemId>,
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
			target: "xcm::fungibles_adapter",
			"can_check_in origin: {:?}, what: {:?}, context: {:?}",
			_origin, what, context,
		);
		// Check we handle this asset.
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		if CheckAsset::contains(&class) {
			if let Some(checking_account) = CheckingAccount::get() {
				// This is an asset whose teleports we track.
				let owner = Assets::owner(&instance, &class);
				ensure!(owner == Some(checking_account), XcmError::NotWithdrawable);
				ensure!(Assets::can_transfer(&instance, &class), XcmError::NotWithdrawable);
			}
		}
		Ok(())
	}

	fn check_in(_origin: &MultiLocation, what: &MultiAsset, context: &XcmContext) {
		log::trace!(
			target: "xcm::fungibles_adapter",
			"check_in origin: {:?}, what: {:?}, context: {:?}",
			_origin, what, context,
		);
		if let Ok((class, instance)) = Matcher::matches_nonfungibles(what) {
			if CheckAsset::contains(&class) {
				let ok = Assets::burn(&instance, &class, None).is_ok();
				debug_assert!(
					ok,
					"`can_check_in` must have returned `true` immediately prior; qed"
				);
			}
		}
	}

	fn check_out(_dest: &MultiLocation, what: &MultiAsset, context: &XcmContext) {
		log::trace!(
			target: "xcm::fungibles_adapter",
			"check_out dest: {:?}, what: {:?}, context: {:?}",
			_dest, what, context,
		);
		if let Ok((class, instance)) = Matcher::matches_nonfungibles(what) {
			if CheckAsset::contains(&class) {
				if let Some(checking_account) = CheckingAccount::get() {
					let ok = Assets::mint_into(&instance, &class, &checking_account).is_ok();
					debug_assert!(ok, "`mint_into` cannot generally fail; qed");
				}
			}
		}
	}

	fn deposit_asset(what: &MultiAsset, who: &MultiLocation, context: &XcmContext) -> XcmResult {
		log::trace!(
			target: "xcm::fungibles_adapter",
			"deposit_asset what: {:?}, who: {:?}, context: {:?}",
			what, who, context,
		);
		// Check we handle this asset.
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		let who = AccountIdConverter::convert_ref(who)
			.map_err(|()| MatchError::AccountIdConversionFailed)?;
		Assets::mint_into(&instance, &class, &who)
			.map_err(|e| XcmError::FailedToTransactAsset(e.into()))
	}

	fn withdraw_asset(
		what: &MultiAsset,
		who: &MultiLocation,
		maybe_context: Option<&XcmContext>,
	) -> result::Result<xcm_executor::Assets, XcmError> {
		log::trace!(
			target: "xcm::fungibles_adapter",
			"withdraw_asset what: {:?}, who: {:?}, maybe_context: {:?}",
			what, who, maybe_context,
		);
		// Check we handle this asset.
		let who = AccountIdConverter::convert_ref(who)
			.map_err(|()| MatchError::AccountIdConversionFailed)?;
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		Assets::burn(&instance, &class, Some(&who))
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
		Matcher: MatchesNonFungibles<Assets::ItemId, Assets::CollectionId>,
		AccountIdConverter: Convert<MultiLocation, AccountId>,
		AccountId: Clone + Eq, // can't get away without it since Currency is generic over it.
		CheckAsset: Contains<Assets::ItemId>,
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
