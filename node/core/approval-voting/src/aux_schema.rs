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

//! Auxiliary DB schema, accessors, and writers for on-disk persisted approval storage
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
use polkadot_node_primitives::approval::{DelayTranche, RelayVRF};
use polkadot_primitives::v1::{
	ValidatorIndex, GroupIndex, CandidateReceipt, SessionIndex, CoreIndex,
	BlockNumber, Hash,
};
use sp_consensus_slots::SlotNumber;

use std::collections::HashMap;
use bitvec::vec::BitVec;

use super::Tick;

const STORED_BLOCKS_KEY: &[u8] = b"StoredBlock";

/// Metadata regarding a specific tranche of assignments for a specific candidate.
pub(crate) struct TrancheEntry {
	tranche: DelayTranche,
	// Assigned validators, and the instant we received their assignment, rounded
	// to the nearest tick.
	assignments: Vec<(ValidatorIndex, Tick)>,
}

/// Metadata regarding approval of a particular candidate within the context of some
/// particular block.
pub(crate) struct ApprovalEntry {
	tranches: Vec<TrancheEntry>,
	backing_group: GroupIndex,
	// When the next wakeup for this entry should occur. This is either to
	// check a no-show or to check if we need to broadcast an assignment.
	next_wakeup: Tick,
	our_assignment: Option<OurAssignment>,
	// `n_validators` bits.
	assignments: BitVec<bitvec::order::Lsb0, u8>,
	approved: bool,
}

/// Metadata regarding approval of a particular candidate.
pub(crate) struct CandidateEntry {
	candidate: CandidateReceipt,
	session: SessionIndex,
	// Assignments are based on blocks, so we need to track assignments separately
	// based on the block we are looking at.
	block_assignments: HashMap<Hash, ApprovalEntry>,
	approvals: BitVec<bitvec::order::Lsb0, u8>,
}

/// Metadata regarding approval of a particular block, by way of approval of the
/// candidates contained within it.
pub(crate) struct BlockEntry {
	block_hash: Hash,
	session: SessionIndex,
	slot: SlotNumber,
	relay_vrf_story: RelayVRF,
	// The candidates included as-of this block and the index of the core they are
	// leaving. Sorted ascending by core index.
	candidates: Vec<(CoreIndex, Hash)>,
	// A bitfield where the i'th bit corresponds to the i'th candidate in `candidates`.
	// The i'th bit is `tru` iff the candidate has been approved in the context of this
	// block. The block can be considered approved if the bitfield has all bits set to `true`.
	approved_bitfield: BitVec<bitvec::order::Lsb0, u8>,
	children: Vec<Hash>,
}

/// A range from earliest..last block number stored within the DB.
pub(crate) struct StoredBlockRange(BlockNumber, BlockNumber);

// TODO [now]: probably in lib.rs
pub(crate) struct OurAssignment { }

pub(crate) fn clear(_: &impl AuxStore) {
	// TODO: [now]
}
