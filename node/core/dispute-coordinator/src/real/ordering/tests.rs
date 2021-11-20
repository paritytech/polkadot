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

use std::sync::Arc;

use assert_matches::assert_matches;

use futures::FutureExt;
use parity_scale_codec::Encode;
use sp_core::testing::TaskExecutor;

use polkadot_node_subsystem::{
	jaeger,
	messages::{
		AllMessages, ChainApiMessage, DisputeCoordinatorMessage, RuntimeApiMessage,
		RuntimeApiRequest,
	},
	ActivatedLeaf, ActiveLeavesUpdate, LeafStatus,
};
use polkadot_node_subsystem_test_helpers::{
	make_subsystem_context, TestSubsystemContext, TestSubsystemContextHandle,
};
use polkadot_node_subsystem_util::reexports::SubsystemContext;
use polkadot_primitives::v1::{
	BlakeTwo256, BlockNumber, CandidateEvent, CandidateReceipt, CoreIndex, GroupIndex, Hash, HashT,
	HeadData,
};

use super::OrderingProvider;

type VirtualOverseer = TestSubsystemContextHandle<DisputeCoordinatorMessage>;

struct TestState {
	next_block_number: BlockNumber,
	ordering: OrderingProvider,
	ctx: TestSubsystemContext<DisputeCoordinatorMessage, TaskExecutor>,
}

impl TestState {
	async fn new() -> Self {
		let (mut ctx, ctx_handle) = make_subsystem_context(TaskExecutor::new());
		let leaf = get_activated_leaf(1);
		launch_virtual_overseer(&mut ctx, ctx_handle);
		Self {
			next_block_number: 2,
			ordering: OrderingProvider::new(ctx.sender(), leaf).await.unwrap(),
			ctx,
		}
	}

	/// Get a new leaf.
	fn next_leaf(&mut self) -> ActivatedLeaf {
		let r = get_activated_leaf(self.next_block_number);
		self.next_block_number += 1;
		r
	}

	async fn process_active_leaves_update(&mut self) {
		let update = self.next_leaf();
		self.ordering
			.process_active_leaves_update(
				self.ctx.sender(),
				&ActiveLeavesUpdate::start_work(update),
			)
			.await
			.unwrap();
	}
}

/// Simulate other subsystems:
fn launch_virtual_overseer(ctx: &mut impl SubsystemContext, ctx_handle: VirtualOverseer) {
	ctx.spawn(
		"serve-active-leaves-update",
		async move { virtual_overseer(ctx_handle).await }.boxed(),
	)
	.unwrap();
}

async fn virtual_overseer(mut ctx_handle: VirtualOverseer) {
	let ev = vec![CandidateEvent::CandidateIncluded(
		CandidateReceipt::default(),
		HeadData::default(),
		CoreIndex::from(0),
		GroupIndex::from(0),
	)];

	assert_matches!(
		ctx_handle.recv().await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_,
				RuntimeApiRequest::CandidateEvents(
					tx,
					)
				)) => {
			tx.send(Ok(Vec::new())).unwrap();
		}
	);
	assert_matches!(
		ctx_handle.recv().await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_,
				RuntimeApiRequest::CandidateEvents(
					tx,
					)
				)) => {
			tx.send(Ok(ev)).unwrap();
		}
	);
	assert_matches!(
		ctx_handle.recv().await,
		AllMessages::ChainApi(ChainApiMessage::BlockNumber(_, tx)) => {
			tx.send(Ok(Some(1))).unwrap();
		}
	);
}

/// Get a dummy `ActivatedLeaf` for a given block number.
fn get_activated_leaf(n: BlockNumber) -> ActivatedLeaf {
	ActivatedLeaf {
		hash: get_block_number_hash(n),
		number: n,
		status: LeafStatus::Fresh,
		span: Arc::new(jaeger::Span::Disabled),
	}
}

/// Get a dummy relay parent hash for dummy block number.
fn get_block_number_hash(n: BlockNumber) -> Hash {
	BlakeTwo256::hash(&n.encode())
}

#[test]
fn ordering_provider_provides_ordering_when_initialized() {
	futures::executor::block_on(async {
		let mut state = TestState::new().await;
		let r = state
			.ordering
			.candidate_comparator(state.ctx.sender(), &CandidateReceipt::default())
			.await
			.unwrap();
		assert!(r.is_none());
		// After next active leaves update we should have a comparator:
		state.process_active_leaves_update().await;
		let r = state
			.ordering
			.candidate_comparator(state.ctx.sender(), &CandidateReceipt::default())
			.await
			.unwrap();
		assert!(r.is_some());
		assert_eq!(r.unwrap().relay_parent_block_number, 1);
	});
}
