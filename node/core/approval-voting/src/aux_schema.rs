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
	BlockNumber, Hash, CandidateHash,
};
use sp_consensus_slots::SlotNumber;
use parity_scale_codec::{Encode, Decode};

use std::collections::BTreeMap;
use bitvec::vec::BitVec;

use super::Tick;

const STORED_BLOCKS_KEY: &[u8] = b"Approvals_StoredBlock";

/// Metadata regarding a specific tranche of assignments for a specific candidate.
#[derive(Debug, Clone, Encode, Decode)]
pub(crate) struct TrancheEntry {
	tranche: DelayTranche,
	// Assigned validators, and the instant we received their assignment, rounded
	// to the nearest tick.
	assignments: Vec<(ValidatorIndex, Tick)>,
}

/// Metadata regarding approval of a particular candidate within the context of some
/// particular block.
#[derive(Debug, Clone, Encode, Decode)]
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
#[derive(Debug, Clone, Encode, Decode)]
pub(crate) struct CandidateEntry {
	candidate: CandidateReceipt,
	session: SessionIndex,
	// Assignments are based on blocks, so we need to track assignments separately
	// based on the block we are looking at.
	block_assignments: BTreeMap<Hash, ApprovalEntry>,
	approvals: BitVec<bitvec::order::Lsb0, u8>,
}

/// Metadata regarding approval of a particular block, by way of approval of the
/// candidates contained within it.
#[derive(Debug, Clone, Encode, Decode)]
pub(crate) struct BlockEntry {
	block_hash: Hash,
	session: SessionIndex,
	slot: SlotNumber,
	relay_vrf_story: RelayVRF,
	// The candidates included as-of this block and the index of the core they are
	// leaving. Sorted ascending by core index.
	candidates: Vec<(CoreIndex, CandidateHash)>,
	// A bitfield where the i'th bit corresponds to the i'th candidate in `candidates`.
	// The i'th bit is `tru` iff the candidate has been approved in the context of this
	// block. The block can be considered approved if the bitfield has all bits set to `true`.
	approved_bitfield: BitVec<bitvec::order::Lsb0, u8>,
	children: Vec<Hash>,
}

/// A range from earliest..last block number stored within the DB.
#[derive(Debug, Clone, Encode, Decode)]
pub(crate) struct StoredBlockRange(BlockNumber, BlockNumber);

// TODO [now]: probably in lib.rs
#[derive(Debug, Clone, Encode, Decode)]
pub(crate) struct OurAssignment { }

pub(crate) fn clear(_: &impl AuxStore) {
	// TODO: [now]
}

/// Load a block entry from the aux store.
pub(crate) fn load_block_entry(store: &impl AuxStore, block_hash: &Hash)
	-> sp_blockchain::Result<Option<BlockEntry>>
{
	match store.get_aux(&block_entry_key(block_hash))? {
		None => Ok(None),
		Some(raw) => BlockEntry::decode(&mut &raw[..])
			.map(Some)
			.map_err(|e| sp_blockchain::Error::Storage(
				format!("Failed to decode block entry in approvals: {:?}", e)
			)),
	}
}

/// Load a candidate entry from the aux store.
pub(crate) fn load_candidate_entry(store: &impl AuxStore, candidate_hash: &CandidateHash)
	-> sp_blockchain::Result<Option<CandidateEntry>>
{
	match store.get_aux(&candidate_entry_key(candidate_hash))? {
		None => Ok(None),
		Some(raw) => CandidateEntry::decode(&mut &raw[..])
			.map(Some)
			.map_err(|e| sp_blockchain::Error::Storage(
				format!("Failed to decode block entry in approvals: {:?}", e)
			)),
	}
}

/// Load a
fn block_entry_key(block_hash: &Hash) -> [u8; 46] {
	const BLOCK_ENTRY_PREFIX: [u8; 14] = *b"Approvals_blck";

	let mut key = [0u8; 14 + 32];
	key[0..14].copy_from_slice(&BLOCK_ENTRY_PREFIX);
	key[14..][..32].copy_from_slice(block_hash.as_ref());

	key
}

fn candidate_entry_key(candidate_hash: &CandidateHash) -> [u8; 46] {
	const CANDIDATE_ENTRY_PREFIX: [u8; 14] = *b"Approvals_cand";

	let mut key = [0u8; 14 + 32];
	key[0..14].copy_from_slice(&CANDIDATE_ENTRY_PREFIX);
	key[14..][..32].copy_from_slice(candidate_hash.0.as_ref());

	key
}
