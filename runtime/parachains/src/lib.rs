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

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Runtime modules for parachains code.
//!
//! It is crucial to include all the modules from this crate in the runtime, in
//! particular the `Initializer` module, as it is responsible for initializing the state
//! of the other modules.

mod configuration;
mod inclusion;
mod inclusion_inherent;
mod initializer;
mod paras;
mod scheduler;
mod validity;

#[cfg(test)]
mod mock;

/// A module exporting runtime API implementation functions for all runtime APIs using v1
/// primitives.
///
/// Runtimes implementing the v1 runtime API are recommended to forward directly to these
/// functions.
pub mod runtime_api_impl_v1 {
	use primitives::v1::{
		ValidatorId, ValidatorIndex, GroupRotationInfo, CoreState, GlobalValidationSchedule,
		Id as ParaId, OccupiedCoreAssumption, LocalValidationData, SessionIndex, ValidationCode,
		CommittedCandidateReceipt, ScheduledCore, OccupiedCore, CoreOccupied, CoreIndex,
	};
	use sp_runtime::traits::{One, BlakeTwo256, Hash as HashT};
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

		let mut core_states: Vec<_> = cores.into_iter().enumerate().map(|(i, core)| match core {
			Some(occupied) => {
				CoreState::Occupied(match occupied {
					CoreOccupied::Parachain => {
						let para = parachains[i];
						let pending_availability = <inclusion::Module<T>>
							::pending_availability(para)
							.expect("Occupied core always has pending availability; qed");

						OccupiedCore {
							para,
							next_up_on_available: <scheduler::Module<T>>::next_up_on_available(
								CoreIndex(i as u32)
							),
							occupied_since: pending_availability.backed_in_number().clone(),
							time_out_at: time_out_at(
								pending_availability.backed_in_number().clone(),
								config.chain_availability_period,
							),
							next_up_on_time_out: <scheduler::Module<T>>::next_up_on_time_out(
								CoreIndex(i as u32)
							),
							availability: pending_availability.availability_votes().clone(),
						}
					}
					CoreOccupied::Parathread(p) => {
						let para = p.claim.0;
						let pending_availability = <inclusion::Module<T>>
							::pending_availability(para)
							.expect("Occupied core always has pending availability; qed");

						OccupiedCore {
							para,
							next_up_on_available: <scheduler::Module<T>>::next_up_on_available(
								CoreIndex(i as u32)
							),
							occupied_since: pending_availability.backed_in_number().clone(),
							time_out_at: time_out_at(
								pending_availability.backed_in_number().clone(),
								config.thread_availability_period,
							),
							next_up_on_time_out: <scheduler::Module<T>>::next_up_on_time_out(
								CoreIndex(i as u32)
							),
							availability: pending_availability.availability_votes().clone(),
						}
					}
				})
			}
			None => CoreState::Free,
		}).collect();

		for scheduled in <scheduler::Module<T>>::scheduled() {
			core_states[scheduled.core.0 as usize] = CoreState::Scheduled(ScheduledCore {
				para: scheduled.para_id,
				collator: scheduled.required_collator().map(|c| c.clone()),
			});
		}

		core_states
	}

	/// Implementation for the `global_validation_schedule` function of the runtime API.
	pub fn global_validation_schedule<T: initializer::Trait>()
		-> GlobalValidationSchedule<T::BlockNumber>
	{
		let config = <configuration::Module<T>>::config();
		GlobalValidationSchedule {
			max_code_size: config.max_code_size,
			max_head_data_size: config.max_head_data_size,
			block_number: <system::Module<T>>::block_number() - One::one(),
		}
	}

	/// Implementation for the `local_validation_data` function of the runtime API.
	pub fn local_validation_data<T: initializer::Trait>(
		para: ParaId,
		assumption: OccupiedCoreAssumption,
	) -> Option<LocalValidationData<T::BlockNumber>> {
		let construct = || {
			Some(LocalValidationData {
				parent_head: <paras::Module<T>>::para_head(&para)?,
				balance: 0,
				validation_code_hash: BlakeTwo256::hash_of(
					&<paras::Module<T>>::current_code(&para)?
				),
				code_upgrade_allowed: unimplemented!(), // TODO [now] add function in `paras` for this.
			})
		};

		match assumption {
			OccupiedCoreAssumption::Included => {
				<inclusion::Module<T>>::force_enact(para);
				construct()
			}
			OccupiedCoreAssumption::TimedOut => {
				construct()
			}
			OccupiedCoreAssumption::Free => {
				if <inclusion::Module<T>>::pending_availability(para).is_some() {
					None
				} else {
					construct()
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
		para: ParaId,
		assumption: OccupiedCoreAssumption,
	) -> Option<ValidationCode> {
		let fetch = || {
			<paras::Module<T>>::current_code(&para)
		};

		match assumption {
			OccupiedCoreAssumption::Included => {
				<inclusion::Module<T>>::force_enact(para);
				fetch()
			}
			OccupiedCoreAssumption::TimedOut => {
				fetch()
			}
			OccupiedCoreAssumption::Free => {
				if <inclusion::Module<T>>::pending_availability(para).is_some() {
					None
				} else {
					fetch()
				}
			}
		}
	}

	/// Implementation for the `candidate_pending_availability` function of the runtime API.
	pub fn candidate_pending_availability<T: initializer::Trait>(para: ParaId)
		-> Option<CommittedCandidateReceipt<T::Hash>>
	{
		<inclusion::Module<T>>::candidate_pending_availability(para)
	}
}
