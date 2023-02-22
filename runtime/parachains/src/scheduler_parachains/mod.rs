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

use primitives::{CoreIndex, CoreOccupied, Id as ParaId};
use sp_runtime::print;

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
		print("pop_assignment_for_core parachains");
		print(core_idx.0);
		<paras::Pallet<T>>::parachains()
			.get(core_idx.0 as usize)
			.copied()
			.map(Assignment::Parachain)
	}

	fn push_assignment_for_core(_: CoreIndex, _: Assignment) {}

	fn push_front_assignment_for_core(_: CoreIndex, _: Assignment) {}

	fn core_para(core_idx: CoreIndex, core_occupied: &CoreOccupied) -> ParaId {
		match core_occupied {
			CoreOccupied::Free => panic!("impossible"),
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

	fn clear(_core_idx: CoreIndex, _assignment: Assignment) {}
}
