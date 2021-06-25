use kvdb::{DBTransaction, KeyValueDB};
use polkadot_node_subsystem::{SubsystemResult};
use polkadot_primitives::v1::{BlockNumber, CandidateHash, Hash};
use parity_scale_codec::{Encode};

use std::collections::HashMap;
use std::sync::Arc;

use super::approval_db;
use super::approval_db::v1::{
	blocks_at_height_key, block_entry_key, candidate_entry_key, STORED_BLOCKS_KEY, Config,
};
use super::ops::StoredBlockRange;
use super::persisted_entries::{BlockEntry, CandidateEntry};

#[derive(Debug)]
pub(super) enum BackendWriteOp {
	WriteStoredBlockRange(StoredBlockRange),
	WriteBlocksAtHeight(BlockNumber, Vec<Hash>),
	WriteBlockEntry(BlockEntry),
	WriteCandidateEntry(CandidateEntry),
	DeleteBlocksAtHeight(BlockNumber),
	DeleteBlockEntry(Hash),
	DeleteCandidateEntry(CandidateHash),
}

pub(super) struct DbBackend {
	inner: Arc<dyn KeyValueDB>,
	pub(super) config: Config,
}

impl DbBackend {
	/// Create a new [`DbBackend`] with the supplied key-value store and
	/// config.
	pub fn new(db: Arc<dyn KeyValueDB>, config: Config) -> Self {
		DbBackend {
			inner: db,
			config,
		}
	}
}

impl Backend for DbBackend {
	fn load_block_entry(
		&self,
		block_hash: &Hash,
	) -> SubsystemResult<Option<BlockEntry>> {
		super::ops::load_block_entry(&*self.inner, &self.config, block_hash)
			.map(|e| e.map(Into::into))
	}

	fn load_candidate_entry(
		&self,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateEntry>> {
		super::ops::load_candidate_entry(&*self.inner, &self.config, candidate_hash)
			.map(|e| e.map(Into::into))
	}

	fn load_blocks_at_height(
		&self,
		block_height: &BlockNumber
	) -> SubsystemResult<Vec<Hash>> {
		super::ops::load_blocks_at_height(&*self.inner, &self.config, block_height)
	}

	fn load_all_blocks(&self) -> SubsystemResult<Vec<Hash>> {
		super::ops::load_all_blocks(&*self.inner, &self.config)
	}

	fn load_stored_blocks(&self) -> SubsystemResult<Option<StoredBlockRange>> {
		super::ops::load_stored_blocks(&*self.inner, &self.config)
	}

	/// Atomically write the list of operations, with later operations taking precedence over prior.
	fn write<I>(&mut self, ops: I) -> SubsystemResult<()>
		where I: IntoIterator<Item = BackendWriteOp>
	{
		let mut tx = DBTransaction::new();
		for op in ops {
			match op {
				BackendWriteOp::WriteStoredBlockRange(stored_block_range) => {
					tx.put_vec(
						self.config.col_data,
						&STORED_BLOCKS_KEY,
						stored_block_range.encode(),
					);
				}
				BackendWriteOp::WriteBlocksAtHeight(h, blocks) => {
					tx.put_vec(
						self.config.col_data,
						&blocks_at_height_key(h),
						blocks.encode(),
					);
				}
				BackendWriteOp::DeleteBlocksAtHeight(h) => {
					tx.delete(
						self.config.col_data,
						&blocks_at_height_key(h),
					);
				}
				BackendWriteOp::WriteBlockEntry(block_entry) => {
					let block_entry: approval_db::v1::BlockEntry = block_entry.into();
					tx.put_vec(
						self.config.col_data,
						&block_entry_key(&block_entry.block_hash),
						block_entry.encode(),
					);
				}
				BackendWriteOp::DeleteBlockEntry(hash) => {
					tx.delete(
						self.config.col_data,
						&block_entry_key(&hash),
					);
				}
				BackendWriteOp::WriteCandidateEntry(candidate_entry) => {
					let candidate_entry: approval_db::v1::CandidateEntry = candidate_entry.into();
					tx.put_vec(
						self.config.col_data,
						&candidate_entry_key(&candidate_entry.candidate.hash()),
						candidate_entry.encode(),
					);
				}
				BackendWriteOp::DeleteCandidateEntry(candidate_hash) => {
					tx.delete(
						self.config.col_data,
						&candidate_entry_key(&candidate_hash),
					);
				}
			}
		}

		let _ = self.inner.write(tx)?;
		Ok(())
	}
}

/// An abstraction over backend storage for the logic of this subsystem.
pub(super) trait Backend {
	/// Load a block entry from the DB.
	fn load_block_entry(&self, hash: &Hash) -> SubsystemResult<Option<BlockEntry>>;
	/// Load a candidate entry from the DB.
	fn load_candidate_entry(&self, candidate_hash: &CandidateHash) -> SubsystemResult<Option<CandidateEntry>>;
	/// Load all blocks at a specific height.
	fn load_blocks_at_height(&self, height: &BlockNumber) -> SubsystemResult<Vec<Hash>>;
	/// Load all block from the DB.
	fn load_all_blocks(&self) -> SubsystemResult<Vec<Hash>>;
	/// Load stored block range form the DB.
	fn load_stored_blocks(&self) -> SubsystemResult<Option<StoredBlockRange>>;
	/// Atomically write the list of operations, with later operations taking precedence over prior.
	fn write<I>(&mut self, ops: I) -> SubsystemResult<()>
		where I: IntoIterator<Item = BackendWriteOp>;
}

/// An in-memory overlay over the backend.
///
/// This maintains read-only access to the underlying backend, but can be
/// converted into a set of write operations which will, when written to
/// the underlying backend, give the same view as the state of the overlay.
pub(super) struct OverlayedBackend<'a, B: 'a> {
	inner: &'a B,

	// `None` means unchanged
	stored_block_range: Option<StoredBlockRange>,
	// `None` means 'deleted', missing means query inner.
	blocks_at_height: HashMap<BlockNumber, Option<Vec<Hash>>>,
	// `None` means 'deleted', missing means query inner.
	block_entries: HashMap<Hash, Option<BlockEntry>>,
	// `None` means 'deleted', missing means query inner.
	candidate_entries: HashMap<CandidateHash, Option<CandidateEntry>>,
}

impl<'a, B: 'a + Backend> OverlayedBackend<'a, B> {
	pub(super) fn new(backend: &'a B) -> Self {
		OverlayedBackend {
			inner: backend,
			stored_block_range: None,
			blocks_at_height: HashMap::new(),
			block_entries: HashMap::new(),
			candidate_entries: HashMap::new(),
		}
	}

	pub(super) fn is_empty(&self) -> bool {
		self.block_entries.is_empty() &&
			self.candidate_entries.is_empty() &&
			self.blocks_at_height.is_empty() &&
			self.stored_block_range.is_none()
	}

	pub(super) fn load_all_blocks(&self) -> SubsystemResult<Vec<Hash>> {
		let mut hashes = Vec::new();
		if let Some(stored_blocks) = self.load_stored_blocks()? {
			for height in stored_blocks.0..stored_blocks.1 {
				hashes.extend(self.load_blocks_at_height(&height)?);
			}
		}

		Ok(hashes)
	}

	pub(super) fn load_stored_blocks(&self) -> SubsystemResult<Option<StoredBlockRange>> {
		if let Some(val) = self.stored_block_range.clone() {
			return Ok(Some(val))
		}

		self.inner.load_stored_blocks()
	}

	pub(super) fn load_blocks_at_height(&self, height: &BlockNumber) -> SubsystemResult<Vec<Hash>> {
		if let Some(val) = self.blocks_at_height.get(&height) {
			return Ok(val.clone().unwrap_or_default())
		}

		self.inner.load_blocks_at_height(height)
	}

	pub(super) fn load_block_entry(&self, hash: &Hash) -> SubsystemResult<Option<BlockEntry>> {
		if let Some(val) = self.block_entries.get(&hash) {
			return Ok(val.clone())
		}

		self.inner.load_block_entry(hash)
	}

	pub(super) fn load_candidate_entry(&self, candidate_hash: &CandidateHash) -> SubsystemResult<Option<CandidateEntry>> {
		if let Some(val) = self.candidate_entries.get(&candidate_hash) {
			return Ok(val.clone())
		}

		self.inner.load_candidate_entry(candidate_hash)
	}

	// The assumption is that stored block range is only None on initialization.
	// Therefore, there is no need to delete_stored_block_range.
	pub(super) fn write_stored_block_range(&mut self, range: StoredBlockRange) {
		self.stored_block_range = Some(range);
	}

	pub(super) fn write_blocks_at_height(&mut self, height: BlockNumber, blocks: Vec<Hash>) {
		self.blocks_at_height.insert(height, Some(blocks));
	}

	pub(super) fn delete_blocks_at_height(&mut self, height: BlockNumber) {
		self.blocks_at_height.insert(height, None);
	}

	pub(super) fn write_block_entry(&mut self, entry: BlockEntry) {
		self.block_entries.insert(entry.block_hash(), Some(entry));
	}

	pub(super) fn delete_block_entry(&mut self, hash: &Hash) {
		self.block_entries.insert(*hash, None);
	}

	pub(super) fn write_candidate_entry(&mut self, entry: CandidateEntry) {
		self.candidate_entries.insert(entry.candidate_receipt().hash(), Some(entry));
	}

	pub(super) fn delete_candidate_entry(&mut self, hash: &CandidateHash) {
		self.candidate_entries.insert(*hash, None);
	}

	/// Transform this backend into a set of write-ops to be written to the
	/// inner backend.
	pub(super) fn into_write_ops(self) -> impl Iterator<Item = BackendWriteOp> {
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

		self.stored_block_range
			.map(|v| BackendWriteOp::WriteStoredBlockRange(v))
			.into_iter()
			.chain(blocks_at_height_ops)
			.chain(block_entry_ops)
			.chain(candidate_entry_ops)
	}
}
