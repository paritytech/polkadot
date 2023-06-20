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

use crate::{configuration, paras, scheduler_common::AssignmentProvider};
pub use pallet::*;
use primitives::{v5::Assignment, CoreIndex, Id as ParaId};

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config
		+ configuration::Config
		+ paras::Config
		+ crate::scheduler_parachains::Config
	{
	}
}

impl<T: Config> AssignmentProvider<T> for Pallet<T> {
	fn session_core_count() -> u32 {
		<crate::scheduler_parachains::Pallet<T>>::session_core_count()
		//+ <configuration::Pallet<T>>::config().parathread_cores
		//crate::scheduler_parathreads::Pallet<T>>::session_core_count()
	}

	fn pop_assignment_for_core(
		core_idx: CoreIndex,
		concluded_para: Option<ParaId>,
	) -> Option<Assignment> {
		let parachains_cores = <crate::scheduler_parachains::Pallet<T>>::session_core_count();
		if (0..parachains_cores).contains(&core_idx.0) {
			<crate::scheduler_parachains::Pallet<T>>::pop_assignment_for_core(
				core_idx,
				concluded_para,
			)
		} else {
			let _core_idx = CoreIndex(core_idx.0 - parachains_cores);
			todo!()
			//<crate::scheduler_parathreads::Pallet<T>>::pop_assignment_for_core(core_idx, concluded_para)
		}
	}

	fn push_assignment_for_core(core_idx: CoreIndex, assignment: Assignment) {
		let parachain_cores = <crate::scheduler_parachains::Pallet<T>>::session_core_count();
		if (0..parachain_cores).contains(&core_idx.0) {
			<crate::scheduler_parachains::Pallet<T>>::push_assignment_for_core(core_idx, assignment)
		} else {
			let _core_idx = CoreIndex(core_idx.0 - parachain_cores);
			todo!()
			//<crate::scheduler_parathreads::Pallet<T>>::push_assignment_for_core(
			//	core_idx, assignment,
			//)
		}
	}

	fn get_availability_period(core_idx: CoreIndex) -> T::BlockNumber {
		let parachains_cores = <crate::scheduler_parachains::Pallet<T>>::session_core_count();
		if (0..parachains_cores).contains(&core_idx.0) {
			<crate::scheduler_parachains::Pallet<T>>::get_availability_period(core_idx)
		} else {
			let _core_idx = CoreIndex(core_idx.0 - parachains_cores);
			todo!()
			//<crate::scheduler_parathreads::Pallet<T>>::get_availability_period(core_idx)
		}
	}

	fn get_max_retries(core_idx: CoreIndex) -> u32 {
		let parachains_cores = <crate::scheduler_parachains::Pallet<T>>::session_core_count();
		if (0..parachains_cores).contains(&core_idx.0) {
			<crate::scheduler_parachains::Pallet<T>>::get_max_retries(core_idx)
		} else {
			let _core_idx = CoreIndex(core_idx.0 - parachains_cores);
			todo!()
			//<crate::scheduler_parathreads::Pallet<T>>::get_max_retries(core_idx)
		}
	}
}
