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

//! The scheduler module for parachains and parathreads.
//!
//! This module is responsible for two main tasks:
//!   - Partitioning validators into groups and assigning groups to parachains and parathreads
//!   - Scheduling parachains and parathreads
//!
//! It aims to achieve these tasks with these goals in mind:
//! - It should be possible to know at least a block ahead-of-time, ideally more, which validators
//!   are going to be assigned to which parachains.
//! - Parachains that have a candidate pending availability in this fork of the chain should not be
//!   assigned.
//! - Validator assignments should not be gameable. Malicious cartels should not be able to
//!   manipulate the scheduler to assign themselves as desired.
//! - High or close to optimal throughput of parachains and parathreads. Work among validator groups
//!   should be balanced.
//!
//! The Scheduler manages resource allocation using the concept of "Availability Cores".
//! There will be one availability core for each parachain, and a fixed number of cores
//! used for multiplexing parathreads. Validators will be partitioned into groups, with the same
//! number of groups as availability cores. Validator groups will be assigned to different
//! availability cores over time.

use crate::{configuration, initializer::SessionChangeNotification, paras};
use frame_support::pallet_prelude::*;
use frame_system::pallet_prelude::BlockNumberFor;
use primitives::{
	v5::ParasEntry, CoreIndex, CoreOccupied, GroupIndex, GroupRotationInfo, Id as ParaId,
	ScheduledCore, ValidatorIndex,
};
use sp_runtime::traits::{One, Saturating};
use sp_std::{
	collections::{btree_map::BTreeMap, vec_deque::VecDeque},
	prelude::*,
};

pub mod common;

use common::{AssignmentProvider, AssignmentProviderConfig, CoreAssignment, FreedReason};

pub use pallet::*;

#[cfg(test)]
mod tests;

const LOG_TARGET: &str = "runtime::parachains::scheduler";
pub mod migration;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

	#[pallet::pallet]
	#[pallet::without_storage_info]
	#[pallet::storage_version(STORAGE_VERSION)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + configuration::Config + paras::Config {
		type AssignmentProvider: AssignmentProvider<BlockNumberFor<Self>>;
	}

	/// All the validator groups. One for each core. Indices are into `ActiveValidators` - not the
	/// broader set of Polkadot validators, but instead just the subset used for parachains during
	/// this session.
	///
	/// Bound: The number of cores is the sum of the numbers of parachains and parathread
	/// multiplexers. Reasonably, 100-1000. The dominant factor is the number of validators: safe
	/// upper bound at 10k.
	#[pallet::storage]
	#[pallet::getter(fn validator_groups)]
	pub(crate) type ValidatorGroups<T> = StorageValue<_, Vec<Vec<ValidatorIndex>>, ValueQuery>;

	/// One entry for each availability core. Entries are `None` if the core is not currently
	/// occupied. Can be temporarily `Some` if scheduled but not occupied.
	/// The i'th parachain belongs to the i'th core, with the remaining cores all being
	/// parathread-multiplexers.
	///
	/// Bounded by the maximum of either of these two values:
	///   * The number of parachains and parathread multiplexers
	///   * The number of validators divided by `configuration.max_validators_per_core`.
	#[pallet::storage]
	#[pallet::getter(fn availability_cores)]
	pub(crate) type AvailabilityCores<T: Config> =
		StorageValue<_, Vec<CoreOccupied<BlockNumberFor<T>>>, ValueQuery>;

	/// The block number where the session start occurred. Used to track how many group rotations
	/// have occurred.
	///
	/// Note that in the context of parachains modules the session change is signaled during
	/// the block and enacted at the end of the block (at the finalization stage, to be exact).
	/// Thus for all intents and purposes the effect of the session change is observed at the
	/// block following the session change, block number of which we save in this storage value.
	#[pallet::storage]
	#[pallet::getter(fn session_start_block)]
	pub(crate) type SessionStartBlock<T: Config> = StorageValue<_, BlockNumberFor<T>, ValueQuery>;

	/// One entry for each availability core. The `VecDeque` represents the assignments to be
	/// scheduled on that core. `None` is used to signal to not schedule the next para of the core
	/// as there is one currently being scheduled. Not using `None` here would overwrite the
	/// `CoreState` in the runtime API. The value contained here will not be valid after the end of
	/// a block. Runtime APIs should be used to determine scheduled cores/ for the upcoming block.
	#[pallet::storage]
	#[pallet::getter(fn claimqueue)]
	pub(crate) type ClaimQueue<T: Config> = StorageValue<
		_,
		BTreeMap<CoreIndex, VecDeque<Option<ParasEntry<BlockNumberFor<T>>>>>,
		ValueQuery,
	>;
}

type PositionInClaimqueue = u32;
type TimedoutParas<T> = BTreeMap<CoreIndex, ParasEntry<BlockNumberFor<T>>>;
type ConcludedParas = BTreeMap<CoreIndex, ParaId>;

impl<T: Config> Pallet<T> {
	/// Called by the initializer to initialize the scheduler pallet.
	pub(crate) fn initializer_initialize(_now: BlockNumberFor<T>) -> Weight {
		Weight::zero()
	}

	/// Called by the initializer to finalize the scheduler pallet.
	pub(crate) fn initializer_finalize() {}

	/// Called before the initializer notifies of a new session.
	pub(crate) fn pre_new_session() {
		Self::push_claimqueue_items_to_assignment_provider();
		Self::push_occupied_cores_to_assignment_provider();
	}

	/// Called by the initializer to note that a new session has started.
	pub(crate) fn initializer_on_new_session(
		notification: &SessionChangeNotification<BlockNumberFor<T>>,
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

		AvailabilityCores::<T>::mutate(|cores| {
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

	/// Free unassigned cores. Provide a list of cores that should be considered newly-freed along
	/// with the reason for them being freed. Returns a tuple of concluded and timedout paras.
	fn free_cores(
		just_freed_cores: impl IntoIterator<Item = (CoreIndex, FreedReason)>,
	) -> (ConcludedParas, TimedoutParas<T>) {
		let mut timedout_paras: BTreeMap<CoreIndex, ParasEntry<BlockNumberFor<T>>> =
			BTreeMap::new();
		let mut concluded_paras = BTreeMap::new();

		AvailabilityCores::<T>::mutate(|cores| {
			let c_len = cores.len();

			just_freed_cores
				.into_iter()
				.filter(|(freed_index, _)| (freed_index.0 as usize) < c_len)
				.for_each(|(freed_index, freed_reason)| {
					match &cores[freed_index.0 as usize] {
						CoreOccupied::Free => {},
						CoreOccupied::Paras(entry) => {
							match freed_reason {
								FreedReason::Concluded => {
									concluded_paras.insert(freed_index, entry.para_id());
								},
								FreedReason::TimedOut => {
									timedout_paras.insert(freed_index, entry.clone());
								},
							};
						},
					};

					cores[freed_index.0 as usize] = CoreOccupied::Free;
				})
		});

		(concluded_paras, timedout_paras)
	}

	/// Note that the given cores have become occupied. Update the claimqueue accordingly.
	pub(crate) fn occupied(
		now_occupied: BTreeMap<CoreIndex, ParaId>,
	) -> BTreeMap<CoreIndex, PositionInClaimqueue> {
		let mut availability_cores = AvailabilityCores::<T>::get();

		log::debug!(target: LOG_TARGET, "[occupied] now_occupied {:?}", now_occupied);

		let pos_mapping: BTreeMap<CoreIndex, PositionInClaimqueue> = now_occupied
			.iter()
			.flat_map(|(core_idx, para_id)| {
				match Self::remove_from_claimqueue(*core_idx, *para_id) {
					Err(e) => {
						log::debug!(
							target: LOG_TARGET,
							"[occupied] error on remove_from_claimqueue {}",
							e
						);
						None
					},
					Ok((pos_in_claimqueue, pe)) => {
						// is this correct?
						availability_cores[core_idx.0 as usize] = CoreOccupied::Paras(pe);

						Some((*core_idx, pos_in_claimqueue))
					},
				}
			})
			.collect();

		// Drop expired claims after processing now_occupied.
		Self::drop_expired_claims_from_claimqueue();

		AvailabilityCores::<T>::set(availability_cores);

		pos_mapping
	}

	/// Iterates through every element in all claim queues and tries to add new assignments from the
	/// `AssignmentProvider`. A claim is considered expired if it's `ttl` field is lower than the
	/// current block height.
	fn drop_expired_claims_from_claimqueue() {
		let now = <frame_system::Pallet<T>>::block_number();
		let availability_cores = AvailabilityCores::<T>::get();

		ClaimQueue::<T>::mutate(|cq| {
			for (idx, _) in (0u32..).zip(availability_cores) {
				let core_idx = CoreIndex(idx);
				if let Some(core_claimqueue) = cq.get_mut(&core_idx) {
					let mut dropped_claims: Vec<Option<ParaId>> = vec![];
					core_claimqueue.retain(|maybe_entry| {
						if let Some(entry) = maybe_entry {
							if entry.ttl < now {
								dropped_claims.push(Some(entry.para_id()));
								return false
							}
						}
						true
					});

					// For all claims dropped due to TTL, attempt to pop a new entry to
					// the back of the claimqueue.
					for drop in dropped_claims {
						match T::AssignmentProvider::pop_assignment_for_core(core_idx, drop) {
							Some(assignment) => {
								let AssignmentProviderConfig { ttl, .. } =
									T::AssignmentProvider::get_provider_config(core_idx);
								core_claimqueue.push_back(Some(ParasEntry::new(
									assignment.clone(),
									now + ttl,
								)));
							},
							None => (),
						}
					}
				}
			}
		});
	}

	/// Get the para (chain or thread) ID assigned to a particular core or index, if any. Core
	/// indices out of bounds will return `None`, as will indices of unassigned cores.
	pub(crate) fn core_para(core_index: CoreIndex) -> Option<ParaId> {
		let cores = AvailabilityCores::<T>::get();
		match cores.get(core_index.0 as usize) {
			None | Some(CoreOccupied::Free) => None,
			Some(CoreOccupied::Paras(entry)) => Some(entry.para_id()),
		}
	}

	/// Get the validators in the given group, if the group index is valid for this session.
	pub(crate) fn group_validators(group_index: GroupIndex) -> Option<Vec<ValidatorIndex>> {
		ValidatorGroups::<T>::get().get(group_index.0 as usize).map(|g| g.clone())
	}

	/// Get the group assigned to a specific core by index at the current block number. Result
	/// undefined if the core index is unknown or the block number is less than the session start
	/// index.
	pub(crate) fn group_assigned_to_core(
		core: CoreIndex,
		at: BlockNumberFor<T>,
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

		let rotations_since_session_start: BlockNumberFor<T> =
			(at - session_start_block) / config.group_rotation_frequency.into();

		let rotations_since_session_start =
			<BlockNumberFor<T> as TryInto<u32>>::try_into(rotations_since_session_start)
				.unwrap_or(0);
		// Error case can only happen if rotations occur only once every u32::max(),
		// so functionally no difference in behavior.

		let group_idx =
			(core.0 as usize + rotations_since_session_start as usize) % validator_groups.len();
		Some(GroupIndex(group_idx as u32))
	}

	/// Returns an optional predicate that should be used for timing out occupied cores.
	///
	/// If `None`, no timing-out should be done. The predicate accepts the index of the core, and
	/// the block number since which it has been occupied, and the respective parachain timeouts,
	/// i.e. only within `config.paras_availability_period` of the last rotation would this return
	/// `Some`, unless there are no rotations.
	///
	/// The timeout used to depend, but does not depend any more on group rotations. First of all
	/// it only matters if a para got another chance (a retry). If there is a retry and it happens
	/// still within the same group rotation a censoring backing group would need to censor again
	/// and lose out again on backing rewards. This is bad for the censoring backing group, it does
	/// not matter for the parachain as long as it is retried often enough (so it eventually gets a
	/// try on another backing group) - the effect is similar to having a prolonged timeout. It
	/// should also be noted that for both malicious and offline backing groups it is actually more
	/// realistic that the candidate will not be backed to begin with, instead of getting backed
	/// and then not made available.
	pub(crate) fn availability_timeout_predicate(
	) -> Option<impl Fn(CoreIndex, BlockNumberFor<T>) -> bool> {
		let now = <frame_system::Pallet<T>>::block_number();
		let config = <configuration::Pallet<T>>::config();
		let session_start = <SessionStartBlock<T>>::get();

		let blocks_since_session_start = now.saturating_sub(session_start);
		let blocks_since_last_rotation =
			blocks_since_session_start % config.group_rotation_frequency.max(1u8.into());

		if blocks_since_last_rotation >= config.paras_availability_period {
			None
		} else {
			Some(|core_index: CoreIndex, pending_since| {
				let availability_cores = AvailabilityCores::<T>::get();
				let AssignmentProviderConfig { availability_period, .. } =
					T::AssignmentProvider::get_provider_config(core_index);
				let now = <frame_system::Pallet<T>>::block_number();
				match availability_cores.get(core_index.0 as usize) {
					None => true, // out-of-bounds, doesn't really matter what is returned.
					Some(CoreOccupied::Free) => true, // core free, still doesn't matter.
					Some(CoreOccupied::Paras(_)) =>
						now.saturating_sub(pending_since) >= availability_period,
				}
			})
		}
	}

	/// Returns a helper for determining group rotation.
	pub(crate) fn group_rotation_info(
		now: BlockNumberFor<T>,
	) -> GroupRotationInfo<BlockNumberFor<T>> {
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
				.find_map(|e| e.as_ref())
				.map(|pe| Self::paras_entry_to_scheduled_core(pe))
		})
	}

	fn paras_entry_to_scheduled_core(pe: &ParasEntry<BlockNumberFor<T>>) -> ScheduledCore {
		ScheduledCore { para_id: pe.para_id(), collator: None }
	}

	/// Return the next thing that will be scheduled on this core assuming it is currently
	/// occupied and the candidate occupying it times out.
	pub(crate) fn next_up_on_time_out(core: CoreIndex) -> Option<ScheduledCore> {
		Self::next_up_on_available(core).or_else(|| {
			// Or, if none, the claim currently occupying the core,
			// as it would be put back on the queue after timing out if number of retries is not at
			// the maximum.
			let cores = AvailabilityCores::<T>::get();
			cores.get(core.0 as usize).and_then(|c| match c {
				CoreOccupied::Free => None,
				CoreOccupied::Paras(pe) => {
					let AssignmentProviderConfig { max_availability_timeouts, .. } =
						T::AssignmentProvider::get_provider_config(core);

					if pe.availability_timeouts < max_availability_timeouts {
						Some(Self::paras_entry_to_scheduled_core(pe))
					} else {
						None
					}
				},
			})
		})
	}

	/// Pushes occupied cores to the assignment provider.
	fn push_occupied_cores_to_assignment_provider() {
		AvailabilityCores::<T>::mutate(|cores| {
			for (core_idx, core) in cores.iter_mut().enumerate() {
				match core {
					CoreOccupied::Free => continue,
					CoreOccupied::Paras(entry) => {
						let core_idx = CoreIndex::from(core_idx as u32);
						Self::maybe_push_assignment(core_idx, entry.clone());
					},
				}
				*core = CoreOccupied::Free;
			}
		});
	}

	// on new session
	fn push_claimqueue_items_to_assignment_provider() {
		for (core_idx, core_claimqueue) in ClaimQueue::<T>::take() {
			// Push back in reverse order so that when we pop from the provider again,
			// the entries in the claimqueue are in the same order as they are right now.
			for para_entry in core_claimqueue.into_iter().flatten().rev() {
				Self::maybe_push_assignment(core_idx, para_entry);
			}
		}
	}

	/// Push assignments back to the provider on session change unless the paras
	/// timed out on availability before.
	fn maybe_push_assignment(core_idx: CoreIndex, pe: ParasEntry<BlockNumberFor<T>>) {
		if pe.availability_timeouts == 0 {
			T::AssignmentProvider::push_assignment_for_core(core_idx, pe.assignment);
		}
	}

	//
	//  ClaimQueue related functions
	//
	fn claimqueue_lookahead() -> u32 {
		<configuration::Pallet<T>>::config().scheduling_lookahead
	}

	/// Updates the claimqueue by moving it to the next paras and filling empty spots with new
	/// paras.
	pub(crate) fn update_claimqueue(
		just_freed_cores: impl IntoIterator<Item = (CoreIndex, FreedReason)>,
		now: BlockNumberFor<T>,
	) -> Vec<CoreAssignment<BlockNumberFor<T>>> {
		Self::move_claimqueue_forward();
		Self::free_cores_and_fill_claimqueue(just_freed_cores, now)
	}

	/// Moves all elements in the claimqueue forward.
	fn move_claimqueue_forward() {
		let mut cq = ClaimQueue::<T>::get();
		for (_, core_queue) in cq.iter_mut() {
			// First pop the finished claims from the front.
			match core_queue.front() {
				None => {},
				Some(None) => {
					core_queue.pop_front();
				},
				Some(_) => {},
			}
		}

		ClaimQueue::<T>::set(cq);
	}

	/// Frees cores and fills the free claimqueue spots by popping from the `AssignmentProvider`.
	fn free_cores_and_fill_claimqueue(
		just_freed_cores: impl IntoIterator<Item = (CoreIndex, FreedReason)>,
		now: BlockNumberFor<T>,
	) -> Vec<CoreAssignment<BlockNumberFor<T>>> {
		let (mut concluded_paras, mut timedout_paras) = Self::free_cores(just_freed_cores);

		// This can only happen on new sessions at which we move all assignments back to the
		// provider. Hence, there's nothing we need to do here.
		if ValidatorGroups::<T>::get().is_empty() {
			vec![]
		} else {
			let n_lookahead = Self::claimqueue_lookahead();
			let n_session_cores = T::AssignmentProvider::session_core_count();
			let cq = ClaimQueue::<T>::get();
			let ttl = <configuration::Pallet<T>>::config().on_demand_ttl;

			for core_idx in 0..n_session_cores {
				let core_idx = CoreIndex::from(core_idx);

				// add previously timedout paras back into the queue
				if let Some(mut entry) = timedout_paras.remove(&core_idx) {
					let AssignmentProviderConfig { max_availability_timeouts, .. } =
						T::AssignmentProvider::get_provider_config(core_idx);
					if entry.availability_timeouts < max_availability_timeouts {
						// Increment the timeout counter.
						entry.availability_timeouts += 1;
						// Reset the ttl so that a timed out assignment.
						entry.ttl = now + ttl;
						Self::add_to_claimqueue(core_idx, entry);
						// The claim has been added back into the claimqueue.
						// Do not pop another assignment for the core.
						continue
					} else {
						// Consider timed out assignments for on demand parachains as concluded for
						// the assignment provider
						let ret = concluded_paras.insert(core_idx, entry.para_id());
						debug_assert!(ret.is_none());
					}
				}

				// We  consider occupied cores to be part of the claimqueue
				let n_lookahead_used = cq.get(&core_idx).map_or(0, |v| v.len() as u32) +
					if Self::is_core_occupied(core_idx) { 1 } else { 0 };
				for _ in n_lookahead_used..n_lookahead {
					let concluded_para = concluded_paras.remove(&core_idx);
					if let Some(assignment) =
						T::AssignmentProvider::pop_assignment_for_core(core_idx, concluded_para)
					{
						Self::add_to_claimqueue(core_idx, ParasEntry::new(assignment, now + ttl));
					}
				}
			}

			debug_assert!(timedout_paras.is_empty());
			debug_assert!(concluded_paras.is_empty());

			Self::scheduled_claimqueue()
		}
	}

	fn is_core_occupied(core_idx: CoreIndex) -> bool {
		match AvailabilityCores::<T>::get().get(core_idx.0 as usize) {
			None | Some(CoreOccupied::Free) => false,
			Some(CoreOccupied::Paras(_)) => true,
		}
	}

	fn add_to_claimqueue(core_idx: CoreIndex, pe: ParasEntry<BlockNumberFor<T>>) {
		ClaimQueue::<T>::mutate(|la| {
			let la_deque = la.entry(core_idx).or_insert_with(|| VecDeque::new());
			la_deque.push_back(Some(pe));
		});
	}

	/// Returns `ParasEntry` with `para_id` at `core_idx` if found.
	fn remove_from_claimqueue(
		core_idx: CoreIndex,
		para_id: ParaId,
	) -> Result<(PositionInClaimqueue, ParasEntry<BlockNumberFor<T>>), &'static str> {
		ClaimQueue::<T>::mutate(|cq| {
			let core_claims = cq.get_mut(&core_idx).ok_or("core_idx not found in lookahead")?;

			let pos = core_claims
				.iter()
				.position(|a| a.as_ref().map_or(false, |pe| pe.para_id() == para_id))
				.ok_or("para id not found at core_idx lookahead")?;

			let pe = core_claims
				.remove(pos)
				.ok_or("remove returned None")?
				.ok_or("Element in Claimqueue was None.")?;

			// Since the core is now occupied, the next entry in the claimqueue in order to achieve
			// 12 second block times needs to be None
			if core_claims.front() != Some(&None) {
				core_claims.push_front(None);
			}
			Ok((pos as u32, pe))
		})
	}

	// TODO: Temporary to imitate the old schedule() call. Will be adjusted when we make the
	// scheduler AB ready
	pub(crate) fn scheduled_claimqueue() -> Vec<CoreAssignment<BlockNumberFor<T>>> {
		let claimqueue = ClaimQueue::<T>::get();

		claimqueue
			.into_iter()
			.flat_map(|(core_idx, v)| {
				v.front()
					.cloned()
					.flatten()
					.map(|pe| CoreAssignment { core: core_idx, paras_entry: pe })
			})
			.collect()
	}

	#[cfg(any(feature = "runtime-benchmarks", test))]
	pub(crate) fn assignment_provider_config(
		core_idx: CoreIndex,
	) -> AssignmentProviderConfig<BlockNumberFor<T>> {
		T::AssignmentProvider::get_provider_config(core_idx)
	}

	#[cfg(any(feature = "try-runtime", test))]
	fn claimqueue_len() -> usize {
		ClaimQueue::<T>::get().iter().map(|la_vec| la_vec.1.len()).sum()
	}

	#[cfg(all(not(feature = "runtime-benchmarks"), test))]
	pub(crate) fn claimqueue_is_empty() -> bool {
		Self::claimqueue_len() == 0
	}

	#[cfg(test)]
	pub(crate) fn set_validator_groups(validator_groups: Vec<Vec<ValidatorIndex>>) {
		ValidatorGroups::<T>::set(validator_groups);
	}
}
