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

//! A module that is responsible for migration of DMP storage.

use frame_support::{
	ensure,
	pallet_prelude::ValueQuery,
	storage::migration::clear_storage_prefix,
	storage_alias,
	traits::{Get, PalletInfoAccess, StorageVersion},
	Twox64Concat,
};

use primitives::v2::BlockNumber;
use sp_std::vec::Vec;

/// The current storage version.
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

pub mod v0 {
	use super::*;

	pub use primitives::v2::{Id as ParaId, InboundDownwardMessage};

	#[storage_alias]
	pub(crate) type DownwardMessageQueues<T: crate::dmp::Config> = StorageMap<
		crate::dmp::Pallet<T>,
		Twox64Concat,
		ParaId,
		Vec<InboundDownwardMessage<BlockNumber>>,
		ValueQuery,
	>;
}
pub mod v1 {
	use super::*;
	use crate::{configuration, dmp, dmp::Config};

	pub fn pre_migrate<T: Config>() -> Result<(), &'static str> {
		let count = v0::DownwardMessageQueues::<T>::iter()
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

	/// This migration converts the storage to a new representation which enables pagination.
	pub fn migrate<T: Config>() -> frame_support::weights::Weight {
		let config = <configuration::Pallet<T>>::config();
		let version = StorageVersion::get::<dmp::Pallet<T>>();

		log::info!(
			target: "runtime",
			"Version {:?} -> Version {:?}",
			version, STORAGE_VERSION
		);

		if version < STORAGE_VERSION {
			let weight = v0::DownwardMessageQueues::<T>::iter()
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
			v0::DownwardMessageQueues::<T>::iter().any(|(_, messages)| messages.len() > 0);

		ensure!(!old_storage_clean, "Upgrade did not cleanup old storage");
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{new_test_ext, MockGenesisConfig, Test};
	use parity_scale_codec::Encode;
	use primitives::v2::{Id as ParaId, MessageQueueChain};
	use sp_runtime::traits::{BlakeTwo256, Hash};

	fn default_genesis_config() -> MockGenesisConfig {
		MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: crate::configuration::HostConfiguration {
					max_downward_message_size: 1024,
					..Default::default()
				},
			},
			..Default::default()
		}
	}

	#[test]
	fn test_migrate_to_v1() {
		let messages = (0..100)
			.map(|v| v0::InboundDownwardMessage {
				sent_at: v,
				msg: format!("message {}", v).encode(),
			})
			.collect::<Vec<_>>();

		let mut mqc = MessageQueueChain::default();

		for (index, msg) in messages.iter().enumerate() {
			mqc = mqc.extend(index as u32, BlakeTwo256::hash_of(&msg.msg));
		}

		new_test_ext(default_genesis_config()).execute_with(|| {
			// Write queues for a few parachains.
			for para_id in 0..40 {
				frame_support::storage::unhashed::put_raw(
					&v0::DownwardMessageQueues::<Test>::hashed_key_for(v0::ParaId::from(para_id)),
					&messages.encode(),
				);
			}

			v1::migrate::<Test>();
			v1::post_migrate::<Test>().unwrap();

			// Iterate all paras after migration and check mqc of all messages.
			for para_id in 0..40 {
				// Get the first 100 pages of messages.
				let all_messages =
					<crate::dmp::Pallet<Test>>::dmq_contents_bounded(ParaId::from(para_id), 0, 100);

				let mut migrated_mqc = MessageQueueChain::default();

				for (index, msg) in all_messages.into_iter().enumerate() {
					migrated_mqc =
						migrated_mqc.extend(index as u32, BlakeTwo256::hash_of(&msg.msg));
				}

				assert_eq!(migrated_mqc.head(), mqc.head())
			}
		});
	}
}
