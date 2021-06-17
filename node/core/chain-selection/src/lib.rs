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

use polkadot_primitives::v1::{BlockNumber, Hash, Header};
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
		match *self {
			Approval::Stagnant => true,
			_ => false,
		}
	}
}

#[derive(Debug, Clone)]
struct ViabilityCriteria {
	// Whether this block has been explicitly reverted by one of its descendants.
	explicitly_reverted: bool,
	// `None` means approved. `Some` means unapproved.
	approval: Approval,
	earliest_non_viable_ancestor: Option<Hash>,
}

impl ViabilityCriteria {
	fn is_viable(&self) -> bool {
		self.is_parent_viable()
			&& !self.explicitly_reverted
			&& !self.approval.is_stagnant()
	}

	fn is_parent_viable(&self) -> bool {
		self.earliest_non_viable_ancestor.is_none()
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
	fn non_viable_ancestor_for_child(&self) -> Option<Hash> {
		if self.viability.is_viable() {
			None
		} else {
			self.viability.earliest_non_viable_ancestor.or(Some(self.block_hash))
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

enum BackendWriteOp {
	WriteBlockEntry(Hash, BlockEntry),
	WriteBlocksByNumber(BlockNumber, Vec<Hash>),
	WriteViableLeaves(LeafEntrySet),
	WriteStagnantAt(Timestamp, Vec<Hash>),
	DeleteBlocksByNumber(BlockNumber),
	DeleteBlockEntry(Hash),
	DeleteStagnantAt(Timestamp),
}

// An abstraction over backend for the logic of this subsystem.
trait Backend {
	/// Load a block entry from the DB.
	fn load_block_entry(&self, hash: &Hash) -> Result<Option<BlockEntry>, Error>;
	/// Load the active-leaves set.
	fn load_leaves(&self) -> Result<LeafEntrySet, Error>;
	/// Load the stagnant list at the given timestamp.
	fn load_stagnant_at(&self, timestamp: Timestamp) -> Result<Vec<Hash>, Error>;
	/// Load all stagnant lists up to and including the given unix timestamp.
	fn load_stagnant_at_up_to(&self, up_to: Timestamp)
		-> Result<Vec<(Timestamp, Vec<Hash>)>, Error>;
	/// Load the earliest kept block number.
	fn load_first_block_number(&self) -> Result<Option<BlockNumber>, Error>;
	/// Load blocks by number.
	fn load_blocks_by_number(&self, number: BlockNumber) -> Result<Vec<Hash>, Error>;

	/// Atomically write the list of operations, with later operations taking precedence over prior.
	fn write(&mut self, ops: Vec<BackendWriteOp>) -> Result<(), Error>;
}

// An in-memory overlay over the backend.
struct OverlayedBackend<'a, B: 'a> {
	inner: &'a B,

	// `None` means 'deleted', missing means query inner.
	block_entries: HashMap<Hash, Option<BlockEntry>>,
	// `None` means 'deleted', missing means query inner.
	blocks_by_number: HashMap<BlockNumber, Option<Vec<Hash>>>,
	// 'None' means 'deleted', missing means query inner.
	stagnant_at: HashMap<Timestamp, Option<Vec<Hash>>>,
	// 'None' means query inner.
	leaves: Option<LeafEntrySet>,
}

impl<'a, B: 'a + Backend> OverlayedBackend<'a, B> {
	fn new(backend: &'a B) -> Self {
		OverlayedBackend {
			inner: backend,
			block_entries: HashMap::new(),
			blocks_by_number: HashMap::new(),
			stagnant_at: HashMap::new(),
			leaves: None,
		}
	}

	fn load_block_entry(&self, hash: &Hash) -> Result<Option<BlockEntry>, Error> {
		if let Some(val) = self.block_entries.get(&hash) {
			return Ok(val.clone())
		}

		self.inner.load_block_entry(hash)
	}

	fn load_blocks_by_number(&self, number: BlockNumber) -> Result<Vec<Hash>, Error> {
		if let Some(val) = self.blocks_by_number.get(&number) {
			return Ok(val.as_ref().map_or(Vec::new(), Clone::clone));
		}

		self.inner.load_blocks_by_number(number)
	}

	fn load_leaves(&self) -> Result<LeafEntrySet, Error> {
		if let Some(ref set) = self.leaves {
			return Ok(set.clone())
		}

		self.inner.load_leaves()
	}

	fn load_stagnant_at(&self, timestamp: Timestamp) -> Result<Vec<Hash>, Error> {
		if let Some(val) = self.stagnant_at.get(&timestamp) {
			return Ok(val.as_ref().map_or(Vec::new(), Clone::clone));
		}

		self.inner.load_stagnant_at(timestamp)
	}

	fn write_block_entry(&mut self, hash: Hash, entry: BlockEntry) {
		self.block_entries.insert(hash, Some(entry));
	}

	fn delete_block_entry(&mut self, hash: &Hash) {
		self.block_entries.remove(hash);
	}

	fn write_blocks_by_number(&mut self, number: BlockNumber, blocks: Vec<Hash>) {
		self.blocks_by_number.insert(number, Some(blocks));
	}

	fn delete_blocks_by_number(&mut self, number: BlockNumber) {
		self.blocks_by_number.insert(number, None);
	}

	fn write_leaves(&mut self, leaves: LeafEntrySet) {
		self.leaves = Some(leaves);
	}

	fn write_stagnant_at(&mut self, timestamp: Timestamp, hashes: Vec<Hash>) {
		self.stagnant_at.insert(timestamp, Some(hashes));
	}

	fn delete_stagnant_at(&mut self, timestamp: Timestamp) {
		self.stagnant_at.insert(timestamp, None);
	}

	fn into_write_ops(self) -> impl Iterator<Item = BackendWriteOp> {
		let block_entry_ops = self.block_entries.into_iter().map(|(h, v)| match v {
			Some(v) => BackendWriteOp::WriteBlockEntry(h, v),
			None => BackendWriteOp::DeleteBlockEntry(h),
		});

		let blocks_by_number_ops = self.blocks_by_number.into_iter().map(|(n, v)| match v {
			Some(v) => BackendWriteOp::WriteBlocksByNumber(n, v),
			None => BackendWriteOp::DeleteBlocksByNumber(n),
		});

		let leaf_ops = self.leaves.into_iter().map(BackendWriteOp::WriteViableLeaves);

		let stagnant_at_ops = self.stagnant_at.into_iter().map(|(n, v)| match v {
			Some(v) => BackendWriteOp::WriteStagnantAt(n, v),
			None => BackendWriteOp::DeleteStagnantAt(n),
		});

		block_entry_ops
			.chain(blocks_by_number_ops)
			.chain(leaf_ops)
			.chain(stagnant_at_ops)
	}
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
	import_block_ignoring_reversions(backend, block_hash, block_header, weight)?;

	// TODO [now]: apply reversions.

	Ok(())
}

fn import_block_ignoring_reversions(
	backend: &mut OverlayedBackend<impl Backend>,
	block_hash: Hash,
	block_header: Header,
	weight: Weight,
) -> Result<(), Error> {
	let parent_hash = block_header.parent_hash;

	let mut leaves = backend.load_leaves()?;
	let parent_entry = backend.load_block_entry(&parent_hash)?;

	let inherited_viability = parent_entry.as_ref()
		.and_then(|parent| parent.non_viable_ancestor_for_child());

	// 1. Add the block to the DB assuming it's not reverted.
	backend.write_block_entry(
		block_hash,
		BlockEntry {
			block_hash,
			parent_hash: parent_hash,
			children: Vec::new(),
			viability: ViabilityCriteria {
				earliest_non_viable_ancestor: inherited_viability,
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
		backend.write_block_entry(parent_hash, parent_entry);
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
