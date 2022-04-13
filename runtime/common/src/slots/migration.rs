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
use crate::crowdloan;
use sp_runtime::traits::AccountIdConversion;

/// Migrations for using fund index to create fund accounts instead of para ID.
pub mod slots_crowdloan_index_migration {
	use super::*;

	// The old way we generated fund accounts.
	fn old_fund_account_id<T: Config + crowdloan::Config>(index: ParaId) -> T::AccountId {
		<T as crowdloan::Config>::PalletId::get().into_sub_account(index)
	}

	pub fn pre_migrate<T: Config + crowdloan::Config>() -> Result<(), &'static str> {
		for (para_id, leases) in Leases::<T>::iter() {
			let old_fund_account = old_fund_account_id::<T>(para_id);

			for maybe_deposit in leases.iter() {
				if let Some((who, _amount)) = maybe_deposit {
					if *who == old_fund_account {
						let crowdloan =
							crowdloan::Funds::<T>::get(para_id).ok_or("no crowdloan found")?;
						log::info!(
							target: "runtime",
							"para_id={:?}, old_fund_account={:?}, fund_id={:?}, leases={:?}",
							para_id, old_fund_account, crowdloan.fund_index, leases,
						);
						break
					}
				}
			}
		}

		Ok(())
	}

	pub fn migrate<T: Config + crowdloan::Config>() -> frame_support::weights::Weight {
		let mut weight = 0;

		for (para_id, mut leases) in Leases::<T>::iter() {
			weight = weight.saturating_add(T::DbWeight::get().reads(2));
			// the para id must have a crowdloan
			if let Some(fund) = crowdloan::Funds::<T>::get(para_id) {
				let old_fund_account = old_fund_account_id::<T>(para_id);
				let new_fund_account = crowdloan::Pallet::<T>::fund_account_id(fund.fund_index);

				// look for places the old account is used, and replace with the new account.
				for maybe_deposit in leases.iter_mut() {
					if let Some((who, _amount)) = maybe_deposit {
						if *who == old_fund_account {
							*who = new_fund_account.clone();
						}
					}
				}

				// insert the changes.
				weight = weight.saturating_add(T::DbWeight::get().writes(1));
				Leases::<T>::insert(para_id, leases);
			}
		}

		weight
	}

	pub fn post_migrate<T: Config + crowdloan::Config>() -> Result<(), &'static str> {
		for (para_id, leases) in Leases::<T>::iter() {
			let old_fund_account = old_fund_account_id::<T>(para_id);
			log::info!(target: "runtime", "checking para_id: {:?}", para_id);
			// check the old fund account doesn't exist anywhere.
			for maybe_deposit in leases.iter() {
				if let Some((who, _amount)) = maybe_deposit {
					if *who == old_fund_account {
						panic!("old fund account found after migration!");
					}
				}
			}
		}
		Ok(())
	}
}
