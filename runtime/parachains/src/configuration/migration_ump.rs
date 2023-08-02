// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! A module that is responsible for migration of UMP storage.

#![allow(unused_imports)] // Since we use features.

use crate::configuration::{self, ActiveConfig, Config, PendingConfigs, WeightInfo, LOG_TARGET};
use parity_scale_codec::{Decode, Encode};

pub mod latest {
	use super::*;
	use frame_support::{ensure, pallet_prelude::Weight, traits::OnRuntimeUpgrade};

	/// Force update the UMP limits in the parachain host config.
	// NOTE: `OnRuntimeUpgrade` does not have a `self`, so everything must be compile time.
	pub struct ScheduleConfigUpdate<
		T,
		const MAX_UPWARD_QUEUE_SIZE: u32,
		const MAX_UPWARD_QUEUE_COUNT: u32,
		const MAX_UPWARD_MESSAGE_SIZE: u32,
		const MAX_UPWARD_MESSAGE_NUM_PER_CANDIDATE: u32,
	>(core::marker::PhantomData<T>);

	impl<
			T: Config,
			const MAX_UPWARD_QUEUE_SIZE: u32,
			const MAX_UPWARD_QUEUE_COUNT: u32,
			const MAX_UPWARD_MESSAGE_SIZE: u32,
			const MAX_UPWARD_MESSAGE_NUM_PER_CANDIDATE: u32,
		> OnRuntimeUpgrade
		for ScheduleConfigUpdate<
			T,
			MAX_UPWARD_QUEUE_SIZE,
			MAX_UPWARD_QUEUE_COUNT,
			MAX_UPWARD_MESSAGE_SIZE,
			MAX_UPWARD_MESSAGE_NUM_PER_CANDIDATE,
		>
	{
		#[cfg(feature = "try-runtime")]
		fn pre_upgrade() -> Result<sp_std::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
			log::info!(target: LOG_TARGET, "pre_upgrade");
			let mut pending = PendingConfigs::<T>::get();
			pending.sort_by_key(|(s, _)| *s);

			log::info!(
				target: LOG_TARGET,
				"Active HostConfig:\n\n{:#?}\n",
				ActiveConfig::<T>::get()
			);
			log::info!(
				target: LOG_TARGET,
				"Last pending HostConfig upgrade:\n\n{:#?}\n",
				pending.last()
			);

			Ok((pending.len() as u32).encode())
		}

		fn on_runtime_upgrade() -> Weight {
			if let Err(e) = configuration::Pallet::<T>::schedule_config_update(|cfg| {
				cfg.max_upward_queue_size = MAX_UPWARD_QUEUE_SIZE;
				cfg.max_upward_queue_count = MAX_UPWARD_QUEUE_COUNT;
				cfg.max_upward_message_size = MAX_UPWARD_MESSAGE_SIZE;
				cfg.max_upward_message_num_per_candidate = MAX_UPWARD_MESSAGE_NUM_PER_CANDIDATE;
			}) {
				log::error!(
					target: LOG_TARGET,
					"\n!!!!!!!!!!!!!!!!!!!!!!!!\nFailed to schedule HostConfig upgrade: {:?}\n!!!!!!!!!!!!!!!!!!!!!!!!\n",
					e,
				);
			}

			T::WeightInfo::set_config_with_block_number()
		}

		#[cfg(feature = "try-runtime")]
		fn post_upgrade(state: sp_std::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
			log::info!(target: LOG_TARGET, "post_upgrade");
			let old_pending: u32 = Decode::decode(&mut &state[..]).expect("Known good");
			let mut pending = PendingConfigs::<T>::get();
			pending.sort_by_key(|(s, _)| *s);

			log::info!(
				target: LOG_TARGET,
				"Last pending HostConfig upgrade:\n\n{:#?}\n",
				pending.last()
			);
			let Some(last) = pending.last() else {
				return Err("There must be a new pending upgrade enqueued".into())
			};
			ensure!(
				pending.len() == old_pending as usize + 1,
				"There must be exactly one new pending upgrade enqueued"
			);
			if let Err(err) = last.1.check_consistency() {
				log::error!(target: LOG_TARGET, "Last PendingConfig is invalidity {:?}", err,);

				return Err("Pending upgrade must be sane but was not".into())
			}
			if let Err(err) = ActiveConfig::<T>::get().check_consistency() {
				log::error!(target: LOG_TARGET, "ActiveConfig is invalid: {:?}", err,);

				return Err("Active upgrade must be sane but was not".into())
			}

			Ok(())
		}
	}
}
