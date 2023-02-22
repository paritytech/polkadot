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

use frame_support::pallet_prelude::*;
use primitives::{CoreIndex, CoreOccupied, Id as ParaId};
use scale_info::TypeInfo;
use sp_runtime::print;

use crate::{
	configuration,
	//initializer::SessionChangeNotification,
	paras,
	scheduler,
	scheduler_common::{Assignment, AssignmentProvider},
};

pub use pallet::*;
use sp_std::collections::btree_map::BTreeMap;

//#[cfg(test)]
//mod tests;

type SubIndex = u32;

#[derive(Encode, Decode, TypeInfo)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub enum ProviderCases {
	ParachainsProvider(SubIndex),
	ParathreadsProvicer(SubIndex),
}

pub type SubIndexMapping = BTreeMap<CoreIndex, ProviderCases>;

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

	#[pallet::storage]
	pub(crate) type ParaIdCoreMap<T> = StorageValue<_, BTreeMap<ParaId, CoreIndex>, ValueQuery>;
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
		print("pop_assignment_for_core polkadot");
		let parachains_cores = <crate::scheduler_parachains::Pallet<T>>::session_core_count();
		if (0..parachains_cores).contains(&core_idx.0) {
			print("parachains");
			<crate::scheduler_parachains::Pallet<T>>::pop_assignment_for_core(core_idx)
		} else {
			print("parathreads");
			let core_idx = CoreIndex(core_idx.0 - parachains_cores);
			<crate::scheduler_parathreads::Pallet<T>>::pop_assignment_for_core(core_idx)
		}
	}

	fn push_assignment_for_core(core_idx: CoreIndex, assignment: Assignment) {
		let parachains_cores = <crate::scheduler_parachains::Pallet<T>>::session_core_count();
		if (0..parachains_cores).contains(&core_idx.0) {
			<crate::scheduler_parachains::Pallet<T>>::push_assignment_for_core(core_idx, assignment)
		} else {
			let core_idx = CoreIndex(core_idx.0 - parachains_cores);
			<crate::scheduler_parathreads::Pallet<T>>::push_assignment_for_core(
				core_idx, assignment,
			)
		}
	}

	fn push_front_assignment_for_core(core_idx: CoreIndex, assignment: Assignment) {
		let parachains_cores = <crate::scheduler_parachains::Pallet<T>>::session_core_count();
		if (0..parachains_cores).contains(&core_idx.0) {
			<crate::scheduler_parachains::Pallet<T>>::push_front_assignment_for_core(
				core_idx, assignment,
			)
		} else {
			let core_idx = CoreIndex(core_idx.0 - parachains_cores);
			<crate::scheduler_parathreads::Pallet<T>>::push_front_assignment_for_core(
				core_idx, assignment,
			)
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

	fn clear(core_idx: CoreIndex, assignment: Assignment) {
		let parachains_cores = <crate::scheduler_parachains::Pallet<T>>::session_core_count();
		if (0..parachains_cores).contains(&core_idx.0) {
			<crate::scheduler_parachains::Pallet<T>>::clear(core_idx, assignment)
		} else {
			let core_idx = CoreIndex(core_idx.0 - parachains_cores);
			<crate::scheduler_parathreads::Pallet<T>>::clear(core_idx, assignment)
		}
	}
}
