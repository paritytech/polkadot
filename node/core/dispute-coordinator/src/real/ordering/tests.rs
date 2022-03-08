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

use std::{sync::Arc, time::Duration};

use assert_matches::assert_matches;

use futures::future::join;
use parity_scale_codec::Encode;
use sp_core::testing::TaskExecutor;

use ::test_helpers::{dummy_collator, dummy_collator_signature, dummy_hash};
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		AllMessages, ChainApiMessage, DisputeCoordinatorMessage, RuntimeApiMessage,
		RuntimeApiRequest,
	},
	ActivatedLeaf, ActiveLeavesUpdate, LeafStatus,
};
use polkadot_node_subsystem_test_helpers::{
	make_subsystem_context, TestSubsystemContext, TestSubsystemContextHandle, TestSubsystemSender,
};
use polkadot_node_subsystem_util::{reexports::SubsystemContext, TimeoutExt};
use polkadot_primitives::v1::{
	BlakeTwo256, BlockNumber, CandidateDescriptor, CandidateEvent, CandidateReceipt, CoreIndex,
	GroupIndex, Hash, HashT, HeadData,
};

use super::OrderingProvider;

type VirtualOverseer = TestSubsystemContextHandle<DisputeCoordinatorMessage>;

const OVERSEER_RECEIVE_TIMEOUT: Duration = Duration::from_secs(2);

async fn overseer_recv(virtual_overseer: &mut VirtualOverseer) -> AllMessages {
	virtual_overseer
		.recv()
		.timeout(OVERSEER_RECEIVE_TIMEOUT)
		.await
		.expect("overseer `recv` timed out")
}

struct TestState {
	chain: Vec<Hash>,
	ordering: OrderingProvider,
	ctx: TestSubsystemContext<DisputeCoordinatorMessage, TaskExecutor>,
}

impl TestState {
	async fn new() -> (Self, VirtualOverseer) {
		let (mut ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
		let leaf = get_activated_leaf(0);
		let chain = vec![get_block_number_hash(0)];

		let finalized_block_number = 0;
		let overseer_fut = async {
			assert_finalized_block_number_request(&mut ctx_handle, finalized_block_number).await;
			// No requests for ancestors since the block is already finalized.
			assert_candidate_events_request(&mut ctx_handle, &chain).await;
		};

		let ordering_provider =
			join(OrderingProvider::new(ctx.sender(), leaf.clone()), overseer_fut)
				.await
				.0
				.unwrap();

		let test_state = Self { chain, ordering: ordering_provider, ctx };

		(test_state, ctx_handle)
	}
}

fn next_block_number(chain: &[Hash]) -> BlockNumber {
	chain.len() as u32
}

/// Get a new leaf.
fn next_leaf(chain: &mut Vec<Hash>) -> ActivatedLeaf {
	let next_block_number = next_block_number(chain);
	let next_hash = get_block_number_hash(next_block_number);
	chain.push(next_hash);
	get_activated_leaf(next_block_number)
}

async fn process_active_leaves_update(
	sender: &mut TestSubsystemSender,
	ordering: &mut OrderingProvider,
	update: ActivatedLeaf,
) {
	ordering
		.process_active_leaves_update(sender, &ActiveLeavesUpdate::start_work(update))
		.await
		.unwrap();
}

fn make_candidate_receipt(relay_parent: Hash) -> CandidateReceipt {
	let zeros = dummy_hash();
	let descriptor = CandidateDescriptor {
		para_id: 0.into(),
		relay_parent,
		collator: dummy_collator(),
		persisted_validation_data_hash: zeros,
		pov_hash: zeros,
		erasure_root: zeros,
		signature: dummy_collator_signature(),
		para_head: zeros,
		validation_code_hash: zeros.into(),
	};
	let candidate = CandidateReceipt { descriptor, commitments_hash: zeros };
	candidate
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

/// Get a dummy event that corresponds to candidate inclusion for the given block number.
fn get_candidate_included_events(block_number: BlockNumber) -> Vec<CandidateEvent> {
	vec![CandidateEvent::CandidateIncluded(
		make_candidate_receipt(get_block_number_hash(block_number)),
		HeadData::default(),
		CoreIndex::from(0),
		GroupIndex::from(0),
	)]
}

async fn assert_candidate_events_request(virtual_overseer: &mut VirtualOverseer, chain: &[Hash]) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			hash,
			RuntimeApiRequest::CandidateEvents(tx),
		)) => {
			let maybe_block_number = chain.iter().position(|h| *h == hash);
			let response = maybe_block_number
				.map(|num| get_candidate_included_events(num as u32))
				.unwrap_or_default();
			tx.send(Ok(response)).unwrap();
		}
	);
}

async fn assert_block_number_request(virtual_overseer: &mut VirtualOverseer, chain: &[Hash]) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::ChainApi(ChainApiMessage::BlockNumber(relay_parent, tx)) => {
			let maybe_block_number =
				chain.iter().position(|hash| *hash == relay_parent).map(|number| number as u32);
			tx.send(Ok(maybe_block_number)).unwrap();
		}
	);
}

async fn assert_finalized_block_number_request(
	virtual_overseer: &mut VirtualOverseer,
	response: BlockNumber,
) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::ChainApi(ChainApiMessage::FinalizedBlockNumber(tx)) => {
			tx.send(Ok(response)).unwrap();
		}
	);
}

async fn assert_block_ancestors_request(virtual_overseer: &mut VirtualOverseer, chain: &[Hash]) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::ChainApi(ChainApiMessage::Ancestors { hash, k, response_channel }) => {
			let maybe_block_position = chain.iter().position(|h| *h == hash);
			let ancestors = maybe_block_position
				.map(|idx| chain[..idx].iter().rev().take(k).copied().collect())
				.unwrap_or_default();
			response_channel.send(Ok(ancestors)).unwrap();
		}
	);
}

async fn overseer_process_active_leaves_update(
	virtual_overseer: &mut VirtualOverseer,
	chain: &[Hash],
	finalized_block: BlockNumber,
	expected_ancestry_len: usize,
) {
	// Before walking through ancestors provider requests latest finalized block number.
	assert_finalized_block_number_request(virtual_overseer, finalized_block).await;
	// Expect block ancestors requests with respect to the ancestry step.
	for _ in (0..expected_ancestry_len).step_by(OrderingProvider::ANCESTRY_CHUNK_SIZE) {
		assert_block_ancestors_request(virtual_overseer, chain).await;
	}
	// For each ancestry and the head return corresponding candidates inclusions.
	for _ in 0..expected_ancestry_len {
		assert_candidate_events_request(virtual_overseer, chain).await;
	}
}

#[test]
fn ordering_provider_provides_ordering_when_initialized() {
	let candidate = make_candidate_receipt(get_block_number_hash(1));
	futures::executor::block_on(async {
		let (state, mut virtual_overseer) = TestState::new().await;

		let TestState { mut chain, mut ordering, mut ctx } = state;

		let r = ordering.candidate_comparator(ctx.sender(), &candidate).await.unwrap();
		assert_matches!(r, None);

		// After next active leaves update we should have a comparator:
		let next_update = next_leaf(&mut chain);

		let finalized_block_number = 0;
		let expected_ancestry_len = 1;
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			expected_ancestry_len,
		);
		join(process_active_leaves_update(ctx.sender(), &mut ordering, next_update), overseer_fut)
			.await;

		let r = join(
			ordering.candidate_comparator(ctx.sender(), &candidate),
			assert_block_number_request(&mut virtual_overseer, &chain),
		)
		.await
		.0;
		assert_matches!(r, Ok(Some(r2)) => {
			assert_eq!(r2.relay_parent_block_number, 1);
		});
	});
}

#[test]
fn ordering_provider_requests_candidates_of_leaf_ancestors() {
	futures::executor::block_on(async {
		// How many blocks should we skip before sending a leaf update.
		const BLOCKS_TO_SKIP: usize = 30;

		let (state, mut virtual_overseer) = TestState::new().await;

		let TestState { mut chain, mut ordering, mut ctx } = state;

		let next_update = (0..BLOCKS_TO_SKIP).map(|_| next_leaf(&mut chain)).last().unwrap();

		let finalized_block_number = 0;
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			BLOCKS_TO_SKIP,
		);
		join(process_active_leaves_update(ctx.sender(), &mut ordering, next_update), overseer_fut)
			.await;

		let next_block_number = next_block_number(&chain);
		for block_number in 1..next_block_number {
			let candidate = make_candidate_receipt(get_block_number_hash(block_number));
			let r = join(
				ordering.candidate_comparator(ctx.sender(), &candidate),
				assert_block_number_request(&mut virtual_overseer, &chain),
			)
			.await
			.0;
			assert_matches!(r, Ok(Some(r2)) => {
				assert_eq!(r2.relay_parent_block_number, block_number);
			});
		}
	});
}

#[test]
fn ordering_provider_requests_candidates_of_non_cached_ancestors() {
	futures::executor::block_on(async {
		// How many blocks should we skip before sending a leaf update.
		const BLOCKS_TO_SKIP: &[usize] = &[30, 15];

		let (state, mut virtual_overseer) = TestState::new().await;

		let TestState { mut chain, mut ordering, mut ctx } = state;

		let next_update = (0..BLOCKS_TO_SKIP[0]).map(|_| next_leaf(&mut chain)).last().unwrap();

		let finalized_block_number = 0;
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			BLOCKS_TO_SKIP[0],
		);
		join(process_active_leaves_update(ctx.sender(), &mut ordering, next_update), overseer_fut)
			.await;

		// Send the second request and verify that we don't go past the cached block.
		let next_update = (0..BLOCKS_TO_SKIP[1]).map(|_| next_leaf(&mut chain)).last().unwrap();
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			BLOCKS_TO_SKIP[1],
		);
		join(process_active_leaves_update(ctx.sender(), &mut ordering, next_update), overseer_fut)
			.await;
	});
}

#[test]
fn ordering_provider_requests_candidates_of_non_finalized_ancestors() {
	futures::executor::block_on(async {
		// How many blocks should we skip before sending a leaf update.
		const BLOCKS_TO_SKIP: usize = 30;

		let (state, mut virtual_overseer) = TestState::new().await;

		let TestState { mut chain, mut ordering, mut ctx } = state;

		let next_update = (0..BLOCKS_TO_SKIP).map(|_| next_leaf(&mut chain)).last().unwrap();

		let finalized_block_number = 17;
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			BLOCKS_TO_SKIP - finalized_block_number as usize, // Expect the provider not to go past finalized block.
		);
		join(process_active_leaves_update(ctx.sender(), &mut ordering, next_update), overseer_fut)
			.await;
	});
}
