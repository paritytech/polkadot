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

//! Adapters to work with `frame_support::traits::Currency` through XCM.

use super::MintLocation;
use frame_support::traits::{ExistenceRequirement::AllowDeath, Get, WithdrawReasons};
use sp_runtime::traits::CheckedSub;
use sp_std::{marker::PhantomData, result};
use xcm::latest::{Error as XcmError, MultiAsset, MultiLocation, Result, XcmContext};
use xcm_executor::{
	traits::{Convert, MatchesFungible, TransactAsset},
	Assets,
};

/// Asset transaction errors.
enum Error {
	/// The given asset is not handled. (According to [`XcmError::AssetNotFound`])
	AssetNotHandled,
	/// `MultiLocation` to `AccountId` conversion failed.
	AccountIdConversionFailed,
}

impl From<Error> for XcmError {
	fn from(e: Error) -> Self {
		use XcmError::FailedToTransactAsset;
		match e {
			Error::AssetNotHandled => XcmError::AssetNotFound,
			Error::AccountIdConversionFailed => FailedToTransactAsset("AccountIdConversionFailed"),
		}
	}
}

/// Simple adapter to use a currency as asset transactor. This type can be used as `type AssetTransactor` in
/// `xcm::Config`.
///
/// # Example
/// ```
/// use parity_scale_codec::Decode;
/// use frame_support::{parameter_types, PalletId};
/// use sp_runtime::traits::{AccountIdConversion, TrailingZeroInput};
/// use xcm::latest::prelude::*;
/// use xcm_builder::{ParentIsPreset, CurrencyAdapter, IsConcrete};
///
/// /// Our chain's account id.
/// type AccountId = sp_runtime::AccountId32;
///
/// /// Our relay chain's location.
/// parameter_types! {
///     pub RelayChain: MultiLocation = Parent.into();
///     pub CheckingAccount: AccountId = PalletId(*b"checking").into_account_truncating();
/// }
///
/// /// Some items that implement `Convert<MultiLocation, AccountId>`. Can be more, but for now we just assume we accept
/// /// messages from the parent (relay chain).
/// pub type LocationConverter = (ParentIsPreset<AccountId>);
///
/// /// Just a dummy implementation of `Currency`. Normally this would be `Balances`.
/// pub type CurrencyImpl = ();
///
/// /// Final currency adapter. This can be used in `xcm::Config` to specify how asset related transactions happen.
/// pub type AssetTransactor = CurrencyAdapter<
///     // Use this `Currency` impl instance:
///     CurrencyImpl,
///     // The matcher: use the currency when the asset is a concrete asset in our relay chain.
///     IsConcrete<RelayChain>,
///     // The local converter: default account of the parent relay chain.
///     LocationConverter,
///     // Our chain's account ID type.
///     AccountId,
///     // The checking account. Can be any deterministic inaccessible account.
///     CheckingAccount,
/// >;
/// ```
pub struct CurrencyAdapter<Currency, Matcher, AccountIdConverter, AccountId, CheckedAccount>(
	PhantomData<(Currency, Matcher, AccountIdConverter, AccountId, CheckedAccount)>,
);

impl<
		Currency: frame_support::traits::Currency<AccountId>,
		Matcher: MatchesFungible<Currency::Balance>,
		AccountIdConverter: Convert<MultiLocation, AccountId>,
		AccountId: Clone, // can't get away without it since Currency is generic over it.
		CheckedAccount: Get<Option<(AccountId, MintLocation)>>,
	> CurrencyAdapter<Currency, Matcher, AccountIdConverter, AccountId, CheckedAccount>
{
	fn can_accrue_checked(_checked_account: AccountId, _amount: Currency::Balance) -> Result {
		Ok(())
	}
	fn can_reduce_checked(checked_account: AccountId, amount: Currency::Balance) -> Result {
		let new_balance = Currency::free_balance(&checked_account)
			.checked_sub(&amount)
			.ok_or(XcmError::NotWithdrawable)?;
		Currency::ensure_can_withdraw(
			&checked_account,
			amount,
			WithdrawReasons::TRANSFER,
			new_balance,
		)
		.map_err(|_| XcmError::NotWithdrawable)
	}
	fn accrue_checked(checked_account: AccountId, amount: Currency::Balance) {
		Currency::deposit_creating(&checked_account, amount);
		Currency::deactivate(amount);
	}
	fn reduce_checked(checked_account: AccountId, amount: Currency::Balance) {
		let ok =
			Currency::withdraw(&checked_account, amount, WithdrawReasons::TRANSFER, AllowDeath)
				.is_ok();
		if ok {
			Currency::reactivate(amount);
		} else {
			frame_support::defensive!(
				"`can_check_in` must have returned `true` immediately prior; qed"
			);
		}
	}
}

impl<
		Currency: frame_support::traits::Currency<AccountId>,
		Matcher: MatchesFungible<Currency::Balance>,
		AccountIdConverter: Convert<MultiLocation, AccountId>,
		AccountId: Clone, // can't get away without it since Currency is generic over it.
		CheckedAccount: Get<Option<(AccountId, MintLocation)>>,
	> TransactAsset
	for CurrencyAdapter<Currency, Matcher, AccountIdConverter, AccountId, CheckedAccount>
{
	fn can_check_in(_origin: &MultiLocation, what: &MultiAsset, _context: &XcmContext) -> Result {
		log::trace!(target: "xcm::currency_adapter", "can_check_in origin: {:?}, what: {:?}", _origin, what);
		// Check we handle this asset.
		let amount: Currency::Balance =
			Matcher::matches_fungible(what).ok_or(Error::AssetNotHandled)?;
		match CheckedAccount::get() {
			Some((checked_account, MintLocation::Local)) =>
				Self::can_reduce_checked(checked_account, amount),
			Some((checked_account, MintLocation::NonLocal)) =>
				Self::can_accrue_checked(checked_account, amount),
			None => Ok(()),
		}
	}

	fn check_in(_origin: &MultiLocation, what: &MultiAsset, _context: &XcmContext) {
		log::trace!(target: "xcm::currency_adapter", "check_in origin: {:?}, what: {:?}", _origin, what);
		if let Some(amount) = Matcher::matches_fungible(what) {
			match CheckedAccount::get() {
				Some((checked_account, MintLocation::Local)) =>
					Self::reduce_checked(checked_account, amount),
				Some((checked_account, MintLocation::NonLocal)) =>
					Self::accrue_checked(checked_account, amount),
				None => (),
			}
		}
	}

	fn can_check_out(_dest: &MultiLocation, what: &MultiAsset, _context: &XcmContext) -> Result {
		log::trace!(target: "xcm::currency_adapter", "check_out dest: {:?}, what: {:?}", _dest, what);
		let amount = Matcher::matches_fungible(what).ok_or(Error::AssetNotHandled)?;
		match CheckedAccount::get() {
			Some((checked_account, MintLocation::Local)) =>
				Self::can_accrue_checked(checked_account, amount),
			Some((checked_account, MintLocation::NonLocal)) =>
				Self::can_reduce_checked(checked_account, amount),
			None => Ok(()),
		}
	}

	fn check_out(_dest: &MultiLocation, what: &MultiAsset, _context: &XcmContext) {
		log::trace!(target: "xcm::currency_adapter", "check_out dest: {:?}, what: {:?}", _dest, what);
		if let Some(amount) = Matcher::matches_fungible(what) {
			match CheckedAccount::get() {
				Some((checked_account, MintLocation::Local)) =>
					Self::accrue_checked(checked_account, amount),
				Some((checked_account, MintLocation::NonLocal)) =>
					Self::reduce_checked(checked_account, amount),
				None => (),
			}
		}
	}

	fn deposit_asset(what: &MultiAsset, who: &MultiLocation, _context: &XcmContext) -> Result {
		log::trace!(target: "xcm::currency_adapter", "deposit_asset what: {:?}, who: {:?}", what, who);
		// Check we handle this asset.
		let amount = Matcher::matches_fungible(&what).ok_or(Error::AssetNotHandled)?;
		let who =
			AccountIdConverter::convert_ref(who).map_err(|()| Error::AccountIdConversionFailed)?;
		let _imbalance = Currency::deposit_creating(&who, amount);
		Ok(())
	}

	fn withdraw_asset(
		what: &MultiAsset,
		who: &MultiLocation,
		_maybe_context: Option<&XcmContext>,
	) -> result::Result<Assets, XcmError> {
		log::trace!(target: "xcm::currency_adapter", "withdraw_asset what: {:?}, who: {:?}", what, who);
		// Check we handle this asset.
		let amount = Matcher::matches_fungible(what).ok_or(Error::AssetNotHandled)?;
		let who =
			AccountIdConverter::convert_ref(who).map_err(|()| Error::AccountIdConversionFailed)?;
		Currency::withdraw(&who, amount, WithdrawReasons::TRANSFER, AllowDeath)
			.map_err(|e| XcmError::FailedToTransactAsset(e.into()))?;
		Ok(what.clone().into())
	}

	fn internal_transfer_asset(
		asset: &MultiAsset,
		from: &MultiLocation,
		to: &MultiLocation,
		_context: &XcmContext,
	) -> result::Result<Assets, XcmError> {
		log::trace!(target: "xcm::currency_adapter", "internal_transfer_asset asset: {:?}, from: {:?}, to: {:?}", asset, from, to);
		let amount = Matcher::matches_fungible(asset).ok_or(Error::AssetNotHandled)?;
		let from =
			AccountIdConverter::convert_ref(from).map_err(|()| Error::AccountIdConversionFailed)?;
		let to =
			AccountIdConverter::convert_ref(to).map_err(|()| Error::AccountIdConversionFailed)?;
		Currency::transfer(&from, &to, amount, AllowDeath)
			.map_err(|e| XcmError::FailedToTransactAsset(e.into()))?;
		Ok(asset.clone().into())
	}
}
