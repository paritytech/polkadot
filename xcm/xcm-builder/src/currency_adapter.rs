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

use sp_std::{result, convert::TryInto, marker::PhantomData};
use xcm::v0::{Error as XcmError, Result, MultiAsset, MultiLocation};
use sp_runtime::traits::SaturatedConversion;
use frame_support::traits::{ExistenceRequirement::AllowDeath, WithdrawReasons};
use xcm_executor::traits::{MatchesFungible, Convert, TransactAsset};
use xcm_executor::Assets;

/// Asset transaction errors.
enum Error {
	/// Asset not found.
	AssetNotFound,
	/// `MultiLocation` to `AccountId` conversion failed.
	AccountIdConversionFailed,
	/// `u128` amount to currency `Balance` conversion failed.
	AmountToBalanceConversionFailed,
}

impl From<Error> for XcmError {
	fn from(e: Error) -> Self {
		match e {
			Error::AssetNotFound => XcmError::FailedToTransactAsset("AssetNotFound"),
			Error::AccountIdConversionFailed =>
				XcmError::FailedToTransactAsset("AccountIdConversionFailed"),
			Error::AmountToBalanceConversionFailed =>
				XcmError::FailedToTransactAsset("AmountToBalanceConversionFailed"),
		}
	}
}

pub struct CurrencyAdapter<Currency, Matcher, AccountIdConverter, AccountId>(
	PhantomData<(Currency, Matcher, AccountIdConverter, AccountId)>
);

impl<
	Matcher: MatchesFungible<Currency::Balance>,
	AccountIdConverter: Convert<MultiLocation, AccountId>,
	Currency: frame_support::traits::Currency<AccountId>,
	AccountId: Clone,	// can't get away without it since Currency is generic over it.
> TransactAsset for CurrencyAdapter<Currency, Matcher, AccountIdConverter, AccountId> {

	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> Result {
		// Check we handle this asset.
		let amount: u128 = Matcher::matches_fungible(&what)
			.ok_or(Error::AssetNotFound)?
			.saturated_into();
		let who = AccountIdConverter::convert_ref(who)
			.map_err(|()| Error::AccountIdConversionFailed)?;
		let balance_amount = amount
			.try_into()
			.map_err(|_| Error::AmountToBalanceConversionFailed)?;
		let _imbalance = Currency::deposit_creating(&who, balance_amount);
		Ok(())
	}

	fn withdraw_asset(
		what: &MultiAsset,
		who: &MultiLocation
	) -> result::Result<Assets, XcmError> {
		// Check we handle this asset.
		let amount: u128 = Matcher::matches_fungible(what)
			.ok_or(Error::AssetNotFound)?
			.saturated_into();
		let who = AccountIdConverter::convert_ref(who)
			.map_err(|()| Error::AccountIdConversionFailed)?;
		let balance_amount = amount
			.try_into()
			.map_err(|_| Error::AmountToBalanceConversionFailed)?;
		Currency::withdraw(&who, balance_amount, WithdrawReasons::TRANSFER, AllowDeath)
			.map_err(|e| XcmError::FailedToTransactAsset(e.into()))?;
		Ok(what.clone().into())
	}
}
