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

//! Put implementations of functions from staging APIs here.

use crate::{configuration, inclusion, initializer, scheduler};
use primitives::{
	vstaging::{CoreOccupied, CoreState, OccupiedCore, ScheduledCore},
	CoreIndex, GroupIndex,
};
use sp_runtime::traits::One;
use sp_std::prelude::*;

/// Implementation for the `availability_cores_staging` function of the runtime API.
pub fn availability_cores<T: initializer::Config>() -> Vec<CoreState<T::Hash, T::BlockNumber>> {
	let cores = <scheduler::Pallet<T>>::availability_cores();
	let config = <configuration::Pallet<T>>::config();
	let now = <frame_system::Pallet<T>>::block_number() + One::one();
	let rotation_info = <scheduler::Pallet<T>>::group_rotation_info(now);

	let time_out_at = |backed_in_number, availability_period| {
		let time_out_at = backed_in_number + availability_period;

		let current_window = rotation_info.last_rotation_at() + availability_period;
		let next_rotation = rotation_info.next_rotation_at();

		// If we are within `period` blocks of rotation, timeouts are being checked
		// actively. We could even time out this block.
		if time_out_at < current_window {
			time_out_at
		} else if time_out_at <= next_rotation {
			// Otherwise, it will time out at the sooner of the next rotation
			next_rotation
		} else {
			// or the scheduled time-out. This is by definition within `period` blocks
			// of `next_rotation` and is thus a valid timeout block.
			time_out_at
		}
	};

	let group_responsible_for =
		|backed_in_number, core_index| match <scheduler::Pallet<T>>::group_assigned_to_core(
			core_index,
			backed_in_number,
		) {
			Some(g) => g,
			None => {
				log::warn!(
					target: "runtime::polkadot-api::v2",
					"Could not determine the group responsible for core extracted \
					from list of cores for some prior block in same session",
				);

				GroupIndex(0)
			},
		};

	let mut core_states: Vec<_> = cores
		.into_iter()
		.enumerate()
		.map(|(i, core)| match core {
			CoreOccupied::Paras(entry) => {
				let pending_availability =
					<inclusion::Pallet<T>>::pending_availability(entry.para_id())
						.expect("Occupied core always has pending availability; qed");

				let backed_in_number = *pending_availability.backed_in_number();
				CoreState::Occupied(OccupiedCore {
					next_up_on_available: <scheduler::Pallet<T>>::next_up_on_available(CoreIndex(
						i as u32,
					)),
					occupied_since: backed_in_number,
					time_out_at: time_out_at(backed_in_number, config.chain_availability_period),
					next_up_on_time_out: <scheduler::Pallet<T>>::next_up_on_time_out(CoreIndex(
						i as u32,
					)),
					availability: pending_availability.availability_votes().clone(),
					group_responsible: group_responsible_for(
						backed_in_number,
						pending_availability.core_occupied(),
					),
					candidate_hash: pending_availability.candidate_hash(),
					candidate_descriptor: pending_availability.candidate_descriptor().clone(),
				})
			},
			CoreOccupied::Free => CoreState::Free,
		})
		.collect();

	// TODO: update to use claimqueue
	// This will overwrite only `Free` cores if the scheduler module is working as intended.
	for scheduled in <scheduler::Pallet<T>>::scheduled_claimqueue(now) {
		core_states[scheduled.core.0 as usize] = CoreState::Scheduled(ScheduledCore {
			para_id: scheduled.para_id(),
			collator_restrictions: scheduled.paras_entry.collator_restrictions().clone(),
		});
	}

	core_states
}
