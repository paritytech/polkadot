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

use polkadot_node_subsystem::{SubsystemResult, SubsystemError};

use polkadot_primitives::v1::{
	CandidateHash, CandidateReceipt, BlockNumber, GroupIndex, Hash,
};
use parity_scale_codec::{Encode, Decode};
use kvdb::{KeyValueDB};
use bitvec::{order::Lsb0 as BitOrderLsb0};

use std::convert::Into;
use std::collections::{BTreeMap, HashMap};
use std::collections::hash_map::Entry;

use super::persisted_entries::{ApprovalEntry, CandidateEntry, BlockEntry};
use super::backend::{Backend, OverlayedBackend};
use super::approval_db::{
	self,
	v1::{
		Config, OurAssignment, STORED_BLOCKS_KEY,
		block_entry_key, blocks_at_height_key, candidate_entry_key, load_decode,
	},
};

/// A range from earliest..last block number stored within the DB.
#[derive(Encode, Decode, Debug, Clone, PartialEq)]
pub struct StoredBlockRange(pub(super) BlockNumber, pub(super) BlockNumber);

/// Information about a new candidate necessary to instantiate the requisite
/// candidate and approval entries.
#[derive(Clone)]
pub(super) struct NewCandidateInfo {
	pub candidate: CandidateReceipt,
	pub backing_group: GroupIndex,
	pub our_assignment: Option<OurAssignment>,
}

/// Canonicalize some particular block, pruning everything before it and
/// pruning any competing branches at the same height.
pub(super) fn canonicalize<T>(
	overlay_db: &mut OverlayedBackend<'_, T>,
	canon_number: BlockNumber,
	canon_hash: Hash,
) -> SubsystemResult<()>
	where T: Backend
{
	let range = match overlay_db.load_stored_blocks()? {
		None => return Ok(()),
		Some(range) => if range.0 >= canon_number {
			return Ok(())
		} else {
			range
		},
	};

	// Storing all candidates in memory is potentially heavy, but should be fine
	// as long as finality doesn't stall for a long while. We could optimize this
	// by keeping only the metadata about which blocks reference each candidate.
	let mut visited_candidates = HashMap::new();

	// All the block heights we visited but didn't necessarily delete everything from.
	let mut visited_heights = HashMap::new();

	let visit_and_remove_block_entry = |
		block_hash: Hash,
		overlayed_db: &mut OverlayedBackend<'_, T>,
		visited_candidates: &mut HashMap<CandidateHash, CandidateEntry>,
	| -> SubsystemResult<Vec<Hash>> {
		let block_entry = match overlayed_db.load_block_entry(&block_hash)? {
			None => return Ok(Vec::new()),
			Some(b) => b,
		};

		overlayed_db.delete_block_entry(&block_hash);
		for &(_, ref candidate_hash) in block_entry.candidates() {
			let candidate = match visited_candidates.entry(*candidate_hash) {
				Entry::Occupied(e) => e.into_mut(),
				Entry::Vacant(e) => {
					e.insert(match overlayed_db.load_candidate_entry(candidate_hash)? {
						None => continue, // Should not happen except for corrupt DB
						Some(c) => c,
					})
				}
			};

			candidate.block_assignments.remove(&block_hash);
		}

		Ok(block_entry.children)
	};

	// First visit everything before the height.
	for i in range.0..canon_number {
		let at_height = overlay_db.load_blocks_at_height(&i)?;
		overlay_db.delete_blocks_at_height(i);

		for b in at_height {
			let _ = visit_and_remove_block_entry(
				b,
				overlay_db,
				&mut visited_candidates,
			)?;
		}
	}

	// Then visit everything at the height.
	let pruned_branches = {
		let at_height = overlay_db.load_blocks_at_height(&canon_number)?;
		overlay_db.delete_blocks_at_height(canon_number);

		// Note that while there may be branches descending from blocks at earlier heights,
		// we have already covered them by removing everything at earlier heights.
		let mut pruned_branches = Vec::new();

		for b in at_height {
			let children = visit_and_remove_block_entry(
				b,
				overlay_db,
				&mut visited_candidates,
			)?;

			if b != canon_hash {
				pruned_branches.extend(children);
			}
		}

		pruned_branches
	};

	// Follow all children of non-canonicalized blocks.
	{
		let mut frontier: Vec<(BlockNumber, Hash)> = pruned_branches.into_iter().map(|h| (canon_number + 1, h)).collect();
		while let Some((height, next_child)) = frontier.pop() {
			let children = visit_and_remove_block_entry(
				next_child,
				overlay_db,
				&mut visited_candidates,
			)?;

			// extend the frontier of branches to include the given height.
			frontier.extend(children.into_iter().map(|h| (height + 1, h)));

			// visit the at-height key for this deleted block's height.
			let at_height = match visited_heights.entry(height) {
				Entry::Occupied(e) => e.into_mut(),
				Entry::Vacant(e) => e.insert(overlay_db.load_blocks_at_height(&height)?),
			};
			if let Some(i) = at_height.iter().position(|x| x == &next_child) {
				at_height.remove(i);
			}
		}
	}

	// Update all `CandidateEntry`s, deleting all those which now have empty `block_assignments`.
	for (candidate_hash, candidate) in visited_candidates.into_iter() {
		if candidate.block_assignments.is_empty() {
			overlay_db.delete_candidate_entry(&candidate_hash);
		} else {
			overlay_db.write_candidate_entry(candidate);
		}
	}

	// Update all blocks-at-height keys, deleting all those which now have empty `block_assignments`.
	for (h, at) in visited_heights.into_iter() {
		if at.is_empty() {
			overlay_db.delete_blocks_at_height(h);
		} else {
			overlay_db.write_blocks_at_height(h, at);
		}
	}

	// due to the fork pruning, this range actually might go too far above where our actual highest block is,
	// if a relatively short fork is canonicalized.
	let new_range = StoredBlockRange(
		canon_number + 1,
		std::cmp::max(range.1, canon_number + 2),
	);

	overlay_db.write_stored_block_range(new_range);

	Ok(())
}

/// Record a new block entry.
///
/// This will update the blocks-at-height mapping, the stored block range, if necessary,
/// and add block and candidate entries. It will also add approval entries to existing
/// candidate entries and add this as a child of any block entry corresponding to the
/// parent hash.
///
/// Has no effect if there is already an entry for the block or `candidate_info` returns
/// `None` for any of the candidates referenced by the block entry. In these cases,
/// no information about new candidates will be referred to by this function.
pub(super) fn add_block_entry(
	store: &mut OverlayedBackend<'_, impl Backend>,
	entry: BlockEntry,
	n_validators: usize,
	candidate_info: impl Fn(&CandidateHash) -> Option<NewCandidateInfo>,
) -> SubsystemResult<Vec<(CandidateHash, CandidateEntry)>> {
	let session = entry.session();
	let parent_hash = entry.parent_hash();
	let number = entry.block_number();

	// Update the stored block range.
	{
		let new_range = match store.load_stored_blocks()? {
			None => Some(StoredBlockRange(number, number + 1)),
			Some(range) => if range.1 <= number {
				Some(StoredBlockRange(range.0, number + 1))
			} else {
				None
			}
		};

		new_range.map(|n| store.write_stored_block_range(n));
	};

	// Update the blocks at height meta key.
	{
		let mut blocks_at_height = store.load_blocks_at_height(&number)?;
		if blocks_at_height.contains(&entry.block_hash()) {
			// seems we already have a block entry for this block. nothing to do here.
			return Ok(Vec::new())
		}

		blocks_at_height.push(entry.block_hash());
		store.write_blocks_at_height(number, blocks_at_height)
	};

	let mut candidate_entries = Vec::with_capacity(entry.candidates().len());

	// read and write all updated entries.
	{
		for &(_, ref candidate_hash) in entry.candidates() {
			let NewCandidateInfo {
				candidate,
				backing_group,
				our_assignment,
			} = match candidate_info(candidate_hash) {
				None => return Ok(Vec::new()),
				Some(info) => info,
			};

			let mut candidate_entry = store.load_candidate_entry(&candidate_hash)?
				.unwrap_or_else(move || CandidateEntry {
					candidate,
					session,
					block_assignments: BTreeMap::new(),
					approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
				});

			candidate_entry.block_assignments.insert(
				entry.block_hash(),
				ApprovalEntry {
					tranches: Vec::new(),
					backing_group,
					our_assignment: our_assignment.map(|v| v.into()),
					our_approval_sig: None,
					assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
					approved: false,
				}
			);

			store.write_candidate_entry(candidate_entry.clone());

			candidate_entries.push((*candidate_hash, candidate_entry));
		}
	};

	// Update the child index for the parent.
	store.load_block_entry(&parent_hash)?.map(|mut e| {
		e.children.push(entry.block_hash());
		store.write_block_entry(e);
	});

	// Put the new block entry in.
	store.write_block_entry(entry);

	Ok(candidate_entries)
}
/// Forcibly approve all candidates included at up to the given relay-chain height in the indicated
/// chain.
pub(super) fn force_approve(
	store: &mut OverlayedBackend<'_, impl Backend>,
	chain_head: Hash,
	up_to: BlockNumber,
) -> SubsystemResult<()> {
	enum State {
		WalkTo,
		Approving,
	}

	let mut cur_hash = chain_head;
	let mut state = State::WalkTo;

	// iterate back to the `up_to` block, and then iterate backwards until all blocks
	// are updated.
	while let Some(mut entry) = store.load_block_entry(&cur_hash)? {

		if entry.block_number() <= up_to {
			state = State::Approving;
		}

		cur_hash = entry.parent_hash();

		match state {
			State::WalkTo => {},
			State::Approving => {
				entry.approved_bitfield.iter_mut().for_each(|mut b| *b = true);
				store.write_block_entry(entry);
			}
		}
	}

	Ok(())
}
/// Return all blocks which have entries in the DB, ascending, by height.
pub fn load_all_blocks(store: &dyn KeyValueDB, config: &Config) -> SubsystemResult<Vec<Hash>> {
	let mut hashes = Vec::new();
	if let Some(stored_blocks) = load_stored_blocks(store, config)? {
		for height in stored_blocks.0..stored_blocks.1 {
			let blocks = load_blocks_at_height(store, config, &height)?;
			hashes.extend(blocks);
		}

	}

	Ok(hashes)
}

/// Load the stored-blocks key from the state.
pub(super) fn load_stored_blocks(
	store: &dyn KeyValueDB,
	config: &Config,
) -> SubsystemResult<Option<StoredBlockRange>> {
	load_decode(store, config.col_data, STORED_BLOCKS_KEY)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))
}

/// Load a blocks-at-height entry for a given block number.
pub(super) fn load_blocks_at_height(
	store: &dyn KeyValueDB,
	config: &Config,
	block_number: &BlockNumber,
) -> SubsystemResult<Vec<Hash>> {
	load_decode(store, config.col_data, &blocks_at_height_key(*block_number))
		.map(|x| x.unwrap_or_default())
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))
}

/// Load a block entry from the aux store.
pub(super) fn load_block_entry(
	store: &dyn KeyValueDB,
	config: &Config,
	block_hash: &Hash,
) -> SubsystemResult<Option<BlockEntry>> {
	load_decode(store, config.col_data, &block_entry_key(block_hash))
		.map(|u: Option<approval_db::v1::BlockEntry>| u.map(|v| v.into()))
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))
}

/// Load a candidate entry from the aux store.
pub(super) fn load_candidate_entry(
	store: &dyn KeyValueDB,
	config: &Config,
	candidate_hash: &CandidateHash,
) -> SubsystemResult<Option<CandidateEntry>> {
	load_decode(store, config.col_data, &candidate_entry_key(candidate_hash))
		.map(|u: Option<approval_db::v1::CandidateEntry>| u.map(|v| v.into()))
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))
}
