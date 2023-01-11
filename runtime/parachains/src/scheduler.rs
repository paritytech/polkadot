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
	CoreIndex, CoreOccupied, GroupIndex, GroupRotationInfo, Id as ParaId, ScheduledCore,
	ValidatorIndex,
};
use sp_runtime::traits::{One, Saturating};
use sp_std::{collections::btree_map::BTreeMap, prelude::*};

use crate::{
	configuration,
	initializer::SessionChangeNotification,
	paras,
	scheduler_common::{CoreAssignment, FreedReason},
	scheduler_parathreads,
};

use crate::scheduler_common::CoreAssigner;
pub use pallet::*;

#[cfg(test)]
mod tests;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config + configuration::Config + paras::Config + scheduler_parathreads::Config
	{
		// I believe having a Vec<CoreAssigner> would be nicer
		type CoreAssigners<T: Config>: CoreAssigner<T>;
	}

	/// All the validator groups. One for each core. Indices are into `ActiveValidators` - not the
	/// broader set of Polkadot validators, but instead just the subset used for parachains during
	/// this session.
	///
	/// Bound: The number of cores is the sum of the numbers of parachains and parathread multiplexers.
	/// Reasonably, 100-1000. The dominant factor is the number of validators: safe upper bound at 10k.
	#[pallet::storage]
	#[pallet::getter(fn validator_groups)]
	pub(crate) type ValidatorGroups<T> = StorageValue<_, Vec<Vec<ValidatorIndex>>, ValueQuery>;

	/// One entry for each availability core. Entries are `None` if the core is not currently occupied. Can be
	/// temporarily `Some` if scheduled but not occupied.
	/// The i'th parachain belongs to the i'th core, with the remaining cores all being
	/// parathread-multiplexers.
	///
	/// Bounded by the maximum of either of these two values:
	///   * The number of parachains and parathread multiplexers
	///   * The number of validators divided by `configuration.max_validators_per_core`.
	#[pallet::storage]
	#[pallet::getter(fn availability_cores)]
	// TODO(?): macro here to check if there is a CoreAssigners impl for every CoreOccupied variant
	pub(crate) type AvailabilityCores<T> = StorageValue<_, Vec<Option<CoreOccupied>>, ValueQuery>;

	/// The block number where the session start occurred. Used to track how many group rotations have occurred.
	///
	/// Note that in the context of parachains modules the session change is signaled during
	/// the block and enacted at the end of the block (at the finalization stage, to be exact).
	/// Thus for all intents and purposes the effect of the session change is observed at the
	/// block following the session change, block number of which we save in this storage value.
	#[pallet::storage]
	#[pallet::getter(fn session_start_block)]
	pub(crate) type SessionStartBlock<T: Config> = StorageValue<_, T::BlockNumber, ValueQuery>;

	/// Currently scheduled cores - free but up to be occupied.
	///
	/// Bounded by the number of cores: one for each parachain and parathread multiplexer.
	///
	/// The value contained here will not be valid after the end of a block. Runtime APIs should be used to determine scheduled cores/
	/// for the upcoming block.
	#[pallet::storage]
	#[pallet::getter(fn scheduled)]
	pub(crate) type Scheduled<T> = StorageValue<_, Vec<CoreAssignment>, ValueQuery>;
	// sorted ascending by CoreIndex.
}

impl<T: Config> Pallet<T> {
	/// Called by the initializer to initialize the scheduler pallet.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		Weight::zero()
	}

	/// Called by the initializer to finalize the scheduler pallet.
	pub(crate) fn initializer_finalize() {}

	/// Called by the initializer to note that a new session has started.
	pub(crate) fn initializer_on_new_session(
		notification: &SessionChangeNotification<T::BlockNumber>,
	) {
		let SessionChangeNotification { validators, new_config, .. } = notification;
		let config = new_config;

		let n_cores = core::cmp::max(
			T::CoreAssigners::<T>::session_cores(),
			match config.max_validators_per_core {
				Some(x) if x != 0 => validators.len() as u32 / x,
				_ => 0,
			},
		);

		// TODO: Can we map assigners to cores in particular? Can we actually iterate over assigners?
		let cores = AvailabilityCores::<T>::get();
		T::CoreAssigners::<T>::initializer_on_new_session(notification, &cores);

		AvailabilityCores::<T>::mutate(|cores| {
			// clear all occupied cores.
			for core in cores.iter_mut() {
				*core = None;
			}

			cores.resize(n_cores as _, None);
		});

		// shuffle validators into groups.
		if n_cores == 0 || validators.is_empty() {
			ValidatorGroups::<T>::set(Vec::new());
		} else {
			let group_base_size = validators.len() / n_cores as usize;
			let n_larger_groups = validators.len() % n_cores as usize;

			// Groups contain indices into the validators from the session change notification,
			// which are already shuffled.

			let mut groups: Vec<Vec<ValidatorIndex>> = Vec::new();
			for i in 0..n_larger_groups {
				let offset = (group_base_size + 1) * i;
				groups.push(
					(0..group_base_size + 1)
						.map(|j| offset + j)
						.map(|j| ValidatorIndex(j as _))
						.collect(),
				);
			}

			for i in 0..(n_cores as usize - n_larger_groups) {
				let offset = (n_larger_groups * (group_base_size + 1)) + (i * group_base_size);
				groups.push(
					(0..group_base_size)
						.map(|j| offset + j)
						.map(|j| ValidatorIndex(j as _))
						.collect(),
				);
			}

			ValidatorGroups::<T>::set(groups);
		}

		let now = <frame_system::Pallet<T>>::block_number() + One::one();
		<SessionStartBlock<T>>::set(now);
	}

	/// Free unassigned cores. Provide a list of cores that should be considered newly-freed along with the reason
	/// for them being freed. The list is assumed to be sorted in ascending order by core index.
	pub(crate) fn free_cores(just_freed_cores: BTreeMap<CoreIndex, FreedReason>) {
		let cores = AvailabilityCores::<T>::get();
		T::CoreAssigners::<T>::free_cores(&just_freed_cores, &cores);

		AvailabilityCores::<T>::mutate(|cores| {
			let c_len = cores.len();
			for freed_index in just_freed_cores
				.keys()
				.map(|free_idx| free_idx.0 as usize)
				.filter(|free_idx| *free_idx < c_len)
			{
				cores[freed_index] = None;
			}
		})
	}

	/// Schedule all unassigned cores, where possible. Provide a list of cores that should be considered
	/// newly-freed along with the reason for them being freed. The list is assumed to be sorted in
	/// ascending order by core index.
	pub(crate) fn schedule(
		just_freed_cores: BTreeMap<CoreIndex, FreedReason>,
		now: T::BlockNumber,
	) {
		Self::free_cores(just_freed_cores);

		if ValidatorGroups::<T>::get().is_empty() {
			return
		}

		let mut scheduled = Scheduled::<T>::get();
		let mut prev_scheduled_in_order = scheduled.iter().enumerate().peekable();

		// Updates to the previous list of scheduled updates and the position of where to insert
		// them, without accounting for prior updates.
		let mut scheduled_updates: Vec<(usize, CoreAssignment)> = Vec::new();

		let cores = AvailabilityCores::<T>::get();
		// single-sweep O(n) in the number of cores.
		for (core_index, _core) in cores.iter().enumerate().filter(|(_, ref c)| c.is_none()) {
			let schedule_and_insert_at = {
				// advance the iterator until just before the core index we are looking at now.
				while prev_scheduled_in_order
					.peek()
					.map_or(false, |(_, assign)| (assign.core.0 as usize) < core_index)
				{
					let _ = prev_scheduled_in_order.next();
				}

				// check the first entry already scheduled with core index >= than the one we
				// are looking at. 3 cases:
				//  1. No such entry, clearly this core is not scheduled, so we need to schedule and put at the end.
				//  2. Entry exists and has same index as the core we are inspecting. do not schedule again.
				//  3. Entry exists and has higher index than the core we are inspecting. schedule and note
				//     insertion position.
				prev_scheduled_in_order.peek().map_or(
					Some(scheduled.len()),
					|(idx_in_scheduled, assign)| {
						if (assign.core.0 as usize) == core_index {
							None
						} else {
							Some(*idx_in_scheduled)
						}
					},
				)
			};

			let schedule_and_insert_at = match schedule_and_insert_at {
				None => continue,
				Some(at) => at,
			};

			let core_idx = CoreIndex(core_index as u32);
			let group_idx = Self::group_assigned_to_core(core_idx, now).expect(
				"core is not out of bounds and we are guaranteed \
										  to be after the most recent session start; qed",
			);

			if let Some(assignment) =
				T::CoreAssigners::<T>::make_core_assignment(core_idx, group_idx)
			{
				scheduled_updates.push((schedule_and_insert_at, assignment))
			}
		}

		// at this point, because `Scheduled` is guaranteed to be sorted and we navigated unassigned
		// core indices in ascending order, we can enact the updates prepared by the previous actions.
		//
		// while inserting, we have to account for the amount of insertions already done.
		//
		// This is O(n) as well, capped at n operations, where n is the number of cores.
		for (num_insertions_before, (insert_at, to_insert)) in
			scheduled_updates.into_iter().enumerate()
		{
			let insert_at = num_insertions_before + insert_at;
			scheduled.insert(insert_at, to_insert);
		}

		// scheduled is guaranteed to be sorted after this point because it was sorted before, and we
		// applied sorted updates at their correct positions, accounting for the offsets of previous
		// insertions.
		Scheduled::<T>::set(scheduled);
	}

	/// Note that the given cores have become occupied. Behavior undefined if any of the given cores were not scheduled
	/// or the slice is not sorted ascending by core index.
	///
	/// Complexity: O(n) in the number of scheduled cores, which is capped at the number of total cores.
	/// This is efficient in the case that most scheduled cores are occupied.
	pub(crate) fn occupied(now_occupied: &[CoreIndex]) {
		if now_occupied.is_empty() {
			return
		}

		let mut availability_cores = AvailabilityCores::<T>::get();
		Scheduled::<T>::mutate(|scheduled| {
			// The constraints on the function require that `now_occupied` is a sorted subset of the
			// `scheduled` cores, which are also sorted.

			let mut occupied_iter = now_occupied.iter().cloned().peekable();
			scheduled.retain(|assignment| {
				let retain = occupied_iter
					.peek()
					.map_or(true, |occupied_idx| occupied_idx != &assignment.core);

				if !retain {
					// remove this entry - it's now occupied. and begin inspecting the next extry
					// of the occupied iterator.
					let _ = occupied_iter.next();

					availability_cores[assignment.core.0 as usize] =
						Some(assignment.to_core_occupied());
				}

				retain
			})
		});

		AvailabilityCores::<T>::set(availability_cores);
	}

	/// Get the para (chain or thread) ID assigned to a particular core or index, if any. Core indices
	/// out of bounds will return `None`, as will indices of unassigned cores.
	pub(crate) fn core_para(core_index: CoreIndex) -> Option<ParaId> {
		let cores = AvailabilityCores::<T>::get();
		match cores.get(core_index.0 as usize).and_then(|c| c.as_ref()) {
			None => None,
			Some(x) => Some(T::CoreAssigners::<T>::core_para(core_index, x)),
		}
	}

	/// Get the validators in the given group, if the group index is valid for this session.
	pub(crate) fn group_validators(group_index: GroupIndex) -> Option<Vec<ValidatorIndex>> {
		ValidatorGroups::<T>::get().get(group_index.0 as usize).map(|g| g.clone())
	}

	/// Get the group assigned to a specific core by index at the current block number. Result undefined if the core index is unknown
	/// or the block number is less than the session start index.
	pub(crate) fn group_assigned_to_core(
		core: CoreIndex,
		at: T::BlockNumber,
	) -> Option<GroupIndex> {
		let config = <configuration::Pallet<T>>::config();
		let session_start_block = <SessionStartBlock<T>>::get();

		if at < session_start_block {
			return None
		}

		let validator_groups = ValidatorGroups::<T>::get();

		if core.0 as usize >= validator_groups.len() {
			return None
		}

		let rotations_since_session_start: T::BlockNumber =
			(at - session_start_block) / config.group_rotation_frequency.into();

		let rotations_since_session_start =
			<T::BlockNumber as TryInto<u32>>::try_into(rotations_since_session_start).unwrap_or(0);
		// Error case can only happen if rotations occur only once every u32::max(),
		// so functionally no difference in behavior.

		let group_idx =
			(core.0 as usize + rotations_since_session_start as usize) % validator_groups.len();
		Some(GroupIndex(group_idx as u32))
	}

	/// Returns an optional predicate that should be used for timing out occupied cores.
	///
	/// If `None`, no timing-out should be done. The predicate accepts the index of the core, and the
	/// block number since which it has been occupied, and the respective parachain and parathread
	/// timeouts, i.e. only within `max(config.chain_availability_period, config.thread_availability_period)`
	/// of the last rotation would this return `Some`, unless there are no rotations.
	///
	/// This really should not be a box, but is working around a compiler limitation filed here:
	/// https://github.com/rust-lang/rust/issues/73226
	/// which prevents us from testing the code if using `impl Trait`.
	pub(crate) fn availability_timeout_predicate(
	) -> Option<Box<dyn Fn(CoreIndex, T::BlockNumber) -> bool>> {
		let now = <frame_system::Pallet<T>>::block_number();
		let config = <configuration::Pallet<T>>::config();

		let session_start = <SessionStartBlock<T>>::get();
		let blocks_since_session_start = now.saturating_sub(session_start);
		let blocks_since_last_rotation =
			blocks_since_session_start % config.group_rotation_frequency;

		let absolute_cutoff =
			sp_std::cmp::max(config.chain_availability_period, config.thread_availability_period);

		let availability_cores = AvailabilityCores::<T>::get();

		if blocks_since_last_rotation >= absolute_cutoff {
			None
		} else {
			let predicate = move |core_index: CoreIndex, pending_since| {
				match availability_cores.get(core_index.0 as usize) {
					None => true,       // out-of-bounds, doesn't really matter what is returned.
					Some(None) => true, // core not occupied, still doesn't really matter.
					Some(Some(_)) => T::CoreAssigners::<T>::availability_timeout_predicate(
						core_index,
						blocks_since_last_rotation,
						pending_since,
					),
				}
			};

			Some(Box::new(predicate))
		}
	}

	/// Returns a helper for determining group rotation.
	pub(crate) fn group_rotation_info(now: T::BlockNumber) -> GroupRotationInfo<T::BlockNumber> {
		let session_start_block = Self::session_start_block();
		let group_rotation_frequency =
			<configuration::Pallet<T>>::config().group_rotation_frequency;

		GroupRotationInfo { session_start_block, now, group_rotation_frequency }
	}

	/// Return the next thing that will be scheduled on this core assuming it is currently
	/// occupied and the candidate occupying it became available.
	///
	/// For parachains, this is always the ID of the parachain and no specified collator.
	/// For parathreads, this is based on the next item in the `ParathreadQueue` assigned to that
	/// core, and is None if there isn't one.
	pub(crate) fn next_up_on_available(core: CoreIndex) -> Option<ScheduledCore> {
		T::CoreAssigners::<T>::next_up_on_available(core)
	}

	/// Return the next thing that will be scheduled on this core assuming it is currently
	/// occupied and the candidate occupying it became available.
	///
	/// For parachains, this is always the ID of the parachain and no specified collator.
	/// For parathreads, this is based on the next item in the `ParathreadQueue` assigned to that
	/// core, or if there isn't one, the claim that is currently occupying the core, as long
	/// as the claim's retries would not exceed the limit. Otherwise None.
	pub(crate) fn next_up_on_time_out(core: CoreIndex) -> Option<ScheduledCore> {
		let cores = AvailabilityCores::<T>::get();
		T::CoreAssigners::<T>::next_up_on_time_out(core, &cores)
	}

	// Free all scheduled cores and return parathread claims to queue, with retries incremented.
	pub(crate) fn clear() {
		let mut vec = vec![];
		for core_assignment in Scheduled::<T>::take() {
			vec.push(core_assignment);
		}

		T::CoreAssigners::<T>::clear(&vec);
	}
}
