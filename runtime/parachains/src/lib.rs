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
		CommittedCandidateReceipt, ScheduledCore, OccupiedCore, CoreOccupied,
	};
	use sp_runtime::traits::One;
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
		let session_start_block = <scheduler::Module<T>>::session_start_block();
		let now = <system::Module<T>>::block_number();
		let group_rotation_frequency = <configuration::Module<T>>::config()
			.parachain_rotation_frequency;

		let rotation_info = GroupRotationInfo {
			session_start_block,
			now,
			group_rotation_frequency,
		};

		(groups, rotation_info)
	}

	/// Implementation for the `availability_cores` function of the runtime API.
	pub fn availability_cores<T: initializer::Trait>() -> Vec<CoreState<T::BlockNumber>> {
		let cores = <scheduler::Module<T>>::availability_cores();
		let parachains = <paras::Module<T>>::parachains();

		let mut core_states: Vec<_> = cores.into_iter().enumerate().map(|(i, core)| match core {
			Some(occupied) => {
				// TODO [now]: flesh out
				CoreState::Occupied(match occupied {
					CoreOccupied::Parachain => {
						let para = parachains[i];
						let next_up = ScheduledCore {
							para,
							collator: None,
						};

						OccupiedCore {
							para,
							next_up_on_available: Some(next_up.clone()),
							occupied_since: unimplemented!(),
							time_out_at: unimplemented!(),
							next_up_on_time_out: Some(next_up.clone()),
							availability: unimplemented!(),
						}
					}
					CoreOccupied::Parathread(p) => {
						OccupiedCore {
							para: p.claim.0,
							next_up_on_available: unimplemented!(),
							occupied_since: unimplemented!(),
							time_out_at: unimplemented!(),
							next_up_on_time_out: unimplemented!(),
							availability: unimplemented!(),
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
		match assumption {
			OccupiedCoreAssumption::Included => {
				// TODO [now]: enact candidate from inclusion module. then construct based on
				// paras module.
				unimplemented!()
			}
			OccupiedCoreAssumption::TimedOut => {
				// TODO [now]: can just construct based on what is in paras module.
				unimplemented!()
			}
			OccupiedCoreAssumption::Free => {
				// TODO [now]: check that the para has no candidate pending availability.
				unimplemented!()
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
		match assumption {
			OccupiedCoreAssumption::Included => {
				// TODO [now]: enact candidate from inclusion module. then construct based on
				// paras module.
				unimplemented!()
			}
			OccupiedCoreAssumption::TimedOut => {
				// TODO [now]: can just construct based on what is in paras module.
				unimplemented!()
			}
			OccupiedCoreAssumption::Free => {
				// TODO [now]: check that the para has no candidate pending availability.
				unimplemented!()
			}
		}
	}

	/// Implementation for the `candidate_pending_availability` function of the runtime API.
	pub fn candidate_pending_availability<T: initializer::Trait>(para: ParaId)
		-> Option<CommittedCandidateReceipt<T::Hash>>
	{
		// TODO [now] draw out from inclusion module.
		unimplemented!()
	}
}
