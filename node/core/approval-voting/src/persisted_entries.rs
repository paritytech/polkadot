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

//! Entries pertaining to approval which need to be persisted.
//!
//! The actual persisting of data is handled by the `approval_db` module.
//! Within that context, things are plain-old-data. Within this module,
//! data and logic are intertwined.

use polkadot_node_primitives::approval::{DelayTranche, RelayVRFStory, AssignmentCert};
use polkadot_primitives::v1::{
	ValidatorIndex, CandidateReceipt, SessionIndex, GroupIndex, CoreIndex,
	Hash, CandidateHash,
};
use sp_consensus_slots::Slot;

use std::collections::BTreeMap;
use bitvec::{slice::BitSlice, vec::BitVec, order::Lsb0 as BitOrderLsb0};

use super::time::Tick;
use super::criteria::OurAssignment;

/// Metadata regarding a specific tranche of assignments for a specific candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct TrancheEntry {
	tranche: DelayTranche,
	// Assigned validators, and the instant we received their assignment, rounded
	// to the nearest tick.
	assignments: Vec<(ValidatorIndex, Tick)>,
}

impl TrancheEntry {
	/// Get the tranche of this entry.
	pub fn tranche(&self) -> DelayTranche {
		self.tranche
	}

	/// Get the assignments for this entry.
	pub fn assignments(&self) -> &[(ValidatorIndex, Tick)] {
		&self.assignments
	}
}

impl From<crate::approval_db::v1::TrancheEntry> for TrancheEntry {
	fn from(entry: crate::approval_db::v1::TrancheEntry) -> Self {
		TrancheEntry {
			tranche: entry.tranche,
			assignments: entry.assignments.into_iter().map(|(v, t)| (v, t.into())).collect(),
		}
	}
}

impl From<TrancheEntry> for crate::approval_db::v1::TrancheEntry {
	fn from(entry: TrancheEntry) -> Self {
		Self {
			tranche: entry.tranche,
			assignments: entry.assignments.into_iter().map(|(v, t)| (v, t.into())).collect(),
		}
	}
}

/// Metadata regarding approval of a particular candidate within the context of some
/// particular block.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalEntry {
	tranches: Vec<TrancheEntry>,
	backing_group: GroupIndex,
	our_assignment: Option<OurAssignment>,
	// `n_validators` bits.
	assignments: BitVec<BitOrderLsb0, u8>,
	approved: bool,
}

impl ApprovalEntry {
	// Access our assignment for this approval entry.
	pub fn our_assignment(&self) -> Option<&OurAssignment> {
		self.our_assignment.as_ref()
	}

	// Note that our assignment is triggered. No-op if already triggered.
	pub fn trigger_our_assignment(&mut self, tick_now: Tick)
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
	pub fn is_assigned(&self, validator_index: ValidatorIndex) -> bool {
		self.assignments.get(validator_index.0 as usize).map(|b| *b).unwrap_or(false)
	}

	/// Import an assignment. No-op if already assigned on the same tranche.
	pub fn import_assignment(
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
		self.assignments.set(validator_index.0 as _, true);
	}

	// Produce a bitvec indicating the assignments of all validators up to and
	// including `tranche`.
	pub fn assignments_up_to(&self, tranche: DelayTranche) -> BitVec<BitOrderLsb0, u8> {
		self.tranches.iter()
			.take_while(|e| e.tranche <= tranche)
			.fold(bitvec::bitvec![BitOrderLsb0, u8; 0; self.assignments.len()], |mut a, e| {
				for &(v, _) in &e.assignments {
					a.set(v.0 as _, true);
				}

				a
			})
	}

	/// Whether the approval entry is approved
	pub fn is_approved(&self) -> bool {
		self.approved
	}

	/// Mark the approval entry as approved.
	pub fn mark_approved(&mut self) {
		self.approved = true;
	}

	/// Access the tranches.
	pub fn tranches(&self) -> &[TrancheEntry] {
		&self.tranches
	}

	/// Get the number of validators in this approval entry.
	pub fn n_validators(&self) -> usize {
		self.assignments.len()
	}

	/// Get the backing group index of the approval entry.
	pub fn backing_group(&self) -> GroupIndex {
		self.backing_group
	}

	/// For tests: set our assignment.
	#[cfg(test)]
	pub fn set_our_assignment(&mut self, our_assignment: OurAssignment) {
		self.our_assignment = Some(our_assignment);
	}
}

impl From<crate::approval_db::v1::ApprovalEntry> for ApprovalEntry {
	fn from(entry: crate::approval_db::v1::ApprovalEntry) -> Self {
		ApprovalEntry {
			tranches: entry.tranches.into_iter().map(Into::into).collect(),
			backing_group: entry.backing_group,
			our_assignment: entry.our_assignment.map(Into::into),
			assignments: entry.assignments,
			approved: entry.approved,
		}
	}
}

impl From<ApprovalEntry> for crate::approval_db::v1::ApprovalEntry {
	fn from(entry: ApprovalEntry) -> Self {
		Self {
			tranches: entry.tranches.into_iter().map(Into::into).collect(),
			backing_group: entry.backing_group,
			our_assignment: entry.our_assignment.map(Into::into),
			assignments: entry.assignments,
			approved: entry.approved,
		}
	}
}

/// Metadata regarding approval of a particular candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateEntry {
	candidate: CandidateReceipt,
	session: SessionIndex,
	// Assignments are based on blocks, so we need to track assignments separately
	// based on the block we are looking at.
	block_assignments: BTreeMap<Hash, ApprovalEntry>,
	approvals: BitVec<BitOrderLsb0, u8>,
}

impl CandidateEntry {
	/// Access the bit-vec of approvals.
	pub fn approvals(&self) -> &BitSlice<BitOrderLsb0, u8> {
		&self.approvals
	}

	/// Note that a given validator has approved. Return the previous approval state.
	pub fn mark_approval(&mut self, validator: ValidatorIndex) -> bool {
		let prev = self.approvals.get(validator.0 as usize).map(|b| *b).unwrap_or(false);
		self.approvals.set(validator.0 as usize, true);
		prev
	}

	/// Get the candidate receipt.
	pub fn candidate_receipt(&self) -> &CandidateReceipt {
		&self.candidate
	}

	/// Get the approval entry, mutably, for this candidate under a specific block.
	pub fn approval_entry_mut(&mut self, block_hash: &Hash) -> Option<&mut ApprovalEntry> {
		self.block_assignments.get_mut(block_hash)
	}

	/// Get the approval entry for this candidate under a specific block.
	pub fn approval_entry(&self, block_hash: &Hash) -> Option<&ApprovalEntry> {
		self.block_assignments.get(block_hash)
	}

	/// Iterate over approval entries.
	pub fn iter_approval_entries(&self) -> impl IntoIterator<Item = (&Hash, &ApprovalEntry)> {
		self.block_assignments.iter()
	}

	#[cfg(test)]
	pub fn add_approval_entry(
		&mut self,
		block_hash: Hash,
		approval_entry: ApprovalEntry,
	) {
		self.block_assignments.insert(block_hash, approval_entry);
	}
}

impl From<crate::approval_db::v1::CandidateEntry> for CandidateEntry {
	fn from(entry: crate::approval_db::v1::CandidateEntry) -> Self {
		CandidateEntry {
			candidate: entry.candidate,
			session: entry.session,
			block_assignments: entry.block_assignments.into_iter().map(|(h, ae)| (h, ae.into())).collect(),
			approvals: entry.approvals,
		}
	}
}

impl From<CandidateEntry> for crate::approval_db::v1::CandidateEntry {
	fn from(entry: CandidateEntry) -> Self {
		Self {
			candidate: entry.candidate,
			session: entry.session,
			block_assignments: entry.block_assignments.into_iter().map(|(h, ae)| (h, ae.into())).collect(),
			approvals: entry.approvals,
		}
	}
}

/// Metadata regarding approval of a particular block, by way of approval of the
/// candidates contained within it.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntry {
	block_hash: Hash,
	session: SessionIndex,
	slot: Slot,
	relay_vrf_story: RelayVRFStory,
	// The candidates included as-of this block and the index of the core they are
	// leaving. Sorted ascending by core index.
	candidates: Vec<(CoreIndex, CandidateHash)>,
	// A bitfield where the i'th bit corresponds to the i'th candidate in `candidates`.
	// The i'th bit is `true` iff the candidate has been approved in the context of this
	// block. The block can be considered approved if the bitfield has all bits set to `true`.
	approved_bitfield: BitVec<BitOrderLsb0, u8>,
	children: Vec<Hash>,
}

impl BlockEntry {
	/// Mark a candidate as fully approved in the bitfield.
	pub fn mark_approved_by_hash(&mut self, candidate_hash: &CandidateHash) {
		if let Some(p) = self.candidates.iter().position(|(_, h)| h == candidate_hash) {
			self.approved_bitfield.set(p, true);
		}
	}

	/// Whether the block entry is fully approved.
	pub fn is_fully_approved(&self) -> bool {
		self.approved_bitfield.all()
	}

	#[cfg(test)]
	pub fn block_hash(&self) -> Hash {
		self.block_hash
	}

	#[cfg(test)]
	pub fn is_candidate_approved(&self, candidate_hash: &CandidateHash) -> bool {
		self.candidates.iter().position(|(_, h)| h == candidate_hash)
			.and_then(|p| self.approved_bitfield.get(p).map(|b| *b))
			.unwrap_or(false)
	}

	/// For tests: Add a candidate to the block entry. Returns the
	/// index where the candidate was added.
	///
	/// Panics if the core is already used.
	#[cfg(test)]
	pub fn add_candidate(&mut self, core: CoreIndex, candidate_hash: CandidateHash) -> usize {
		let pos = self.candidates
			.binary_search_by_key(&core, |(c, _)| *c)
			.unwrap_err();

		self.candidates.insert(pos, (core, candidate_hash));

		// bug in bitvec?
		if pos < self.approved_bitfield.len() {
			self.approved_bitfield.insert(pos, false);
		} else {
			self.approved_bitfield.push(false);
		}

		pos
	}

	/// Get the slot of the block.
	pub fn slot(&self) -> Slot {
		self.slot
	}

	/// Get the relay-vrf-story of the block.
	pub fn relay_vrf_story(&self) -> RelayVRFStory {
		self.relay_vrf_story.clone()
	}

	/// Get the session index of the block.
	pub fn session(&self) -> SessionIndex {
		self.session
	}

	/// Get the i'th candidate.
	pub fn candidate(&self, i: usize) -> Option<&(CoreIndex, CandidateHash)> {
		self.candidates.get(i)
	}

	/// Access the underlying candidates as a slice.
	pub fn candidates(&self) -> &[(CoreIndex, CandidateHash)] {
		&self.candidates
	}
}

impl From<crate::approval_db::v1::BlockEntry> for BlockEntry {
	fn from(entry: crate::approval_db::v1::BlockEntry) -> Self {
		BlockEntry {
			block_hash: entry.block_hash,
			session: entry.session,
			slot: entry.slot,
			relay_vrf_story: RelayVRFStory(entry.relay_vrf_story),
			candidates: entry.candidates,
			approved_bitfield: entry.approved_bitfield,
			children: entry.children,
		}
	}
}

impl From<BlockEntry> for crate::approval_db::v1::BlockEntry {
	fn from(entry: BlockEntry) -> Self {
		Self {
			block_hash: entry.block_hash,
			session: entry.session,
			slot: entry.slot,
			relay_vrf_story: entry.relay_vrf_story.0,
			candidates: entry.candidates,
			approved_bitfield: entry.approved_bitfield,
			children: entry.children,
		}
	}
}
