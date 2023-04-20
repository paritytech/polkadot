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

use primitives::{CoreIndex, Id as ParaId};

use crate::{configuration, paras};

pub use pallet::*;
use primitives::v4::ParasEntry;

use crate::scheduler_common::AssignmentProvider;

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
		+ crate::scheduler::pallet::Config
	{
	}
}

impl<T: Config> AssignmentProvider<T> for Pallet<T> {
	fn session_core_count() -> u32 {
		<paras::Pallet<T>>::parachains().len() as u32
	}

	fn new_session() {}

	fn pop_assignment_for_core(
		core_idx: CoreIndex,
		_concluded_para: Option<ParaId>,
	) -> Option<ParasEntry> {
		<paras::Pallet<T>>::parachains()
			.get(core_idx.0 as usize)
			.copied()
			.map(|para_id| ParasEntry { para_id, collator: None, retries: 0 })
	}

	fn push_parasentry_for_core(_: CoreIndex, _: ParasEntry) {}

	fn get_availability_period(_: CoreIndex) -> T::BlockNumber {
		<configuration::Pallet<T>>::config().chain_availability_period
	}

	fn get_max_retries(_: CoreIndex) -> u32 {
		0
	}
}
