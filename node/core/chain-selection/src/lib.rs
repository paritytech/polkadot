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

use crate::backend::{Backend, OverlayedBackend, BackendWriteOp};

mod backend;
mod tree;

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

	fn into_hashes_descending(self) -> impl IntoIterator<Item = Hash> {
		self.inner.into_iter().map(|e| e.block_hash)
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
				for leaf in update.activated {
					let write_ops = handle_active_leaf(
						ctx,
						&*backend,
						leaf.hash,
					).await?;

					backend.write(write_ops)?;
				}
			}
			FromOverseer::Signal(OverseerSignal::BlockFinalized(h, n)) => {
				handle_finalized_block(backend, h, n)?
			}
			FromOverseer::Communication { msg } => match msg {
				ChainSelectionMessage::Approved(hash) => {
					handle_approved_block(backend, hash)?
				}
				ChainSelectionMessage::Leaves(tx) => {
					let leaves = load_leaves(ctx, &*backend).await?;
					let _ = tx.send(leaves);
				}
				ChainSelectionMessage::BestLeafContaining(required, tx) => {
					let best_containing = crate::backend::find_best_leaf_containing(
						&*backend,
						required,
					)?;

					let _ = tx.send(best_containing);
				}
			}
		};
	}
}

async fn fetch_finalized(
	ctx: &mut impl SubsystemContext,
) -> Result<(Hash, BlockNumber), Error> {
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
		None => fetch_finalized(ctx).await?.1,
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
		crate::tree::import_block(&mut overlay, hash, header, weight)?;
	}

	Ok(overlay.into_write_ops().collect())
}

// Handle a finalized block event.
fn handle_finalized_block(
	backend: &mut impl Backend,
	finalized_hash: Hash,
	finalized_number: BlockNumber,
) -> Result<(), Error> {
	let ops = crate::tree::finalize_block(
		&*backend,
		finalized_hash,
		finalized_number,
	)?.into_write_ops();

	backend.write(ops)
}

// Handle an approved block event.
fn handle_approved_block(
	backend: &mut impl Backend,
	approved_block: Hash,
) -> Result<(), Error> {
	let ops = {
		let mut overlay = OverlayedBackend::new(&*backend);

		crate::tree::approve_block(
			&mut overlay,
			approved_block,
		)?;

		overlay.into_write_ops()
	};

	backend.write(ops)
}

// Load the leaves from the backend. If there are no leaves, then return
// the finalized block.
async fn load_leaves(
	ctx: &mut impl SubsystemContext,
	backend: &impl Backend,
) -> Result<Vec<Hash>, Error> {
	let leaves: Vec<_> = backend.load_leaves()?
		.into_hashes_descending()
		.into_iter()
		.collect();

	if leaves.is_empty() {
		let finalized_hash = fetch_finalized(ctx).await?.0;
		Ok(vec![finalized_hash])
	} else {
		Ok(leaves)
	}
}
