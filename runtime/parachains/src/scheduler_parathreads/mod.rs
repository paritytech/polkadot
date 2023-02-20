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
use primitives::{CoreIndex, CoreOccupied, Id as ParaId, ParathreadClaim};
use scale_info::TypeInfo;

use crate::{
	configuration,
	//initializer::SessionChangeNotification,
	paras,
	scheduler_common::{Assignment, AssignmentProvider},
};

pub use pallet::*;

//#[cfg(test)]
//mod tests;

use sp_std::collections::vec_deque::VecDeque;

#[derive(Encode, Decode, TypeInfo)]
#[cfg_attr(test, derive(PartialEq, Debug))]
/// Bounded claim queue. Bounded by config.n_parathread_cores * config.n_lookahead
pub struct ClaimQueue {
	queue: VecDeque<ParathreadClaim>,
}

impl Default for ClaimQueue {
	fn default() -> Self {
		let queue = VecDeque::new(); // move this to scheduler
		Self { queue }
	}
}

impl ClaimQueue {
	//fn update(&mut self, n_lookahead: u32) {
	//	self.bound = n_lookahead + 1;
	//}

	fn add_claim(&mut self, claim: ParathreadClaim) {
		self.queue.push_back(claim);
	}

	fn pop_claim(&mut self) -> Option<ParathreadClaim> {
		self.queue.pop_front()
	}

	fn push_front_claim(&mut self, claim: ParathreadClaim) {
		self.queue.push_front(claim)
	}
}

pub trait ClaimQueueI<T: crate::scheduler::pallet::Config> {
	fn add_claim(&mut self, claim: ParathreadClaim) {
		<self::Pallet<T>>::add_claim(claim)
	}
}

impl<T: crate::scheduler::pallet::Config> AssignmentProvider<T> for Pallet<T> {
	fn on_new_session(n_lookahead: u32) {
		<self::Pallet<T>>::on_new_session(n_lookahead)
	}

	fn session_core_count() -> u32 {
		<self::Pallet<T>>::session_core_count()
	}

	fn pop_assignment_for_core(_core_idx: CoreIndex) -> Option<Assignment> {
		<self::Pallet<T>>::pop_assignment_for_core().map(Assignment::ParathreadA)
	}

	fn push_assignment_for_core(_core_idx: CoreIndex, assignment: Assignment) {
		match assignment {
			Assignment::ParathreadA(claim) => <self::Pallet<T>>::push_assignment_for_core(claim),
			Assignment::Parachain(_) => panic!("impossible"),
		}
	}

	fn push_front_assignment_for_core(_core_idx: CoreIndex, assignment: Assignment) {
		match assignment {
			Assignment::ParathreadA(claim) =>
				<self::Pallet<T>>::push_front_assignment_for_core_inner(claim),
			Assignment::Parachain(_) => panic!("impossible"),
		}
	}

	fn core_para(_core_idx: CoreIndex, core_occupied: &CoreOccupied) -> ParaId {
		match core_occupied {
			CoreOccupied::Free => panic!("impossible"),
			CoreOccupied::Parachain => panic!("impossible"),
			CoreOccupied::Parathread(x) => x.claim.0,
		}
	}

	fn get_availability_period(_core_idx: CoreIndex) -> T::BlockNumber {
		<configuration::Pallet<T>>::config().thread_availability_period
	}

	fn clear(core_idx: CoreIndex, assignment: Assignment) {
		Self::push_front_assignment_for_core(core_idx, assignment)
	}
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + configuration::Config + paras::Config {}

	/// A queue of upcoming claims.
	///
	/// The number of queued claims is bounded at `config.n_block_lookahead`
	/// multiplied by the number of parathread multiplexer cores. Reasonably, 10 * 50 = 500.
	#[pallet::storage]
	pub(crate) type ParathreadQueue<T> = StorageValue<_, ClaimQueue, ValueQuery>;
}

impl<T: Config> Pallet<T> {
	fn on_new_session(_n_lookahead: u32) {
		//ParathreadQueue::<T>::mutate(|queue| queue.update(n_lookahead))
	}

	fn session_core_count() -> u32 {
		let config = <configuration::Pallet<T>>::config();
		config.parathread_cores
	}

	fn pop_assignment_for_core() -> Option<ParathreadClaim> {
		ParathreadQueue::<T>::mutate(|queue| queue.pop_claim())
	}

	fn push_assignment_for_core(claim: ParathreadClaim) {
		ParathreadQueue::<T>::mutate(|queue| queue.add_claim(claim))
	}

	fn push_front_assignment_for_core_inner(claim: ParathreadClaim) {
		ParathreadQueue::<T>::mutate(|queue| queue.push_front_claim(claim))
	}

	pub(crate) fn add_claim(claim: ParathreadClaim) {
		ParathreadQueue::<T>::mutate(|queue| queue.add_claim(claim))
	}
}
