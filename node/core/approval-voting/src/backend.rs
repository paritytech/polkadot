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

//! An abstraction over storage used by the chain selection subsystem.
//!
//! This provides both a [`Backend`] trait and an [`OverlayedBackend`]
//! struct which allows in-memory changes to be applied on top of a
//! [`Backend`], maintaining consistency between queries and temporary writes,
//! before any commit to the underlying storage is made.

use polkadot_node_subsystem::SubsystemResult;
use polkadot_primitives::v2::{BlockNumber, CandidateHash, Hash};

use std::collections::HashMap;

use super::{
	approval_db::v1::StoredBlockRange,
	persisted_entries::{BlockEntry, CandidateEntry},
};

#[derive(Debug)]
pub enum BackendWriteOp {
	WriteStoredBlockRange(StoredBlockRange),
	WriteBlocksAtHeight(BlockNumber, Vec<Hash>),
	WriteBlockEntry(BlockEntry),
	WriteCandidateEntry(CandidateEntry),
	DeleteStoredBlockRange,
	DeleteBlocksAtHeight(BlockNumber),
	DeleteBlockEntry(Hash),
	DeleteCandidateEntry(CandidateHash),
}

/// An abstraction over backend storage for the logic of this subsystem.
pub trait Backend {
	/// Load a block entry from the DB.
	fn load_block_entry(&self, hash: &Hash) -> SubsystemResult<Option<BlockEntry>>;
	/// Load a candidate entry from the DB.
	fn load_candidate_entry(
		&self,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateEntry>>;
	/// Load all blocks at a specific height.
	fn load_blocks_at_height(&self, height: &BlockNumber) -> SubsystemResult<Vec<Hash>>;
	/// Load all block from the DB.
	fn load_all_blocks(&self) -> SubsystemResult<Vec<Hash>>;
	/// Load stored block range form the DB.
	fn load_stored_blocks(&self) -> SubsystemResult<Option<StoredBlockRange>>;
	/// Atomically write the list of operations, with later operations taking precedence over prior.
	fn write<I>(&mut self, ops: I) -> SubsystemResult<()>
	where
		I: IntoIterator<Item = BackendWriteOp>;
}

// Status of block range in the `OverlayedBackend`.
#[derive(PartialEq)]
enum BlockRangeStatus {
	// Value has not been modified.
	NotModified,
	// Value has been deleted
	Deleted,
	// Value has been updated.
	Inserted(StoredBlockRange),
}

/// An in-memory overlay over the backend.
///
/// This maintains read-only access to the underlying backend, but can be
/// converted into a set of write operations which will, when written to
/// the underlying backend, give the same view as the state of the overlay.
pub struct OverlayedBackend<'a, B: 'a> {
	inner: &'a B,
	// `Some(None)` means deleted. Missing (`None`) means query inner.
	stored_block_range: BlockRangeStatus,
	// `None` means 'deleted', missing means query inner.
	blocks_at_height: HashMap<BlockNumber, Option<Vec<Hash>>>,
	// `None` means 'deleted', missing means query inner.
	block_entries: HashMap<Hash, Option<BlockEntry>>,
	// `None` means 'deleted', missing means query inner.
	candidate_entries: HashMap<CandidateHash, Option<CandidateEntry>>,
}

impl<'a, B: 'a + Backend> OverlayedBackend<'a, B> {
	pub fn new(backend: &'a B) -> Self {
		OverlayedBackend {
			inner: backend,
			stored_block_range: BlockRangeStatus::NotModified,
			blocks_at_height: HashMap::new(),
			block_entries: HashMap::new(),
			candidate_entries: HashMap::new(),
		}
	}

	pub fn is_empty(&self) -> bool {
		self.block_entries.is_empty() &&
			self.candidate_entries.is_empty() &&
			self.blocks_at_height.is_empty() &&
			self.stored_block_range == BlockRangeStatus::NotModified
	}

	pub fn load_all_blocks(&self) -> SubsystemResult<Vec<Hash>> {
		let mut hashes = Vec::new();
		if let Some(stored_blocks) = self.load_stored_blocks()? {
			for height in stored_blocks.0..stored_blocks.1 {
				hashes.extend(self.load_blocks_at_height(&height)?);
			}
		}

		Ok(hashes)
	}

	pub fn load_stored_blocks(&self) -> SubsystemResult<Option<StoredBlockRange>> {
		match self.stored_block_range {
			BlockRangeStatus::Inserted(ref value) => Ok(Some(value.clone())),
			BlockRangeStatus::Deleted => Ok(None),
			BlockRangeStatus::NotModified => self.inner.load_stored_blocks(),
		}
	}

	pub fn load_blocks_at_height(&self, height: &BlockNumber) -> SubsystemResult<Vec<Hash>> {
		if let Some(val) = self.blocks_at_height.get(&height) {
			return Ok(val.clone().unwrap_or_default())
		}

		self.inner.load_blocks_at_height(height)
	}

	pub fn load_block_entry(&self, hash: &Hash) -> SubsystemResult<Option<BlockEntry>> {
		if let Some(val) = self.block_entries.get(&hash) {
			return Ok(val.clone())
		}

		self.inner.load_block_entry(hash)
	}

	pub fn load_candidate_entry(
		&self,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateEntry>> {
		if let Some(val) = self.candidate_entries.get(&candidate_hash) {
			return Ok(val.clone())
		}

		self.inner.load_candidate_entry(candidate_hash)
	}

	pub fn write_stored_block_range(&mut self, range: StoredBlockRange) {
		self.stored_block_range = BlockRangeStatus::Inserted(range);
	}

	pub fn delete_stored_block_range(&mut self) {
		self.stored_block_range = BlockRangeStatus::Deleted;
	}

	pub fn write_blocks_at_height(&mut self, height: BlockNumber, blocks: Vec<Hash>) {
		self.blocks_at_height.insert(height, Some(blocks));
	}

	pub fn delete_blocks_at_height(&mut self, height: BlockNumber) {
		self.blocks_at_height.insert(height, None);
	}

	pub fn write_block_entry(&mut self, entry: BlockEntry) {
		self.block_entries.insert(entry.block_hash(), Some(entry));
	}

	pub fn delete_block_entry(&mut self, hash: &Hash) {
		self.block_entries.insert(*hash, None);
	}

	pub fn write_candidate_entry(&mut self, entry: CandidateEntry) {
		self.candidate_entries.insert(entry.candidate_receipt().hash(), Some(entry));
	}

	pub fn delete_candidate_entry(&mut self, hash: &CandidateHash) {
		self.candidate_entries.insert(*hash, None);
	}

	/// Transform this backend into a set of write-ops to be written to the
	/// inner backend.
	pub fn into_write_ops(self) -> impl Iterator<Item = BackendWriteOp> {
		let blocks_at_height_ops = self.blocks_at_height.into_iter().map(|(h, v)| match v {
			Some(v) => BackendWriteOp::WriteBlocksAtHeight(h, v),
			None => BackendWriteOp::DeleteBlocksAtHeight(h),
		});

		let block_entry_ops = self.block_entries.into_iter().map(|(h, v)| match v {
			Some(v) => BackendWriteOp::WriteBlockEntry(v),
			None => BackendWriteOp::DeleteBlockEntry(h),
		});

		let candidate_entry_ops = self.candidate_entries.into_iter().map(|(h, v)| match v {
			Some(v) => BackendWriteOp::WriteCandidateEntry(v),
			None => BackendWriteOp::DeleteCandidateEntry(h),
		});

		let stored_block_range_ops = match self.stored_block_range {
			BlockRangeStatus::Inserted(val) => Some(BackendWriteOp::WriteStoredBlockRange(val)),
			BlockRangeStatus::Deleted => Some(BackendWriteOp::DeleteStoredBlockRange),
			BlockRangeStatus::NotModified => None,
		};

		stored_block_range_ops
			.into_iter()
			.chain(blocks_at_height_ops)
			.chain(block_entry_ops)
			.chain(candidate_entry_ops)
	}
}
