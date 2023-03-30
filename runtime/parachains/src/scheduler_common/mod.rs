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
	CollatorId, CoreIndex, CoreOccupied, GroupIndex, Id as ParaId, ParathreadClaim,
	ParathreadEntry, ScheduledCore,
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

/// The assignment type.
#[derive(Clone, Encode, Decode, Debug, PartialEq, TypeInfo)]
//#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub enum AssignmentKind {
	/// A parachain.
	Parachain,
	/// A parathread.
	Parathread(CollatorId, u32), // u32 is retries
}

impl AssignmentKind {
	pub fn get_collator(&self) -> Option<CollatorId> {
		match self {
			AssignmentKind::Parachain => None,
			AssignmentKind::Parathread(collator_id, _) => Some(collator_id.clone()),
		}
	}
}

#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
pub enum Assignment {
	Parachain(ParaId),
	ParathreadA(ParathreadClaim, u32), // retries
}

impl Assignment {
	pub fn para_id(&self) -> ParaId {
		match self {
			Assignment::Parachain(para_id) => *para_id,
			Assignment::ParathreadA(ParathreadClaim(para_id, _), _) => *para_id,
		}
	}

	pub fn kind(&self) -> AssignmentKind {
		match self {
			Assignment::Parachain(_) => AssignmentKind::Parachain,
			Assignment::ParathreadA(ParathreadClaim(_, collator_id), retries) =>
				AssignmentKind::Parathread(collator_id.clone(), *retries),
		}
	}

	pub fn to_core_occupied(&self) -> CoreOccupied {
		match self {
			Assignment::Parachain(para_id) => CoreOccupied::Parachain(*para_id),
			Assignment::ParathreadA(claim, retries) => CoreOccupied::Parathread(ParathreadEntry {
				claim: claim.clone(),
				retries: *retries,
			}),
		}
	}

	pub fn from_core_occupied(co: CoreOccupied) -> Option<Assignment> {
		match co {
			CoreOccupied::Parachain(para_id) => Some(Assignment::Parachain(para_id)),
			CoreOccupied::Parathread(entry) =>
				Some(Assignment::ParathreadA(entry.claim, entry.retries)),
			CoreOccupied::Free => None,
		}
	}

	pub fn to_core_assignment(&self, core_idx: CoreIndex, group_idx: GroupIndex) -> CoreAssignment {
		CoreAssignment { core: core_idx, group_idx, kind: self.kind(), para_id: self.para_id() }
	}

	pub fn from_core_assignment(ca: CoreAssignment) -> Assignment {
		match ca.kind {
			AssignmentKind::Parachain => Assignment::Parachain(ca.para_id),
			AssignmentKind::Parathread(collator_id, retries) =>
				Assignment::ParathreadA(ParathreadClaim(ca.para_id, collator_id), retries),
		}
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

	fn get_availability_period(core_idx: CoreIndex) -> T::BlockNumber;
}

/// How a free core is scheduled to be assigned.
#[derive(Clone, Encode, Decode, PartialEq, Debug, TypeInfo)]
//#[cfg_attr(feature = "std", derive(PartialEq, Debug))]
pub struct CoreAssignment {
	/// The core that is assigned.
	pub core: CoreIndex,
	/// The unique ID of the para that is assigned to the core.
	pub para_id: ParaId,
	/// The kind of the assignment.
	pub kind: AssignmentKind,
	/// The index of the validator group assigned to the core.
	pub group_idx: GroupIndex,
}

impl CoreAssignment {
	pub fn new(
		core: CoreIndex,
		para_id: ParaId,
		kind: AssignmentKind,
		group_idx: GroupIndex,
	) -> Self {
		CoreAssignment { core, para_id, kind, group_idx }
	}

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
			AssignmentKind::Parachain => CoreOccupied::Parachain(self.para_id),
			AssignmentKind::Parathread(ref collator, retries) =>
				CoreOccupied::Parathread(ParathreadEntry {
					claim: ParathreadClaim(self.para_id, collator.clone()),
					retries,
				}),
		}
	}

	pub fn to_assignment(&self) -> Assignment {
		match self.kind.clone() {
			AssignmentKind::Parachain => Assignment::Parachain(self.para_id),
			AssignmentKind::Parathread(collator, retries) =>
				Assignment::ParathreadA(ParathreadClaim(self.para_id, collator), retries),
		}
	}

	pub fn to_scheduled_core(&self) -> ScheduledCore {
		ScheduledCore { para_id: self.para_id, collator: self.kind.get_collator() }
	}
}
