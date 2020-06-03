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
//!   - Paritioning validators into groups and assigning groups to parachains and parathreads
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

use sp_std::prelude::*;
use sp_std::convert::{TryFrom, TryInto};
use primitives::{
	parachain::{ValidatorId, Id as ParaId, CollatorId, ValidatorIndex},
};
use frame_support::{
	decl_storage, decl_module, decl_error,
	dispatch::DispatchResult,
	weights::{DispatchClass, Weight},
};
use codec::{Encode, Decode};
use system::ensure_root;
use sp_runtime::traits::Zero;

use rand::{SeedableRng, seq::SliceRandom};
use rand_chacha::ChaCha20Rng;

use crate::{configuration, paras, initializer::SessionChangeNotification};

/// The unique (during session) index of a core.
#[derive(Encode, Decode, Default, PartialOrd, Ord, Eq, PartialEq, Clone, Copy)]
pub struct CoreIndex(u32);

/// The unique (during session) index of a validator group.
#[derive(Encode, Decode, Default, Clone, Copy)]
pub struct GroupIndex(u32);

/// A claim on authoring the next block for a given parathread.
#[derive(Clone, Encode, Decode, Default)]
pub struct ParathreadClaim(pub ParaId, pub CollatorId);

/// An entry tracking a claim to ensure it does not pass the maximum number of retries.
#[derive(Clone, Encode, Decode, Default)]
pub struct ParathreadEntry {
	claim: ParathreadClaim,
	retries: u32,
}

/// A queued parathread entry, pre-assigned to a core.
#[derive(Encode, Decode, Default)]
pub struct QueuedParathread {
	claim: ParathreadEntry,
	core_offset: u32,
}

/// The queue of all parathread claims.
#[derive(Encode, Decode, Default)]
pub struct ParathreadClaimQueue {
	queue: Vec<QueuedParathread>,
	// this value is between 0 and config.parathread_cores
	next_core_offset: u32,
}

impl ParathreadClaimQueue {
	// Queue a parathread entry to be processed.
	//
	// Provide the entry and the number of parathread cores, which must be greater than 0.
	fn queue_entry(&mut self, entry: ParathreadEntry, n_parathread_cores: u32) {
		let core_offset = self.next_core_offset;
		self.next_core_offset = (self.next_core_offset + 1) % n_parathread_cores;

		self.queue.push(QueuedParathread {
			claim: entry,
			core_offset,
		})
	}

	// Take next queued entry with given core offset, if any.
	fn take_next_on_core(&mut self, core_offset: u32) -> Option<ParathreadEntry> {
		let pos = self.queue.iter().position(|queued| queued.core_offset == core_offset);
		pos.map(|i| self.queue.remove(i).claim)
	}
}

/// What is occupying a specific availability core.
#[derive(Clone, Encode, Decode)]
pub(crate) enum CoreOccupied {
	Parathread(ParathreadEntry),
	Parachain,
}

/// The assignment type.
#[derive(Clone, Encode, Decode)]
pub enum AssignmentKind {
	Parachain,
	Parathread(CollatorId, u32),
}

/// How a free core is scheduled to be assigned.
#[derive(Encode, Decode)]
pub struct CoreAssignment {
	/// The core that is assigned.
	pub core: CoreIndex,
	/// The unique ID of the para that is assigned to the core.
	pub para_id: ParaId,
	/// The kind of the assigment.
	pub kind: AssignmentKind,
	/// The index of the validator group assigned to the core.
	pub group_idx: GroupIndex,
}

impl CoreAssignment {
	/// Get the ID of a collator who is required to collate this block.
	pub(crate) fn required_collator(&self) -> Option<&CollatorId> {
		match self.kind {
			AssignmentKind::Parachain => None,
			AssignmentKind::Parathread(ref id, _) => Some(id),
		}
	}
}

/// Reasons a core might be freed
pub enum FreedReason {
	/// The core's work concluded and the parablock assigned to it is considered available.
	Concluded,
	/// The core's work timed out.
	TimedOut,
}

pub trait Trait: system::Trait + configuration::Trait + paras::Trait { }

decl_storage! {
	trait Store for Module<T: Trait> as Scheduler {
		/// All the validator groups. One for each core.
		ValidatorGroups: Vec<Vec<ValidatorIndex>>;
		/// A queue of upcoming claims and which core they should be mapped onto.
		ParathreadQueue: ParathreadClaimQueue;
		/// One entry for each availability core. Entries are `None` if the core is not currently occupied. Can be
		/// temporarily `Some` if scheduled but not occupied.
		/// The i'th parachain belongs to the i'th core, with the remaining cores all being
		/// parathread-multiplexers.
		AvailabilityCores: Vec<Option<CoreOccupied>>;
		/// An index used to ensure that only one claim on a parathread exists in the queue or is
		/// currently being handled by an occupied core.
		ParathreadClaimIndex: Vec<ParaId>;
		/// The block number where the session start occurred. Used to track how many group rotations have occurred.
		SessionStartBlock: T::BlockNumber;
		/// Currently scheduled cores - free but up to be occupied. Ephemeral storage item that's wiped on finalization.
		Scheduled get(fn scheduled): Vec<CoreAssignment>; // sorted ascending by CoreIndex.
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The scheduler module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		type Error = Error<T>;
	}
}

impl<T: Trait> Module<T> {
	/// Called by the initializer to initialize the scheduler module.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		Self::schedule(Vec::new());

		0
	}

	/// Called by the initializer to finalize the scheduler module.
	pub(crate) fn initializer_finalize() {
		// Free all scheduled cores and return parathread claims to queue, with retries incremented.
		let config = <configuration::Module<T>>::config();
		ParathreadQueue::mutate(|queue| {
			for core_assignment in Scheduled::take() {
				if let AssignmentKind::Parathread(collator, retries) = core_assignment.kind {
					let entry = ParathreadEntry {
						claim: ParathreadClaim(core_assignment.para_id, collator),
						retries: retries + 1,
					};

					if entry.retries > config.parathread_retries { continue }
					queue.queue_entry(entry, config.parathread_cores);
				}
			}
		})

	 }

	/// Called by the initializer to note that a new session has started.
	pub(crate) fn initializer_on_new_session(notification: &SessionChangeNotification<T::BlockNumber>) {
		let &SessionChangeNotification {
			ref validators,
			ref random_seed,
			ref new_config,
			..
		} = notification;
		let config = new_config;

		let mut thread_queue = ParathreadQueue::get();
		let n_parachains = <paras::Module<T>>::parachains().len() as u32;
		let n_cores = n_parachains + config.parathread_cores;

		<SessionStartBlock<T>>::set(<system::Module<T>>::block_number());
		AvailabilityCores::mutate(|cores| {
			// clear all occupied cores.
			for maybe_occupied in cores.iter_mut() {
				if let Some(CoreOccupied::Parathread(claim)) = maybe_occupied.take() {
					let queued = QueuedParathread {
						claim,
						core_offset: 0, // this gets set later in the re-balancing.
					};

					thread_queue.queue.push(queued);
				}
			}

			cores.resize(n_cores as _, None);
		});

		// shuffle validators into groups.
		if n_cores == 0 || validators.is_empty() {
			ValidatorGroups::set(Vec::new());
		} else {
			let mut rng: ChaCha20Rng = SeedableRng::from_seed(notification.random_seed);

			let mut shuffled_indices: Vec<_> = (0..validators.len())
				.enumerate()
				.map(|(i, _)| i as ValidatorIndex)
				.collect();

			shuffled_indices.shuffle(&mut rng);

			let group_base_size = validators.len() / n_cores as usize;
			let larger_groups = validators.len() % n_cores as usize;
			let groups: Vec<Vec<_>> = (0..n_cores).map(|core_id| {
				let n_members = if (core_id as usize) < larger_groups {
					group_base_size + 1
				} else {
					group_base_size
				};

				shuffled_indices.drain(shuffled_indices.len() - n_members ..).rev().collect()
			}).collect();

			ValidatorGroups::set(groups);
		}

		// prune out all parathread claims with too many retries.
		// assign all non-pruned claims to new cores, if they've changed.
		ParathreadClaimIndex::mutate(|claim_index| {
			// wipe all parathread metadata if no parathread cores are configured.
			if config.parathread_cores == 0 {
				thread_queue = ParathreadClaimQueue {
					queue: Vec::new(),
					next_core_offset: 0,
				};
				claim_index.clear();
				return;
			}

			// prune out all entries beyond retry.
			thread_queue.queue.retain(|queued| {
				let will_keep = queued.claim.retries <= config.parathread_retries;

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
					queued.core_offset = (i as u32) % config.parathread_cores;
				}

				thread_queue.next_core_offset =
					((thread_queue.queue.len() + 1) as u32) % config.parathread_cores;
			}
		});
		ParathreadQueue::set(thread_queue);
	}

	/// Add a parathread claim to the queue. If there is a competing claim in the queue or currently
	/// assigned to a core, this call will fail. This call will also fail if the queue is full.
	pub fn add_parathread_claim(claim: ParathreadClaim) {
		let config = <configuration::Module<T>>::config();
		let queue_max_size = config.parathread_cores * config.scheduling_lookahead;

		ParathreadQueue::mutate(|queue| {
			if queue.queue.len() >= queue_max_size as usize { return }

			let para_id = claim.0;

			let competes_with_another = ParathreadClaimIndex::mutate(|index| {
				match index.binary_search(&para_id) {
					Ok(_) => true,
					Err(i) => {
						index.insert(i, para_id);
						false
					}
				}
			});

			if competes_with_another { return }

			let entry = ParathreadEntry { claim, retries: 0 };
			queue.queue_entry(entry, config.parathread_cores);
		})
	}

	pub(crate) fn schedule(just_freed_cores: Vec<(CoreIndex, FreedReason)>) {
		let mut cores = AvailabilityCores::get();
		let config = <configuration::Module<T>>::config();

		for (freed_index, freed_reason) in just_freed_cores {
			if (freed_index.0 as usize) < cores.len() {
				match cores[freed_index.0 as usize].take() {
					None => continue,
					Some(CoreOccupied::Parachain) => {},
					Some(CoreOccupied::Parathread(entry)) => {
						match freed_reason {
							FreedReason::Concluded => {
								// After a parathread candidate has successfully been included,
								// open it up for further claims!
								ParathreadClaimIndex::mutate(|index| {
									if let Ok(i) = index.binary_search(&entry.claim.0) {
										index.remove(i);
									}
								})
							}
							FreedReason::TimedOut => {
								// If a parathread candidate times out, it's not the collator's fault,
								// so we don't increment retries.
								ParathreadQueue::mutate(|queue| {
									queue.queue_entry(entry, config.parathread_cores);
								})
							}
						}
					}
				}
			}
		}

		let parachains = <paras::Module<T>>::parachains();
		let mut scheduled = Scheduled::get();
		let mut parathread_queue = ParathreadQueue::get();
		let now = <system::Module<T>>::block_number();

		{
			let mut prev_scheduled_in_order = scheduled.iter().enumerate().peekable();

			// Updates to the previous list of scheduled updates and the position of where to insert
			// them, without accounting for prior updates.
			let mut scheduled_updates: Vec<(usize, CoreAssignment)> = Vec::new();

			// single-sweep O(n) in the number of cores.
			for (core_index, _core) in cores.iter().enumerate().filter(|(_, ref c)| c.is_none()) {
				let schedule_and_insert_at = {
					// advance the iterator until just before the core index we are looking at now.
					while prev_scheduled_in_order.peek().map_or(
						false,
						|(_, assign)| (assign.core.0 as usize) < core_index,
					) {
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
						|(idx_in_scheduled, assign)| if (assign.core.0 as usize) == core_index {
							None
						} else {
							Some(*idx_in_scheduled)
						},
					)
				};

				let schedule_and_insert_at = match schedule_and_insert_at {
					None => continue,
					Some(at) => at,
				};

				let core = CoreIndex(core_index as u32);

				let core_assignment = if core_index < parachains.len() {
					// parachain core.
					Some(CoreAssignment {
						kind: AssignmentKind::Parachain,
						para_id: parachains[core_index],
						core: core.clone(),
						group_idx: Self::group_assigned_to_core(core, now)
							.expect("core is not out of bounds and we are guaranteed \
									to be after the most recent session start; qed"),
					})
				} else {
					// parathread core offset, rel. to beginning.
					let core_offset = (core_index - parachains.len()) as u32;

					parathread_queue.take_next_on_core(core_offset).map(|entry| CoreAssignment {
						kind: AssignmentKind::Parathread(entry.claim.1, entry.retries),
						para_id: entry.claim.0,
						core: core.clone(),
						group_idx: Self::group_assigned_to_core(core, now)
							.expect("core is not out of bounds and we are guaranteed \
									to be after the most recent session start; qed"),
					})
				};

				if let Some(assignment) = core_assignment {
					scheduled_updates.push((schedule_and_insert_at, assignment))
				}
			}

			// at this point, because `Scheduled` is guaranteed to be sorted and we navigated unassigned
			// core indices in ascending order, we can enact the updates prepared by the previous actions.
			//
			// while inserting, we have to account for the amount of insertions already done.
			//
			// This is O(n) as well, capped at n operations, where n is the number of cores.
			for (num_insertions_before, (insert_at, to_insert)) in scheduled_updates.into_iter().enumerate() {
				let insert_at = num_insertions_before + insert_at;
				scheduled.insert(insert_at, to_insert);
			}

			// scheduled is guaranteed to be sorted after this point because it was sorted before, and we
			// applied sorted updates at their correct positions, accounting for the offsets of previous
			// insertions.
		}

		Scheduled::set(scheduled);
		ParathreadQueue::set(parathread_queue);
	}

	/// Get the para (chain or thread) ID assigned to a particular core or index, if any. Core indices
	/// out of bounds will return `None`, as will indices of unassigned cores.
	pub(crate) fn core_para(core_index: CoreIndex) -> Option<ParaId> {
		let cores = AvailabilityCores::get();
		match cores.get(core_index.0 as usize).and_then(|c| c.as_ref()) {
			None => None,
			Some(CoreOccupied::Parachain) => {
				let parachains = <paras::Module<T>>::parachains();
				Some(parachains[core_index.0 as usize])
			}
			Some(CoreOccupied::Parathread(ref entry)) => Some(entry.claim.0),
		}
	}

	/// Get the validators in the given group, if the group index is valid for this session.
	pub(crate) fn group_validators(group_index: GroupIndex) -> Option<Vec<ValidatorIndex>> {
		ValidatorGroups::get().get(group_index.0 as usize).map(|g| g.clone())
	}

	/// Get the group assigned to a specific core by index at the current block number. Result undefined if the core index is unknown
	/// or the block number is less than the session start index.
	pub(crate) fn group_assigned_to_core(core: CoreIndex, at: T::BlockNumber) -> Option<GroupIndex> {
		let config = <configuration::Module<T>>::config();
		let session_start_block = <SessionStartBlock<T>>::get();

		if at < session_start_block { return None }

		if config.parachain_rotation_frequency.is_zero() {
			// interpret this as "no rotations"
			return Some(GroupIndex(core.0));
		}

		let validator_groups = ValidatorGroups::get();

		if core.0 as usize >= validator_groups.len() { return None }

		let rotations_since_session_start: T::BlockNumber =
			(at - session_start_block) / config.parachain_rotation_frequency.into();

		let rotations_since_session_start
			= match <T::BlockNumber as TryInto<u32>>::try_into(rotations_since_session_start)
		{
			Ok(i) => i,
			Err(_) => 0, // can only happen if rotations occur only once every u32::max(),
			             // so functionally no difference in behavior.
		};

		let group_idx = (core.0 as usize + rotations_since_session_start as usize) % validator_groups.len();
		Some(GroupIndex(group_idx as u32))
	}
}
