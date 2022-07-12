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

//! A module that is responsible for migration of dmp storage.

use frame_support::{storage_alias, traits::StorageVersion, Twox64Concat};

/// The current storage version. 
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

pub mod v3 {
	use super::*;
	use crate::{
		configuration, dmp,
		dmp::{Config, InboundDownwardMessage},
		ParaId,
	};
	use frame_support::{
		ensure,
		pallet_prelude::ValueQuery,
		storage::migration::clear_storage_prefix,
		traits::{Get, PalletInfoAccess},
	};
	use primitives::v2::BlockNumber;
	use sp_std::vec::Vec;

	#[storage_alias]
	pub(crate) type DownwardMessageQueues<T: Config> = StorageMap<
		dmp::Pallet<T>,
		Twox64Concat,
		ParaId,
		Vec<InboundDownwardMessage<BlockNumber>>,
		ValueQuery,
	>;

	pub fn pre_migrate<T: Config>() -> Result<(), &'static str> {
		let count = DownwardMessageQueues::<T>::iter()
			.filter(|(para_id, messages)| {
				if messages.len() > 0 {
					log::debug!(
						target: "runtime",
						"dmp queue for para {} needs migration; message count = {}",
						u32::from(*para_id),
						messages.len()
					);
					true
				} else {
					false
				}
			})
			.count();

		log::info!(
			target: "runtime",
			"Migrating dmp storage for {} parachains",
			count,
		);

		Ok(())
	}

	/// This migration converts the storage to a new represatation which enables pagination.
	pub fn migrate<T: Config>() -> frame_support::weights::Weight {
		let config = <configuration::Pallet<T>>::config();
		let version = StorageVersion::get::<dmp::Pallet<T>>();

		log::info!(
			target: "runtime",
			"Version {:?} -> Version {:?}",
			version, STORAGE_VERSION
		);

		if version < STORAGE_VERSION {
			let weight = DownwardMessageQueues::<T>::iter()
				.map(|(para_id, messages)| {
					let mut weight = 0u64;
					for message in messages {
						let queue_weight =
							<dmp::Pallet<T>>::queue_downward_message(&config, para_id, message.msg)
								.expect("Failed to queue a message");
						weight = weight.saturating_add(
							queue_weight.saturating_add(T::DbWeight::get().reads(1)),
						);
					}
					weight
				})
				.sum::<u64>();

			STORAGE_VERSION.put::<dmp::Pallet<T>>();

			// Remove old queues, but we'll keep the MQC head around until collators receive their node/runtime upgrades.
			let results = clear_storage_prefix(
				<dmp::Pallet<T>>::name().as_bytes(),
				b"DownwardMessageQueues",
				&[],
				None,
				None,
			);

			weight.saturating_add(T::DbWeight::get().writes(results.unique as u64))
		} else {
			0
		}
	}

	pub fn post_migrate<T: Config>() -> Result<(), &'static str> {
		let old_storage_clean =
			DownwardMessageQueues::<T>::iter().any(|(_, messages)| messages.len() > 0);

		ensure!(!old_storage_clean, "Upgrade did not cleanup old storage");
		Ok(())
	}
}
