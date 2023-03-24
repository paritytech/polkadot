// Copyright 2023 Parity Technologies (UK) Ltd.
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
use primitives::{CoreIndex, Id as ParaId};

use crate::{
	configuration,
	//initializer::SessionChangeNotification,
	paras,
	scheduler_common::{Assignment, AssignmentProvider},
};

pub use pallet::*;

//#[cfg(test)]
//mod tests;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + configuration::Config + paras::Config {
		type ParachainsAssignmentProvider: AssignmentProvider<Self::BlockNumber>;
		type OnDemandAssignmentProvider: AssignmentProvider<Self::BlockNumber>;
	}

	#[pallet::storage]
	pub(crate) type NumParachains<T> = StorageValue<_, Option<u32>, ValueQuery>;
}

// Aliases to make the impl more readable.
type ParachainAssigner<T> = <T as Config>::ParachainsAssignmentProvider;
type OnDemandAssigner<T> = <T as Config>::OnDemandAssignmentProvider;

impl<T: Config> AssignmentProvider<T::BlockNumber> for Pallet<T> {
	fn session_core_count() -> u32 {
		let parachain_cores =
			<ParachainAssigner<T> as AssignmentProvider<T::BlockNumber>>::session_core_count();
		let on_demand_cores =
			<OnDemandAssigner<T> as AssignmentProvider<T::BlockNumber>>::session_core_count();

		parachain_cores.saturating_add(on_demand_cores)
	}

	fn new_session() {
		let n_parachains =
			<ParachainAssigner<T> as AssignmentProvider<T::BlockNumber>>::session_core_count();
		NumParachains::<T>::mutate(|val| *val = Some(n_parachains));
	}

	fn pop_assignment_for_core(
		core_idx: CoreIndex,
		concluded_para: Option<ParaId>,
	) -> Option<Assignment> {
		let parachain_cores =
			<ParachainAssigner<T> as AssignmentProvider<T::BlockNumber>>::session_core_count();

		if (0..parachain_cores).contains(&core_idx.0) {
			<ParachainAssigner<T> as AssignmentProvider<T::BlockNumber>>::pop_assignment_for_core(
				core_idx,
				concluded_para,
			)
		} else {
			<OnDemandAssigner<T> as AssignmentProvider<T::BlockNumber>>::pop_assignment_for_core(
				core_idx,
				concluded_para,
			)
		}
	}

	fn push_assignment_for_core(core_idx: CoreIndex, assignment: Assignment) {
		let parachain_cores = NumParachains::<T>::get().unwrap_or_else(|| {
			<ParachainAssigner<T> as AssignmentProvider<T::BlockNumber>>::session_core_count()
		});
		if (0..parachain_cores).contains(&core_idx.0) {
			<ParachainAssigner<T> as AssignmentProvider<T::BlockNumber>>::push_assignment_for_core(
				core_idx, assignment,
			)
		} else {
			<OnDemandAssigner<T> as AssignmentProvider<T::BlockNumber>>::push_assignment_for_core(
				core_idx, assignment,
			)
		}
	}

	fn get_availability_period(core_idx: CoreIndex) -> T::BlockNumber {
		let parachain_cores =
			<ParachainAssigner<T> as AssignmentProvider<T::BlockNumber>>::session_core_count();

		if (0..parachain_cores).contains(&core_idx.0) {
			<ParachainAssigner<T> as AssignmentProvider<T::BlockNumber>>::get_availability_period(
				core_idx,
			)
		} else {
			<OnDemandAssigner<T> as AssignmentProvider<T::BlockNumber>>::get_availability_period(
				core_idx,
			)
		}
	}
}
