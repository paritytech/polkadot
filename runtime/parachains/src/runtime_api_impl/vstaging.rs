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

use crate::{disputes, inclusion, initializer, scheduler};
use primitives::{vstaging, CandidateHash, CoreIndex, DisputeState, GroupIndex, SessionIndex};
use sp_runtime::traits::One;
use sp_std::prelude::*;

/// Implementation for `get_session_disputes` function from the runtime API
pub fn get_session_disputes<T: disputes::Config>(
) -> Vec<(SessionIndex, CandidateHash, DisputeState<T::BlockNumber>)> {
	<disputes::Pallet<T>>::disputes()
}

/// Implementation of `unapplied_slashes` runtime API
pub fn unapplied_slashes<T: disputes::slashing::Config>(
) -> Vec<(SessionIndex, CandidateHash, vstaging::slashing::PendingSlashes)> {
	<disputes::slashing::Pallet<T>>::unapplied_slashes()
}

/// Implementation of `submit_report_dispute_lost` runtime API
pub fn submit_unsigned_slashing_report<T: disputes::slashing::Config>(
	dispute_proof: vstaging::slashing::DisputeProof,
	key_ownership_proof: vstaging::slashing::OpaqueKeyOwnershipProof,
) -> Option<()> {
	let key_ownership_proof = key_ownership_proof.decode()?;

	<disputes::slashing::Pallet<T>>::submit_unsigned_slashing_report(
		dispute_proof,
		key_ownership_proof,
	)
}

/// Implementation for the `availability_cores_staging` function of the runtime API.
pub fn availability_cores<T: initializer::Config>(
) -> Vec<vstaging::CoreState<T::Hash, T::BlockNumber>> {
	let cores = <scheduler::Pallet<T>>::availability_cores();
	let now = <frame_system::Pallet<T>>::block_number() + One::one();

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
			vstaging::CoreOccupied::Paras(entry) => {
				let pending_availability =
					<inclusion::Pallet<T>>::pending_availability(entry.para_id())
						.expect("Occupied core always has pending availability; qed");

				let backed_in_number = *pending_availability.backed_in_number();
				let core_index = CoreIndex::from(i as u32);
				vstaging::CoreState::Occupied(vstaging::OccupiedCore {
					next_up_on_available: <scheduler::Pallet<T>>::next_up_on_available(core_index),
					occupied_since: backed_in_number,
					time_out_at: backed_in_number +
						<scheduler::Pallet<T>>::availability_period(core_index),
					next_up_on_time_out: <scheduler::Pallet<T>>::next_up_on_time_out(core_index),
					availability: pending_availability.availability_votes().clone(),
					group_responsible: group_responsible_for(
						backed_in_number,
						pending_availability.core_occupied(),
					),
					candidate_hash: pending_availability.candidate_hash(),
					candidate_descriptor: pending_availability.candidate_descriptor().clone(),
				})
			},
			vstaging::CoreOccupied::Free => vstaging::CoreState::Free,
		})
		.collect();

	// TODO: update to use claimqueue
	// This will overwrite only `Free` cores if the scheduler module is working as intended.
	for scheduled in <scheduler::Pallet<T>>::scheduled_claimqueue(now) {
		core_states[scheduled.core.0 as usize] =
			vstaging::CoreState::Scheduled(vstaging::ScheduledCore {
				para_id: scheduled.para_id(),
				collator_restrictions: scheduled.paras_entry.collator_restrictions().clone(),
			});
	}

	core_states
}
