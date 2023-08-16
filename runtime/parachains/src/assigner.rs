// Copyright (C) Parity Technologies (UK) Ltd.
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

//! The Polkadot multiplexing assignment provider.
//! Provides blockspace assignments for both bulk and on demand parachains.
use frame_system::pallet_prelude::BlockNumberFor;
use primitives::{v5::Assignment, CoreIndex, Id as ParaId};

use crate::{
	configuration, paras,
	scheduler::common::{AssignmentProvider, AssignmentProviderConfig},
};

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + configuration::Config + paras::Config {
		type ParachainsAssignmentProvider: AssignmentProvider<BlockNumberFor<Self>>;
		type OnDemandAssignmentProvider: AssignmentProvider<BlockNumberFor<Self>>;
	}
}

// Aliases to make the impl more readable.
type ParachainAssigner<T> = <T as Config>::ParachainsAssignmentProvider;
type OnDemandAssigner<T> = <T as Config>::OnDemandAssignmentProvider;

impl<T: Config> Pallet<T> {
	// Helper fn for the AssignmentProvider implementation.
	// Assumes that the first allocation of cores is to bulk parachains.
	// This function will return false if there are no cores assigned to the bulk parachain
	// assigner.
	fn is_bulk_core(core_idx: &CoreIndex) -> bool {
		let parachain_cores =
			<ParachainAssigner<T> as AssignmentProvider<BlockNumberFor<T>>>::session_core_count();
		(0..parachain_cores).contains(&core_idx.0)
	}
}

impl<T: Config> AssignmentProvider<BlockNumberFor<T>> for Pallet<T> {
	fn session_core_count() -> u32 {
		let parachain_cores =
			<ParachainAssigner<T> as AssignmentProvider<BlockNumberFor<T>>>::session_core_count();
		let on_demand_cores =
			<OnDemandAssigner<T> as AssignmentProvider<BlockNumberFor<T>>>::session_core_count();

		parachain_cores.saturating_add(on_demand_cores)
	}

	/// Pops an `Assignment` from a specified `CoreIndex`
	fn pop_assignment_for_core(
		core_idx: CoreIndex,
		concluded_para: Option<ParaId>,
	) -> Option<Assignment> {
		if Pallet::<T>::is_bulk_core(&core_idx) {
			<ParachainAssigner<T> as AssignmentProvider<BlockNumberFor<T>>>::pop_assignment_for_core(
				core_idx,
				concluded_para,
			)
		} else {
			<OnDemandAssigner<T> as AssignmentProvider<BlockNumberFor<T>>>::pop_assignment_for_core(
				core_idx,
				concluded_para,
			)
		}
	}

	fn push_assignment_for_core(core_idx: CoreIndex, assignment: Assignment) {
		if Pallet::<T>::is_bulk_core(&core_idx) {
			<ParachainAssigner<T> as AssignmentProvider<BlockNumberFor<T>>>::push_assignment_for_core(
				core_idx, assignment,
			)
		} else {
			<OnDemandAssigner<T> as AssignmentProvider<BlockNumberFor<T>>>::push_assignment_for_core(
				core_idx, assignment,
			)
		}
	}

	fn get_provider_config(core_idx: CoreIndex) -> AssignmentProviderConfig<BlockNumberFor<T>> {
		if Pallet::<T>::is_bulk_core(&core_idx) {
			<ParachainAssigner<T> as AssignmentProvider<BlockNumberFor<T>>>::get_provider_config(
				core_idx,
			)
		} else {
			<OnDemandAssigner<T> as AssignmentProvider<BlockNumberFor<T>>>::get_provider_config(
				core_idx,
			)
		}
	}
}
