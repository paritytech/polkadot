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

use super::*;
use frame_support::{
	dispatch::GetStorageVersion,
	traits::{Contains, OnRuntimeUpgrade, StorageVersion},
};

#[derive(Encode, Decode)]
pub struct ParaInfoV1<Account, Balance> {
	manager: Account,
	deposit: Balance,
	locked: bool,
}

pub struct MigrateToV1<T, UnlockParaIds>(sp_std::marker::PhantomData<(T, UnlockParaIds)>);
impl<T: Config, UnlockParaIds: Contains<ParaId>> OnRuntimeUpgrade
	for MigrateToV1<T, UnlockParaIds>
{
	fn on_runtime_upgrade() -> Weight {
		let onchain_version = Pallet::<T>::on_chain_storage_version();

		if onchain_version == 0 {
			let mut count = 1u64; // storage version read write
			Paras::<T>::translate::<ParaInfoV1<T::AccountId, BalanceOf<T>>, _>(|key, v1| {
				count.saturating_inc();
				Some(ParaInfo {
					manager: v1.manager,
					deposit: v1.deposit,
					locked: if UnlockParaIds::contains(&key) { None } else { Some(v1.locked) },
				})
			});

			StorageVersion::new(1).put::<Pallet<T>>();
			log::info!(target: "runtime::registrar", "Upgraded {} storages to version 1", count);
			T::DbWeight::get().reads_writes(count, count)
		} else {
			log::info!(target: "runtime::registrar",  "Migration did not execute. This probably should be removed");
			T::DbWeight::get().reads(1)
		}
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
		Ok(Vec::new())
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(_: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
		Ok(())
	}
}
