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

use xcm::v0::{XcmError, XcmResult, MultiAsset, MultiLocation};
use sp_arithmetic::traits::SaturatedConversion;
use frame_support::traits::{Currency, ExistenceRequirement::AllowDeath, WithdrawReason};
use crate::traits::{MatchesAsset, PunnFromLocation};

type BalanceOf<T> = <<T as Trait>::Currency as Currency<<T as frame_system::Trait>::AccountId>>::Balance;

pub struct CurrencyAdapter<Currency, Matcher, AccountIdConverter, AccountId>;

impl<
	Matcher: MatchesAsset,
	AccountIdConverter: PunnFromLocation<AccountId>,
	Currency: Currency<AccountId>,
	AccountId,	// can't get away without it since Currency is generic over it.
> TransactAsset for CurrencyAdapter<Currency, Matcher, AccountIdConverter, AccountId> {

	fn deposit_asset(what: &MultiAsset, who: &MultiLocation) -> XcmResult {
		// Check we handle this asset.
		let amount = Matcher::matches_asset(&what).ok_or(())?.saturating_into();
		let who = AccountIdConverter::punn_from_location(who)?;
		T::Currency::deposit_creating(&who, amount).map_err(|_| ())?;
		Ok(())
	}

	fn withdraw_asset(what: &MultiAsset, who: &MultiLocation) -> Result<MultiAsset, XcmError> {
		// Check we handle this asset.
		let amount = Matcher::matches_asset(&what).ok_or(())?.saturating_into();
		let who = AccountIdConverter::punn_from_location(who)?;
		T::Currency::withdraw(&who, amount, WithdrawReason::Transfer, AllowDeath).map_err(|_| ())?;
		Ok(what.clone())
	}
}
