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

use frame_support::{pallet_prelude::*, traits::Len};
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

use crate::scheduler_common::{Assignment, AssignmentKind, AssignmentProvider};
pub use pallet::*;

#[cfg(test)]
mod tests;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use crate::scheduler_common::AssignmentProvider;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config + configuration::Config + paras::Config + scheduler_parathreads::Config
	{
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

	/// Currently scheduled cores - free but up to be occupied.
	///
	/// Bounded by the number of cores: one for each parachain and parathread multiplexer.
	///
	/// The value contained here will not be valid after the end of a block. Runtime APIs should be used to determine scheduled cores/
	/// for the upcoming block.
	#[pallet::storage]
	//#[pallet::getter(fn scheduled)]
	pub(crate) type Scheduled<T> = StorageValue<_, Vec<CoreAssignment>, ValueQuery>;
	// sorted ascending by CoreIndex.

	#[pallet::storage]
	#[pallet::getter(fn lookahead)]
	pub(crate) type Lookahead<T> =
		StorageValue<_, BTreeMap<CoreIndex, Vec<CoreAssignment>>, ValueQuery>;
	// CoreIndex is used as index into the Vec

	/// This structure is used to enforce the rule that a para cannot produce more than one block per relay-chain block.
	/// This is done by ensuring that paras are always scheduled onto the same core for one relay-chain block.
	#[pallet::storage]
	#[pallet::getter(fn para_core_mappping)]
	pub(crate) type ParaCoreMapping<T> =
		StorageValue<_, BTreeMap<ParaId, LookaheadInfo>, ValueQuery>;
}

pub type NumAssignmentsInLookahead = u32;

#[derive(Encode, Decode, TypeInfo)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub struct LookaheadInfo {
	core_idx: CoreIndex,
	n_in_lookahead: NumAssignmentsInLookahead,
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
			T::AssignmentProvider::session_core_count(),
			match config.max_validators_per_core {
				Some(x) if x != 0 => validators.len() as u32 / x,
				_ => 0,
			},
		);
		T::AssignmentProvider::on_new_session(config.scheduling_lookahead);

		Self::reschedule_occupied_cores(AvailabilityCores::<T>::get());

		AvailabilityCores::<T>::mutate(|cores| {
			// clear all occupied cores.
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
	/// for them being freed. The list is assumed to be sorted in ascending order by core index.
	pub(crate) fn update_lookahead_free_cores(just_freed_cores: BTreeMap<CoreIndex, FreedReason>) {
		AvailabilityCores::<T>::mutate(|cores| {
			let c_len = cores.len();

			just_freed_cores
				.into_iter()
				.filter(|(freed_index, _)| (freed_index.0 as usize) < c_len)
				.for_each(|(freed_index, freed_reason)| {
					match &cores[freed_index.0 as usize] {
						CoreOccupied::Free => {},
						CoreOccupied::Parachain => {},
						CoreOccupied::Parathread(entry) => {
							match freed_reason {
								FreedReason::Concluded => {
									let _ignore =
										Self::remove_from_lookahead(freed_index, entry.claim.0);
								},
								FreedReason::TimedOut => {},
							};
						},
					};

					cores[freed_index.0 as usize] = CoreOccupied::Free;
				})
		});
	}

	/// Note that the given cores have become occupied. Behavior undefined if any of the given cores were not scheduled
	/// or the slice is not sorted ascending by core index.
	///
	/// Complexity: O(n) in the number of scheduled cores, which is capped at the number of total cores.
	/// This is efficient in the case that most scheduled cores are occupied.
	pub(crate) fn occupied(now_occupied: Vec<(CoreIndex, ParaId)>) {
		if now_occupied.is_empty() {
			return
		}

		let mut availability_cores = AvailabilityCores::<T>::get();
		for (core_idx, para_id) in now_occupied {
			// We remove as we assume the happy case that availability will usually conclude.
			// If it times out, we will need to reinsert into parathread queue... there should be a better way, I believe
			match Self::remove_from_lookahead(core_idx, para_id) {
				Err(_) => todo!(),
				Ok(assignment) =>
					availability_cores[core_idx.0 as usize] = assignment.to_core_occupied(),
			}
		}

		AvailabilityCores::<T>::set(availability_cores);
	}

	/// Get the para (chain or thread) ID assigned to a particular core or index, if any. Core indices
	/// out of bounds will return `None`, as will indices of unassigned cores.
	pub(crate) fn core_para(core_index: CoreIndex) -> Option<ParaId> {
		let cores = AvailabilityCores::<T>::get();
		match cores.get(core_index.0 as usize) {
			None => None,
			Some(CoreOccupied::Free) => None,
			// NOTE: This is temporary
			Some(x) => Some(T::AssignmentProvider::core_para(core_index, x)),
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
	///
	/// For parachains, this is always the ID of the parachain and no specified collator.
	/// For parathreads, this is based on the next item in the `ParathreadQueue` assigned to that
	/// core, and is None if there isn't one.
	pub(crate) fn next_up_on_available(core: CoreIndex) -> Option<ScheduledCore> {
		Lookahead::<T>::get()
			.get(&core)
			.and_then(|a| a.first().map(Self::assignment_to_scheduled_core))
	}

	fn assignment_to_scheduled_core(ca: &CoreAssignment) -> ScheduledCore {
		match ca.kind.clone() {
			AssignmentKind::Parachain => ScheduledCore { para_id: ca.para_id, collator: None },
			AssignmentKind::Parathread(collator, _) =>
				ScheduledCore { para_id: ca.para_id, collator: Some(collator) },
		}
	}

	/// Return the next thing that will be scheduled on this core assuming it is currently
	/// occupied and the candidate occupying it became available.
	///
	/// For parachains, this is always the ID of the parachain and no specified collator.
	/// For parathreads, this is based on the next item in the `ParathreadQueue` assigned to that
	/// core, or if there isn't one, the claim that is currently occupying the core, as long
	/// as the claim's retries would not exceed the limit. Otherwise None.
	pub(crate) fn next_up_on_time_out(core: CoreIndex) -> Option<ScheduledCore> {
		Self::next_up_on_available(core)
	}

	// on new session
	fn reschedule_occupied_cores(cores: Vec<CoreOccupied>) {
		for (core_idx, core) in cores.iter().enumerate() {
			match core {
				CoreOccupied::Free => continue,
				CoreOccupied::Parachain => continue,
				CoreOccupied::Parathread(entry) => T::AssignmentProvider::push_assignment_for_core(
					CoreIndex(core_idx as u32),
					Assignment::ParathreadA(entry.claim.clone()),
				),
			}
		}
	}

	//
	// Lookahead related functions
	//
	fn backing_lookahead() -> u32 {
		// quck hack to give a value of 1 for tests
		match <configuration::Pallet<T>>::config().scheduling_lookahead {
			0 => 1,
			n => n,
		}
	}

	pub(crate) fn clear_and_fill_lookahead(
		just_freed_cores: BTreeMap<CoreIndex, FreedReason>,
		now: T::BlockNumber,
	) {
		Self::clear_lookahead();
		Self::fill_lookahead(just_freed_cores, now);
		// TODO: return Lookahead?
	}

	// Clear lookahead
	fn clear_lookahead() {
		for (_, cas) in Lookahead::<T>::take() {
			for ca in cas.into_iter() {
				T::AssignmentProvider::clear(ca.core, ca.to_assignment())
			}
		}
	}

	fn fill_lookahead(just_freed_cores: BTreeMap<CoreIndex, FreedReason>, now: T::BlockNumber) {
		sp_runtime::print("Filling lookahead");
		Self::update_lookahead_free_cores(just_freed_cores);

		if ValidatorGroups::<T>::get().is_empty() {
			return
		}

		let n_lookahead = Self::backing_lookahead();
		let n_session_cores = T::AssignmentProvider::session_core_count();
		let mut push_back_buffer = Vec::with_capacity((n_session_cores * n_lookahead) as usize);

		sp_runtime::print("n_session_cores");
		sp_runtime::print(n_session_cores);
		sp_runtime::print("n_lookahead");
		sp_runtime::print(n_lookahead);

		for core_idx in 0..n_session_cores {
			let core_idx = CoreIndex(core_idx);
			let group_idx = Self::group_assigned_to_core(core_idx, now).expect(
				"core is not out of bounds and we are guaranteed \
										  to be after the most recent session start; qed",
			);

			for _ in 0..n_lookahead {
				// TODO: try to fill lookahead while lookahead is not full OR pop_assignment() returns None
				// doesn_t work! parachains assigner never returns None...
				// TODO: write tests for optimal allocation; check invariants
				match Self::add_to_lookahead(core_idx, group_idx) {
					Err((_, None)) => break,                           // TODO: logging
					Err((_, Some(tup))) => push_back_buffer.push(tup), // TODO: logging
					Ok(_) => (),
				}
			}
		}

		// We could not schedule these assignments due to conflicts
		for (core_idx, ass) in push_back_buffer.into_iter().rev() {
			sp_runtime::print("push_front lookahead idx:");
			sp_runtime::print(core_idx.0);
			T::AssignmentProvider::push_front_assignment_for_core(core_idx, ass);
		}
		let l = "lookahead len: ";
		let s = Lookahead::<T>::get().len();
		sp_runtime::print(l);
		sp_runtime::print(s);
	}

	fn add_to_lookahead(
		core_idx: CoreIndex,
		group_idx: GroupIndex,
	) -> Result<(), (&'static str, Option<(CoreIndex, Assignment)>)> {
		sp_runtime::print("add_to_lookahead");
		match T::AssignmentProvider::pop_assignment_for_core(core_idx) {
			None => Err(("no assignments at core_idx", None)),
			Some(ass) => {
				sp_runtime::print("popped Some");
				// TODO: remove core_idx dependency
				let lookahead_core_idx = ParaCoreMapping::<T>::get()
					.get(&ass.para_id())
					.map_or(core_idx, |li| li.core_idx);

				if Lookahead::<T>::get().get(&lookahead_core_idx).len() as u32 >=
					Self::backing_lookahead()
				{
					sp_runtime::print("lookahead full");
					// If assigner is parathreads and we don't pop, we'd be stuck trying to add
					// the same assignment again and again until the top level loop is over.
					// We wouldn't fill any of the remaining lookahead buffer.
					Err(("lookahead full", Some((core_idx, ass))))
				} else {
					Self::insert_to_lookahead(
						lookahead_core_idx,
						ass.to_core_assignment(core_idx, group_idx),
					);

					Ok(())
				}
			},
		}
	}

	fn insert_to_lookahead(lookahead_core_idx: CoreIndex, ca: CoreAssignment) {
		sp_runtime::print("insert_to_lookahead");
		sp_runtime::print(lookahead_core_idx.0);
		let ca_para_id = ca.para_id;
		Lookahead::<T>::mutate(|la| match la.get_mut(&lookahead_core_idx) {
			None => {
				la.insert(lookahead_core_idx, vec![ca]);
			},
			Some(la_vec) => la_vec.push(ca),
		});

		ParaCoreMapping::<T>::mutate(|pcm| {
			let entry = pcm.entry(ca_para_id).or_insert_with(|| LookaheadInfo {
				core_idx: lookahead_core_idx,
				n_in_lookahead: 0,
			});
			entry.n_in_lookahead += 1;
		});
	}

	fn print_lookahead() {
		frame_support::print("PRINT LOOKAHEAD");
		for tup in Lookahead::<T>::get() {
			frame_support::print("lookahead index");
			frame_support::debug(&tup.0 .0);
			frame_support::print("vec");
			//frame_support::debug(&tup.1);
		}
	}

	fn remove_from_lookahead(
		lookahead_core_idx: CoreIndex,
		para_id: ParaId,
	) -> Result<CoreAssignment, &'static str> {
		Self::print_lookahead();
		sp_runtime::print("remove from lookahead");
		sp_runtime::print(lookahead_core_idx.0);

		let assignment = Lookahead::<T>::mutate(|la| match la.get_mut(&lookahead_core_idx) {
			None => Err("core_idx not found in lookahead"),
			Some(la_vec) => match la_vec.iter().position(|a| a.para_id == para_id) {
				None => Err("para id not found at core_idx lookahead"),
				Some(idx) => Ok(la_vec.remove(idx)),
			},
		})?;

		sp_runtime::print("REMOVED from lookahead");

		ParaCoreMapping::<T>::mutate(|pcm| match pcm.get_mut(&para_id) {
			None => Err("impossible"),
			Some(entry) => {
				entry.n_in_lookahead -= 1;

				if entry.n_in_lookahead <= 0 {
					pcm.remove(&para_id);
				}

				Ok(())
			},
		})?;

		Ok(assignment)
	}

	#[cfg(test)]
	fn lookahead_sizes() -> Vec<usize> {
		Self::print_lookahead();
		Lookahead::<T>::get().iter().map(|la_vec| la_vec.1.len()).collect()
	}

	#[cfg(test)]
	pub(crate) fn lookahead_is_empty() -> bool {
		let sum = Self::lookahead_sizes().iter().sum::<usize>();
		sp_runtime::print("SUM");
		sp_runtime::print(sum);

		sum == 0
	}
}
