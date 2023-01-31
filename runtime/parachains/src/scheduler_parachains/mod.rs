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

use primitives::{CoreIndex, CoreOccupied, Id as ParaId};

use crate::{
	configuration,
	//initializer::SessionChangeNotification,
	paras,
	scheduler_common::Assignment,
};

pub use pallet::*;

use crate::scheduler_common::AssignmentProvider;

//#[cfg(test)]
//mod tests;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config
		+ configuration::Config
		+ paras::Config
		+ crate::scheduler::pallet::Config
	{
	}
}

impl<T: crate::scheduler::pallet::Config> AssignmentProvider<T> for Pallet<T> {
	fn on_new_session(_n_lookahead: u32) {}

	fn session_core_count() -> u32 {
		<paras::Pallet<T>>::parachains().len() as u32
	}

	fn pop_assignment_for_core(core_idx: CoreIndex) -> Option<Assignment> {
		<paras::Pallet<T>>::parachains()
			.get(core_idx.0 as usize)
			.copied()
			.map(Assignment::Parachain)
	}

	fn core_para(core_idx: CoreIndex, core_occupied: &CoreOccupied) -> ParaId {
		match core_occupied {
			CoreOccupied::Parathread(_) => panic!("impossible"),
			CoreOccupied::Parachain => {
				let parachains = <paras::Pallet<T>>::parachains();
				parachains[core_idx.0 as usize]
			},
		}
	}

	fn get_availability_period(_core_idx: CoreIndex) -> T::BlockNumber {
		<configuration::Pallet<T>>::config().chain_availability_period
	}
}
