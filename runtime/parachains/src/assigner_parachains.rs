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

//! The bulk (parachain slot auction) blockspace assignment provider.
//! This provider is tightly coupled with the configuration and paras modules.

use crate::{configuration, paras, scheduler_common::AssignmentProvider};
use frame_system::pallet_prelude::BlockNumberFor;
pub use pallet::*;
use primitives::{v5::Assignment, CoreIndex, Id as ParaId};

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + configuration::Config + paras::Config {}
}

impl<T: Config> AssignmentProvider<BlockNumberFor<T>> for Pallet<T> {
	fn session_core_count() -> u32 {
		<paras::Pallet<T>>::parachains().len() as u32
	}

	fn pop_assignment_for_core(
		core_idx: CoreIndex,
		_concluded_para: Option<ParaId>,
	) -> Option<Assignment> {
		<paras::Pallet<T>>::parachains()
			.get(core_idx.0 as usize)
			.copied()
			.map(|para_id| Assignment::new(para_id))
	}

	/// Bulk assignment has no need to push the assignment back on a session change,
	/// this is a no-op in the case of a bulk assignment slot.
	fn push_assignment_for_core(_: CoreIndex, _: Assignment) {}

	fn get_availability_period(_: CoreIndex) -> BlockNumberFor<T> {
		<configuration::Pallet<T>>::config().paras_availability_period
	}

	/// There are retries set up in bulk assignment as the next slot already goes to
	/// same [`ParaId`].
	fn get_max_retries(_: CoreIndex) -> u32 {
		0
	}
}
