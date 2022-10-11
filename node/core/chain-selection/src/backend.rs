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

//! An abstraction over storage used by the chain selection subsystem.
//!
//! This provides both a [`Backend`] trait and an [`OverlayedBackend`]
//! struct which allows in-memory changes to be applied on top of a
//! [`Backend`], maintaining consistency between queries and temporary writes,
//! before any commit to the underlying storage is made.

use polkadot_primitives::v2::{BlockNumber, Hash};

use std::collections::HashMap;

use crate::{BlockEntry, Error, LeafEntrySet, Timestamp};

pub(super) enum BackendWriteOp {
	WriteBlockEntry(BlockEntry),
	WriteBlocksByNumber(BlockNumber, Vec<Hash>),
	WriteViableLeaves(LeafEntrySet),
	WriteStagnantAt(Timestamp, Vec<Hash>),
	DeleteBlocksByNumber(BlockNumber),
	DeleteBlockEntry(Hash),
	DeleteStagnantAt(Timestamp),
}

/// An abstraction over backend storage for the logic of this subsystem.
pub(super) trait Backend {
	/// Load a block entry from the DB.
	fn load_block_entry(&self, hash: &Hash) -> Result<Option<BlockEntry>, Error>;
	/// Load the active-leaves set.
	fn load_leaves(&self) -> Result<LeafEntrySet, Error>;
	/// Load the stagnant list at the given timestamp.
	fn load_stagnant_at(&self, timestamp: Timestamp) -> Result<Vec<Hash>, Error>;
	/// Load all stagnant lists up to and including the given Unix timestamp
	/// in ascending order. Stop fetching stagnant entries upon reaching `max_elements`.
	fn load_stagnant_at_up_to(
		&self,
		up_to: Timestamp,
		max_elements: usize,
	) -> Result<Vec<(Timestamp, Vec<Hash>)>, Error>;
	/// Load the earliest kept block number.
	fn load_first_block_number(&self) -> Result<Option<BlockNumber>, Error>;
	/// Load blocks by number.
	fn load_blocks_by_number(&self, number: BlockNumber) -> Result<Vec<Hash>, Error>;

	/// Atomically write the list of operations, with later operations taking precedence over prior.
	fn write<I>(&mut self, ops: I) -> Result<(), Error>
	where
		I: IntoIterator<Item = BackendWriteOp>;
}

/// An in-memory overlay over the backend.
///
/// This maintains read-only access to the underlying backend, but can be
/// converted into a set of write operations which will, when written to
/// the underlying backend, give the same view as the state of the overlay.
pub(super) struct OverlayedBackend<'a, B: 'a> {
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
	pub(super) fn new(backend: &'a B) -> Self {
		OverlayedBackend {
			inner: backend,
			block_entries: HashMap::new(),
			blocks_by_number: HashMap::new(),
			stagnant_at: HashMap::new(),
			leaves: None,
		}
	}

	pub(super) fn load_block_entry(&self, hash: &Hash) -> Result<Option<BlockEntry>, Error> {
		if let Some(val) = self.block_entries.get(&hash) {
			return Ok(val.clone())
		}

		self.inner.load_block_entry(hash)
	}

	pub(super) fn load_blocks_by_number(&self, number: BlockNumber) -> Result<Vec<Hash>, Error> {
		if let Some(val) = self.blocks_by_number.get(&number) {
			return Ok(val.as_ref().map_or(Vec::new(), Clone::clone))
		}

		self.inner.load_blocks_by_number(number)
	}

	pub(super) fn load_leaves(&self) -> Result<LeafEntrySet, Error> {
		if let Some(ref set) = self.leaves {
			return Ok(set.clone())
		}

		self.inner.load_leaves()
	}

	pub(super) fn load_stagnant_at(&self, timestamp: Timestamp) -> Result<Vec<Hash>, Error> {
		if let Some(val) = self.stagnant_at.get(&timestamp) {
			return Ok(val.as_ref().map_or(Vec::new(), Clone::clone))
		}

		self.inner.load_stagnant_at(timestamp)
	}

	pub(super) fn write_block_entry(&mut self, entry: BlockEntry) {
		self.block_entries.insert(entry.block_hash, Some(entry));
	}

	pub(super) fn delete_block_entry(&mut self, hash: &Hash) {
		self.block_entries.insert(*hash, None);
	}

	pub(super) fn write_blocks_by_number(&mut self, number: BlockNumber, blocks: Vec<Hash>) {
		if blocks.is_empty() {
			self.blocks_by_number.insert(number, None);
		} else {
			self.blocks_by_number.insert(number, Some(blocks));
		}
	}

	pub(super) fn delete_blocks_by_number(&mut self, number: BlockNumber) {
		self.blocks_by_number.insert(number, None);
	}

	pub(super) fn write_leaves(&mut self, leaves: LeafEntrySet) {
		self.leaves = Some(leaves);
	}

	pub(super) fn write_stagnant_at(&mut self, timestamp: Timestamp, hashes: Vec<Hash>) {
		self.stagnant_at.insert(timestamp, Some(hashes));
	}

	pub(super) fn delete_stagnant_at(&mut self, timestamp: Timestamp) {
		self.stagnant_at.insert(timestamp, None);
	}

	/// Transform this backend into a set of write-ops to be written to the
	/// inner backend.
	pub(super) fn into_write_ops(self) -> impl Iterator<Item = BackendWriteOp> {
		let block_entry_ops = self.block_entries.into_iter().map(|(h, v)| match v {
			Some(v) => BackendWriteOp::WriteBlockEntry(v),
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

/// Attempt to find the given ancestor in the chain with given head.
///
/// If the ancestor is the most recently finalized block, and the `head` is
/// a known unfinalized block, this will return `true`.
///
/// If the ancestor is an unfinalized block and `head` is known, this will
/// return true if `ancestor` is in `head`'s chain.
///
/// If the ancestor is an older finalized block, this will return `false`.
fn contains_ancestor(backend: &impl Backend, head: Hash, ancestor: Hash) -> Result<bool, Error> {
	let mut current_hash = head;
	loop {
		if current_hash == ancestor {
			return Ok(true)
		}
		match backend.load_block_entry(&current_hash)? {
			Some(e) => current_hash = e.parent_hash,
			None => break,
		}
	}

	Ok(false)
}

/// This returns the best unfinalized leaf containing the required block.
///
/// If the required block is finalized but not the most recent finalized block,
/// this will return `None`.
///
/// If the required block is unfinalized but not an ancestor of any viable leaf,
/// this will return `None`.
//
// Note: this is O(N^2) in the depth of `required` and the number of leaves.
// We expect the number of unfinalized blocks to be small, as in, to not exceed
// single digits in practice, and exceedingly unlikely to surpass 1000.
//
// However, if we need to, we could implement some type of skip-list for
// fast ancestry checks.
pub(super) fn find_best_leaf_containing(
	backend: &impl Backend,
	required: Hash,
) -> Result<Option<Hash>, Error> {
	let leaves = backend.load_leaves()?;
	for leaf in leaves.into_hashes_descending() {
		if contains_ancestor(backend, leaf, required)? {
			return Ok(Some(leaf))
		}
	}

	// If there are no viable leaves containing the ancestor
	Ok(None)
}
