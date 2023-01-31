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
use primitives::{CoreIndex, CoreOccupied, Id as ParaId};
use scale_info::TypeInfo;

use crate::{
	configuration,
	//initializer::SessionChangeNotification,
	paras,
	scheduler,
	scheduler_common::{Assignment, AssignmentProvider},
};

pub use pallet::*;

//#[cfg(test)]
//mod tests;

type SubIndex = u32;

#[derive(Encode, Decode, TypeInfo)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub enum ProviderCases {
	ParachainsProvider(SubIndex),
	ParathreadsProvicer(SubIndex),
}

pub type SubIndexMapping = sp_std::collections::btree_map::BTreeMap<CoreIndex, ProviderCases>;

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
		+ crate::scheduler_parathreads::Config
		+ crate::scheduler_parachains::Config
	{
	}

	#[pallet::storage]
	pub(crate) type SubIndex<T> = StorageValue<_, SubIndexMapping, ValueQuery>;
}

impl<T: scheduler::pallet::Config> AssignmentProvider<T> for Pallet<T> {
	fn on_new_session(n_lookahead: u32) {
		<crate::scheduler_parachains::Pallet<T>>::on_new_session(n_lookahead);
		<crate::scheduler_parathreads::Pallet<T>>::on_new_session(n_lookahead);
	}

	fn session_core_count() -> u32 {
		<crate::scheduler_parachains::Pallet<T>>::session_core_count() +
			<crate::scheduler_parathreads::Pallet<T>>::session_core_count()
	}

	fn pop_assignment_for_core(core_idx: CoreIndex) -> Option<Assignment> {
		let parachains_cores = <crate::scheduler_parachains::Pallet<T>>::session_core_count();
		if (0..parachains_cores).contains(&core_idx.0) {
			<crate::scheduler_parachains::Pallet<T>>::pop_assignment_for_core(core_idx)
		} else {
			let core_idx = CoreIndex(core_idx.0 - parachains_cores);
			<crate::scheduler_parathreads::Pallet<T>>::pop_assignment_for_core(core_idx)
		}
	}

	fn core_para(core_idx: CoreIndex, core_occupied: &CoreOccupied) -> ParaId {
		let parachains_cores = <crate::scheduler_parachains::Pallet<T>>::session_core_count();
		if (0..parachains_cores).contains(&core_idx.0) {
			<crate::scheduler_parachains::Pallet<T>>::core_para(core_idx, core_occupied)
		} else {
			let core_idx = CoreIndex(core_idx.0 - parachains_cores);
			<crate::scheduler_parathreads::Pallet<T>>::core_para(core_idx, core_occupied)
		}
	}

	fn get_availability_period(core_idx: CoreIndex) -> T::BlockNumber {
		let parachains_cores = <crate::scheduler_parachains::Pallet<T>>::session_core_count();
		if (0..parachains_cores).contains(&core_idx.0) {
			<crate::scheduler_parachains::Pallet<T>>::get_availability_period(core_idx)
		} else {
			let core_idx = CoreIndex(core_idx.0 - parachains_cores);
			<crate::scheduler_parathreads::Pallet<T>>::get_availability_period(core_idx)
		}
	}
}
