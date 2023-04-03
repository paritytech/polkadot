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
	CoreIndex, CoreOccupied, GroupIndex, GroupRotationInfo, Id as ParaId, ParathreadEntry,
	ScheduledCore, ValidatorIndex,
};
use sp_runtime::traits::{One, Saturating};
use sp_std::{
	collections::{btree_map::BTreeMap, vec_deque::VecDeque},
	prelude::*,
};

use crate::{
	configuration,
	initializer::SessionChangeNotification,
	paras,
	scheduler_common::{CoreAssignment, FreedReason},
};

use crate::scheduler_common::{Assignment, AssignmentProvider};
pub use pallet::*;

#[cfg(test)]
mod tests;

/// The current storage version
const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);
const LOG_TARGET: &str = "runtime::scheduler";
pub mod migration;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use crate::scheduler_common::AssignmentProvider;

	#[pallet::pallet]
	#[pallet::without_storage_info]
	#[pallet::storage_version(STORAGE_VERSION)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + configuration::Config + paras::Config {
		type AssignmentProvider: AssignmentProvider<Self>;
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
	pub(crate) type AvailabilityCores<T> = StorageValue<_, Vec<CoreOccupied>, ValueQuery>;

	/// The block number where the session start occurred. Used to track how many group rotations have occurred.
	///
	/// Note that in the context of parachains modules the session change is signaled during
	/// the block and enacted at the end of the block (at the finalization stage, to be exact).
	/// Thus for all intents and purposes the effect of the session change is observed at the
	/// block following the session change, block number of which we save in this storage value.
	#[pallet::storage]
	#[pallet::getter(fn session_start_block)]
	pub(crate) type SessionStartBlock<T: Config> = StorageValue<_, T::BlockNumber, ValueQuery>;

	/// One entry for each availability core. The `VecDeque` represents the assignments to be scheduled on that core.
	#[pallet::storage]
	#[pallet::getter(fn claimqueue)]
	pub(crate) type ClaimQueue<T> =
		StorageValue<_, BTreeMap<CoreIndex, VecDeque<Option<CoreAssignment>>>, ValueQuery>;
}

type PositionInClaimqueue = u32;
type TimedoutParas = BTreeMap<CoreIndex, Assignment>;
type ConcludedParas = BTreeMap<CoreIndex, ParaId>;

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
			T::AssignmentProvider::session_core_count(),
			match config.max_validators_per_core {
				Some(x) if x != 0 => validators.len() as u32 / x,
				_ => 0,
			},
		);

		Self::reschedule_occupied_cores(AvailabilityCores::<T>::get());
		Self::clear_claimqueue();
		T::AssignmentProvider::new_session();

		// clear all cores
		AvailabilityCores::<T>::mutate(|cores| {
			for core in cores.iter_mut() {
				*core = CoreOccupied::Free;
			}

			cores.resize(n_cores as _, CoreOccupied::Free);
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
	/// for them being freed.
	fn free_cores(
		just_freed_cores: BTreeMap<CoreIndex, FreedReason>,
	) -> (ConcludedParas, TimedoutParas) {
		let mut timedout_paras = BTreeMap::new();
		let mut concluded_paras = BTreeMap::new();
		let config = <configuration::Pallet<T>>::config();

		AvailabilityCores::<T>::mutate(|cores| {
			let c_len = cores.len();

			just_freed_cores
				.into_iter()
				.filter(|(freed_index, _)| (freed_index.0 as usize) < c_len)
				.for_each(|(freed_index, freed_reason)| {
					match &cores[freed_index.0 as usize] {
						CoreOccupied::Free => {},
						CoreOccupied::Parachain(_) => {}, // If we ever do slot sharing parachains, this case needs to be handled
						CoreOccupied::Parathread(entry) => {
							match freed_reason {
								FreedReason::Concluded => {
									concluded_paras.insert(freed_index, entry.claim.0);
								},
								FreedReason::TimedOut => {
									if entry.retries < config.parathread_retries {
										let entry = ParathreadEntry {
											retries: entry.retries + 1,
											claim: entry.claim.clone(),
										};
										timedout_paras
											.insert(freed_index, Assignment::ParathreadA(entry));
									} else {
										// Consider max retried parathreads as concluded for the assignment provider
										concluded_paras.insert(freed_index, entry.claim.0);
									}
								},
							};
						},
					};

					cores[freed_index.0 as usize] = CoreOccupied::Free;
				})
		});

		(concluded_paras, timedout_paras)
	}

	/// Note that the given cores have become occupied. Behavior undefined if any of the given cores were not scheduled
	/// or the slice is not sorted ascending by core index.
	///
	/// Complexity: O(n) in the number of scheduled cores, which is capped at the number of total cores.
	/// This is efficient in the case that most scheduled cores are occupied.
	pub(crate) fn occupied(
		now_occupied: BTreeMap<CoreIndex, ParaId>,
	) -> BTreeMap<CoreIndex, PositionInClaimqueue> {
		let mut availability_cores = AvailabilityCores::<T>::get();

		let pos_mapping = now_occupied
			.into_iter()
			.flat_map(|(core_idx, para_id)| match Self::remove_from_claimqueue(core_idx, para_id) {
				Err(_) => None, // TODO: report back?
				Ok((pos_in_claimqueue, assignment)) => {
					// is this correct?
					availability_cores[core_idx.0 as usize] = assignment.to_core_occupied();

					Some((core_idx, pos_in_claimqueue))
				},
			})
			.collect();

		AvailabilityCores::<T>::set(availability_cores);

		pos_mapping
	}

	/// Get the para (chain or thread) ID assigned to a particular core or index, if any. Core indices
	/// out of bounds will return `None`, as will indices of unassigned cores.
	pub(crate) fn core_para(core_index: CoreIndex) -> Option<ParaId> {
		let cores = AvailabilityCores::<T>::get();
		match cores.get(core_index.0 as usize) {
			None | Some(CoreOccupied::Free) => None,
			Some(CoreOccupied::Parachain(para_id)) => Some(*para_id),
			Some(CoreOccupied::Parathread(entry)) => Some(entry.claim.0),
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
					None => true, // out-of-bounds, doesn't really matter what is returned.
					Some(CoreOccupied::Free) => true, // core not occupied, still doesn't really matter.
					Some(_) => {
						// core not occupied, still doesn't really matter.
						let availability_period =
							T::AssignmentProvider::get_availability_period(core_index);
						if blocks_since_last_rotation >= availability_period {
							false // no pruning except recently after rotation.
						} else {
							let now = <frame_system::Pallet<T>>::block_number();
							now.saturating_sub(pending_since) >= availability_period
						}
					},
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
	pub(crate) fn next_up_on_available(core: CoreIndex) -> Option<ScheduledCore> {
		ClaimQueue::<T>::get().get(&core).and_then(|a| {
			a.iter()
				.position(|e| e.is_some())
				.and_then(|pos| Self::assignment_to_scheduled_core(&a[pos]))
		})
	}

	fn assignment_to_scheduled_core(ca: &Option<CoreAssignment>) -> Option<ScheduledCore> {
		match ca {
			None => None,
			Some(ca) => match ca.kind.clone() {
				Assignment::Parachain(_para_id) =>
					Some(ScheduledCore { para_id: ca.kind.para_id(), collator: None }),
				Assignment::ParathreadA(entry) =>
					Some(ScheduledCore { para_id: ca.kind.para_id(), collator: entry.claim.1 }),
			},
		}
	}

	/// Return the next thing that will be scheduled on this core assuming it is currently
	/// occupied and the candidate occupying it times out.
	pub(crate) fn next_up_on_time_out(core: CoreIndex) -> Option<ScheduledCore> {
		Self::next_up_on_available(core)
	}

	// on new session
	fn reschedule_occupied_cores(cores: Vec<CoreOccupied>) {
		for (core_idx, core) in cores.into_iter().enumerate() {
			match Assignment::from_core_occupied(core) {
				None => continue,
				Some(ass) => T::AssignmentProvider::push_assignment_for_core(
					CoreIndex::from(core_idx as u32),
					ass,
				),
			}
		}
	}

	//
	//  ClaimQueue related functions
	//
	fn claimqueue_lookahead() -> u32 {
		match <configuration::Pallet<T>>::config().scheduling_lookahead {
			0 => 1,
			n => n,
		}
	}

	// on new session
	fn clear_claimqueue() {
		for (core_idx, cqv) in ClaimQueue::<T>::take() {
			for ca in cqv.into_iter().flatten() {
				T::AssignmentProvider::push_assignment_for_core(
					core_idx,
					Assignment::from_core_assignment(ca),
				);
			}
		}
	}

	pub(crate) fn update_claimqueue(
		just_freed_cores: BTreeMap<CoreIndex, FreedReason>,
		now: T::BlockNumber,
	) -> Vec<CoreAssignment> {
		Self::move_claimqueue_forward();
		Self::fill_claimqueue(just_freed_cores, now)
	}

	fn move_claimqueue_forward() {
		let mut cq = ClaimQueue::<T>::get();
		for (_, vec) in cq.iter_mut() {
			match vec.front() {
				None => {},
				Some(None) => {
					vec.pop_front();
				},
				Some(_) => {},
			}
		}

		ClaimQueue::<T>::set(cq);
	}

	fn fill_claimqueue(
		just_freed_cores: BTreeMap<CoreIndex, FreedReason>,
		now: T::BlockNumber,
	) -> Vec<CoreAssignment> {
		let (mut concluded_paras, mut timedout_paras) = Self::free_cores(just_freed_cores);

		// This can only happen on new sessions at which we move all assignments back to the provider.
		// Hence, there's nothing we need to do here.
		if ValidatorGroups::<T>::get().is_empty() {
			vec![]
		} else {
			let n_lookahead = Self::claimqueue_lookahead();
			let n_session_cores = T::AssignmentProvider::session_core_count();
			let cq = ClaimQueue::<T>::get();

			for core_idx in 0..n_session_cores {
				let core_idx = CoreIndex(core_idx);
				let group_idx = Self::group_assigned_to_core(core_idx, now).expect(
					"core is not out of bounds and we are guaranteed \
										  to be after the most recent session start; qed",
				);

				// add previously timedout paras back into the queue
				if let Some(assignment) = timedout_paras.remove(&core_idx) {
					let ca = assignment.to_core_assignment(core_idx, group_idx);
					Self::add_to_claimqueue(ca);
				}

				let n_lookahead_used = cq.get(&core_idx).map_or(0, |v| v.len() as u32);
				for _ in n_lookahead_used..n_lookahead {
					let concluded_para = concluded_paras.remove(&core_idx);
					if let Some(ass) =
						T::AssignmentProvider::pop_assignment_for_core(core_idx, concluded_para)
					{
						let ca = ass.to_core_assignment(core_idx, group_idx);
						Self::add_to_claimqueue(ca);
					}
				}
			}

			assert!(timedout_paras.is_empty());
			assert!(concluded_paras.is_empty());

			Self::scheduled_claimqueue()
		}
	}

	fn add_to_claimqueue(ca: CoreAssignment) {
		ClaimQueue::<T>::mutate(|la| match la.get_mut(&ca.core) {
			None => {
				la.insert(ca.core, vec![Some(ca)].into_iter().collect());
			},
			Some(la_vec) => la_vec.push_back(Some(ca)),
		});
	}

	/// Returns `CoreAssignment` with `para_id` at `core_idx` if found.
	fn remove_from_claimqueue(
		core_idx: CoreIndex,
		para_id: ParaId,
	) -> Result<(PositionInClaimqueue, CoreAssignment), &'static str> {
		let mut cq = ClaimQueue::<T>::get();
		let la_vec = cq.get_mut(&core_idx).ok_or_else(|| "core_idx not found in lookahead")?;

		let pos = la_vec
			.iter()
			.position(|a| a.as_ref().map_or(false, |v| v.kind.para_id() == para_id))
			.ok_or_else(|| "para id not found at core_idx lookahead")?;

		let ca = la_vec
			.remove(pos)
			.expect("position() above tells us this element exist.")
			.expect("position() above tells us this element exist.");

		// Since the core is now occupied, the next entry in the claimqueue in order to achieve 12 second block times needs to be None
		if la_vec.front() != Some(&None) {
			la_vec.push_front(None);
		}
		ClaimQueue::<T>::set(cq);

		Ok((pos as u32, ca))
	}

	// Temporary to imitate the old schedule() call. Will disappear when we make the scheduler AB ready
	pub(crate) fn scheduled_claimqueue() -> Vec<CoreAssignment> {
		let claimqueue = ClaimQueue::<T>::get();
		claimqueue.into_iter().flat_map(|(_, v)| v.front().cloned()).flatten().collect()
	}

	#[cfg(any(feature = "try-runtime", test))]
	fn claimqueue_len() -> usize {
		ClaimQueue::<T>::get().iter().map(|la_vec| la_vec.1.len()).sum()
	}

	//#[cfg(any(feature = "try-runtime", test))]
	//pub(crate) fn claimqueue_is_empty() -> bool {
	//	Self::claimqueue_len() == 0
	//}

	#[cfg(test)]
	pub(crate) fn claimqueue_contains_only_none() -> bool {
		let mut cq = ClaimQueue::<T>::get();
		for (_, v) in cq.iter_mut() {
			v.retain(|e| e.is_some());
		}

		cq.iter().map(|(_, v)| v.len()).sum::<usize>() == 0
	}

	#[cfg(test)]
	pub(crate) fn claimqueue_contains_para_ids(pids: Vec<ParaId>) -> bool {
		use sp_std::collections::btree_set::BTreeSet;

		let set: BTreeSet<ParaId> = ClaimQueue::<T>::get()
			.into_iter()
			.flat_map(|(_, assignments)| {
				assignments
					.into_iter()
					.filter_map(|assignment| assignment.and_then(|ca| Some(ca.kind.para_id())))
			})
			.collect();

		pids.into_iter().all(|pid| set.contains(&pid))
	}
}
