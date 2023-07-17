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
use frame_system::pallet_prelude::BlockNumberFor;
use primitives::{
	CollatorId, CoreIndex, CoreOccupied, GroupIndex, Id as ParaId, ParathreadEntry, ScheduledCore,
};
use scale_info::TypeInfo;
use sp_std::prelude::*;

/// Reasons a core might be freed
#[derive(Clone, Copy)]
pub enum FreedReason {
	/// The core's work concluded and the parablock assigned to it is considered available.
	Concluded,
	/// The core's work timed out.
	TimedOut,
}

#[derive(Clone, Encode, Decode, PartialEq, TypeInfo)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum Assignment {
	Parachain(ParaId),
	ParathreadA(ParathreadEntry),
}

impl Assignment {
	pub fn para_id(&self) -> ParaId {
		match self {
			Assignment::Parachain(para_id) => *para_id,
			Assignment::ParathreadA(entry) => entry.claim.0,
		}
	}

	pub fn get_collator(&self) -> Option<CollatorId> {
		match self {
			Assignment::Parachain(_) => None,
			Assignment::ParathreadA(entry) => entry.claim.1.clone(),
		}
	}

	// Note: this happens on session change. We don't rescheduled pay-as-you-go parachains if they have been tried to run at least once
	pub fn from_core_occupied(co: CoreOccupied) -> Option<Assignment> {
		match co {
			CoreOccupied::Parachain(para_id) => Some(Assignment::Parachain(para_id)),
			CoreOccupied::Parathread(entry) =>
				if entry.retries > 0 {
					None
				} else {
					Some(Assignment::ParathreadA(entry))
				},
			CoreOccupied::Free => None,
		}
	}

	pub fn to_core_assignment(self, core_idx: CoreIndex, group_idx: GroupIndex) -> CoreAssignment {
		CoreAssignment { core: core_idx, group_idx, kind: self }
	}

	pub fn from_core_assignment(ca: CoreAssignment) -> Assignment {
		ca.kind
	}
}

pub trait AssignmentProvider<T: crate::scheduler::pallet::Config> {
	fn session_core_count() -> u32;

	fn new_session();

	fn pop_assignment_for_core(
		core_idx: CoreIndex,
		concluded_para: Option<ParaId>,
	) -> Option<Assignment>;

	// on session change
	fn push_assignment_for_core(core_idx: CoreIndex, assignment: Assignment);

	fn get_availability_period(core_idx: CoreIndex) -> BlockNumberFor<T>;

	fn get_max_retries(core_idx: CoreIndex) -> u32;
}

/// How a free core is scheduled to be assigned.
#[derive(Clone, Encode, Decode, PartialEq, TypeInfo)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct CoreAssignment {
	/// The core that is assigned.
	pub core: CoreIndex,
	/// The kind of the assignment.
	pub kind: Assignment,
	/// The index of the validator group assigned to the core.
	pub group_idx: GroupIndex,
}

impl CoreAssignment {
	pub fn new(core: CoreIndex, kind: Assignment, group_idx: GroupIndex) -> Self {
		CoreAssignment { core, kind, group_idx }
	}

	/// Get the ID of a collator who is required to collate this block.
	pub fn required_collator(&self) -> Option<&CollatorId> {
		match &self.kind {
			Assignment::Parachain(_) => None,
			Assignment::ParathreadA(entry) => entry.claim.1.as_ref(),
		}
	}

	/// Get the `CoreOccupied` from this.
	pub fn to_core_occupied(self) -> CoreOccupied {
		match self.kind {
			Assignment::Parachain(para_id) => CoreOccupied::Parachain(para_id),
			Assignment::ParathreadA(entry) => CoreOccupied::Parathread(entry),
		}
	}

	pub fn to_scheduled_core(self) -> ScheduledCore {
		ScheduledCore { para_id: self.kind.para_id(), collator: self.kind.get_collator() }
	}
}
