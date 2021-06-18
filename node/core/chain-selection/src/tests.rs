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

//! Tests for the subsystem.
//!
//! These primarily revolve around having a backend which is shared between
//! both the test code and the tested subsystem, and which also gives the
//! test code the ability to wait for write operations to occur.

use super::*;
use std::collections::{HashMap, BTreeMap};
use std::sync::Arc;

use parking_lot::Mutex;

#[derive(Default)]
struct TestBackendInner {
	leaves: LeafEntrySet,
	block_entries: HashMap<Hash, BlockEntry>,
	blocks_by_number: BTreeMap<BlockNumber, Vec<Hash>>,
	stagnant_at: BTreeMap<Timestamp, Vec<Hash>>,
}

struct TestBackend {
	inner: Arc<Mutex<TestBackendInner>>,
}

impl Default for TestBackend {
	fn default() -> Self {
		TestBackend {
			inner: Default::default(),
		}
	}
}

impl Backend for TestBackend {
	fn load_block_entry(&self, hash: &Hash) -> Result<Option<BlockEntry>, Error> {
		Ok(self.inner.lock().block_entries.get(hash).map(|e| e.clone()))
	}
	fn load_leaves(&self) -> Result<LeafEntrySet, Error> {
		Ok(self.inner.lock().leaves.clone())
	}
	fn load_stagnant_at(&self, timestamp: Timestamp) -> Result<Vec<Hash>, Error> {
		Ok(self.inner.lock().stagnant_at.get(&timestamp).map_or(Vec::new(), |s| s.clone()))
	}
	fn load_stagnant_at_up_to(&self, up_to: Timestamp)
		-> Result<Vec<(Timestamp, Vec<Hash>)>, Error>
	{
		Ok(self.inner.lock().stagnant_at.range(..=up_to).map(|(t, v)| (*t, v.clone())).collect())
	}
	fn load_first_block_number(&self) -> Result<Option<BlockNumber>, Error> {
		Ok(self.inner.lock().blocks_by_number.range(..).map(|(k, _)| *k).next())
	}
	fn load_blocks_by_number(&self, number: BlockNumber) -> Result<Vec<Hash>, Error> {
		Ok(self.inner.lock().blocks_by_number.get(&number).map_or(Vec::new(), |v| v.clone()))
	}

	fn write<I>(&mut self, ops: I) -> Result<(), Error>
		where I: IntoIterator<Item = BackendWriteOp>
	{
		let mut inner = self.inner.lock();
		for op in ops {
			match op {
				BackendWriteOp::WriteBlockEntry(entry) => {
					inner.block_entries.insert(entry.block_hash, entry);
				}
				BackendWriteOp::WriteBlocksByNumber(number, hashes) => {
					inner.blocks_by_number.insert(number, hashes);
				}
				BackendWriteOp::WriteViableLeaves(leaves) => {
					inner.leaves = leaves;
				}
				BackendWriteOp::WriteStagnantAt(time, hashes) => {
					inner.stagnant_at.insert(time, hashes);
				}
				BackendWriteOp::DeleteBlocksByNumber(number) => {
					inner.blocks_by_number.remove(&number);
				}
				BackendWriteOp::DeleteBlockEntry(hash) => {
					inner.block_entries.remove(&hash);
				}
				BackendWriteOp::DeleteStagnantAt(time) => {
					inner.stagnant_at.remove(&time);
				}
			}
		}

		Ok(())
	}
}

// TODO [now]: importing a block without reversion
// TODO [now]: importing a block with reversion

// TODO [now]: finalize a viable block
// TODO [now]: finalize an unviable block with viable descendants
// TODO [now]: finalize an unviable block with unviable descendants down the line

// TODO [now]: mark blocks as stagnant.
// TODO [now]: approve stagnant block with unviable descendant.

// TODO [now]; test find best leaf containing with no leaves.
// TODO [now]: find best leaf containing when required is finalized
// TODO [now]: find best leaf containing when required is unfinalized.
// TODO [now]: find best leaf containing when required is ancestor of many leaves.

// TODO [now]: test assumption that each active leaf update gives 1 DB write.
// TODO [now]: test assumption that each approved block gives 1 DB write.
// TODO [now]: test assumption that each finalized block gives 1 DB write.
