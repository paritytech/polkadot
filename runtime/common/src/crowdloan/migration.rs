// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

use super::*;
use frame_support::generate_storage_alias;

/// Migrations for using fund index to create fund accounts instead of para ID.
pub mod crowdloan_index_migration {
	// The old way we generated fund accounts.
	fn old_fund_account_id<T: Config>(index: ParaId) -> T::AccountId {
		T::PalletId::get().into_sub_account(index)
	}

	pub fn pre_migration<T: Config>() {
		// `NextTrieIndex` should have a value.
		generate_storage_alias!(Crowdloan, NextTrieIndex => Value<FundIndex>);
		let next_index = NextTrieIndex::take().unwrap_or_default();
		assert!(next_index > 0);

		// Each fund should have some non-zero balance.
		Funds::<T>::iter().for_each(|(para_id, fund)| {
			let old_fund_account = old_fund_account_id::<T>(para_id);
			let total_balance = T::Currency::total_balance(old_fund_account);

			assert_eq!(total_balance, fund.raised);
			assert!(total_balance > Zero::zero());
		});
	}

	/// This migration converts crowdloans to use a crowdloan index rather than the parachain id as a
	/// unique identifier. This makes it easier to swap two crowdloans between parachains.
	pub fn migrate<T: Config>() {
		// First migrate `NextTrieIndex` counter to `NextFundIndex`.
		generate_storage_alias!(Crowdloan, NextTrieIndex => Value<FundIndex>);

		let next_index = NextTrieIndex::take().unwrap_or_default();
		NextFundIndex::<T>::set(next_index);

		// Migrate all accounts from `old_fund_account` to `fund_account` using `fund_index`.
		Funds::<T>::iter().for_each(|(para_id, fund)| {
			let old_fund_account = old_fund_account_id::<T>(para_id);
			let new_fund_account = Pallet::<T>::fund_account_id(fund.fund_index);

			// Funds should only have a free balance and a reserve balance. Both of these are in the
			// `Account` storage item, so we just swap them.
			let account_info = frame_system::Account::<T>::take(old_fund_account);
			frame_system::Account::<T>::insert(new_fund_account, account_info);
		});
	}

	pub fn post_migrate<T: Config>() {
		// `NextTrieIndex` should not have a value, and `NextFundIndex` should.
		generate_storage_alias!(Crowdloan, NextTrieIndex => Value<FundIndex>);
		assert!(NextTrieIndex::take().is_none());
		assert!(NextFundIndex::get() > 0);

		// Each fund should have balance migrated correctly.
		Funds::<T>::iter().for_each(|(para_id, fund)| {
			// Old fund account is deleted.
			let old_fund_account = old_fund_account_id::<T>(para_id);
			assert_eq!(frame_system::Account::<T>::get(old_fund_account_id), Default::default());

			// New fund account has the correct balance.
			let new_fund_account = Pallet::<T>::fund_account_id(fund.fund_index);
			let total_balance = T::Currency::total_balance(new_fund_account);

			assert_eq!(total_balance, fund.raised);
			assert!(total_balance > Zero::zero());
		});
	}
}
