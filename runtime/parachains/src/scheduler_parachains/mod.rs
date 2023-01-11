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

//! The scheduler module for parachains and parathreads.
//!
//! This module is responsible for two main tasks:
//!   - Partitioning validators into groups and assigning groups to parachains and parathreads
//!   - Scheduling parachains and parathreads
//!
//! It aims to achieve these tasks with these goals in mind:
//! - It should be possible to know at least a block ahead-of-time, ideally more,
//!   which validators are going to be assigned to which parachains.
//! - Parachains that have a candidate pending availability in this fork of the chain
//!   should not be assigned.
//! - Validator assignments should not be gameable. Malicious cartels should not be able to
//!   manipulate the scheduler to assign themselves as desired.
//! - High or close to optimal throughput of parachains and parathreads. Work among validator groups should be balanced.
//!
//! The Scheduler manages resource allocation using the concept of "Availability Cores".
//! There will be one availability core for each parachain, and a fixed number of cores
//! used for multiplexing parathreads. Validators will be partitioned into groups, with the same
//! number of groups as availability cores. Validator groups will be assigned to different availability cores
//! over time.

use frame_support::pallet_prelude::*;
use primitives::v2::{CoreIndex, CoreOccupied, GroupIndex, Id as ParaId, ScheduledCore};
use sp_runtime::traits::Saturating;
use sp_std::collections::btree_map::BTreeMap;

use crate::{
	configuration,
	initializer::SessionChangeNotification,
	paras,
	scheduler_common::{AssignmentKind, CoreAssignment, FreedReason},
};

use crate::scheduler_common::CoreAssigner;

//#[cfg(test)]
//mod tests;

pub struct ParachainsScheduler;
impl<T: crate::scheduler::pallet::Config> CoreAssigner<T> for ParachainsScheduler {
	fn session_cores() -> u32 {
		<paras::Pallet<T>>::parachains().len() as u32
	}

	fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		Weight::zero()
	}

	fn initializer_finalize() {}

	fn initializer_on_new_session(
		_notification: &SessionChangeNotification<T::BlockNumber>,
		_cores: &[Option<CoreOccupied>],
	) {
	}

	fn free_cores(
		_just_freed_cores: &BTreeMap<CoreIndex, FreedReason>,
		_cores: &[Option<CoreOccupied>],
	) {
	}

	fn make_core_assignment(core_idx: CoreIndex, group_idx: GroupIndex) -> Option<CoreAssignment> {
		let parachains = <paras::Pallet<T>>::parachains();

		Some(CoreAssignment {
			kind: AssignmentKind::Parachain,
			para_id: parachains[core_idx.0 as usize],
			core: core_idx,
			group_idx,
		})
	}

	fn clear(_scheduled: &[CoreAssignment]) {}

	fn core_para(core_index: CoreIndex, core_occupied: &CoreOccupied) -> ParaId {
		match core_occupied {
			CoreOccupied::Parachain => {
				let parachains = <paras::Pallet<T>>::parachains();
				parachains[core_index.0 as usize]
			},
			CoreOccupied::Parathread(_) => panic!("impossible"),
		}
	}

	fn availability_timeout_predicate(
		_core_index: CoreIndex,
		blocks_since_last_rotation: T::BlockNumber,
		pending_since: T::BlockNumber,
	) -> bool {
		let config = <configuration::Pallet<T>>::config();

		if blocks_since_last_rotation >= config.chain_availability_period {
			false // no pruning except recently after rotation.
		} else {
			let now = <frame_system::Pallet<T>>::block_number();
			now.saturating_sub(pending_since) >= config.chain_availability_period
		}
	}

	fn next_up_on_available(core_idx: CoreIndex) -> Option<ScheduledCore> {
		let parachains = <paras::Pallet<T>>::parachains();
		Some(ScheduledCore { para_id: parachains[core_idx.0 as usize], collator: None })
	}

	fn next_up_on_time_out(
		core_idx: CoreIndex,
		_cores: &[Option<CoreOccupied>],
	) -> Option<ScheduledCore> {
		let parachains = <paras::Pallet<T>>::parachains();
		Some(ScheduledCore { para_id: parachains[core_idx.0 as usize], collator: None })
	}
}
