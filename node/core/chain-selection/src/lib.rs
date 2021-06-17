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

//! Implements the Chain Selection Subsystem.

use polkadot_primitives::v1::{BlockNumber, Hash, Header, ConsensusLog};
use polkadot_subsystem::{
	Subsystem, SubsystemContext, SubsystemResult, SubsystemError, SpawnedSubsystem,
	OverseerSignal, FromOverseer,
	messages::{ChainSelectionMessage, ChainApiMessage},
	errors::ChainApiError,
};

use parity_scale_codec::Error as CodecError;
use futures::channel::oneshot;

use std::collections::HashMap;
use std::time::{UNIX_EPOCH, SystemTime};

use crate::backend::{Backend, OverlayedBackend, BackendWriteOp};

mod backend;

const LOG_TARGET: &str = "parachain::chain-selection";

type Weight = u64;
type Timestamp = u64;

#[derive(Debug, Clone)]
enum Approval {
	// Approved
	Approved,
	// Unapproved but not stagnant
	Unapproved,
	// Unapproved and stagnant.
	Stagnant,
}

impl Approval {
	fn is_stagnant(&self) -> bool {
		matches!(*self, Approval::Stagnant)
	}
}

#[derive(Debug, Clone)]
struct ViabilityCriteria {
	// Whether this block has been explicitly reverted by one of its descendants.
	explicitly_reverted: bool,
	// The approval state of this block specifically.
	approval: Approval,
	// The earliest unviable ancestor - the hash of the earliest unfinalized
	// block in the ancestry which is explicitly reverted or stagnant.
	earliest_unviable_ancestor: Option<Hash>,
}

impl ViabilityCriteria {
	fn is_viable(&self) -> bool {
		self.is_parent_viable() && self.is_explicitly_viable()
	}

	// Whether the current block is explicitly viable.
	// That is, whether the current block is neither reverted nor stagnant.
	fn is_explicitly_viable(&self) -> bool {
		!self.explicitly_reverted && !self.approval.is_stagnant()
	}

	// Whether the parent is viable. This assumes that the parent
	// descends from the finalized chain.
	fn is_parent_viable(&self) -> bool {
		self.earliest_unviable_ancestor.is_none()
	}
}

#[derive(Debug, Clone)]
struct LeafEntry {
	weight: Weight,
	block_hash: Hash,
}

#[derive(Debug, Clone)]
struct LeafEntrySet {
	inner: Vec<LeafEntry>
}

impl LeafEntrySet {
	fn contains(&self, hash: &Hash) -> bool {
		self.inner.iter().position(|e| &e.block_hash == hash).is_some()
	}

	fn remove(&mut self, hash: &Hash) -> bool {
		match self.inner.iter().position(|e| &e.block_hash == hash) {
			None => false,
			Some(i) => {
				self.inner.remove(i);
				true
			}
		}
	}

	fn insert(&mut self, new: LeafEntry) {
		match self.inner.iter().position(|e| e.weight < new.weight) {
			None => self.inner.push(new),
			Some(i) => if self.inner[i].block_hash != new.block_hash {
				self.inner.insert(i, new);
			}
		}
	}
}

#[derive(Debug, Clone)]
struct BlockEntry {
	block_hash: Hash,
	parent_hash: Hash,
	children: Vec<Hash>,
	viability: ViabilityCriteria,
	weight: Weight,
}

impl BlockEntry {
	fn leaf_entry(&self) -> LeafEntry {
		LeafEntry {
			block_hash: self.block_hash,
			weight: self.weight,
		}
	}

	fn non_viable_ancestor_for_child(&self) -> Option<Hash> {
		if self.viability.is_viable() {
			None
		} else {
			self.viability.earliest_unviable_ancestor.or(Some(self.block_hash))
		}
	}
}

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Io(#[from] std::io::Error),

	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),

	#[error(transparent)]
	Subsystem(#[from] SubsystemError),

	#[error(transparent)]
	Codec(#[from] CodecError),
}

impl Error {
	fn trace(&self) {
		match self {
			// don't spam the log with spurious errors
			Self::Oneshot(_) => tracing::debug!(target: LOG_TARGET, err = ?self),
			// it's worth reporting otherwise
			_ => tracing::warn!(target: LOG_TARGET, err = ?self),
		}
	}
}

fn timestamp_now() -> Timestamp {
	// `SystemTime` is notoriously non-monotonic, so our timers might not work
	// exactly as expected. Regardless, stagnation is detected on the order of minutes,
	// and slippage of a few seconds in either direction won't cause any major harm.
	//
	// The exact time that a block becomes stagnant in the local node is always expected
	// to differ from other nodes due to network asynchrony and delays in block propagation.
	// Non-monotonicity exarcerbates that somewhat, but not meaningfully.

	match SystemTime::now().duration_since(UNIX_EPOCH) {
		Ok(d) => d.as_secs(),
		Err(e) => {
			tracing::warn!(
				target: LOG_TARGET,
				err = ?e,
				"Current time is before unix epoch. Validation will not work correctly."
			);

			0
		}
	}
}

fn stagnant_timeout_from_now() -> Timestamp {
	// If a block isn't approved in 120 seconds, nodes will abandon it
	// and begin building on another chain.
	const STAGNANT_TIMEOUT: Timestamp = 120;

	timestamp_now() + STAGNANT_TIMEOUT
}

async fn run<Context, B>(mut ctx: Context, mut backend: B)
	where
		Context: SubsystemContext<Message = ChainSelectionMessage>,
		B: Backend,
{
	loop {
		let res = run_iteration(&mut ctx, &mut backend).await;
		match res {
			Err(e) => {
				e.trace();

				if let Error::Subsystem(SubsystemError::Context(_)) = e {
					break;
				}
			}
			Ok(()) => {
				tracing::info!(target: LOG_TARGET, "received `Conclude` signal, exiting");
				break;
			}
		}
	}
}

// Run the subsystem until an error is encountered or a `conclude` signal is received.
// Most errors are non-fatal and should lead to another call to this function.
//
// A return value of `Ok` indicates that an exit should be made, while non-fatal errors
// lead to another call to this function.
async fn run_iteration<Context, B>(ctx: &mut Context, backend: &mut B)
	-> Result<(), Error>
	where
		Context: SubsystemContext<Message = ChainSelectionMessage>,
		B: Backend,
{
	loop {
		match ctx.recv().await? {
			FromOverseer::Signal(OverseerSignal::Conclude) => {
				return Ok(())
			}
			FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
				unimplemented!()
			}
			FromOverseer::Signal(OverseerSignal::BlockFinalized(_, _)) => {
				unimplemented!()
			}
			FromOverseer::Communication { msg } => {
				unimplemented!()
			}
		};
	}
}

async fn fetch_finalized_number(
	ctx: &mut impl SubsystemContext,
) -> Result<BlockNumber, Error> {
	unimplemented!()
}

async fn fetch_header(
	ctx: &mut impl SubsystemContext,
	hash: Hash,
) -> Result<Option<Header>, Error> {
	let (h_tx, h_rx) = oneshot::channel();
	ctx.send_message(ChainApiMessage::BlockHeader(hash, h_tx).into()).await;

	match h_rx.await?? {
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				?hash,
				"Missing header for new head",
			);
			Ok(None)
		}
		Some(h) => Ok(Some(h)),
	}
}

// Handle a new active leaf.
async fn handle_active_leaf(
	ctx: &mut impl SubsystemContext,
	backend: &impl Backend,
	hash: Hash,
) -> Result<Vec<BackendWriteOp>, Error> {
	let lower_bound = match backend.load_first_block_number()? {
		Some(l) => l,
		None => fetch_finalized_number(ctx).await?,
	};

	let header = match fetch_header(ctx, hash).await? {
		None => return Ok(Vec::new()),
		Some(h) => h,
	};

	let new_blocks = polkadot_node_subsystem_util::determine_new_blocks(
		ctx.sender(),
		|h| backend.load_block_entry(h).map(|b| b.is_some()),
		hash,
		&header,
		lower_bound,
	).await?;

	let mut overlay = OverlayedBackend::new(backend);

	// determine_new_blocks gives blocks in descending order.
	// for this, we want ascending order.
	for (hash, header) in new_blocks.into_iter().rev() {
		let weight = unimplemented!();
		import_block(&mut overlay, hash, header, weight)?;
	}

	Ok(overlay.into_write_ops().collect())
}

fn import_block(
	backend: &mut OverlayedBackend<impl Backend>,
	block_hash: Hash,
	block_header: Header,
	weight: Weight,
) -> Result<(), Error> {
	import_block_ignoring_reversions(backend, block_hash, &block_header, weight)?;
	apply_imported_block_reversions(backend, block_hash, &block_header)?;

	Ok(())
}

fn import_block_ignoring_reversions(
	backend: &mut OverlayedBackend<impl Backend>,
	block_hash: Hash,
	block_header: &Header,
	weight: Weight,
) -> Result<(), Error> {
	let parent_hash = block_header.parent_hash;

	let mut leaves = backend.load_leaves()?;
	let parent_entry = backend.load_block_entry(&parent_hash)?;

	let inherited_viability = parent_entry.as_ref()
		.and_then(|parent| parent.non_viable_ancestor_for_child());

	// 1. Add the block to the DB assuming it's not reverted.
	backend.write_block_entry(
		BlockEntry {
			block_hash,
			parent_hash,
			children: Vec::new(),
			viability: ViabilityCriteria {
				earliest_unviable_ancestor: inherited_viability,
				explicitly_reverted: false,
				approval: Approval::Unapproved,
			},
			weight,
		}
	);

	// 2. Update leaves if parent was a viable leaf or the parent is unknown.
	if leaves.remove(&parent_hash) || parent_entry.is_none() {
		leaves.insert(LeafEntry { block_hash, weight });
		backend.write_leaves(leaves);
	}

	// 3. Update and write the parent
	if let Some(mut parent_entry) = parent_entry {
		parent_entry.children.push(block_hash);
		backend.write_block_entry(parent_entry);
	}

	// 4. Add to blocks-by-number.
	let mut blocks_by_number = backend.load_blocks_by_number(block_header.number)?;
	blocks_by_number.push(block_hash);
	backend.write_blocks_by_number(block_header.number, blocks_by_number);

	// 5. Add stagnation timeout.
	let stagnant_at = stagnant_timeout_from_now();
	let mut stagnant_at_list = backend.load_stagnant_at(stagnant_at)?;
	stagnant_at_list.push(block_hash);
	backend.write_stagnant_at(stagnant_at, stagnant_at_list);

	Ok(())
}

// Extract all reversion logs from a header in ascending order.
//
// Ignores logs with number >= the block header number.
fn extract_reversion_logs(header: &Header) -> Vec<BlockNumber> {
	let number = header.number;
	let mut logs = header.digest.logs()
		.iter()
		.enumerate()
		.filter_map(|(i, d)| match ConsensusLog::from_digest_item(d) {
			Err(e) => {
				tracing::warn!(
					target: LOG_TARGET,
					err = ?e,
					index = i,
					block_hash = ?header.hash(),
					"Digest item failed to encode"
				);

				None
			}
			Ok(Some(ConsensusLog::Revert(b))) if b < number => Some(b),
			Ok(Some(ConsensusLog::Revert(b))) => {
				tracing::warn!(
					target: LOG_TARGET,
					revert_target = b,
					block_number = number,
					block_hash = ?header.hash(),
					"Block issued invalid revert digest targeting itself or future"
				);

				None
			}
			Ok(_) => None,
		})
		.collect::<Vec<_>>();

	logs.sort();

	logs
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
	if block_number <= ancestor_number { return Ok(None) }

	let mut current_hash = block_hash;
	let mut current_entry = None;

	let segment_length = (block_number - ancestor_number) + 1;
	for _ in std::iter::repeat(()).take(segment_length as usize) {
		match backend.load_block_entry(&current_hash)? {
			None => return Ok(None),
			Some(entry) => {
				let parent_hash = entry.parent_hash;
				current_entry = Some(entry);
				current_hash = parent_hash;
			}
		}
	}

	// Current entry should always be `Some` here.
	Ok(current_entry)
}

// A viability update to be applied to a block.
struct ViabilityUpdate(Option<Hash>);

impl ViabilityUpdate {
	// Apply the viability update to a single block, yielding the updated
	// block entry along with a vector of children and the updates to apply
	// to them.
	fn apply(self, mut entry: BlockEntry) -> (
		BlockEntry,
		Vec<(Hash, ViabilityUpdate)>
	) {
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

		let recurse = entry.children.iter()
			.cloned()
			.map(move |c| (c, ViabilityUpdate(next_earliest_unviable)))
			.collect();

		(entry, recurse)
	}
}

// Propagate viability update to descendants of the given block.
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
					tracing::warn!(
						target: LOG_TARGET,
						block_hash = ?hash,
						"Missing expected block entry"
					);

					continue;
				}
				Some(entry) => entry,
			}
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

		tree_frontier.extend(
			children.into_iter().map(|(h, update)| (BlockEntryRef::Hash(h), update))
		);
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
			}
			Some(entry) => {
				if entry.children.len() == pivot_count {
					viable_leaves.insert(entry.leaf_entry());
				}
			}
		}
	}

	backend.write_leaves(viable_leaves);

	Ok(())
}

// Assuming that a block is already imported, scans the header of the block
// for revert signals and applies those to relevant ancestors, and recursively
// updates the viability of those ancestors' descendants.
fn apply_imported_block_reversions(
	backend: &mut OverlayedBackend<impl Backend>,
	block_hash: Hash,
	block_header: &Header,
) -> Result<(), Error> {
	let logs = extract_reversion_logs(&block_header);

	// Note: since revert numbers are returned from `extract_reversion_logs`
	// in ascending order, the expensive propagation of unviability is
	// only heavy on the first log.
	for revert_number in logs {
		let mut ancestor_entry = match load_ancestor(
			backend,
			block_hash,
			block_header.number,
			revert_number,
		)? {
			None => {
				tracing::warn!(
					target: LOG_TARGET,
					?block_hash,
					block_number = block_header.number,
					revert_target = revert_number,
					"The hammer has dropped. \
					A block has indicated that its finalized ancestor be reverted. \
					Please inform an adult.",
				);

				continue
			}
			Some(ancestor_entry) => {
				tracing::info!(
					target: LOG_TARGET,
					?block_hash,
					block_number = block_header.number,
					revert_target = revert_number,
					revert_hash = ?ancestor_entry.block_hash,
					"A block has signaled that its ancestor be reverted due to a bad parachain block.",
				);

				ancestor_entry
			}
		};

		ancestor_entry.viability.explicitly_reverted = true;
		backend.write_block_entry(ancestor_entry.clone());

		propagate_viability_update(backend, ancestor_entry)?;
	}

	Ok(())
}
