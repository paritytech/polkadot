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

use futures::channel::oneshot;
use parking_lot::Mutex;
use sp_core::testing::TaskExecutor;
use assert_matches::assert_matches;

use polkadot_primitives::v1::{BlakeTwo256, HashT};
use polkadot_subsystem::messages::AllMessages;
use polkadot_node_subsystem_test_helpers as test_helpers;

#[derive(Default)]
struct TestBackendInner {
	leaves: LeafEntrySet,
	block_entries: HashMap<Hash, BlockEntry>,
	blocks_by_number: BTreeMap<BlockNumber, Vec<Hash>>,
	stagnant_at: BTreeMap<Timestamp, Vec<Hash>>,
	// earlier wakers at the back.
	write_wakers: Vec<oneshot::Sender<()>>,
}

#[derive(Clone)]
struct TestBackend {
	inner: Arc<Mutex<TestBackendInner>>,
}

impl TestBackend {
	// Yields a receiver which will be woken up on some future write
	// to the backend along with its position (starting at 0) in the
	// queue.
	//
	// Our tests assume that there is only one task calling this function
	// and the index is useful to get a waker that will trigger after
	// some known amount of writes to the backend that happen internally
	// inside the subsystem.
	//
	// It's important to call this function at points where no writes
	// are pending to the backend. This requires knowing some details
	// about the internals of the subsystem, so the abstraction leaks
	// somewhat, but this is acceptable enough.
	fn next_write(&self) -> (usize, oneshot::Receiver<()>) {
		let (tx, rx) = oneshot::channel();

		let mut inner = self.inner.lock();
		let pos = inner.write_wakers.len();
		inner.write_wakers.insert(0, tx);

		(pos, rx)
	}
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

		if let Some(waker) = inner.write_wakers.pop() {
			let _ = waker.send(());
		}
		Ok(())
	}
}

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<ChainSelectionMessage>;

fn test_harness<T: Future<Output=VirtualOverseer>>(
	test: impl FnOnce(TestBackend, VirtualOverseer) -> T
) {
	let pool = TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool);

	let backend = TestBackend::default();
	let subsystem = crate::run(context, backend.clone());

	let test_fut = test(backend, virtual_overseer);
	let test_and_conclude = async move {
		let mut virtual_overseer = test_fut.await;
		virtual_overseer.send(OverseerSignal::Conclude.into()).await;

		// Ensure no messages are pending when the subsystem shuts down.
		assert!(virtual_overseer.try_recv().await.is_none());
	};
	futures::executor::block_on(futures::future::join(subsystem, test_and_conclude));
}

// Answer requests from the subsystem about the finalized block.
async fn answer_finalized_block_info(
	overseer: &mut VirtualOverseer,
	finalized_hash: Hash,
	finalized_number: BlockNumber,
) {
	assert_matches!(
		overseer.recv().await,
		AllMessages::ChainApi(ChainApiMessage::FinalizedBlockNumber(tx)) => {
			let _ = tx.send(Ok(finalized_number));
		}
	);

	assert_matches!(
		overseer.recv().await,
		AllMessages::ChainApi(ChainApiMessage::FinalizedBlockHash(n, tx)) => {
			assert_eq!(n, finalized_number);
			let _ = tx.send(Ok(Some(finalized_hash)));
		}
	);
}

fn child_header(parent_hash: Hash, parent_number: BlockNumber) -> Header {
	child_header_with_salt(parent_hash, parent_number, &[])
}

fn child_header_with_salt(
	parent_hash: Hash,
	parent_number: BlockNumber,
	salt: &[u8], // so siblings can have different hashes.
) -> Header {
	Header {
		parent_hash,
		number: parent_number + 1,
		state_root: BlakeTwo256::hash(salt),
		extrinsics_root: Default::default(),
		digest: Default::default()
	}
}

#[test]
fn no_op_subsystem_run() {
	test_harness(|_, virtual_overseer| async move { virtual_overseer });
}

#[test]
fn import_direct_child_of_finalized_on_empty() {
	test_harness(|backend, mut virtual_overseer| async move {


		virtual_overseer
	})
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

// TODO [now]: leaf tiebreakers are based on height.

// TODO [now]: test assumption that each active leaf update gives 1 DB write.
// TODO [now]: test assumption that each approved block gives 1 DB write.
// TODO [now]: test assumption that each finalized block gives 1 DB write.
