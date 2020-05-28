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
//! The Scheduler manages resource allocation using the concept of "Execution Cores".
//! There will be one execution core for each parachain, and a fixed number of cores
//! used for multiplexing parathreads. Validators will be partitioned into groups, with the same
//! number of groups as execution cores. Validator groups will be assigned to different execution cores
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

use crate::{configuration, paras};

// A claim on authorship for a specific parathread.
#[derive(Encode, Decode)]
struct ParathreadClaim(ParaId, CollatorId);

// A parathread that is scheduled onto a specific core.
#[derive(Encode, Decode)]
struct ParathreadEntry {
	claim: ParathreadClaim,
	core: CoreIndex,
}

// what a core is occupied by
#[derive(Encode, Decode)]
enum CoreOccupied {
	Parathread(ParathreadClaim, u32), // claim & retries
	Parachain,
}

/// The unique (during session) index of a core.
#[derive(Encode, Decode)]
pub(crate) struct CoreIndex(u32);

pub trait Trait: system::Trait + configuration::Trait + paras::Trait { }

decl_storage! {
	trait Store for Module<T: Trait> as Scheduler {
		/// All of the validator groups, one for each core.
		ValidatorGroups: Vec<Vec<ValidatorIndex>>;
		/// A queue of upcoming claims and which core they should be mapped onto.
		ParathreadQueue: Vec<ParathreadEntry>;
		/// One entry for each execution core. Entries are `None` if the core is not currently occupied.
		/// Can be temporarily `Some` if scheduled but not occupied.
		/// The i'th parachain belongs to the i'th core, with the remaining cores all being
		/// parathread-multiplexers.
		ExecutionCores: Vec<Option<CoreOccupied>>;
		/// An index used to ensure that only one claim on a parathread exists in the queue or retry queue
		/// or is currently being handled by an occupied core.
		ParathreadClaimIndex: Vec<(ParaId, CollatorId)>;
		/// The block number where the session start occurred. Used to track how many group rotation have
		/// occurred.
		SessionStartBlock: BlockNumber,
		/// Currently scheduled cores - free but up to be occupied. Ephemeral storage item that's wiped on
		/// finalization.
		Scheduled get(fn scheduled): Vec<CoreAssignment>, // sorted by ParaId
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
	pub(crate) fn initializer_on_new_session(_validators: &[ValidatorId], _queued: &[ValidatorId]) {
		let config = <configuration::Module<T>>::config();

		SessionStartBlock::set(<system::Module<T>>::block_number());
		ExecutionCores::mutate(|cores| {
			// clear all occupied cores.
			for maybe_occupied in cores.iter_mut() {
				if let Some(CoreOccupied::Parathread(claim, retries)) = maybe_occupied.take() {
					// TODO [now]: return to parathread queue, do not increment retries
				}
			}

			let n_parachains = <paras::Module<T>>::parachains().len();

			cores.resize(n_parachains + config.parathread_cores, None);
		});

		// TODO [now]: shuffle validators into groups

		// TODO [now]: prune out all parathread claims with too many retries.
	}

	pub(crate) fn schedule(just_freed_cores: Vec<CoreIndex>) {
		// TODO [now]: schedule new core assignments.
	}
}
