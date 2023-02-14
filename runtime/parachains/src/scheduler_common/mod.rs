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
	CollatorId, CoreIndex, CoreOccupied, GroupIndex, Id as ParaId, ParathreadClaim, ParathreadEntry,
};
use scale_info::TypeInfo;
use sp_std::prelude::*;

//use crate::initializer::SessionChangeNotification;

//#[cfg(test)]
//mod tests;

/// Reasons a core might be freed
#[derive(Clone, Copy)]
pub enum FreedReason {
	/// The core's work concluded and the parablock assigned to it is considered available.
	Concluded,
	/// The core's work timed out.
	TimedOut,
}

/// The assignment type.
#[derive(Clone, Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub enum AssignmentKind {
	/// A parachain.
	Parachain,
	/// A parathread.
	Parathread(CollatorId, u32),
}

#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
pub enum Assignment {
	Parachain(ParaId),
	ParathreadA(ParathreadClaim),
}

impl Assignment {
	pub fn para_id(self) -> ParaId {
		match self {
			Assignment::Parachain(para_id) => para_id,
			Assignment::ParathreadA(ParathreadClaim(para_id, _)) => para_id,
		}
	}

	pub fn kind(self) -> AssignmentKind {
		match self {
			Assignment::Parachain(_) => AssignmentKind::Parachain,
			Assignment::ParathreadA(ParathreadClaim(_, collator_id)) =>
				AssignmentKind::Parathread(collator_id, 3),
		}
	}
}

pub trait AssignmentProvider<T: crate::scheduler::pallet::Config> {
	fn on_new_session(n_lookahead: u32);

	fn session_core_count() -> u32;

	fn pop_assignment_for_core(core_idx: CoreIndex) -> Option<Assignment>;

	fn peek_assignment_for_core(core_idx: CoreIndex) -> Option<Assignment>;

	fn push_assignment_for_core(core_idx: CoreIndex, assignment: Assignment);

	fn push_front_assignment_for_core(core_idx: CoreIndex, assignment: Assignment);

	fn core_para(core_idx: CoreIndex, core_occupied: &CoreOccupied) -> ParaId;

	fn get_availability_period(core_idx: CoreIndex) -> T::BlockNumber;
}

/// How a free core is scheduled to be assigned.
#[derive(Clone, Encode, Decode, TypeInfo)]
#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub struct CoreAssignment {
	/// The core that is assigned.
	pub core: CoreIndex,
	/// The unique ID of the para that is assigned to the core.
	pub para_id: ParaId, // TODO: move this into AssigmentKind
	/// The kind of the assignment.
	pub kind: AssignmentKind,
	/// The index of the validator group assigned to the core.
	pub group_idx: GroupIndex,
}

impl CoreAssignment {
	/// Get the ID of a collator who is required to collate this block.
	pub fn required_collator(&self) -> Option<&CollatorId> {
		match self.kind {
			AssignmentKind::Parachain => None,
			AssignmentKind::Parathread(ref id, _) => Some(id),
		}
	}

	/// Get the `CoreOccupied` from this.
	pub fn to_core_occupied(&self) -> CoreOccupied {
		match self.kind {
			AssignmentKind::Parachain => CoreOccupied::Parachain,
			AssignmentKind::Parathread(ref collator, retries) =>
				CoreOccupied::Parathread(ParathreadEntry {
					claim: ParathreadClaim(self.para_id, collator.clone()),
					retries,
				}),
		}
	}
}
