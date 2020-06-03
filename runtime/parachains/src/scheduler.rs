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

use rand::{SeedableRng, seq::SliceRandom};
use rand_chacha::ChaCha20Rng;

use crate::{configuration, paras, initializer::SessionChangeNotification};

/// The unique (during session) index of a core.
#[derive(Encode, Decode, Default)]
pub(crate) struct CoreIndex(u32);

/// The unique (during session) index of a validator group.
#[derive(Encode, Decode, Default)]
pub(crate) struct GroupIndex(u32);

/// A claim on authoring the next block for a given parathread.
#[derive(Clone, Encode, Decode, Default)]
pub(crate) struct ParathreadClaim(ParaId, CollatorId);

/// An entry tracking a claim to ensure it does not pass the maximum number of retries.
#[derive(Clone, Encode, Decode, Default)]
pub(crate) struct ParathreadEntry {
	claim: ParathreadClaim,
	retries: u32,
}

/// A queued parathread entry, pre-assigned to a core.
#[derive(Encode, Decode, Default)]
pub(crate) struct QueuedParathread {
	claim: ParathreadEntry,
	core_offset: u32,
}

/// The queue of all parathread claims.
#[derive(Encode, Decode, Default)]
pub(crate) struct ParathreadClaimQueue {
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
}

/// What is occupying a specific availability core.
#[derive(Clone, Encode, Decode)]
pub(crate) enum CoreOccupied {
	Parathread(ParathreadEntry),
	Parachain,
}

/// The assignment type.
#[derive(Clone, Encode, Decode)]
pub(crate) enum AssignmentKind {
	Parachain,
	Parathread(CollatorId, u32),
}

/// How a free core is scheduled to be assigned.
#[derive(Encode, Decode)]
pub(crate) struct CoreAssignment {
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
		Scheduled: Vec<CoreAssignment>; // sorted ascending by CoreIndex.
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

	pub(crate) fn schedule(just_freed_cores: Vec<CoreIndex>) {
		// TODO [now]: schedule new core assignments.
	}
}
