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
use primitives::{
	CollatorId, CoreIndex, CoreOccupied, GroupIndex, Id as ParaId, ParathreadClaim,
	ParathreadEntry, ScheduledCore,
};
use scale_info::TypeInfo;
use sp_std::prelude::*;

use crate::initializer::SessionChangeNotification;

//#[cfg(test)]
//mod tests;

/// Reasons a core might be freed
#[derive(Clone, Copy)]
pub enum FreedReason {
	/// The core's work concluded and the parablock assigned to it is considered available.
	Concluded,
	/// The core's work timed out.
	TimedOut,
}

/// The assignment type.
#[derive(Clone, Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub enum AssignmentKind {
	/// A parachain.
	Parachain,
	/// A parathread.
	Parathread(CollatorId, u32),
}

/// How a free core is scheduled to be assigned.
#[derive(Clone, Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub struct CoreAssignment {
	/// The core that is assigned.
	pub core: CoreIndex,
	/// The unique ID of the para that is assigned to the core.
	pub para_id: ParaId,
	/// The kind of the assignment.
	pub kind: AssignmentKind,
	/// The index of the validator group assigned to the core.
	pub group_idx: GroupIndex,
}

impl CoreAssignment {
	/// Get the ID of a collator who is required to collate this block.
	pub fn required_collator(&self) -> Option<&CollatorId> {
		match self.kind {
			AssignmentKind::Parachain => None,
			AssignmentKind::Parathread(ref id, _) => Some(id),
		}
	}

	/// Get the `CoreOccupied` from this.
	pub fn to_core_occupied(&self) -> CoreOccupied {
		match self.kind {
			AssignmentKind::Parachain => CoreOccupied::Parachain,
			AssignmentKind::Parathread(ref collator, retries) =>
				CoreOccupied::Parathread(ParathreadEntry {
					claim: ParathreadClaim(self.para_id, collator.clone()),
					retries,
				}),
		}
	}
}

// TODO: need to link CoreOccupied variants to CoreAssigner impls
// TOOD: Change interface to remove CoreIndex and GroupIndex to not leak abstraction
pub trait CoreAssigner<T: crate::scheduler::pallet::Config> {
	fn session_core_count() -> u32;

	fn is_my_core(core_index: CoreIndex, offset: u32) -> bool {
		let cores = Self::session_core_count();
		(offset..offset + cores).contains(&(core_index.0 as u32))
	}

	fn initializer_initialize(now: T::BlockNumber) -> Weight;

	fn initializer_finalize();

	fn initializer_on_new_session(
		notification: &SessionChangeNotification<T::BlockNumber>,
		cores: &[Option<CoreOccupied>],
	);

	fn free_cores(just_freed_cores: &[(CoreOccupied, FreedReason)]);

	fn make_core_assignment(core_idx: CoreIndex, group_idx: GroupIndex) -> Option<CoreAssignment>;

	fn clear(scheduled: &[CoreAssignment]);

	fn core_para(core_index: CoreIndex, core_occupied: &CoreOccupied) -> ParaId;

	fn availability_timeout_predicate(
		core_index: CoreIndex,
		blocks_since_last_rotation: T::BlockNumber,
		pending_since: T::BlockNumber,
	) -> bool;

	fn next_up_on_available(core_idx: CoreIndex) -> Option<ScheduledCore>;

	fn next_up_on_time_out(
		core_idx: CoreIndex,
		cores: &[Option<CoreOccupied>],
	) -> Option<ScheduledCore>;
}

// NOTE: Does NOT work for nested tuples a la (A, (B, C)), yet
impl<A, B, T> CoreAssigner<T> for (A, B)
where
	T: crate::scheduler::pallet::Config,
	A: CoreAssigner<T>,
	B: CoreAssigner<T>,
{
	fn session_core_count() -> u32 {
		A::session_core_count() + B::session_core_count()
	}

	fn initializer_initialize(now: T::BlockNumber) -> Weight {
		A::initializer_initialize(now).max(B::initializer_initialize(now))
	}

	fn initializer_finalize() {
		A::initializer_finalize();
		B::initializer_finalize();
	}

	fn initializer_on_new_session(
		notification: &SessionChangeNotification<T::BlockNumber>,
		cores: &[Option<CoreOccupied>],
	) {
		A::initializer_on_new_session(notification, cores);
		B::initializer_on_new_session(notification, cores);
	}

	fn free_cores(just_freed_cores: &[(CoreOccupied, FreedReason)]) {
		A::free_cores(just_freed_cores);
		B::free_cores(just_freed_cores);
	}

	fn make_core_assignment(core_idx: CoreIndex, group_idx: GroupIndex) -> Option<CoreAssignment> {
		if A::is_my_core(core_idx, 0) {
			A::make_core_assignment(core_idx, group_idx)
		} else {
			B::make_core_assignment(core_idx, group_idx)
		}
	}

	fn clear(scheduled: &[CoreAssignment]) {
		A::clear(scheduled);
		B::clear(scheduled);
	}

	fn core_para(core_idx: CoreIndex, core_occupied: &CoreOccupied) -> ParaId {
		if A::is_my_core(core_idx, 0) {
			A::core_para(core_idx, core_occupied)
		} else {
			B::core_para(core_idx, core_occupied)
		}
	}

	fn availability_timeout_predicate(
		core_idx: CoreIndex,
		blocks_since_last_rotation: T::BlockNumber,
		pending_since: T::BlockNumber,
	) -> bool {
		if A::is_my_core(core_idx, 0) {
			A::availability_timeout_predicate(core_idx, blocks_since_last_rotation, pending_since)
		} else {
			B::availability_timeout_predicate(core_idx, blocks_since_last_rotation, pending_since)
		}
	}

	fn next_up_on_available(core_idx: CoreIndex) -> Option<ScheduledCore> {
		if A::is_my_core(core_idx, 0) {
			A::next_up_on_available(core_idx)
		} else {
			B::next_up_on_available(core_idx)
		}
	}

	fn next_up_on_time_out(
		core_idx: CoreIndex,
		cores: &[Option<CoreOccupied>],
	) -> Option<ScheduledCore> {
		if A::is_my_core(core_idx, 0) {
			A::next_up_on_time_out(core_idx, cores)
		} else {
			B::next_up_on_time_out(core_idx, cores)
		}
	}
}

// NOTE: This is terrible. Better generalise (A, B) so we can use it for nested tuples like (A, (B, C))
//		 instead of having a number of flat tuple impls
impl<A, B, C, T> CoreAssigner<T> for (A, B, C)
where
	T: crate::scheduler::pallet::Config,
	A: CoreAssigner<T>,
	B: CoreAssigner<T>,
	C: CoreAssigner<T>,
{
	fn session_core_count() -> u32 {
		A::session_core_count() + B::session_core_count() + C::session_core_count()
	}

	fn initializer_initialize(now: T::BlockNumber) -> Weight {
		A::initializer_initialize(now)
			.max(B::initializer_initialize(now))
			.max(C::initializer_initialize(now))
	}

	fn initializer_finalize() {
		A::initializer_finalize();
		B::initializer_finalize();
		C::initializer_finalize();
	}

	fn initializer_on_new_session(
		notification: &SessionChangeNotification<T::BlockNumber>,
		cores: &[Option<CoreOccupied>],
	) {
		A::initializer_on_new_session(notification, cores);
		B::initializer_on_new_session(notification, cores);
		C::initializer_on_new_session(notification, cores);
	}

	fn free_cores(just_freed_cores: &[(CoreOccupied, FreedReason)]) {
		A::free_cores(just_freed_cores);
		B::free_cores(just_freed_cores);
		C::free_cores(just_freed_cores);
	}

	fn make_core_assignment(core_idx: CoreIndex, group_idx: GroupIndex) -> Option<CoreAssignment> {
		if A::is_my_core(core_idx, 0) {
			A::make_core_assignment(core_idx, group_idx)
		} else if B::is_my_core(core_idx, A::session_core_count()) {
			B::make_core_assignment(core_idx, group_idx)
		} else {
			C::make_core_assignment(core_idx, group_idx)
		}
	}

	fn clear(scheduled: &[CoreAssignment]) {
		A::clear(scheduled);
		B::clear(scheduled);
		C::clear(scheduled);
	}

	fn core_para(core_idx: CoreIndex, core_occupied: &CoreOccupied) -> ParaId {
		if A::is_my_core(core_idx, 0) {
			A::core_para(core_idx, core_occupied)
		} else if B::is_my_core(core_idx, A::session_core_count()) {
			B::core_para(core_idx, core_occupied)
		} else {
			C::core_para(core_idx, core_occupied)
		}
	}

	fn availability_timeout_predicate(
		core_idx: CoreIndex,
		blocks_since_last_rotation: T::BlockNumber,
		pending_since: T::BlockNumber,
	) -> bool {
		if A::is_my_core(core_idx, 0) {
			A::availability_timeout_predicate(core_idx, blocks_since_last_rotation, pending_since)
		} else if B::is_my_core(core_idx, A::session_core_count()) {
			B::availability_timeout_predicate(core_idx, blocks_since_last_rotation, pending_since)
		} else {
			C::availability_timeout_predicate(core_idx, blocks_since_last_rotation, pending_since)
		}
	}

	fn next_up_on_available(core_idx: CoreIndex) -> Option<ScheduledCore> {
		if A::is_my_core(core_idx, 0) {
			A::next_up_on_available(core_idx)
		} else if B::is_my_core(core_idx, A::session_core_count()) {
			B::next_up_on_available(core_idx)
		} else {
			C::next_up_on_available(core_idx)
		}
	}

	fn next_up_on_time_out(
		core_idx: CoreIndex,
		cores: &[Option<CoreOccupied>],
	) -> Option<ScheduledCore> {
		if A::is_my_core(core_idx, 0) {
			A::next_up_on_time_out(core_idx, cores)
		} else if B::is_my_core(core_idx, A::session_core_count()) {
			B::next_up_on_time_out(core_idx, cores)
		} else {
			C::next_up_on_time_out(core_idx, cores)
		}
	}
}
