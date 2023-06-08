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

//! Staging Primitives.

// Put any primitives used by staging APIs functions here
pub use crate::{v4, v4::*};
pub mod slashing;
use bitvec::prelude::BitVec;
use parity_scale_codec::{Decode, Encode};
use primitives::{OpaquePeerId, RuntimeDebug};
use scale_info::TypeInfo;
use sp_std::{collections::btree_set::BTreeSet, prelude::*};

#[cfg(feature = "std")]
use sc_network::PeerId;

/// Candidate's acceptance limitations for asynchronous backing per relay parent.
#[derive(
	RuntimeDebug,
	Copy,
	Clone,
	PartialEq,
	Encode,
	Decode,
	TypeInfo,
	serde::Serialize,
	serde::Deserialize,
)]
pub struct AsyncBackingParams {
	/// The maximum number of para blocks between the para head in a relay parent
	/// and a new candidate. Restricts nodes from building arbitrary long chains
	/// and spamming other validators.
	///
	/// When async backing is disabled, the only valid value is 0.
	pub max_candidate_depth: u32,
	/// How many ancestors of a relay parent are allowed to build candidates on top
	/// of.
	///
	/// When async backing is disabled, the only valid value is 0.
	pub allowed_ancestry_len: u32,
}

/// What is occupying a specific availability core.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub enum CoreOccupied {
	/// The core is not occupied.
	Free,
	/// A paras.
	Paras(ParasEntry),
}

impl CoreOccupied {
	/// Is core free?
	pub fn is_free(&self) -> bool {
		match self {
			Self::Free => true,
			Self::Paras(_) => false,
		}
	}
}

/// An Assignemnt for a paras going to produce a paras block.
#[derive(Clone, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug)]
pub struct Assignment {
	/// Assignment's ParaId
	pub para_id: Id,
	/// Assignment's CollatorRestrictions
	pub collator_restrictions: CollatorRestrictions,
}

impl Assignment {
	/// Create a new `Assignment`.
	pub fn new(para_id: Id, collator_restrictions: CollatorRestrictions) -> Self {
		Assignment { para_id, collator_restrictions }
	}
}

/// Restrictions on collators for a specific paras block.
#[derive(Clone, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug)]
pub struct CollatorRestrictions {
	/// Collators to prefer/allow.
	/// Empty set means no restrictions.
	collator_peer_ids: BTreeSet<OpaquePeerId>,
	restriction_kind: CollatorRestrictionKind,
}

impl CollatorRestrictions {
	/// Specialised new function for parachains.
	pub fn none() -> Self {
		CollatorRestrictions {
			collator_peer_ids: BTreeSet::new(),
			restriction_kind: CollatorRestrictionKind::Preferred,
		}
	}

	/// Create a new `CollatorRestrictions`.
	pub fn new(
		collator_peer_ids: BTreeSet<OpaquePeerId>,
		restriction_kind: CollatorRestrictionKind,
	) -> Self {
		CollatorRestrictions { collator_peer_ids, restriction_kind }
	}

	/// Is `peer_id` allowed to collate?
	#[cfg(feature = "std")]
	pub fn can_collate(&self, peer_id: &PeerId) -> bool {
		self.collator_peer_ids.is_empty() ||
			match self.restriction_kind {
				CollatorRestrictionKind::Preferred => true,
				CollatorRestrictionKind::Required => {
					let peer_id = OpaquePeerId(peer_id.to_bytes());
					self.collator_peer_ids.contains(&peer_id)
				},
			}
	}
}

/// How to apply the collator restrictions.
#[derive(Clone, Encode, Decode, PartialEq, TypeInfo, RuntimeDebug)]
pub enum CollatorRestrictionKind {
	/// peer ids mentioned will be preferred in connections, but others are still allowed.
	Preferred,
	/// Any collator with a `PeerId` not in the set of `CollatorRestrictions` will be rejected.
	Required,
}

/// An entry tracking a paras
#[derive(Clone, Encode, Decode, TypeInfo, PartialEq, RuntimeDebug)]
pub struct ParasEntry {
	/// The `Assignment`
	pub assignment: Assignment,
	/// Number of times this has been retried.
	pub retries: u32,
}

impl From<Assignment> for ParasEntry {
	fn from(assignment: Assignment) -> Self {
		Self::new(assignment)
	}
}

impl ParasEntry {
	/// Create a new `ParasEntry`.
	pub fn new(assignment: Assignment) -> Self {
		ParasEntry { assignment, retries: 0 }
	}

	/// Return `Id` from the underlying `Assignment`.
	pub fn para_id(&self) -> Id {
		self.assignment.para_id
	}

	/// Return `CollatorRestrictions` from the underlying `Assignment`.
	pub fn collator_restrictions(&self) -> &CollatorRestrictions {
		&self.assignment.collator_restrictions
	}
}

/// Information about a core which is currently occupied.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct OccupiedCore<H = Hash, N = BlockNumber> {
	// NOTE: this has no ParaId as it can be deduced from the candidate descriptor.
	/// If this core is freed by availability, this is the assignment that is next up on this
	/// core, if any. None if there is nothing queued for this core.
	pub next_up_on_available: Option<ScheduledCore>,
	/// The relay-chain block number this began occupying the core at.
	pub occupied_since: N,
	/// The relay-chain block this will time-out at.
	pub time_out_at: N,
	/// If this core is freed by being timed-out, this is the assignment that is next up on this
	/// core. None if there is nothing queued for this core or the currently occupying assignment
	/// already has exhausted its retry counter.
	pub next_up_on_time_out: Option<ScheduledCore>,
	/// A bitfield with 1 bit for each validator in the set. `1` bits mean that the corresponding
	/// validators has attested to availability on-chain. A 2/3+ majority of `1` bits means that
	/// this will be available.
	pub availability: BitVec<u8, bitvec::order::Lsb0>,
	/// The group assigned to distribute availability pieces and backing of this candidate.
	pub group_responsible: GroupIndex,
	/// The hash of the candidate occupying the core.
	pub candidate_hash: CandidateHash,
	/// The descriptor of the candidate occupying the core.
	pub candidate_descriptor: CandidateDescriptor<H>,
}

impl<H, N> OccupiedCore<H, N> {
	/// Get the Para currently occupying this core.
	pub fn para_id(&self) -> Id {
		self.candidate_descriptor.para_id
	}
}

/// Upgrade a v4 OccupiedCore to a vstaging one.
pub fn occupied_core_from_v4(oc: v4::OccupiedCore) -> OccupiedCore {
	OccupiedCore {
		next_up_on_available: oc.next_up_on_available.map(scheduled_core_from_v4),
		occupied_since: oc.occupied_since,
		time_out_at: oc.time_out_at,
		next_up_on_time_out: oc.next_up_on_time_out.map(scheduled_core_from_v4),
		availability: oc.availability,
		group_responsible: oc.group_responsible,
		candidate_hash: oc.candidate_hash,
		candidate_descriptor: oc.candidate_descriptor,
	}
}

/// Information about a core which is currently occupied.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub struct ScheduledCore {
	// TODO: Is the same as Assignment
	/// The ID of a para scheduled.
	pub para_id: Id,
	/// The collator restrictions.
	pub collator_restrictions: CollatorRestrictions,
}

impl ScheduledCore {
	/// Converts its current form of itself to the v4 version of itself
	pub fn to_v4(self) -> crate::v4::ScheduledCore {
		crate::v4::ScheduledCore { para_id: self.para_id, collator: None }
	}
}

/// Upgrade a v4 ScheduledCore to a vstaging one.
pub fn scheduled_core_from_v4(sc: v4::ScheduledCore) -> ScheduledCore {
	ScheduledCore {
		para_id: sc.para_id,
		collator_restrictions: CollatorRestrictions::none(), // v4 is all leased parachains
	}
}

/// The state of a particular availability core.
#[derive(Clone, Encode, Decode, TypeInfo, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub enum CoreState<H = Hash, N = BlockNumber> {
	/// The core is currently occupied.
	#[codec(index = 0)]
	Occupied(OccupiedCore<H, N>),
	/// The core is currently free, with a para scheduled and given the opportunity
	/// to occupy.
	///
	/// If a particular Collator is required to author this block, that is also present in this
	/// variant.
	#[codec(index = 1)]
	Scheduled(ScheduledCore),
	/// The core is currently free and there is nothing scheduled. This can be the case for on-demand parachains
	/// cores when there are no on-demand blocks queued. Leased parachain cores will never be left idle.
	#[codec(index = 2)]
	Free,
}

impl<N> CoreState<N> {
	/// If this core state has a `para_id`, return it.
	pub fn para_id(&self) -> Option<Id> {
		match self {
			Self::Occupied(ref core) => Some(core.para_id()),
			Self::Scheduled(core) => Some(core.para_id),
			Self::Free => None,
		}
	}

	/// Is this core state `Self::Occupied`?
	pub fn is_occupied(&self) -> bool {
		matches!(self, Self::Occupied(_))
	}
}

/// Upgrade a v4 CoreState to a vstaging one.
pub fn corestate_from_v4(cs: v4::CoreState) -> CoreState {
	match cs {
		v4::CoreState::Free => CoreState::Free,
		v4::CoreState::Occupied(occupied_core) =>
			CoreState::Occupied(occupied_core_from_v4(occupied_core)),
		v4::CoreState::Scheduled(scheduled_core) =>
			CoreState::Scheduled(scheduled_core_from_v4(scheduled_core)),
	}
}
