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
	core: CoreIndex,
}

/// The queue of all parathread claims.
#[derive(Encode, Decode, Default)]
pub(crate) struct ParathreadClaimQueue {
	queue: Vec<QueuedParathread>,
	// this value is between 0 and config.parathread_cores
	next_core: CoreIndex,
}

/// What is occupying a specific availability core.
#[derive(Clone, Encode, Decode)]
pub(crate) enum CoreOccupied {
  Parathread(ParathreadEntry),
  Parachain,
}

/// How a free core is scheduled to be assigned.
#[derive(Encode, Decode, Default)]
pub(crate) struct CoreAssignment {
  core: CoreIndex,
  para_id: ParaId,
  collator: Option<CollatorId>,
  group_idx: GroupIndex,
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
		// TODO [now]: free all scheduled cores and return parathread claims to queue, with retries incremented.
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
						core: CoreIndex(0), // this gets set later in the re-balancing.
					};

					thread_queue.queue.push(queued);
				}
			}

			cores.resize(n_cores as _, None);
		});

		// TODO [now]: shuffle validators into groups
		if n_cores == 0 || validators.is_empty() {
			ValidatorGroups::set(Vec::new());
		} else {
			let group_base_size = validators.len() as u32 / n_cores;
		}

		// TODO [now]: prune out all parathread claims with too many retries.
		ParathreadQueue::set(thread_queue);
	}

	pub(crate) fn schedule(just_freed_cores: Vec<CoreIndex>) {
		// TODO [now]: schedule new core assignments.
	}
}
