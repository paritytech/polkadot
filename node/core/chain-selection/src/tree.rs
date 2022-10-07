// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Implements the tree-view over the data backend which we use to determine
//! viable leaves.
//!
//! The metadata is structured as a tree, with the root implicitly being the
//! finalized block, which is not stored as part of the tree.
//!
//! Each direct descendant of the finalized block acts as its own sub-tree,
//! and as the finalized block advances, orphaned sub-trees are entirely pruned.

use polkadot_node_primitives::BlockWeight;
use polkadot_node_subsystem::ChainApiError;
use polkadot_primitives::v2::{BlockNumber, Hash};

use std::collections::HashMap;

use super::{Approval, BlockEntry, Error, LeafEntry, Timestamp, ViabilityCriteria, LOG_TARGET};
use crate::backend::{Backend, OverlayedBackend};

// A viability update to be applied to a block.
struct ViabilityUpdate(Option<Hash>);

impl ViabilityUpdate {
	// Apply the viability update to a single block, yielding the updated
	// block entry along with a vector of children and the updates to apply
	// to them.
	fn apply(self, mut entry: BlockEntry) -> (BlockEntry, Vec<(Hash, ViabilityUpdate)>) {
		// 1. When an ancestor has changed from unviable to viable,
		// we erase the `earliest_unviable_ancestor` of all descendants
		// until encountering a explicitly unviable descendant D.
		//
		// We then update the `earliest_unviable_ancestor` for all
		// descendants of D to be equal to D.
		//
		// 2. When an ancestor A has changed from viable to unviable,
		// we update the `earliest_unviable_ancestor` for all blocks
		// to A.
		//
		// The following algorithm covers both cases.
		//
		// Furthermore, if there has been any change in viability,
		// it is necessary to visit every single descendant of the root
		// block.
		//
		// If a block B was unviable and is now viable, then every descendant
		// has an `earliest_unviable_ancestor` which must be updated either
		// to nothing or to the new earliest unviable ancestor.
		//
		// If a block B was viable and is now unviable, then every descendant
		// has an `earliest_unviable_ancestor` which needs to be set to B.

		let maybe_earliest_unviable = self.0;
		let next_earliest_unviable = {
			if maybe_earliest_unviable.is_none() && !entry.viability.is_explicitly_viable() {
				Some(entry.block_hash)
			} else {
				maybe_earliest_unviable
			}
		};
		entry.viability.earliest_unviable_ancestor = maybe_earliest_unviable;

		let recurse = entry
			.children
			.iter()
			.cloned()
			.map(move |c| (c, ViabilityUpdate(next_earliest_unviable)))
			.collect();

		(entry, recurse)
	}
}

// Propagate viability update to descendants of the given block. This writes
// the `base` entry as well as all descendants. If the parent of the block
// entry is not viable, this will not affect any descendants.
//
// If the block entry provided is self-unviable, then it's assumed that an
// unviability update needs to be propagated to descendants.
//
// If the block entry provided is self-viable, then it's assumed that a
// viability update needs to be propagated to descendants.
fn propagate_viability_update(
	backend: &mut OverlayedBackend<impl Backend>,
	base: BlockEntry,
) -> Result<(), Error> {
	enum BlockEntryRef {
		Explicit(BlockEntry),
		Hash(Hash),
	}

	if !base.viability.is_parent_viable() {
		// If the parent of the block is still unviable,
		// then the `earliest_viable_ancestor` will not change
		// regardless of the change in the block here.
		//
		// Furthermore, in such cases, the set of viable leaves
		// does not change at all.
		backend.write_block_entry(base);
		return Ok(())
	}

	let mut viable_leaves = backend.load_leaves()?;

	// A mapping of Block Hash -> number
	// Where the hash is the hash of a viable block which has
	// at least 1 unviable child.
	//
	// The number is the number of known unviable children which is known
	// as the pivot count.
	let mut viability_pivots = HashMap::new();

	// If the base block is itself explicitly unviable,
	// this will change to a `Some(base_hash)` after the first
	// invocation.
	let viability_update = ViabilityUpdate(None);

	// Recursively apply update to tree.
	//
	// As we go, we remove any blocks from the leaves which are no longer viable
	// leaves. We also add blocks to the leaves-set which are obviously viable leaves.
	// And we build up a frontier of blocks which may either be viable leaves or
	// the ancestors of one.
	let mut tree_frontier = vec![(BlockEntryRef::Explicit(base), viability_update)];
	while let Some((entry_ref, update)) = tree_frontier.pop() {
		let entry = match entry_ref {
			BlockEntryRef::Explicit(entry) => entry,
			BlockEntryRef::Hash(hash) => match backend.load_block_entry(&hash)? {
				None => {
					gum::warn!(
						target: LOG_TARGET,
						block_hash = ?hash,
						"Missing expected block entry"
					);

					continue
				},
				Some(entry) => entry,
			},
		};

		let (new_entry, children) = update.apply(entry);

		if new_entry.viability.is_viable() {
			// A block which is viable has a parent which is obviously not
			// in the viable leaves set.
			viable_leaves.remove(&new_entry.parent_hash);

			// Furthermore, if the block is viable and has no children,
			// it is viable by definition.
			if new_entry.children.is_empty() {
				viable_leaves.insert(new_entry.leaf_entry());
			}
		} else {
			// A block which is not viable is certainly not a viable leaf.
			viable_leaves.remove(&new_entry.block_hash);

			// When the parent is viable but the entry itself is not, that means
			// that the parent is a viability pivot. As we visit the children
			// of a viability pivot, we build up an exhaustive pivot count.
			if new_entry.viability.is_parent_viable() {
				*viability_pivots.entry(new_entry.parent_hash).or_insert(0) += 1;
			}
		}

		backend.write_block_entry(new_entry);

		tree_frontier
			.extend(children.into_iter().map(|(h, update)| (BlockEntryRef::Hash(h), update)));
	}

	// Revisit the viability pivots now that we've traversed the entire subtree.
	// After this point, the viable leaves set is fully updated. A proof follows.
	//
	// If the base has become unviable, then we've iterated into all descendants,
	// made them unviable and removed them from the set. We know that the parent is
	// viable as this function is a no-op otherwise, so we need to see if the parent
	// has other children or not.
	//
	// If the base has become viable, then we've iterated into all descendants,
	// and found all blocks which are viable and have no children. We've already added
	// those blocks to the leaf set, but what we haven't detected
	// is blocks which are viable and have children, but all of the children are
	// unviable.
	//
	// The solution of viability pivots addresses both of these:
	//
	// When the base has become unviable, the parent's viability is unchanged and therefore
	// any leaves descending from parent but not base are still in the viable leaves set.
	// If the parent has only one child which is the base, the parent is now a viable leaf.
	// We've already visited the base in recursive search so the set of pivots should
	// contain only a single entry `(parent, 1)`. qed.
	//
	// When the base has become viable, we've already iterated into every descendant
	// of the base and thus have collected a set of pivots whose corresponding pivot
	// counts have already been exhaustively computed from their children. qed.
	for (pivot, pivot_count) in viability_pivots {
		match backend.load_block_entry(&pivot)? {
			None => {
				// This means the block is finalized. We might reach this
				// code path when the base is a child of the finalized block
				// and has become unviable.
				//
				// Each such child is the root of its own tree
				// which, as an invariant, does not depend on the viability
				// of the finalized block. So no siblings need to be inspected
				// and we can ignore it safely.
				//
				// Furthermore, if the set of viable leaves is empty, the
				// finalized block is implicitly the viable leaf.
				continue
			},
			Some(entry) =>
				if entry.children.len() == pivot_count {
					viable_leaves.insert(entry.leaf_entry());
				},
		}
	}

	backend.write_leaves(viable_leaves);

	Ok(())
}

/// Imports a new block and applies any reversions to ancestors.
pub(crate) fn import_block(
	backend: &mut OverlayedBackend<impl Backend>,
	block_hash: Hash,
	block_number: BlockNumber,
	parent_hash: Hash,
	reversion_logs: Vec<BlockNumber>,
	weight: BlockWeight,
	stagnant_at: Timestamp,
) -> Result<(), Error> {
	add_block(backend, block_hash, block_number, parent_hash, weight, stagnant_at)?;
	apply_reversions(backend, block_hash, block_number, reversion_logs)?;

	Ok(())
}

// Load the given ancestor's block entry, in descending order from the `block_hash`.
// The ancestor_number must be at least one block less than the `block_number`.
//
// The returned entry will be `None` if the range is invalid or any block in the path had
// no entry present. If any block entry was missing, it can safely be assumed to
// be finalized.
fn load_ancestor(
	backend: &mut OverlayedBackend<impl Backend>,
	block_hash: Hash,
	block_number: BlockNumber,
	ancestor_number: BlockNumber,
) -> Result<Option<BlockEntry>, Error> {
	if block_number <= ancestor_number {
		return Ok(None)
	}

	let mut current_hash = block_hash;
	let mut current_entry = None;

	let segment_length = (block_number - ancestor_number) + 1;
	for _ in 0..segment_length {
		match backend.load_block_entry(&current_hash)? {
			None => return Ok(None),
			Some(entry) => {
				let parent_hash = entry.parent_hash;
				current_entry = Some(entry);
				current_hash = parent_hash;
			},
		}
	}

	// Current entry should always be `Some` here.
	Ok(current_entry)
}

// Add a new block to the tree, which is assumed to be unreverted and unapproved,
// but not stagnant. It inherits viability from its parent, if any.
//
// This updates the parent entry, if any, and updates the viable leaves set accordingly.
// This also schedules a stagnation-check update and adds the block to the blocks-by-number
// mapping.
fn add_block(
	backend: &mut OverlayedBackend<impl Backend>,
	block_hash: Hash,
	block_number: BlockNumber,
	parent_hash: Hash,
	weight: BlockWeight,
	stagnant_at: Timestamp,
) -> Result<(), Error> {
	let mut leaves = backend.load_leaves()?;
	let parent_entry = backend.load_block_entry(&parent_hash)?;

	let inherited_viability =
		parent_entry.as_ref().and_then(|parent| parent.non_viable_ancestor_for_child());

	// 1. Add the block to the DB assuming it's not reverted.
	backend.write_block_entry(BlockEntry {
		block_hash,
		block_number,
		parent_hash,
		children: Vec::new(),
		viability: ViabilityCriteria {
			earliest_unviable_ancestor: inherited_viability,
			explicitly_reverted: false,
			approval: Approval::Unapproved,
		},
		weight,
	});

	// 2. Update leaves if inherited viability is fine.
	if inherited_viability.is_none() {
		leaves.remove(&parent_hash);
		leaves.insert(LeafEntry { block_hash, block_number, weight });
		backend.write_leaves(leaves);
	}

	// 3. Update and write the parent
	if let Some(mut parent_entry) = parent_entry {
		parent_entry.children.push(block_hash);
		backend.write_block_entry(parent_entry);
	}

	// 4. Add to blocks-by-number.
	let mut blocks_by_number = backend.load_blocks_by_number(block_number)?;
	blocks_by_number.push(block_hash);
	backend.write_blocks_by_number(block_number, blocks_by_number);

	// 5. Add stagnation timeout.
	let mut stagnant_at_list = backend.load_stagnant_at(stagnant_at)?;
	stagnant_at_list.push(block_hash);
	backend.write_stagnant_at(stagnant_at, stagnant_at_list);

	Ok(())
}

// Assuming that a block is already imported, accepts the number of the block
// as well as a list of reversions triggered by the block in ascending order.
fn apply_reversions(
	backend: &mut OverlayedBackend<impl Backend>,
	block_hash: Hash,
	block_number: BlockNumber,
	reversions: Vec<BlockNumber>,
) -> Result<(), Error> {
	// Note: since revert numbers are  in ascending order, the expensive propagation
	// of unviability is only heavy on the first log.
	for revert_number in reversions {
		let mut ancestor_entry =
			match load_ancestor(backend, block_hash, block_number, revert_number)? {
				None => {
					gum::warn!(
						target: LOG_TARGET,
						?block_hash,
						block_number,
						revert_target = revert_number,
						"The hammer has dropped. \
					A block has indicated that its finalized ancestor be reverted. \
					Please inform an adult.",
					);

					continue
				},
				Some(ancestor_entry) => {
					gum::info!(
						target: LOG_TARGET,
						?block_hash,
						block_number,
						revert_target = revert_number,
						revert_hash = ?ancestor_entry.block_hash,
						"A block has signaled that its ancestor be reverted due to a bad parachain block.",
					);

					ancestor_entry
				},
			};

		ancestor_entry.viability.explicitly_reverted = true;
		propagate_viability_update(backend, ancestor_entry)?;
	}

	Ok(())
}

/// Finalize a block with the given number and hash.
///
/// This will prune all sub-trees not descending from the given block,
/// all block entries at or before the given height,
/// and will update the viability of all sub-trees descending from the given
/// block if the finalized block was not viable.
///
/// This is assumed to start with a fresh backend, and will produce
/// an overlay over the backend with all the changes applied.
pub(super) fn finalize_block<'a, B: Backend + 'a>(
	backend: &'a B,
	finalized_hash: Hash,
	finalized_number: BlockNumber,
) -> Result<OverlayedBackend<'a, B>, Error> {
	let earliest_stored_number = backend.load_first_block_number()?;
	let mut backend = OverlayedBackend::new(backend);

	let earliest_stored_number = match earliest_stored_number {
		None => {
			// This implies that there are no unfinalized blocks and hence nothing
			// to update.
			return Ok(backend)
		},
		Some(e) => e,
	};

	let mut viable_leaves = backend.load_leaves()?;

	// Walk all numbers up to the finalized number and remove those entries.
	for number in earliest_stored_number..finalized_number {
		let blocks_at = backend.load_blocks_by_number(number)?;
		backend.delete_blocks_by_number(number);

		for block in blocks_at {
			viable_leaves.remove(&block);
			backend.delete_block_entry(&block);
		}
	}

	// Remove all blocks at the finalized height, with the exception of the finalized block,
	// and their descendants, recursively.
	{
		let blocks_at_finalized_height = backend.load_blocks_by_number(finalized_number)?;
		backend.delete_blocks_by_number(finalized_number);

		let mut frontier: Vec<_> = blocks_at_finalized_height
			.into_iter()
			.filter(|h| h != &finalized_hash)
			.map(|h| (h, finalized_number))
			.collect();

		while let Some((dead_hash, dead_number)) = frontier.pop() {
			let entry = backend.load_block_entry(&dead_hash)?;
			backend.delete_block_entry(&dead_hash);
			viable_leaves.remove(&dead_hash);

			// This does a few extra `clone`s but is unlikely to be
			// a bottleneck. Code complexity is very low as a result.
			let mut blocks_at_height = backend.load_blocks_by_number(dead_number)?;
			blocks_at_height.retain(|h| h != &dead_hash);
			backend.write_blocks_by_number(dead_number, blocks_at_height);

			// Add all children to the frontier.
			let next_height = dead_number + 1;
			frontier.extend(entry.into_iter().flat_map(|e| e.children).map(|h| (h, next_height)));
		}
	}

	// Visit and remove the finalized block, fetching its children.
	let children_of_finalized = {
		let finalized_entry = backend.load_block_entry(&finalized_hash)?;
		backend.delete_block_entry(&finalized_hash);
		viable_leaves.remove(&finalized_hash);

		finalized_entry.into_iter().flat_map(|e| e.children)
	};

	backend.write_leaves(viable_leaves);

	// Update the viability of each child.
	for child in children_of_finalized {
		if let Some(mut child) = backend.load_block_entry(&child)? {
			// Finalized blocks are always viable.
			child.viability.earliest_unviable_ancestor = None;

			propagate_viability_update(&mut backend, child)?;
		} else {
			gum::debug!(
				target: LOG_TARGET,
				?finalized_hash,
				finalized_number,
				child_hash = ?child,
				"Missing child of finalized block",
			);

			// No need to do anything, but this is an inconsistent state.
		}
	}

	Ok(backend)
}

/// Mark a block as approved and update the viability of itself and its
/// descendants accordingly.
pub(super) fn approve_block(
	backend: &mut OverlayedBackend<impl Backend>,
	approved_hash: Hash,
) -> Result<(), Error> {
	if let Some(mut entry) = backend.load_block_entry(&approved_hash)? {
		let was_viable = entry.viability.is_viable();
		entry.viability.approval = Approval::Approved;
		let is_viable = entry.viability.is_viable();

		// Approval can change the viability in only one direction.
		// If the viability has changed, then we propagate that to children
		// and recalculate the viable leaf set.
		if !was_viable && is_viable {
			propagate_viability_update(backend, entry)?;
		} else {
			backend.write_block_entry(entry);
		}
	} else {
		gum::debug!(
			target: LOG_TARGET,
			block_hash = ?approved_hash,
			"Missing entry for freshly-approved block. Ignoring"
		);
	}

	Ok(())
}

/// Check whether any blocks up to the given timestamp are stagnant and update
/// accordingly.
///
/// This accepts a fresh backend and returns an overlay on top of it representing
/// all changes made.
pub(super) fn detect_stagnant<'a, B: 'a + Backend>(
	backend: &'a B,
	up_to: Timestamp,
	max_elements: usize,
) -> Result<OverlayedBackend<'a, B>, Error> {
	let stagnant_up_to = backend.load_stagnant_at_up_to(up_to, max_elements)?;
	let mut backend = OverlayedBackend::new(backend);

	let (min_ts, max_ts) = match stagnant_up_to.len() {
		0 => (0 as Timestamp, 0 as Timestamp),
		1 => (stagnant_up_to[0].0, stagnant_up_to[0].0),
		n => (stagnant_up_to[0].0, stagnant_up_to[n - 1].0),
	};

	// As this is in ascending order, only the earliest stagnant
	// blocks will involve heavy viability propagations.
	gum::debug!(
		target: LOG_TARGET,
		?up_to,
		?min_ts,
		?max_ts,
		"Prepared {} stagnant entries for checking/pruning",
		stagnant_up_to.len()
	);

	for (timestamp, maybe_stagnant) in stagnant_up_to {
		backend.delete_stagnant_at(timestamp);

		for block_hash in maybe_stagnant {
			if let Some(mut entry) = backend.load_block_entry(&block_hash)? {
				let was_viable = entry.viability.is_viable();
				if let Approval::Unapproved = entry.viability.approval {
					entry.viability.approval = Approval::Stagnant;
				}
				let is_viable = entry.viability.is_viable();
				gum::trace!(
					target: LOG_TARGET,
					?block_hash,
					?timestamp,
					?was_viable,
					?is_viable,
					"Found existing stagnant entry"
				);

				if was_viable && !is_viable {
					propagate_viability_update(&mut backend, entry)?;
				} else {
					backend.write_block_entry(entry);
				}
			} else {
				gum::trace!(
					target: LOG_TARGET,
					?block_hash,
					?timestamp,
					"Found non-existing stagnant entry"
				);
			}
		}
	}

	Ok(backend)
}

/// Prune stagnant entries at some timestamp without other checks
/// This function is intended just to clean leftover entries when the real
/// stagnant checks are disabled
pub(super) fn prune_only_stagnant<'a, B: 'a + Backend>(
	backend: &'a B,
	up_to: Timestamp,
	max_elements: usize,
) -> Result<OverlayedBackend<'a, B>, Error> {
	let stagnant_up_to = backend.load_stagnant_at_up_to(up_to, max_elements)?;
	let mut backend = OverlayedBackend::new(backend);

	let (min_ts, max_ts) = match stagnant_up_to.len() {
		0 => (0 as Timestamp, 0 as Timestamp),
		1 => (stagnant_up_to[0].0, stagnant_up_to[0].0),
		n => (stagnant_up_to[0].0, stagnant_up_to[n - 1].0),
	};

	gum::debug!(
		target: LOG_TARGET,
		?up_to,
		?min_ts,
		?max_ts,
		"Prepared {} stagnant entries for pruning",
		stagnant_up_to.len()
	);

	for (timestamp, _) in stagnant_up_to {
		backend.delete_stagnant_at(timestamp);
	}

	Ok(backend)
}

/// Revert the tree to the block relative to `hash`.
///
/// This accepts a fresh backend and returns an overlay on top of it representing
/// all changes made.
pub(super) fn revert_to<'a, B: Backend + 'a>(
	backend: &'a B,
	hash: Hash,
) -> Result<OverlayedBackend<'a, B>, Error> {
	let first_number = backend.load_first_block_number()?.unwrap_or_default();

	let mut backend = OverlayedBackend::new(backend);

	let mut entry = match backend.load_block_entry(&hash)? {
		Some(entry) => entry,
		None => {
			// May be a revert to the last finalized block. If this is the case,
			// then revert to this block should be handled specially since no
			// information about finalized blocks is persisted within the tree.
			//
			// We use part of the information contained in the finalized block
			// children (that are expected to be in the tree) to construct a
			// dummy block entry for the last finalized block. This will be
			// wiped as soon as the next block is finalized.

			let blocks = backend.load_blocks_by_number(first_number)?;

			let block = blocks
				.first()
				.and_then(|hash| backend.load_block_entry(hash).ok())
				.flatten()
				.ok_or_else(|| {
					ChainApiError::from(format!(
						"Lookup failure for block at height {}",
						first_number
					))
				})?;

			// The parent is expected to be the last finalized block.
			if block.parent_hash != hash {
				return Err(ChainApiError::from("Can't revert below last finalized block").into())
			}

			// The weight is set to the one of the first child. Even though this is
			// not accurate, it does the job. The reason is that the revert point is
			// the last finalized block, i.e. this is the best and only choice.
			let block_number = first_number.saturating_sub(1);
			let viability = ViabilityCriteria {
				explicitly_reverted: false,
				approval: Approval::Approved,
				earliest_unviable_ancestor: None,
			};
			let entry = BlockEntry {
				block_hash: hash,
				block_number,
				parent_hash: Hash::default(),
				children: blocks,
				viability,
				weight: block.weight,
			};
			// This becomes the first entry according to the block number.
			backend.write_blocks_by_number(block_number, vec![hash]);
			entry
		},
	};

	let mut stack: Vec<_> = std::mem::take(&mut entry.children)
		.into_iter()
		.map(|h| (h, entry.block_number + 1))
		.collect();

	// Write revert point block entry without the children.
	backend.write_block_entry(entry.clone());

	let mut viable_leaves = backend.load_leaves()?;

	viable_leaves.insert(LeafEntry {
		block_hash: hash,
		block_number: entry.block_number,
		weight: entry.weight,
	});

	while let Some((hash, number)) = stack.pop() {
		let entry = backend.load_block_entry(&hash)?;
		backend.delete_block_entry(&hash);

		viable_leaves.remove(&hash);

		let mut blocks_at_height = backend.load_blocks_by_number(number)?;
		blocks_at_height.retain(|h| h != &hash);
		backend.write_blocks_by_number(number, blocks_at_height);

		stack.extend(entry.into_iter().flat_map(|e| e.children).map(|h| (h, number + 1)));
	}

	backend.write_leaves(viable_leaves);

	Ok(backend)
}
