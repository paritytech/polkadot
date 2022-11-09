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

//! Middleware interface that leverages low-level database operations
//! to provide a clean API for processing block and candidate imports.

use polkadot_node_subsystem::{SubsystemError, SubsystemResult};

use bitvec::order::Lsb0 as BitOrderLsb0;
use polkadot_primitives::v2::{BlockNumber, CandidateHash, CandidateReceipt, GroupIndex, Hash};

use std::collections::{hash_map::Entry, BTreeMap, HashMap};

use super::{
	approval_db::v1::{OurAssignment, StoredBlockRange},
	backend::{Backend, OverlayedBackend},
	persisted_entries::{ApprovalEntry, BlockEntry, CandidateEntry},
	LOG_TARGET,
};

/// Information about a new candidate necessary to instantiate the requisite
/// candidate and approval entries.
#[derive(Clone)]
pub struct NewCandidateInfo {
	candidate: CandidateReceipt,
	backing_group: GroupIndex,
	our_assignment: Option<OurAssignment>,
}

impl NewCandidateInfo {
	/// Convenience constructor
	pub fn new(
		candidate: CandidateReceipt,
		backing_group: GroupIndex,
		our_assignment: Option<OurAssignment>,
	) -> Self {
		Self { candidate, backing_group, our_assignment }
	}
}

fn visit_and_remove_block_entry(
	block_hash: Hash,
	overlayed_db: &mut OverlayedBackend<'_, impl Backend>,
	visited_candidates: &mut HashMap<CandidateHash, CandidateEntry>,
) -> SubsystemResult<Vec<Hash>> {
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
			},
		};

		candidate.block_assignments.remove(&block_hash);
	}

	Ok(block_entry.children)
}

/// Canonicalize some particular block, pruning everything before it and
/// pruning any competing branches at the same height.
pub fn canonicalize(
	overlay_db: &mut OverlayedBackend<'_, impl Backend>,
	canon_number: BlockNumber,
	canon_hash: Hash,
) -> SubsystemResult<()> {
	let range = match overlay_db.load_stored_blocks()? {
		None => return Ok(()),
		Some(range) if range.0 >= canon_number => return Ok(()),
		Some(range) => range,
	};

	// Storing all candidates in memory is potentially heavy, but should be fine
	// as long as finality doesn't stall for a long while. We could optimize this
	// by keeping only the metadata about which blocks reference each candidate.
	let mut visited_candidates = HashMap::new();

	// All the block heights we visited but didn't necessarily delete everything from.
	let mut visited_heights = HashMap::new();

	// First visit everything before the height.
	for i in range.0..canon_number {
		let at_height = overlay_db.load_blocks_at_height(&i)?;
		overlay_db.delete_blocks_at_height(i);

		for b in at_height {
			let _ = visit_and_remove_block_entry(b, overlay_db, &mut visited_candidates)?;
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
			let children = visit_and_remove_block_entry(b, overlay_db, &mut visited_candidates)?;

			if b != canon_hash {
				pruned_branches.extend(children);
			}
		}

		pruned_branches
	};

	// Follow all children of non-canonicalized blocks.
	{
		let mut frontier: Vec<(BlockNumber, Hash)> =
			pruned_branches.into_iter().map(|h| (canon_number + 1, h)).collect();
		while let Some((height, next_child)) = frontier.pop() {
			let children =
				visit_and_remove_block_entry(next_child, overlay_db, &mut visited_candidates)?;

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
	// TODO https://github.com/paritytech/polkadot/issues/3389
	let new_range = StoredBlockRange(canon_number + 1, std::cmp::max(range.1, canon_number + 2));

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
pub fn add_block_entry(
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
			Some(range) if range.1 <= number => Some(StoredBlockRange(range.0, number + 1)),
			Some(_) => None,
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
			let NewCandidateInfo { candidate, backing_group, our_assignment } =
				match candidate_info(candidate_hash) {
					None => return Ok(Vec::new()),
					Some(info) => info,
				};

			let mut candidate_entry =
				store.load_candidate_entry(&candidate_hash)?.unwrap_or_else(move || {
					CandidateEntry {
						candidate,
						session,
						block_assignments: BTreeMap::new(),
						approvals: bitvec::bitvec![u8, BitOrderLsb0; 0; n_validators],
					}
				});

			candidate_entry.block_assignments.insert(
				entry.block_hash(),
				ApprovalEntry::new(
					Vec::new(),
					backing_group,
					our_assignment.map(|v| v.into()),
					None,
					bitvec::bitvec![u8, BitOrderLsb0; 0; n_validators],
					false,
				),
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
pub fn force_approve(
	store: &mut OverlayedBackend<'_, impl Backend>,
	chain_head: Hash,
	up_to: BlockNumber,
) -> SubsystemResult<Vec<Hash>> {
	#[derive(PartialEq, Eq)]
	enum State {
		WalkTo,
		Approving,
	}
	let mut approved_hashes = Vec::new();

	let mut cur_hash = chain_head;
	let mut state = State::WalkTo;
	let mut cur_block_number: BlockNumber = 0;

	// iterate back to the `up_to` block, and then iterate backwards until all blocks
	// are updated.
	while let Some(mut entry) = store.load_block_entry(&cur_hash)? {
		cur_block_number = entry.block_number();
		if cur_block_number <= up_to {
			if state == State::WalkTo {
				gum::debug!(
					target: LOG_TARGET,
					block_hash = ?chain_head,
					?cur_hash,
					?cur_block_number,
					"Start forced approval from block",
				);
			}
			state = State::Approving;
		}

		cur_hash = entry.parent_hash();

		match state {
			State::WalkTo => {},
			State::Approving => {
				entry.approved_bitfield.iter_mut().for_each(|mut b| *b = true);
				approved_hashes.push(entry.block_hash());
				store.write_block_entry(entry);
			},
		}
	}

	if state == State::WalkTo {
		gum::warn!(
			target: LOG_TARGET,
			?chain_head,
			?cur_hash,
			?cur_block_number,
			?up_to,
			"Missing block in the chain, cannot start force approval"
		);
	}

	Ok(approved_hashes)
}

/// Revert to the block corresponding to the specified `hash`.
/// The operation is not allowed for blocks older than the last finalized one.
pub fn revert_to(
	overlay: &mut OverlayedBackend<'_, impl Backend>,
	hash: Hash,
) -> SubsystemResult<()> {
	let mut stored_range = overlay.load_stored_blocks()?.ok_or_else(|| {
		SubsystemError::Context("no available blocks to infer revert point height".to_string())
	})?;

	let (children, children_height) = match overlay.load_block_entry(&hash)? {
		Some(mut entry) => {
			let children_height = entry.block_number() + 1;
			let children = std::mem::take(&mut entry.children);
			// Write revert point block entry without the children.
			overlay.write_block_entry(entry);
			(children, children_height)
		},
		None => {
			let children_height = stored_range.0;
			let children = overlay.load_blocks_at_height(&children_height)?;

			let child_entry = children
				.first()
				.and_then(|hash| overlay.load_block_entry(hash).ok())
				.flatten()
				.ok_or_else(|| {
					SubsystemError::Context("lookup failure for first block".to_string())
				})?;

			// The parent is expected to be the revert point
			if child_entry.parent_hash() != hash {
				return Err(SubsystemError::Context(
					"revert below last finalized block or corrupted storage".to_string(),
				))
			}

			(children, children_height)
		},
	};

	let mut stack: Vec<_> = children.into_iter().map(|h| (h, children_height)).collect();
	let mut range_end = stored_range.1;

	while let Some((hash, number)) = stack.pop() {
		let mut blocks_at_height = overlay.load_blocks_at_height(&number)?;
		blocks_at_height.retain(|h| h != &hash);

		// Check if we need to update the range top
		if blocks_at_height.is_empty() && number < range_end {
			range_end = number;
		}

		overlay.write_blocks_at_height(number, blocks_at_height);

		if let Some(entry) = overlay.load_block_entry(&hash)? {
			overlay.delete_block_entry(&hash);

			// Cleanup the candidate entries by removing any reference to the
			// removed block. If for a candidate entry the block block_assignments
			// drops to zero then we remove the entry.
			for (_, candidate_hash) in entry.candidates() {
				if let Some(mut candidate_entry) = overlay.load_candidate_entry(candidate_hash)? {
					candidate_entry.block_assignments.remove(&hash);
					if candidate_entry.block_assignments.is_empty() {
						overlay.delete_candidate_entry(candidate_hash);
					} else {
						overlay.write_candidate_entry(candidate_entry);
					}
				}
			}

			stack.extend(entry.children.into_iter().map(|h| (h, number + 1)));
		}
	}

	// Check if our modifications to the dag has reduced the range top
	if range_end != stored_range.1 {
		if stored_range.0 < range_end {
			stored_range.1 = range_end;
			overlay.write_stored_block_range(stored_range);
		} else {
			overlay.delete_stored_block_range();
		}
	}

	Ok(())
}
