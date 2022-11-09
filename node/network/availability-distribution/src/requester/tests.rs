// Copyright 2022 Parity Technologies (UK) Ltd.
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

use std::{future::Future, sync::Arc};

use futures::FutureExt;

use polkadot_node_network_protocol::jaeger;
use polkadot_node_primitives::{BlockData, ErasureChunk, PoV};
use polkadot_node_subsystem_util::runtime::RuntimeInfo;
use polkadot_primitives::v2::{
	BlockNumber, CoreState, GroupIndex, Hash, Id as ParaId, ScheduledCore, SessionIndex,
	SessionInfo,
};
use sp_core::traits::SpawnNamed;

use polkadot_node_subsystem::{
	messages::{
		AllMessages, AvailabilityDistributionMessage, AvailabilityStoreMessage, ChainApiMessage,
		NetworkBridgeTxMessage, RuntimeApiMessage, RuntimeApiRequest,
	},
	ActivatedLeaf, ActiveLeavesUpdate, LeafStatus, SpawnGlue,
};
use polkadot_node_subsystem_test_helpers::{
	make_subsystem_context, mock::make_ferdie_keystore, TestSubsystemContext,
	TestSubsystemContextHandle,
};

use sp_core::testing::TaskExecutor;

use crate::tests::mock::{get_valid_chunk_data, make_session_info, OccupiedCoreBuilder};

use super::Requester;

fn get_erasure_chunk() -> ErasureChunk {
	let pov = PoV { block_data: BlockData(vec![45, 46, 47]) };
	get_valid_chunk_data(pov).1
}

#[derive(Clone)]
struct TestState {
	/// Simulated relay chain heads. For each block except genesis
	/// there exists a single corresponding candidate, handled in [`spawn_virtual_overseer`].
	pub relay_chain: Vec<Hash>,
	pub session_info: SessionInfo,
	// Defines a way to compute a session index for the block with
	// a given number. Returns 1 for all blocks by default.
	pub session_index_for_block: fn(BlockNumber) -> SessionIndex,
}

impl TestState {
	fn new() -> Self {
		let relay_chain: Vec<_> = (0u8..10).map(Hash::repeat_byte).collect();
		let session_info = make_session_info();
		let session_index_for_block = |_| 1;
		Self { relay_chain, session_info, session_index_for_block }
	}
}

fn spawn_virtual_overseer(
	pool: TaskExecutor,
	test_state: TestState,
	mut ctx_handle: TestSubsystemContextHandle<AvailabilityDistributionMessage>,
) {
	pool.spawn(
		"virtual-overseer",
		None,
		async move {
			loop {
				let msg = ctx_handle.try_recv().await;
				if msg.is_none() {
					break
				}
				match msg.unwrap() {
					AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(..)) => {},
					AllMessages::AvailabilityStore(AvailabilityStoreMessage::QueryChunk(
						..,
						tx,
					)) => {
						let chunk = get_erasure_chunk();
						tx.send(Some(chunk)).expect("Receiver is expected to be alive");
					},
					AllMessages::AvailabilityStore(AvailabilityStoreMessage::StoreChunk {
						tx,
						..
					}) => {
						// Silently accept it.
						tx.send(Ok(())).expect("Receiver is expected to be alive");
					},
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(hash, req)) => {
						match req {
							RuntimeApiRequest::SessionIndexForChild(tx) => {
								let chain = &test_state.relay_chain;
								let block_number = chain
									.iter()
									.position(|h| *h == hash)
									.expect("Invalid session index request");
								// Compute session index.
								let session_index_for_block = test_state.session_index_for_block;

								tx.send(Ok(session_index_for_block(block_number as u32 + 1)))
									.expect("Receiver should still be alive");
							},
							RuntimeApiRequest::SessionInfo(_, tx) => {
								tx.send(Ok(Some(test_state.session_info.clone())))
									.expect("Receiver should be alive.");
							},
							RuntimeApiRequest::AvailabilityCores(tx) => {
								let para_id = ParaId::from(1_u32);
								let maybe_block_position =
									test_state.relay_chain.iter().position(|h| *h == hash);
								let cores = match maybe_block_position {
									Some(block_num) => {
										let core = if block_num == 0 {
											CoreState::Scheduled(ScheduledCore {
												para_id,
												collator: None,
											})
										} else {
											CoreState::Occupied(
												OccupiedCoreBuilder {
													group_responsible: GroupIndex(1),
													para_id,
													relay_parent: hash,
												}
												.build()
												.0,
											)
										};
										vec![core]
									},
									None => Vec::new(),
								};
								tx.send(Ok(cores)).expect("Receiver should be alive.")
							},
							_ => {
								panic!("Unexpected runtime request: {:?}", req);
							},
						}
					},
					AllMessages::ChainApi(ChainApiMessage::Ancestors {
						hash,
						k,
						response_channel,
					}) => {
						let chain = &test_state.relay_chain;
						let maybe_block_position = chain.iter().position(|h| *h == hash);
						let ancestors = maybe_block_position
							.map(|idx| chain[..idx].iter().rev().take(k).copied().collect())
							.unwrap_or_default();
						response_channel
							.send(Ok(ancestors))
							.expect("Receiver is expected to be alive");
					},
					msg => panic!("Unexpected overseer message: {:?}", msg),
				}
			}
		}
		.boxed(),
	);
}

fn test_harness<T: Future<Output = ()>>(
	test_state: TestState,
	test_fx: impl FnOnce(
		TestSubsystemContext<AvailabilityDistributionMessage, SpawnGlue<TaskExecutor>>,
	) -> T,
) {
	let pool = TaskExecutor::new();
	let (ctx, ctx_handle) = make_subsystem_context(pool.clone());

	spawn_virtual_overseer(pool, test_state, ctx_handle);

	futures::executor::block_on(test_fx(ctx));
}

#[test]
fn check_ancestry_lookup_in_same_session() {
	let test_state = TestState::new();
	let mut requester = Requester::new(Default::default());
	let keystore = make_ferdie_keystore();
	let mut runtime = RuntimeInfo::new(Some(keystore));

	test_harness(test_state.clone(), |mut ctx| async move {
		let chain = &test_state.relay_chain;

		let block_number = 1;
		let update = ActiveLeavesUpdate {
			activated: Some(ActivatedLeaf {
				hash: chain[block_number],
				number: block_number as u32,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			}),
			deactivated: Vec::new().into(),
		};

		requester
			.update_fetching_heads(&mut ctx, &mut runtime, update)
			.await
			.expect("Leaf processing failed");
		let fetch_tasks = &requester.fetches;
		assert_eq!(fetch_tasks.len(), 1);
		let block_1_candidate =
			*fetch_tasks.keys().next().expect("A task is checked to be present; qed");

		let block_number = 2;
		let update = ActiveLeavesUpdate {
			activated: Some(ActivatedLeaf {
				hash: chain[block_number],
				number: block_number as u32,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			}),
			deactivated: Vec::new().into(),
		};

		requester
			.update_fetching_heads(&mut ctx, &mut runtime, update)
			.await
			.expect("Leaf processing failed");
		let fetch_tasks = &requester.fetches;
		assert_eq!(fetch_tasks.len(), 2);
		let task = fetch_tasks.get(&block_1_candidate).expect("Leaf hasn't been deactivated yet");
		// The task should be live in both blocks 1 and 2.
		assert_eq!(task.live_in.len(), 2);
		let block_2_candidate = *fetch_tasks
			.keys()
			.find(|hash| **hash != block_1_candidate)
			.expect("Two tasks are present, the first one corresponds to block 1 candidate; qed");

		// Deactivate both blocks but keep the second task as a
		// part of ancestry.
		let block_number = 2 + Requester::LEAF_ANCESTRY_LEN_WITHIN_SESSION;
		let update = ActiveLeavesUpdate {
			activated: Some(ActivatedLeaf {
				hash: test_state.relay_chain[block_number],
				number: block_number as u32,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			}),
			deactivated: vec![chain[1], chain[2]].into(),
		};
		requester
			.update_fetching_heads(&mut ctx, &mut runtime, update)
			.await
			.expect("Leaf processing failed");
		let fetch_tasks = &requester.fetches;
		// The leaf + K its ancestors.
		assert_eq!(fetch_tasks.len(), Requester::LEAF_ANCESTRY_LEN_WITHIN_SESSION + 1);

		let block_2_task = fetch_tasks
			.get(&block_2_candidate)
			.expect("Expected to be live as a part of ancestry");
		assert_eq!(block_2_task.live_in.len(), 1);
	});
}

#[test]
fn check_ancestry_lookup_in_different_sessions() {
	let mut test_state = TestState::new();
	let mut requester = Requester::new(Default::default());
	let keystore = make_ferdie_keystore();
	let mut runtime = RuntimeInfo::new(Some(keystore));

	test_state.session_index_for_block = |block_number| match block_number {
		0..=3 => 1,
		_ => 2,
	};

	test_harness(test_state.clone(), |mut ctx| async move {
		let chain = &test_state.relay_chain;

		let block_number = 3;
		let update = ActiveLeavesUpdate {
			activated: Some(ActivatedLeaf {
				hash: chain[block_number],
				number: block_number as u32,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			}),
			deactivated: Vec::new().into(),
		};

		requester
			.update_fetching_heads(&mut ctx, &mut runtime, update)
			.await
			.expect("Leaf processing failed");
		let fetch_tasks = &requester.fetches;
		assert_eq!(fetch_tasks.len(), 3.min(Requester::LEAF_ANCESTRY_LEN_WITHIN_SESSION + 1));

		let block_number = 4;
		let update = ActiveLeavesUpdate {
			activated: Some(ActivatedLeaf {
				hash: chain[block_number],
				number: block_number as u32,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			}),
			deactivated: vec![chain[1], chain[2], chain[3]].into(),
		};

		requester
			.update_fetching_heads(&mut ctx, &mut runtime, update)
			.await
			.expect("Leaf processing failed");
		let fetch_tasks = &requester.fetches;
		assert_eq!(fetch_tasks.len(), 1);

		let block_number = 5;
		let update = ActiveLeavesUpdate {
			activated: Some(ActivatedLeaf {
				hash: chain[block_number],
				number: block_number as u32,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			}),
			deactivated: vec![chain[4]].into(),
		};

		requester
			.update_fetching_heads(&mut ctx, &mut runtime, update)
			.await
			.expect("Leaf processing failed");
		let fetch_tasks = &requester.fetches;
		assert_eq!(fetch_tasks.len(), 2.min(Requester::LEAF_ANCESTRY_LEN_WITHIN_SESSION + 1));
	});
}
