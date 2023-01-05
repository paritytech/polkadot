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
use polkadot_node_primitives::DISPUTE_CANDIDATE_LIFETIME_AFTER_FINALIZATION;
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		AllMessages, ChainApiMessage, DisputeCoordinatorMessage, RuntimeApiMessage,
		RuntimeApiRequest,
	},
	ActivatedLeaf, ActiveLeavesUpdate, LeafStatus, SpawnGlue,
};
use polkadot_node_subsystem_test_helpers::{
	make_subsystem_context, TestSubsystemContext, TestSubsystemContextHandle, TestSubsystemSender,
};
use polkadot_node_subsystem_util::{reexports::SubsystemContext, TimeoutExt};
use polkadot_primitives::v2::{
	BlakeTwo256, BlockNumber, CandidateDescriptor, CandidateEvent, CandidateReceipt, CoreIndex,
	GroupIndex, Hash, HashT, HeadData, Id as ParaId,
};

use crate::LOG_TARGET;

use super::ChainScraper;

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
	scraper: ChainScraper,
	ctx: TestSubsystemContext<DisputeCoordinatorMessage, SpawnGlue<TaskExecutor>>,
}

impl TestState {
	async fn new() -> (Self, VirtualOverseer) {
		let (mut ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
		let chain = vec![get_block_number_hash(0), get_block_number_hash(1)];
		let leaf = get_activated_leaf(1);

		let finalized_block_number = 0;
		let overseer_fut = async {
			assert_finalized_block_number_request(&mut ctx_handle, finalized_block_number).await;
			gum::trace!(target: LOG_TARGET, "After assert_finalized_block_number");
			// No ancestors requests, as list would be empty.
			assert_candidate_events_request(
				&mut ctx_handle,
				&chain,
				get_backed_and_included_candidate_events,
			)
			.await;
			assert_chain_vote_request(&mut ctx_handle, &chain).await;
		};

		let (scraper, _) = join(ChainScraper::new(ctx.sender(), leaf.clone()), overseer_fut)
			.await
			.0
			.unwrap();
		gum::trace!(target: LOG_TARGET, "After launching chain scraper");

		let test_state = Self { chain, scraper, ctx };

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
	scraper: &mut ChainScraper,
	update: ActivatedLeaf,
) {
	scraper
		.process_active_leaves_update(sender, &ActiveLeavesUpdate::start_work(update))
		.await
		.unwrap();
}

fn process_finalized_block(scraper: &mut ChainScraper, finalized: &BlockNumber) {
	scraper.process_finalized_block(&finalized)
}

fn make_candidate_receipt(relay_parent: Hash) -> CandidateReceipt {
	let zeros = dummy_hash();
	let descriptor = CandidateDescriptor {
		para_id: ParaId::from(0_u32),
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
fn get_backed_and_included_candidate_events(block_number: BlockNumber) -> Vec<CandidateEvent> {
	let candidate_receipt = make_candidate_receipt(get_block_number_hash(block_number));
	vec![
		CandidateEvent::CandidateIncluded(
			candidate_receipt.clone(),
			HeadData::default(),
			CoreIndex::from(0),
			GroupIndex::from(0),
		),
		CandidateEvent::CandidateBacked(
			candidate_receipt,
			HeadData::default(),
			CoreIndex::from(0),
			GroupIndex::from(0),
		),
	]
}

fn get_backed_candidate_event(block_number: BlockNumber) -> Vec<CandidateEvent> {
	let candidate_receipt = make_candidate_receipt(get_block_number_hash(block_number));
	vec![CandidateEvent::CandidateBacked(
		candidate_receipt,
		HeadData::default(),
		CoreIndex::from(0),
		GroupIndex::from(0),
	)]
}
/// Hash for a 'magic' candidate. This is meant to be a special candidate used to verify special cases.
fn get_magic_candidate_hash() -> Hash {
	BlakeTwo256::hash(&"abc".encode())
}
/// Get a dummy event that corresponds to candidate inclusion for a hardcoded block number.
/// Used to simulate candidates included multiple times at different block heights.
fn get_backed_and_included_magic_candidate_events(
	_block_number: BlockNumber,
) -> Vec<CandidateEvent> {
	let candidate_receipt = make_candidate_receipt(get_magic_candidate_hash());
	vec![
		CandidateEvent::CandidateIncluded(
			candidate_receipt.clone(),
			HeadData::default(),
			CoreIndex::from(0),
			GroupIndex::from(0),
		),
		CandidateEvent::CandidateBacked(
			candidate_receipt,
			HeadData::default(),
			CoreIndex::from(0),
			GroupIndex::from(0),
		),
	]
}

async fn assert_candidate_events_request<F>(
	virtual_overseer: &mut VirtualOverseer,
	chain: &[Hash],
	event_generator: F,
) where
	F: Fn(u32) -> Vec<CandidateEvent>,
{
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			hash,
			RuntimeApiRequest::CandidateEvents(tx),
		)) => {
			let maybe_block_number = chain.iter().position(|h| *h == hash);
			let response = maybe_block_number
				.map(|num| event_generator(num as u32))
				.unwrap_or_default();
			tx.send(Ok(response)).unwrap();
		}
	);
}

async fn assert_chain_vote_request(virtual_overseer: &mut VirtualOverseer, _chain: &[Hash]) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			_hash,
			RuntimeApiRequest::FetchOnChainVotes(tx),
		)) => {
			tx.send(Ok(None)).unwrap();
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

async fn overseer_process_active_leaves_update<F>(
	virtual_overseer: &mut VirtualOverseer,
	chain: &[Hash],
	finalized_block: BlockNumber,
	expected_ancestry_len: usize,
	event_generator: F,
) where
	F: Fn(u32) -> Vec<CandidateEvent> + Clone,
{
	// Before walking through ancestors provider requests latest finalized block number.
	assert_finalized_block_number_request(virtual_overseer, finalized_block).await;
	// Expect block ancestors requests with respect to the ancestry step.
	for _ in (0..expected_ancestry_len).step_by(ChainScraper::ANCESTRY_CHUNK_SIZE as usize) {
		assert_block_ancestors_request(virtual_overseer, chain).await;
	}
	// For each ancestry and the head return corresponding candidates inclusions.
	for _ in 0..expected_ancestry_len {
		assert_candidate_events_request(virtual_overseer, chain, event_generator.clone()).await;
		assert_chain_vote_request(virtual_overseer, chain).await;
	}
}

#[test]
fn scraper_provides_included_state_when_initialized() {
	let candidate_1 = make_candidate_receipt(get_block_number_hash(1));
	let candidate_2 = make_candidate_receipt(get_block_number_hash(2));
	futures::executor::block_on(async {
		let (state, mut virtual_overseer) = TestState::new().await;

		let TestState { mut chain, mut scraper, mut ctx } = state;

		assert!(!scraper.is_candidate_included(&candidate_2.hash()));
		assert!(!scraper.is_candidate_backed(&candidate_2.hash()));
		assert!(scraper.is_candidate_included(&candidate_1.hash()));
		assert!(scraper.is_candidate_backed(&candidate_1.hash()));

		// After next active leaves update we should see the candidate included.
		let next_update = next_leaf(&mut chain);

		let finalized_block_number = 0;
		let expected_ancestry_len = 1;
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			expected_ancestry_len,
			get_backed_and_included_candidate_events,
		);
		join(process_active_leaves_update(ctx.sender(), &mut scraper, next_update), overseer_fut)
			.await;

		assert!(scraper.is_candidate_included(&candidate_2.hash()));
		assert!(scraper.is_candidate_backed(&candidate_2.hash()));
	});
}

#[test]
fn scraper_requests_candidates_of_leaf_ancestors() {
	futures::executor::block_on(async {
		// How many blocks should we skip before sending a leaf update.
		const BLOCKS_TO_SKIP: usize = 30;

		let (state, mut virtual_overseer) = TestState::new().await;

		let TestState { mut chain, mut scraper, mut ctx } = state;

		let next_update = (0..BLOCKS_TO_SKIP).map(|_| next_leaf(&mut chain)).last().unwrap();

		let finalized_block_number = 0;
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			BLOCKS_TO_SKIP,
			get_backed_and_included_candidate_events,
		);
		join(process_active_leaves_update(ctx.sender(), &mut scraper, next_update), overseer_fut)
			.await;

		let next_block_number = next_block_number(&chain);
		for block_number in 1..next_block_number {
			let candidate = make_candidate_receipt(get_block_number_hash(block_number));
			assert!(scraper.is_candidate_included(&candidate.hash()));
			assert!(scraper.is_candidate_backed(&candidate.hash()));
		}
	});
}

#[test]
fn scraper_requests_candidates_of_non_cached_ancestors() {
	futures::executor::block_on(async {
		// How many blocks should we skip before sending a leaf update.
		const BLOCKS_TO_SKIP: &[usize] = &[30, 15];

		let (state, mut virtual_overseer) = TestState::new().await;

		let TestState { mut chain, scraper: mut ordering, mut ctx } = state;

		let next_update = (0..BLOCKS_TO_SKIP[0]).map(|_| next_leaf(&mut chain)).last().unwrap();

		let finalized_block_number = 0;
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			BLOCKS_TO_SKIP[0],
			get_backed_and_included_candidate_events,
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
			get_backed_and_included_candidate_events,
		);
		join(process_active_leaves_update(ctx.sender(), &mut ordering, next_update), overseer_fut)
			.await;
	});
}

#[test]
fn scraper_requests_candidates_of_non_finalized_ancestors() {
	futures::executor::block_on(async {
		// How many blocks should we skip before sending a leaf update.
		const BLOCKS_TO_SKIP: usize = 30;

		let (state, mut virtual_overseer) = TestState::new().await;

		let TestState { mut chain, scraper: mut ordering, mut ctx } = state;

		// 1 because `TestState` starts at leaf 1.
		let next_update = (1..BLOCKS_TO_SKIP).map(|_| next_leaf(&mut chain)).last().unwrap();

		let finalized_block_number = 17;
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			BLOCKS_TO_SKIP - finalized_block_number as usize, // Expect the provider not to go past finalized block.
			get_backed_and_included_candidate_events,
		);
		join(process_active_leaves_update(ctx.sender(), &mut ordering, next_update), overseer_fut)
			.await;
	});
}

#[test]
fn scraper_prunes_finalized_candidates() {
	const TEST_TARGET_BLOCK_NUMBER: BlockNumber = 2;

	// How many blocks should we skip before sending a leaf update.
	const BLOCKS_TO_SKIP: usize = 3;

	futures::executor::block_on(async {
		let (state, mut virtual_overseer) = TestState::new().await;

		let TestState { mut chain, mut scraper, mut ctx } = state;

		// 1 because `TestState` starts at leaf 1.
		let next_update = (1..BLOCKS_TO_SKIP).map(|_| next_leaf(&mut chain)).last().unwrap();

		let mut finalized_block_number = 1;
		let expected_ancestry_len = BLOCKS_TO_SKIP - finalized_block_number as usize;
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			expected_ancestry_len,
			|block_num| {
				if block_num == TEST_TARGET_BLOCK_NUMBER {
					get_backed_and_included_candidate_events(block_num)
				} else {
					vec![]
				}
			},
		);
		join(process_active_leaves_update(ctx.sender(), &mut scraper, next_update), overseer_fut)
			.await;

		let candidate = make_candidate_receipt(get_block_number_hash(TEST_TARGET_BLOCK_NUMBER));

		// After `DISPUTE_CANDIDATE_LIFETIME_AFTER_FINALIZATION` blocks the candidate should be removed
		finalized_block_number =
			TEST_TARGET_BLOCK_NUMBER + DISPUTE_CANDIDATE_LIFETIME_AFTER_FINALIZATION;
		process_finalized_block(&mut scraper, &finalized_block_number);

		assert!(!scraper.is_candidate_backed(&candidate.hash()));
		assert!(!scraper.is_candidate_included(&candidate.hash()));
	});
}

#[test]
fn scraper_handles_backed_but_not_included_candidate() {
	const TEST_TARGET_BLOCK_NUMBER: BlockNumber = 2;

	// How many blocks should we skip before sending a leaf update.
	const BLOCKS_TO_SKIP: usize = 3;

	futures::executor::block_on(async {
		let (state, mut virtual_overseer) = TestState::new().await;

		let TestState { mut chain, mut scraper, mut ctx } = state;

		let next_update = (1..BLOCKS_TO_SKIP as BlockNumber)
			.map(|_| next_leaf(&mut chain))
			.last()
			.unwrap();

		// Add `ActiveLeavesUpdate` containing `CandidateBacked` event for block `BLOCK_WITH_EVENTS`
		let mut finalized_block_number = 1;
		let expected_ancestry_len = BLOCKS_TO_SKIP - finalized_block_number as usize;
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			expected_ancestry_len,
			|block_num| {
				if block_num == TEST_TARGET_BLOCK_NUMBER {
					get_backed_candidate_event(block_num)
				} else {
					vec![]
				}
			},
		);
		join(process_active_leaves_update(ctx.sender(), &mut scraper, next_update), overseer_fut)
			.await;

		// Finalize blocks to enforce pruning of scraped events
		finalized_block_number += 1;
		process_finalized_block(&mut scraper, &finalized_block_number);

		// `FIRST_TEST_BLOCK` is finalized, which is within `BACKED_CANDIDATE_LIFETIME_AFTER_FINALIZATION` window.
		// The candidate should still be backed.
		let candidate = make_candidate_receipt(get_block_number_hash(TEST_TARGET_BLOCK_NUMBER));
		assert!(!scraper.is_candidate_included(&candidate.hash()));
		assert!(scraper.is_candidate_backed(&candidate.hash()));

		// Bump the finalized block outside `BACKED_CANDIDATE_LIFETIME_AFTER_FINALIZATION`.
		// The candidate should be removed.
		assert!(
			finalized_block_number <
				TEST_TARGET_BLOCK_NUMBER + DISPUTE_CANDIDATE_LIFETIME_AFTER_FINALIZATION
		);
		finalized_block_number +=
			TEST_TARGET_BLOCK_NUMBER + DISPUTE_CANDIDATE_LIFETIME_AFTER_FINALIZATION;
		process_finalized_block(&mut scraper, &finalized_block_number);

		assert!(!scraper.is_candidate_included(&candidate.hash()));
		assert!(!scraper.is_candidate_backed(&candidate.hash()));
	});
}

#[test]
fn scraper_handles_the_same_candidate_incuded_in_two_different_block_heights() {
	// Same candidate will be inclued in these two leaves
	let test_targets = vec![2, 3];

	// How many blocks should we skip before sending a leaf update.
	const BLOCKS_TO_SKIP: usize = 3;

	futures::executor::block_on(async {
		let (state, mut virtual_overseer) = TestState::new().await;

		let TestState { mut chain, mut scraper, mut ctx } = state;

		// 1 because `TestState` starts at leaf 1.
		let next_update = (1..BLOCKS_TO_SKIP).map(|_| next_leaf(&mut chain)).last().unwrap();

		// Now we will add the same magic candidate at two different block heights.
		// Check `get_backed_and_included_magic_candidate_event` implementation
		let mut finalized_block_number = 1;
		let expected_ancestry_len = BLOCKS_TO_SKIP - finalized_block_number as usize;
		let overseer_fut = overseer_process_active_leaves_update(
			&mut virtual_overseer,
			&chain,
			finalized_block_number,
			expected_ancestry_len,
			|block_num| {
				if test_targets.contains(&block_num) {
					get_backed_and_included_magic_candidate_events(block_num)
				} else {
					vec![]
				}
			},
		);
		join(process_active_leaves_update(ctx.sender(), &mut scraper, next_update), overseer_fut)
			.await;

		// Finalize blocks to enforce pruning of scraped events.
		// The magic candidate was added twice, so it shouldn't be removed if we finalize two more blocks.
		finalized_block_number = test_targets.first().expect("there are two block nums") +
			DISPUTE_CANDIDATE_LIFETIME_AFTER_FINALIZATION;
		process_finalized_block(&mut scraper, &finalized_block_number);

		let magic_candidate = make_candidate_receipt(get_magic_candidate_hash());
		assert!(scraper.is_candidate_backed(&magic_candidate.hash()));
		assert!(scraper.is_candidate_included(&magic_candidate.hash()));

		// On the next finalization the magic candidate should be removed
		finalized_block_number += 1;
		process_finalized_block(&mut scraper, &finalized_block_number);

		assert!(!scraper.is_candidate_backed(&magic_candidate.hash()));
		assert!(!scraper.is_candidate_included(&magic_candidate.hash()));
	});
}
