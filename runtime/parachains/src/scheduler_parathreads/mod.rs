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
	CoreIndex, CoreOccupied, GroupIndex, Id as ParaId, ParathreadClaim, ParathreadEntry,
	ScheduledCore,
};
use scale_info::TypeInfo;
use sp_runtime::Saturating;
use sp_std::{collections::btree_map::BTreeMap, prelude::*};

use crate::{
	configuration,
	initializer::SessionChangeNotification,
	paras,
	scheduler_common::{AssignmentKind, CoreAssigner, CoreAssignment, FreedReason},
};

pub use pallet::*;

//#[cfg(test)]
//mod tests;

/// A queued parathread entry, pre-assigned to a core.
#[derive(Encode, Decode, TypeInfo)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub struct QueuedParathread {
	pub claim: ParathreadEntry,
	pub core_offset: u32,
}

/// The queue of all parathread claims.
#[derive(Encode, Decode, TypeInfo)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub struct ParathreadClaimQueue {
	pub queue: Vec<QueuedParathread>,
	// this value is between 0 and config.parathread_cores
	pub next_core_offset: u32,
}

impl ParathreadClaimQueue {
	/// Queue a parathread entry to be processed.
	///
	/// Provide the entry and the number of parathread cores, which must be greater than 0.
	pub fn enqueue_entry(&mut self, entry: ParathreadEntry, n_parathread_cores: u32) {
		let core_offset = self.next_core_offset;
		self.next_core_offset = (self.next_core_offset + 1) % n_parathread_cores;

		self.queue.push(QueuedParathread { claim: entry, core_offset })
	}

	/// Take next queued entry with given core offset, if any.
	fn take_next_on_core(&mut self, core_offset: u32) -> Option<ParathreadEntry> {
		let pos = self.queue.iter().position(|queued| queued.core_offset == core_offset);
		pos.map(|i| self.queue.remove(i).claim)
	}

	/// Get the next queued entry with given core offset, if any.
	pub fn get_next_on_core(&self, core_offset: u32) -> Option<&ParathreadEntry> {
		let pos = self.queue.iter().position(|queued| queued.core_offset == core_offset);
		pos.map(|i| &self.queue[i].claim)
	}
}

impl Default for ParathreadClaimQueue {
	fn default() -> Self {
		Self { queue: vec![], next_core_offset: 0 }
	}
}

pub struct ParathreadsScheduler;
impl<T: crate::scheduler::pallet::Config> CoreAssigner<T> for ParathreadsScheduler {
	fn session_cores() -> u32 {
		let config = <configuration::Pallet<T>>::config();
		config.parathread_cores
	}

	fn initializer_initialize(now: T::BlockNumber) -> Weight {
		<self::Pallet<T>>::initializer_initialize(now)
	}

	fn initializer_finalize() {
		<self::Pallet<T>>::initializer_finalize()
	}

	fn initializer_on_new_session(
		notification: &SessionChangeNotification<T::BlockNumber>,
		cores: &[Option<CoreOccupied>],
	) {
		let SessionChangeNotification { new_config, .. } = notification;
		let config = new_config;

		<self::Pallet<T>>::initializer_on_new_session(
			notification,
			cores,
			config.parathread_cores,
			config.parathread_retries,
		);
	}

	fn free_cores(
		just_freed_cores: &BTreeMap<CoreIndex, FreedReason>,
		cores: &[Option<CoreOccupied>],
	) {
		let config = <configuration::Pallet<T>>::config();

		<self::Pallet<T>>::free_cores(just_freed_cores, cores, config.parathread_cores);
	}

	fn make_core_assignment(core_idx: CoreIndex, group_idx: GroupIndex) -> Option<CoreAssignment> {
		<self::Pallet<T>>::take_on_next_core(core_idx, group_idx)
	}

	fn clear(scheduled: &[CoreAssignment]) {
		let config = <configuration::Pallet<T>>::config();

		<self::Pallet<T>>::clear(config.parathread_cores, config.parathread_retries, scheduled);
	}

	fn core_para(_core_index: CoreIndex, core_occupied: &CoreOccupied) -> ParaId {
		match core_occupied {
			CoreOccupied::Parathread(ref entry) => entry.claim.0,
			CoreOccupied::Parachain => panic!("impossible"),
		}
	}

	fn availability_timeout_predicate(
		_core_index: CoreIndex,
		blocks_since_last_rotation: T::BlockNumber,
		pending_since: T::BlockNumber,
	) -> bool {
		let config = <configuration::Pallet<T>>::config();

		if blocks_since_last_rotation >= config.thread_availability_period {
			false // no pruning except recently after rotation.
		} else {
			let now = <frame_system::Pallet<T>>::block_number();
			now.saturating_sub(pending_since) >= config.thread_availability_period
		}
	}

	fn next_up_on_available(core_idx: CoreIndex) -> Option<ScheduledCore> {
		<self::Pallet<T>>::next_up_on_available(core_idx)
	}

	fn next_up_on_time_out(
		core_idx: CoreIndex,
		cores: &[Option<CoreOccupied>],
	) -> Option<ScheduledCore> {
		<self::Pallet<T>>::next_up_on_time_out(core_idx, cores)
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

	/// A queue of upcoming claims and which core they should be mapped onto.
	///
	/// The number of queued claims is bounded at the `scheduling_lookahead`
	/// multiplied by the number of parathread multiplexer cores. Reasonably, 10 * 50 = 500.
	#[pallet::storage]
	pub(crate) type ParathreadQueue<T> = StorageValue<_, ParathreadClaimQueue, ValueQuery>;

	/// An index used to ensure that only one claim on a parathread exists in the queue or is
	/// currently being handled by an occupied core.
	///
	/// Bounded by the number of parathread cores and scheduling lookahead. Reasonably, 10 * 50 = 500.
	#[pallet::storage]
	pub(crate) type ParathreadClaimIndex<T> = StorageValue<_, Vec<ParaId>, ValueQuery>;
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
		_notification: &SessionChangeNotification<T::BlockNumber>,
		cores: &[Option<CoreOccupied>],
		n_parathread_cores: u32,
		n_parathread_retries: u32,
	) {
		let mut thread_queue = ParathreadQueue::<T>::get();
		for maybe_occupied in cores.iter().flatten() {
			if let CoreOccupied::Parathread(claim) = maybe_occupied {
				let queued = QueuedParathread {
					claim: claim.clone(),
					core_offset: 0, // this gets set later in the re-balancing.
				};

				thread_queue.queue.push(queued);
			}
		}

		// prune out all parathread claims with too many retries.
		// assign all non-pruned claims to new cores, if they've changed.
		ParathreadClaimIndex::<T>::mutate(|claim_index| {
			// wipe all parathread metadata if no parathread cores are configured.
			if n_parathread_cores == 0 {
				thread_queue = ParathreadClaimQueue { queue: Vec::new(), next_core_offset: 0 };
				claim_index.clear();
				return
			}

			// prune out all entries beyond retry or that no longer correspond to live parathread.
			thread_queue.queue.retain(|queued| {
				let will_keep = queued.claim.retries <= n_parathread_retries &&
					<paras::Pallet<T>>::is_parathread(queued.claim.claim.0);

				if !will_keep {
					let claim_para = queued.claim.claim.0;

					// clean up the pruned entry from the index.
					if let Ok(i) = claim_index.binary_search(&claim_para) {
						claim_index.remove(i);
					}
				}

				will_keep
			});

			// do re-balancing of claims.
			{
				for (i, queued) in thread_queue.queue.iter_mut().enumerate() {
					queued.core_offset = (i as u32) % n_parathread_cores;
				}

				thread_queue.next_core_offset =
					((thread_queue.queue.len()) as u32) % n_parathread_cores;
			}
		});

		ParathreadQueue::<T>::set(thread_queue);
	}

	/// Add a parathread claim to the queue. If there is a competing claim in the queue or currently
	/// assigned to a core, this call will fail. This call will also fail if the queue is full.
	///
	/// Fails if the claim does not correspond to any live parathread.
	#[allow(unused)]
	pub fn add_parathread_claim(claim: ParathreadClaim) {
		if !<paras::Pallet<T>>::is_parathread(claim.0) {
			return
		}

		let config = <configuration::Pallet<T>>::config();
		let queue_max_size = config.parathread_cores * config.scheduling_lookahead;

		ParathreadQueue::<T>::mutate(|queue| {
			if queue.queue.len() >= queue_max_size as usize {
				return
			}

			let para_id = claim.0;

			let competes_with_another =
				ParathreadClaimIndex::<T>::mutate(|index| match index.binary_search(&para_id) {
					Ok(_) => true,
					Err(i) => {
						index.insert(i, para_id);
						false
					},
				});

			if competes_with_another {
				return
			}

			let entry = ParathreadEntry { claim, retries: 0 };
			queue.enqueue_entry(entry, config.parathread_cores);
		})
	}

	/// Free unassigned cores. Provide a list of cores that should be considered newly-freed along with the reason
	/// for them being freed. The list is assumed to be sorted in ascending order by core index.
	pub(crate) fn free_cores(
		just_freed_cores: &BTreeMap<CoreIndex, FreedReason>,
		cores: &[Option<CoreOccupied>],
		n_parathread_cores: u32,
	) {
		for (freed_index, freed_reason) in just_freed_cores {
			if (freed_index.0 as usize) < cores.len() {
				match &cores[freed_index.0 as usize] {
					None => continue,
					Some(CoreOccupied::Parachain) => {},
					Some(CoreOccupied::Parathread(entry)) => {
						match freed_reason {
							FreedReason::Concluded => {
								// After a parathread candidate has successfully been included,
								// open it up for further claims!
								ParathreadClaimIndex::<T>::mutate(|index| {
									if let Ok(i) = index.binary_search(&entry.claim.0) {
										index.remove(i);
									}
								})
							},
							FreedReason::TimedOut => {
								// If a parathread candidate times out, it's not the collator's fault,
								// so we don't increment retries.
								ParathreadQueue::<T>::mutate(|queue| {
									queue.enqueue_entry(entry.clone(), n_parathread_cores);
								})
							},
						}
					},
				}
			}
		}
	}

	/// Schedule all unassigned cores, where possible. Provide a list of cores that should be considered
	/// newly-freed along with the reason for them being freed. The list is assumed to be sorted in
	/// ascending order by core index.
	pub(crate) fn take_on_next_core(
		core_idx: CoreIndex,
		group_idx: GroupIndex,
	) -> Option<CoreAssignment> {
		let mut parathread_queue = ParathreadQueue::<T>::get();

		// parathread core offset, rel. to beginning.
		let core_offset = (core_idx.0 as usize - <paras::Pallet<T>>::parachains().len()) as u32;
		let r = parathread_queue.take_next_on_core(core_offset).map(|entry| CoreAssignment {
			kind: AssignmentKind::Parathread(entry.claim.1, entry.retries),
			para_id: entry.claim.0,
			core: core_idx,
			group_idx,
		});

		ParathreadQueue::<T>::set(parathread_queue);

		return r
	}

	/// Return the next thing that will be scheduled on this core assuming it is currently
	/// occupied and the candidate occupying it became available.
	///
	/// For parachains, this is always the ID of the parachain and no specified collator.
	/// For parathreads, this is based on the next item in the `ParathreadQueue` assigned to that
	/// core, and is None if there isn't one.
	pub(crate) fn next_up_on_available(core: CoreIndex) -> Option<ScheduledCore> {
		let queue = ParathreadQueue::<T>::get();
		let core_offset = (core.0 as usize - <paras::Pallet<T>>::parachains().len()) as u32;
		queue.get_next_on_core(core_offset).map(|entry| ScheduledCore {
			para_id: entry.claim.0,
			collator: Some(entry.claim.1.clone()),
		})
	}

	/// Return the next thing that will be scheduled on this core assuming it is currently
	/// occupied and the candidate occupying it became available.
	///
	/// For parachains, this is always the ID of the parachain and no specified collator.
	/// For parathreads, this is based on the next item in the `ParathreadQueue` assigned to that
	/// core, or if there isn't one, the claim that is currently occupying the core, as long
	/// as the claim's retries would not exceed the limit. Otherwise None.
	pub(crate) fn next_up_on_time_out(
		core: CoreIndex,
		cores: &[Option<CoreOccupied>],
	) -> Option<ScheduledCore> {
		let queue = ParathreadQueue::<T>::get();

		// This is the next scheduled para on this core.
		let core_offset = (core.0 as usize - <paras::Pallet<T>>::parachains().len()) as u32;
		queue
			.get_next_on_core(core_offset)
			.map(|entry| ScheduledCore {
				para_id: entry.claim.0,
				collator: Some(entry.claim.1.clone()),
			})
			.or_else(|| {
				// Or, if none, the claim currently occupying the core,
				// as it would be put back on the queue after timing out.
				cores.get(core.0 as usize).and_then(|c| c.as_ref()).and_then(|o| {
					match o {
						CoreOccupied::Parathread(entry) => Some(ScheduledCore {
							para_id: entry.claim.0,
							collator: Some(entry.claim.1.clone()),
						}),
						CoreOccupied::Parachain => None, // defensive; not possible.
					}
				})
			})
	}

	// Free all scheduled cores and return parathread claims to queue, with retries incremented.
	pub(crate) fn clear(
		n_parathread_cores: u32,
		n_parathread_retries: u32,
		scheduled: &[CoreAssignment],
	) {
		ParathreadQueue::<T>::mutate(|queue| {
			for core_assignment in scheduled {
				if let AssignmentKind::Parathread(collator, retries) = &core_assignment.kind {
					if !<paras::Pallet<T>>::is_parathread(core_assignment.para_id) {
						continue
					}

					let entry = ParathreadEntry {
						claim: ParathreadClaim(core_assignment.para_id, collator.clone()),
						retries: retries + 1,
					};

					if entry.retries <= n_parathread_retries {
						queue.enqueue_entry(entry, n_parathread_cores);
					}
				}
			}
		});
	}
}
