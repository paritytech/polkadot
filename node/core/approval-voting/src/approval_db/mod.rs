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

//! Approval DB accessors and writers for on-disk persisted approval storage
//! data.
//!
//! We persist data to disk although it is not intended to be used across runs of the
//! program. This is because under medium to long periods of finality stalling, for whatever
//! reason that may be, the amount of data we'd need to keep would be potentially too large
//! for memory.
//!
//! With tens or hundreds of parachains, hundreds of validators, and parablocks
//! in every relay chain block, there can be a humongous amount of information to reference
//! at any given time.
//!
//! As such, we provide a function from this module to clear the database on start-up.
//! In the future, we may use a temporary DB which doesn't need to be wiped, but for the
//! time being we share the same DB with the rest of Substrate.

use sc_client_api::backend::AuxStore;
use polkadot_node_primitives::approval::{DelayTranche, RelayVRFStory, AssignmentCert};
use polkadot_primitives::v1::{
	ValidatorIndex, GroupIndex, CandidateReceipt, SessionIndex, CoreIndex,
	BlockNumber, Hash, CandidateHash,
};
use sp_consensus_slots::Slot;
use parity_scale_codec::{Encode, Decode};

use std::collections::{BTreeMap, HashMap};
use std::collections::hash_map::Entry;
use bitvec::{slice::BitSlice, vec::BitVec, order::Lsb0 as BitOrderLsb0};

use super::time::Tick;
use super::criteria::OurAssignment;

pub mod v1;

/// Metadata regarding a specific tranche of assignments for a specific candidate.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TrancheEntry {
	pub tranche: DelayTranche,
	// Assigned validators, and the instant we received their assignment, rounded
	// to the nearest tick.
	pub assignments: Vec<(ValidatorIndex, Tick)>,
}

/// Metadata regarding approval of a particular candidate within the context of some
/// particular block.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ApprovalEntry {
	pub tranches: Vec<TrancheEntry>,
	pub backing_group: GroupIndex,
	pub our_assignment: Option<OurAssignment>,
	// `n_validators` bits.
	pub assignments: BitVec<BitOrderLsb0, u8>,
	pub approved: bool,
}

impl ApprovalEntry {
	// Note that our assignment is triggered. No-op if already triggered.
	pub(crate) fn trigger_our_assignment(&mut self, tick_now: Tick)
		-> Option<(AssignmentCert, ValidatorIndex)>
	{
		let our = self.our_assignment.as_mut().and_then(|a| {
			if a.triggered() { return None }
			a.mark_triggered();

			Some(a.clone())
		});

		our.map(|a| {
			self.import_assignment(a.tranche(), a.validator_index(), tick_now);

			(a.cert().clone(), a.validator_index())
		})
	}

	/// Whether a validator is already assigned.
	pub(crate) fn is_assigned(&self, validator_index: ValidatorIndex) -> bool {
		self.assignments.get(validator_index as usize).map(|b| *b).unwrap_or(false)
	}

	/// Import an assignment. No-op if already assigned on the same tranche.
	pub(crate) fn import_assignment(
		&mut self,
		tranche: DelayTranche,
		validator_index: ValidatorIndex,
		tick_now: Tick,
	) {
		// linear search probably faster than binary. not many tranches typically.
		let idx = match self.tranches.iter().position(|t| t.tranche >= tranche) {
			Some(pos) => {
				if self.tranches[pos].tranche > tranche {
					self.tranches.insert(pos, TrancheEntry {
						tranche: tranche,
						assignments: Vec::new(),
					});
				}

				pos
			}
			None => {
				self.tranches.push(TrancheEntry {
					tranche: tranche,
					assignments: Vec::new(),
				});

				self.tranches.len() - 1
			}
		};

		self.tranches[idx].assignments.push((validator_index, tick_now));
		self.assignments.set(validator_index as _, true);
	}

	// Produce a bitvec indicating the assignments of all validators up to and
	// including `tranche`.
	pub(crate) fn assignments_up_to(&self, tranche: DelayTranche) -> BitVec<BitOrderLsb0, u8> {
		self.tranches.iter()
			.take_while(|e| e.tranche <= tranche)
			.fold(bitvec::bitvec![BitOrderLsb0, u8; 0; self.assignments.len()], |mut a, e| {
				for &(v, _) in &e.assignments {
					a.set(v as _, true);
				}

				a
			})
	}

	/// Whether the approval entry is approved
	pub(crate) fn is_approved(&self) -> bool {
		self.approved
	}

	/// Mark the approval entry as approved.
	pub(crate) fn mark_approved(&mut self) {
		self.approved = true;
	}
}

/// Metadata regarding approval of a particular candidate.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CandidateEntry {
	pub candidate: CandidateReceipt,
	pub session: SessionIndex,
	// Assignments are based on blocks, so we need to track assignments separately
	// based on the block we are looking at.
	pub block_assignments: BTreeMap<Hash, ApprovalEntry>,
	pub approvals: BitVec<BitOrderLsb0, u8>,
}

impl CandidateEntry {
	/// Access the bit-vec of approvals.
	pub(crate) fn approvals(&self) -> &BitSlice<BitOrderLsb0, u8> {
		&self.approvals
	}

	/// Note that a given validator has approved. Return the previous approval state.
	pub(crate) fn mark_approval(&mut self, validator: ValidatorIndex) -> bool {
		let prev = self.approvals.get(validator as usize).map(|b| *b).unwrap_or(false);
		self.approvals.set(validator as usize, true);
		prev
	}

	/// Get the next tick this should be woken up to process the approval entry
	/// under the given block hash, if any.
	///
	/// Returns `None` if there is no approval entry, the approval entry is fully
	/// approved, or there are no assignments in the approval entry.
	pub(crate) fn next_wakeup(
		&self,
		block_hash: &Hash,
		block_tick: Tick,
		no_show_duration: Tick,
	) -> Option<Tick> {
		let approval_entry = self.block_assignments.get(block_hash)?;
		if approval_entry.is_approved() { return None }

		let our_assignment_tick = approval_entry.our_assignment.as_ref()
			.and_then(|a| if a.triggered() { None } else { Some(a.tranche()) })
			.map(|assignment_tranche| assignment_tranche as Tick + block_tick);

		// The earliest-received assignment which has no corresponding approval
		let next_no_show = approval_entry.tranches.iter()
			.flat_map(|t| t.assignments.iter())
			.filter(|(v, _)| self.approvals.get(*v as usize).map(|b| *b).unwrap_or(false))
			.map(|(_, tick)| tick + no_show_duration)
			.min();

		match (our_assignment_tick, next_no_show) {
			(None, None) => None,
			(Some(t), None) | (None, Some(t)) => Some(t),
			(Some(t1), Some(t2)) => Some(std::cmp::min(t1, t2)),
		}
	}
}

/// Metadata regarding approval of a particular block, by way of approval of the
/// candidates contained within it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BlockEntry {
	pub block_hash: Hash,
	pub session: SessionIndex,
	pub slot: Slot,
	pub relay_vrf_story: RelayVRFStory,
	// The candidates included as-of this block and the index of the core they are
	// leaving. Sorted ascending by core index.
	pub candidates: Vec<(CoreIndex, CandidateHash)>,
	// A bitfield where the i'th bit corresponds to the i'th candidate in `candidates`.
	// The i'th bit is `true` iff the candidate has been approved in the context of this
	// block. The block can be considered approved if the bitfield has all bits set to `true`.
	pub approved_bitfield: BitVec<BitOrderLsb0, u8>,
	pub children: Vec<Hash>,
}

impl BlockEntry {
	/// Mark a candidate as fully approved in the bitfield.
	pub(crate) fn mark_approved_by_hash(&mut self, candidate_hash: &CandidateHash) {
		if let Some(p) = self.candidates.iter().position(|(_, h)| h == candidate_hash) {
			self.approved_bitfield.set(p, true);
		}
	}

	/// Whether the block entry is fully approved.
	pub(crate) fn is_fully_approved(&self) -> bool {
		self.approved_bitfield.all()
	}
}
