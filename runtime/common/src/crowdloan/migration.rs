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
use frame_support::{storage_alias, Twox64Concat};

/// Migrations for using fund index to create fund accounts instead of para ID.
pub mod crowdloan_index_migration {
	use super::*;

	#[storage_alias]
	type NextTrieIndex<T: Config> = StorageValue<Pallet<T>, FundIndex>;

	#[storage_alias]
	type Leases<T: Config> = StorageMap<
		Slots,
		Twox64Concat,
		ParaId,
		Vec<Option<(<T as frame_system::Config>::AccountId, BalanceOf<T>)>>,
	>;

	// The old way we generated fund accounts.
	fn old_fund_account_id<T: Config>(index: ParaId) -> T::AccountId {
		T::PalletId::get().into_sub_account_truncating(index)
	}

	pub fn pre_migrate<T: Config>() -> Result<(), &'static str> {
		// `NextTrieIndex` should have a value.

		let next_index = NextTrieIndex::<T>::get().unwrap_or_default();
		ensure!(next_index > 0, "Next index is zero, which implies no migration is needed.");

		log::info!(
			target: "runtime",
			"next trie index: {:?}",
			next_index,
		);

		for (para_id, fund) in Funds::<T>::iter() {
			let old_fund_account = old_fund_account_id::<T>(para_id);
			let total_balance = CurrencyOf::<T>::total_balance(&old_fund_account);

			log::info!(
				target: "runtime",
				"para_id={:?}, old_fund_account={:?}, total_balance={:?}, fund.raised={:?}",
				para_id, old_fund_account, total_balance, fund.raised
			);

			// Each fund should have some non-zero balance.
			ensure!(
				total_balance >= fund.raised,
				"Total balance is not equal to the funds raised."
			);

			let leases = Leases::<T>::get(para_id).unwrap_or_default();
			let mut found_lease_deposit = false;
			for (who, _amount) in leases.iter().flatten() {
				if *who == old_fund_account {
					found_lease_deposit = true;
					break
				}
			}
			if found_lease_deposit {
				log::info!(
					target: "runtime",
					"para_id={:?}, old_fund_account={:?}, leases={:?}",
					para_id, old_fund_account, leases,
				);
			}
		}

		Ok(())
	}

	/// This migration converts crowdloans to use a crowdloan index rather than the parachain id as a
	/// unique identifier. This makes it easier to swap two crowdloans between parachains.
	pub fn migrate<T: Config>() -> frame_support::weights::Weight {
		let mut weight = Weight::zero();

		// First migrate `NextTrieIndex` counter to `NextFundIndex`.

		let next_index = NextTrieIndex::<T>::take().unwrap_or_default();
		NextFundIndex::<T>::set(next_index);

		weight = weight.saturating_add(T::DbWeight::get().reads_writes(1, 2));

		// Migrate all accounts from `old_fund_account` to `fund_account` using `fund_index`.
		for (para_id, fund) in Funds::<T>::iter() {
			let old_fund_account = old_fund_account_id::<T>(para_id);
			let new_fund_account = Pallet::<T>::fund_account_id(fund.fund_index);

			// Funds should only have a free balance and a reserve balance. Both of these are in the
			// `Account` storage item, so we just swap them.
			let account_info = frame_system::Account::<T>::take(&old_fund_account);
			frame_system::Account::<T>::insert(&new_fund_account, account_info);

			weight = weight.saturating_add(T::DbWeight::get().reads_writes(1, 2));

			let mut leases = Leases::<T>::get(para_id).unwrap_or_default();
			for (who, _amount) in leases.iter_mut().flatten() {
				if *who == old_fund_account {
					*who = new_fund_account.clone();
				}
			}

			Leases::<T>::insert(para_id, leases);
		}

		weight
	}

	pub fn post_migrate<T: Config>() -> Result<(), &'static str> {
		// `NextTrieIndex` should not have a value, and `NextFundIndex` should.
		ensure!(NextTrieIndex::<T>::get().is_none(), "NextTrieIndex still has a value.");

		let next_index = NextFundIndex::<T>::get();
		log::info!(
			target: "runtime",
			"next fund index: {:?}",
			next_index,
		);

		ensure!(
			next_index > 0,
			"NextFundIndex was not migrated or is zero. We assume it cannot be zero else no migration is needed."
		);

		// Each fund should have balance migrated correctly.
		for (para_id, fund) in Funds::<T>::iter() {
			// Old fund account is deleted.
			let old_fund_account = old_fund_account_id::<T>(para_id);
			ensure!(
				frame_system::Account::<T>::get(&old_fund_account) == Default::default(),
				"Old account wasn't reset to default value."
			);

			// New fund account has the correct balance.
			let new_fund_account = Pallet::<T>::fund_account_id(fund.fund_index);
			let total_balance = CurrencyOf::<T>::total_balance(&new_fund_account);

			ensure!(
				total_balance >= fund.raised,
				"Total balance in new account is different than the funds raised."
			);

			let leases = Leases::<T>::get(para_id).unwrap_or_default();
			let mut new_account_found = false;
			for (who, _amount) in leases.iter().flatten() {
				if *who == old_fund_account {
					panic!("Old fund account found after migration!");
				} else if *who == new_fund_account {
					new_account_found = true;
				}
			}
			if new_account_found {
				log::info!(
					target: "runtime::crowdloan",
					"para_id={:?}, new_fund_account={:?}, leases={:?}",
					para_id, new_fund_account, leases,
				);
			}
		}

		Ok(())
	}
}
