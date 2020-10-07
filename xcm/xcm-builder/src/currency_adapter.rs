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
use xcm::v0::{Error, Result, MultiAsset, MultiLocation};
use sp_arithmetic::traits::SaturatedConversion;
use frame_support::traits::{ExistenceRequirement::AllowDeath, WithdrawReason};
use xcm_executor::traits::{MatchesFungible, LocationConversion, TransactAsset};

pub struct CurrencyAdapter<Currency, Matcher, AccountIdConverter, AccountId>(
	PhantomData<Currency>,
	PhantomData<Matcher>,
	PhantomData<AccountIdConverter>,
	PhantomData<AccountId>,
);

impl<
	Matcher: MatchesFungible<Currency::Balance>,
	AccountIdConverter: LocationConversion<AccountId>,
	Currency: frame_support::traits::Currency<AccountId>,
	AccountId,	// can't get away without it since Currency is generic over it.
> TransactAsset for CurrencyAdapter<Currency, Matcher, AccountIdConverter, AccountId> {

	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> Result {
		// Check we handle this asset.
		let amount = Matcher::matches_fungible(&what).ok_or(())?.saturated_into();
		let who = AccountIdConverter::from_location(who).ok_or(())?;
		let balance_amount = amount.try_into().map_err(|_| ())?;
		let _imbalance = Currency::deposit_creating(&who, balance_amount);
		Ok(())
	}

	fn withdraw_asset(what: &MultiAsset, who: &MultiLocation) -> result::Result<MultiAsset, Error> {
		// Check we handle this asset.
		let amount = Matcher::matches_fungible(&what).ok_or(())?.saturated_into();
		let who = AccountIdConverter::from_location(who).ok_or(())?;
		let balance_amount = amount.try_into().map_err(|_| ())?;
		Currency::withdraw(&who, balance_amount, WithdrawReason::Transfer.into(), AllowDeath).map_err(|_| ())?;
		Ok(what.clone())
	}
}
