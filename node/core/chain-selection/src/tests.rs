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
use polkadot_subsystem::{jaeger, ActiveLeavesUpdate, ActivatedLeaf, LeafStatus};
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
	fn await_next_write(&self) -> (usize, oneshot::Receiver<()>) {
		let (tx, rx) = oneshot::channel();

		let mut inner = self.inner.lock();
		let pos = inner.write_wakers.len();
		inner.write_wakers.insert(0, tx);

		(pos, rx)
	}

	// return a receiver, expecting its position to be the given one.
	fn await_next_write_expecting(&self, expected_pos: usize) -> oneshot::Receiver<()> {
		let (pos, rx) = self.await_next_write();
		assert_eq!(pos, expected_pos);

		rx
	}

	// return a receiver that will wake up after n other receivers,
	// inserting receivers as necessary.
	//
	// panics if there are already more than n receivers.
	fn await_nth_write(&self, n: usize) -> oneshot::Receiver<()> {
		assert_ne!(n, 0, "invalid parameter 0");
		let expected_pos = n - 1;

		loop {
			let (pos, rx) = self.await_next_write();
			assert!(pos <= expected_pos, "pending awaits {} > {}", pos, expected_pos);
			if pos == expected_pos {
				break rx;
			}
		}
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
	finalized_number: BlockNumber,
	finalized_hash: Hash,
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

async fn answer_header_request(
	overseer: &mut VirtualOverseer,
	maybe_header: impl Into<Option<Header>>,
) {
	assert_matches!(
		overseer.recv().await,
		AllMessages::ChainApi(ChainApiMessage::BlockHeader(hash, tx)) => {
			let maybe_header = maybe_header.into();
			assert!(maybe_header.as_ref().map_or(true, |h| h.hash() == hash));
			let _ = tx.send(Ok(maybe_header));
		}
	)
}

async fn answer_weight_request(
	overseer: &mut VirtualOverseer,
	hash: Hash,
	weight: impl Into<Option<BlockWeight>>,
) {
	assert_matches!(
		overseer.recv().await,
		AllMessages::ChainApi(ChainApiMessage::BlockWeight(h, tx)) => {
			assert_eq!(h, hash);
			let _ = tx.send(Ok(weight.into()));
		}
	)
}

fn child_header(parent_number: BlockNumber, parent_hash: Hash) -> Header {
	Header {
		parent_hash,
		number: parent_number + 1,
		state_root: Default::default(),
		extrinsics_root: Default::default(),
		digest: Default::default()
	}
}

fn salt_header(header: &mut Header, salt: &[u8]) {
	header.state_root = BlakeTwo256::hash(salt)
}

// Builds a chain on top of the given base. Returns the chain (ascending)
// along with the head hash.
fn construct_chain_on_base(
	len: usize,
	base_number: BlockNumber,
	base_hash: Hash,
	mut mutate: impl FnMut(&mut Header),
) -> (Hash, Vec<Header>) {
	let mut parent_number = base_number;
	let mut parent_hash = base_hash;

	let mut chain = Vec::new();
	for _ in 0..len {
		let mut header = child_header(parent_number, parent_hash);
		mutate(&mut header);

		parent_number = header.number;
		parent_hash = header.hash();
		chain.push(header);
	}

	(parent_hash, chain)
}

fn zip_chain_and_weights(headers: &[Header], weights: &[BlockWeight])
	-> Vec<(Header, BlockWeight)>
{
	headers.iter().cloned().zip(weights.iter().cloned()).collect()
}

async fn answer_ancestry_requests(
	virtual_overseer: &mut VirtualOverseer,
	finalized_answer: Option<(BlockNumber, Hash)>,
	answers: Vec<(Header, BlockWeight)>,
) {
	if let Some((f_n, f_h)) = finalized_answer {
		answer_finalized_block_info(virtual_overseer, f_n, f_h).await;
	}

	// headers in reverse order,
	// TODO [now]: answer ancestor requests.
	for &(ref header, _) in answers.iter().rev() {
		answer_header_request(virtual_overseer, header.clone()).await;
	}

	// Then weights going up.
	for &(ref header, weight) in answers.iter() {
		let hash = header.hash();
		answer_weight_request(virtual_overseer, hash, weight).await;
	}
}

fn assert_backend_contains<'a>(
	backend: &TestBackend,
	headers: impl IntoIterator<Item = &'a Header>,
) {
	for header in headers {
		let hash = header.hash();
		assert!(
			backend.load_blocks_by_number(header.number).unwrap().contains(&hash),
			"blocks at {} does not contain {}",
			header.number,
			hash,
		);
		assert!(
			backend.load_block_entry(&hash).unwrap().is_some(),
			"no entry found for {}",
			hash,
		);
	}
}

fn assert_leaves(
	backend: &TestBackend,
	leaves: Vec<Hash>,
) {
	assert_eq!(
		backend.load_leaves().unwrap().into_hashes_descending().into_iter().collect::<Vec<_>>(),
		leaves,
	)
}

#[test]
fn no_op_subsystem_run() {
	test_harness(|_, virtual_overseer| async move { virtual_overseer });
}

#[test]
fn import_direct_child_of_finalized_on_empty() {
	test_harness(|backend, mut virtual_overseer| async move {
		let finalized_number = 0;
		let finalized_hash = Hash::repeat_byte(0);

		let child = child_header(finalized_number, finalized_hash);
		let child_hash = child.hash();
		let child_weight = 1;
		let child_number = child.number;

		let write_rx = backend.await_next_write_expecting(0);
		virtual_overseer.send(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			ActivatedLeaf {
				hash: child_hash,
				number: child_number,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			}
		)).into()).await;

		answer_ancestry_requests(
			&mut virtual_overseer,
			Some((finalized_number, finalized_hash)),
			vec![(child.clone(), child_weight)],
		).await;

		write_rx.await.unwrap();

		assert_eq!(backend.load_first_block_number().unwrap().unwrap(), child_number);
		assert_backend_contains(&backend, &[child]);
		assert_leaves(&backend, vec![child_hash]);

		virtual_overseer
	})
}

#[test]
fn import_chain_on_finalized_incrementally() {
	test_harness(|backend, mut virtual_overseer| async move {
		let finalized_number = 0;
		let finalized_hash = Hash::repeat_byte(0);

		let (head_hash, chain) = construct_chain_on_base(
			5,
			finalized_number,
			finalized_hash,
			|_| {}
		);

		let chain = zip_chain_and_weights(
			&chain,
			&[1, 2, 3, 4, 5],
		);

		let write_rx = backend.await_nth_write(5);
		for &(ref header, weight) in &chain {
			virtual_overseer.send(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
				ActivatedLeaf {
					hash: header.hash(),
					number: header.number,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}
			)).into()).await;

			answer_ancestry_requests(
				&mut virtual_overseer,
				Some((finalized_number, finalized_hash)).filter(|_| header.number == 1),
				vec![(header.clone(), weight)]
			).await;
		}

		write_rx.await.unwrap();

		assert_eq!(backend.load_first_block_number().unwrap().unwrap(), 1);
		assert_backend_contains(&backend, chain.iter().map(|&(ref h, _)| h));
		assert_leaves(&backend, vec![head_hash]);

		virtual_overseer
	})
}

#[test]
fn import_chain_on_finalized_at_once() {
	test_harness(|backend, mut virtual_overseer| async move {
		let finalized_number = 0;
		let finalized_hash = Hash::repeat_byte(0);

		let (head_hash, chain) = construct_chain_on_base(
			5,
			finalized_number,
			finalized_hash,
			|_| {}
		);

		let chain = zip_chain_and_weights(
			&chain,
			&[1, 2, 3, 4, 5],
		);

		let write_rx = backend.await_next_write_expecting(0);
		virtual_overseer.send(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			ActivatedLeaf {
				hash: head_hash,
				number: 5,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			}
		)).into()).await;

		answer_ancestry_requests(
			&mut virtual_overseer,
			Some((finalized_number, finalized_hash)),
			chain.clone(),
		).await;

		write_rx.await.unwrap();

		assert_eq!(backend.load_first_block_number().unwrap().unwrap(), 1);
		assert_backend_contains(&backend, chain.iter().map(|&(ref h, _)| h));
		assert_leaves(&backend, vec![head_hash]);

		virtual_overseer
	})
}

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
