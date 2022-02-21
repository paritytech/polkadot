// Copyright 2020 Parity Technologies (UK) Ltd.
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

use frame_support::traits::{tokens::nonfungibles, Contains, Get};
use sp_std::{marker::PhantomData, prelude::*, result};
use xcm::latest::prelude::*;
use xcm_executor::traits::{Convert, Error as MatchError, MatchesNonFungibles, TransactAsset};
use frame_support::ensure;

pub struct NonFungiblesTransferAdapter<Assets, Matcher, AccountIdConverter, AccountId>(
	PhantomData<(Assets, Matcher, AccountIdConverter, AccountId)>,
);
impl<
		Assets: nonfungibles::Transfer<AccountId>,
		Matcher: MatchesNonFungibles<Assets::ClassId, Assets::InstanceId>,
		AccountIdConverter: Convert<MultiLocation, AccountId>,
		AccountId: Clone, // can't get away without it since Currency is generic over it.
	> TransactAsset for NonFungiblesTransferAdapter<Assets, Matcher, AccountIdConverter, AccountId>
{
	fn transfer_asset(
		what: &MultiAsset,
		from: &MultiLocation,
		to: &MultiLocation,
	) -> result::Result<xcm_executor::Assets, XcmError> {
		log::trace!(
			target: "xcm::non_fungibles_adapter",
			"transfer_asset what: {:?}, from: {:?}, to: {:?}",
			what, from, to
		);
		// Check we handle this asset.
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		let destination = AccountIdConverter::convert_ref(to)
			.map_err(|()| MatchError::AccountIdConversionFailed)?;
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
		Matcher: MatchesNonFungibles<Assets::ClassId, Assets::InstanceId>,
		AccountIdConverter: Convert<MultiLocation, AccountId>,
		AccountId: Clone + Eq, // can't get away without it since Currency is generic over it.
		CheckAsset: Contains<Assets::ClassId>,
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
	fn can_check_in(_origin: &MultiLocation, what: &MultiAsset) -> XcmResult {
		log::trace!(
			target: "xcm::fungibles_adapter",
			"can_check_in origin: {:?}, what: {:?}",
			_origin, what
		);
		// Check we handle this asset.
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		if CheckAsset::contains(&class) {
			if let Some(checking_account) = CheckingAccount::get() {
				// This is an asset whose teleports we track.
				let owner = Assets::owner(&class, &instance);
				ensure!(owner == Some(checking_account), XcmError::NotWithdrawable);
				ensure!(Assets::can_transfer(&class, &instance), XcmError::NotWithdrawable);
			}
		}
		Ok(())
	}

	fn check_in(_origin: &MultiLocation, what: &MultiAsset) {
		log::trace!(
			target: "xcm::fungibles_adapter",
			"check_in origin: {:?}, what: {:?}",
			_origin, what
		);
		if let Ok((class, instance)) = Matcher::matches_nonfungibles(what) {
			if CheckAsset::contains(&class) {
				let ok = Assets::burn_from(&class, &instance).is_ok();
				debug_assert!(
					ok,
					"`can_check_in` must have returned `true` immediately prior; qed"
				);
			}
		}
	}

	fn check_out(_dest: &MultiLocation, what: &MultiAsset) {
		log::trace!(
			target: "xcm::fungibles_adapter",
			"check_out dest: {:?}, what: {:?}",
			_dest, what
		);
		if let Ok((class, instance)) = Matcher::matches_nonfungibles(what) {
			if CheckAsset::contains(&class) {
				if let Some(checking_account) = CheckingAccount::get() {
					let ok = Assets::mint_into(&class, &instance, &checking_account).is_ok();
					debug_assert!(ok, "`mint_into` cannot generally fail; qed");
				}
			}
		}
	}

	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> XcmResult {
		log::trace!(
			target: "xcm::fungibles_adapter",
			"deposit_asset what: {:?}, who: {:?}",
			what, who,
		);
		// Check we handle this asset.
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		let who = AccountIdConverter::convert_ref(who)
			.map_err(|()| MatchError::AccountIdConversionFailed)?;
		Assets::mint_into(&class, &instance, &who)
			.map_err(|e| XcmError::FailedToTransactAsset(e.into()))
	}

	fn withdraw_asset(
		what: &MultiAsset,
		who: &MultiLocation,
	) -> result::Result<xcm_executor::Assets, XcmError> {
		log::trace!(
			target: "xcm::fungibles_adapter",
			"withdraw_asset what: {:?}, who: {:?}",
			what, who,
		);
		// Check we handle this asset.
		let (class, instance) = Matcher::matches_nonfungibles(what)?;
		Assets::burn_from(&class, &instance)
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
		Matcher: MatchesNonFungibles<Assets::ClassId, Assets::InstanceId>,
		AccountIdConverter: Convert<MultiLocation, AccountId>,
		AccountId: Clone + Eq, // can't get away without it since Currency is generic over it.
		CheckAsset: Contains<Assets::ClassId>,
		CheckingAccount: Get<Option<AccountId>>,
	> TransactAsset
	for NonFungiblesAdapter<Assets, Matcher, AccountIdConverter, AccountId, CheckAsset, CheckingAccount>
{
	fn can_check_in(origin: &MultiLocation, what: &MultiAsset) -> XcmResult {
		NonFungiblesMutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
		>::can_check_in(origin, what)
	}

	fn check_in(origin: &MultiLocation, what: &MultiAsset) {
		NonFungiblesMutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
		>::check_in(origin, what)
	}

	fn check_out(dest: &MultiLocation, what: &MultiAsset) {
		NonFungiblesMutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
		>::check_out(dest, what)
	}

	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> XcmResult {
		NonFungiblesMutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
		>::deposit_asset(what, who)
	}

	fn withdraw_asset(
		what: &MultiAsset,
		who: &MultiLocation,
	) -> result::Result<xcm_executor::Assets, XcmError> {
		NonFungiblesMutateAdapter::<
			Assets,
			Matcher,
			AccountIdConverter,
			AccountId,
			CheckAsset,
			CheckingAccount,
		>::withdraw_asset(what, who)
	}

	fn transfer_asset(
		what: &MultiAsset,
		from: &MultiLocation,
		to: &MultiLocation,
	) -> result::Result<xcm_executor::Assets, XcmError> {
		NonFungiblesTransferAdapter::<Assets, Matcher, AccountIdConverter, AccountId>::transfer_asset(
			what, from, to,
		)
	}
}
