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

//! A module exporting runtime API implementation functions for all runtime APIs using v1
//! primitives.
//!
//! Runtimes implementing the v1 runtime API are recommended to forward directly to these
//! functions.

use primitives::v1::{
	ValidatorId, ValidatorIndex, GroupRotationInfo, CoreState, GlobalValidationData,
	Id as ParaId, OccupiedCoreAssumption, LocalValidationData, SessionIndex, ValidationCode,
	CommittedCandidateReceipt, ScheduledCore, OccupiedCore, CoreOccupied, CoreIndex,
	GroupIndex, CandidateEvent,
};
use sp_runtime::traits::Zero;
use frame_support::debug;
use crate::{initializer, inclusion, scheduler, configuration, paras};

/// Implementation for the `validators` function of the runtime API.
pub fn validators<T: initializer::Trait>() -> Vec<ValidatorId> {
	<inclusion::Module<T>>::validators()
}

/// Implementation for the `validator_groups` function of the runtime API.
pub fn validator_groups<T: initializer::Trait>() -> (
	Vec<Vec<ValidatorIndex>>,
	GroupRotationInfo<T::BlockNumber>,
) {
	let groups = <scheduler::Module<T>>::validator_groups();
	let rotation_info = <scheduler::Module<T>>::group_rotation_info();

	(groups, rotation_info)
}

/// Implementation for the `availability_cores` function of the runtime API.
pub fn availability_cores<T: initializer::Trait>() -> Vec<CoreState<T::BlockNumber>> {
	let cores = <scheduler::Module<T>>::availability_cores();
	let parachains = <paras::Module<T>>::parachains();
	let config = <configuration::Module<T>>::config();

	let rotation_info = <scheduler::Module<T>>::group_rotation_info();

	let time_out_at = |backed_in_number, availability_period| {
		let time_out_at = backed_in_number + availability_period;

		if rotation_info.group_rotation_frequency == Zero::zero() {
			return time_out_at;
		}

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

	let group_responsible_for = |backed_in_number, core_index| {
		match <scheduler::Module<T>>::group_assigned_to_core(core_index, backed_in_number) {
			Some(g) => g,
			None =>  {
				debug::warn!("Could not determine the group responsible for core extracted \
					from list of cores for some prior block in same session");

				GroupIndex(0)
			}
		}
	};

	let mut core_states: Vec<_> = cores.into_iter().enumerate().map(|(i, core)| match core {
		Some(occupied) => {
			CoreState::Occupied(match occupied {
				CoreOccupied::Parachain => {
					let para_id = parachains[i];
					let pending_availability = <inclusion::Module<T>>
						::pending_availability(para_id)
						.expect("Occupied core always has pending availability; qed");

					let backed_in_number = pending_availability.backed_in_number().clone();
					OccupiedCore {
						para_id,
						next_up_on_available: <scheduler::Module<T>>::next_up_on_available(
							CoreIndex(i as u32)
						),
						occupied_since: backed_in_number,
						time_out_at: time_out_at(
							backed_in_number,
							config.chain_availability_period,
						),
						next_up_on_time_out: <scheduler::Module<T>>::next_up_on_time_out(
							CoreIndex(i as u32)
						),
						availability: pending_availability.availability_votes().clone(),
						group_responsible: group_responsible_for(
							backed_in_number,
							pending_availability.core_occupied(),
						),
					}
				}
				CoreOccupied::Parathread(p) => {
					let para_id = p.claim.0;
					let pending_availability = <inclusion::Module<T>>
						::pending_availability(para_id)
						.expect("Occupied core always has pending availability; qed");

					let backed_in_number = pending_availability.backed_in_number().clone();
					OccupiedCore {
						para_id,
						next_up_on_available: <scheduler::Module<T>>::next_up_on_available(
							CoreIndex(i as u32)
						),
						occupied_since: backed_in_number,
						time_out_at: time_out_at(
							backed_in_number,
							config.thread_availability_period,
						),
						next_up_on_time_out: <scheduler::Module<T>>::next_up_on_time_out(
							CoreIndex(i as u32)
						),
						availability: pending_availability.availability_votes().clone(),
						group_responsible: group_responsible_for(
							backed_in_number,
							pending_availability.core_occupied(),
						),
					}
				}
			})
		}
		None => CoreState::Free,
	}).collect();

	// This will overwrite only `Free` cores if the scheduler module is working as intended.
	for scheduled in <scheduler::Module<T>>::scheduled() {
		core_states[scheduled.core.0 as usize] = CoreState::Scheduled(ScheduledCore {
			para_id: scheduled.para_id,
			collator: scheduled.required_collator().map(|c| c.clone()),
		});
	}

	core_states
}

/// Implementation for the `global_validation_data` function of the runtime API.
pub fn global_validation_data<T: initializer::Trait>()
	-> GlobalValidationData<T::BlockNumber>
{
	<configuration::Module<T>>::global_validation_data()
}

/// Implementation for the `local_validation_data` function of the runtime API.
pub fn local_validation_data<T: initializer::Trait>(
	para_id: ParaId,
	assumption: OccupiedCoreAssumption,
) -> Option<LocalValidationData<T::BlockNumber>> {
	match assumption {
		OccupiedCoreAssumption::Included => {
			<inclusion::Module<T>>::force_enact(para_id);
			<paras::Module<T>>::local_validation_data(para_id)
		}
		OccupiedCoreAssumption::TimedOut => {
			<paras::Module<T>>::local_validation_data(para_id)
		}
		OccupiedCoreAssumption::Free => {
			if <inclusion::Module<T>>::pending_availability(para_id).is_some() {
				None
			} else {
				<paras::Module<T>>::local_validation_data(para_id)
			}
		}
	}
}

/// Implementation for the `session_index_for_child` function of the runtime API.
pub fn session_index_for_child<T: initializer::Trait>() -> SessionIndex {
	// Just returns the session index from `inclusion`. Runtime APIs follow
	// initialization so the initializer will have applied any pending session change
	// which is expected at the child of the block whose context the runtime API was invoked
	// in.
	//
	// Incidentally, this is also the rationale for why it is OK to query validators or
	// occupied cores or etc. and expect the correct response "for child".
	<inclusion::Module<T>>::session_index()
}

/// Implementation for the `validation_code` function of the runtime API.
pub fn validation_code<T: initializer::Trait>(
	para_id: ParaId,
	assumption: OccupiedCoreAssumption,
) -> Option<ValidationCode> {
	let fetch = || {
		<paras::Module<T>>::current_code(&para_id)
	};

	match assumption {
		OccupiedCoreAssumption::Included => {
			<inclusion::Module<T>>::force_enact(para_id);
			fetch()
		}
		OccupiedCoreAssumption::TimedOut => {
			fetch()
		}
		OccupiedCoreAssumption::Free => {
			if <inclusion::Module<T>>::pending_availability(para_id).is_some() {
				None
			} else {
				fetch()
			}
		}
	}
}

/// Implementation for the `candidate_pending_availability` function of the runtime API.
pub fn candidate_pending_availability<T: initializer::Trait>(para_id: ParaId)
	-> Option<CommittedCandidateReceipt<T::Hash>>
{
	<inclusion::Module<T>>::candidate_pending_availability(para_id)
}

/// Implementation for the `candidate_events` function of the runtime API.
// NOTE: this runs without block initialization, as it accesses events.
// this means it can run in a different session than other runtime APIs at the same block.
pub fn candidate_events<T: initializer::Trait>(
	extract_event: impl Fn(<T as system::Trait>::Event) -> Option<inclusion::Event<T>>,
) -> Vec<CandidateEvent<T::Hash>> {
	use inclusion::Event as RawEvent;

	<system::Module<T>>::events().into_iter()
		.filter_map(|record| extract_event(record.event))
		.map(|event| match event {
			RawEvent::<T>::CandidateBacked(c, h) => CandidateEvent::CandidateBacked(c, h),
			RawEvent::<T>::CandidateIncluded(c, h) => CandidateEvent::CandidateIncluded(c, h),
			RawEvent::<T>::CandidateTimedOut(c, h) => CandidateEvent::CandidateTimedOut(c, h),
		})
		.collect()
}
