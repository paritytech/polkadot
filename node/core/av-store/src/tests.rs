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

use super::*;

use assert_matches::assert_matches;
use futures::{
	future,
	channel::oneshot,
	executor,
	Future,
};
use smallvec::smallvec;

use polkadot_primitives::v1::{
	AvailableData, BlockData, CandidateDescriptor, CandidateReceipt, HeadData,
	PersistedValidationData, PoV, Id as ParaId,
};
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_subsystem::ActiveLeavesUpdate;
use polkadot_node_subsystem_test_helpers as test_helpers;

struct TestHarness {
	virtual_overseer: test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
}

#[derive(Default)]
struct TestCandidateBuilder {
	para_id: ParaId,
	pov_hash: Hash,
	relay_parent: Hash,
	commitments_hash: Hash,
}

impl TestCandidateBuilder {
	fn build(self) -> CandidateReceipt {
		CandidateReceipt {
			descriptor: CandidateDescriptor {
				para_id: self.para_id,
				pov_hash: self.pov_hash,
				relay_parent: self.relay_parent,
				..Default::default()
			},
			commitments_hash: self.commitments_hash,
		}
	}
}

struct TestState {
	persisted_validation_data: PersistedValidationData,
	pruning_config: PruningConfig,
}

impl Default for TestState {
	fn default() -> Self {
		let persisted_validation_data = PersistedValidationData {
			parent_head: HeadData(vec![7, 8, 9]),
			block_number: 5,
			hrmp_mqc_heads: Vec::new(),
			dmq_mqc_head: Default::default(),
		};

		let pruning_config = PruningConfig {
			keep_stored_block_for: Duration::from_secs(1),
			keep_finalized_block_for: Duration::from_secs(2),
			keep_finalized_chunk_for: Duration::from_secs(2),
		};

		Self {
			persisted_validation_data,
			pruning_config,
		}
	}
}

fn test_harness<T: Future<Output=()>>(
	pruning_config: PruningConfig,
	store: Arc<dyn KeyValueDB>,
	test: impl FnOnce(TestHarness) -> T,
) {
	let _ = env_logger::builder()
		.is_test(true)
		.filter(
			Some("polkadot_node_core_av_store"),
			log::LevelFilter::Trace,
		)
		.filter(
			Some(LOG_TARGET),
			log::LevelFilter::Trace,
		)
		.try_init();

	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = AvailabilityStoreSubsystem::new_in_memory(store, pruning_config);
	let subsystem = run(subsystem, context);

	let test_fut = test(TestHarness {
		virtual_overseer,
	});

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	executor::block_on(future::select(test_fut, subsystem));
}

const TIMEOUT: Duration = Duration::from_millis(100);

async fn overseer_send(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	msg: AvailabilityStoreMessage,
) {
	log::trace!("Sending message:\n{:?}", &msg);
	overseer
		.send(FromOverseer::Communication { msg })
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough for sending messages.", TIMEOUT));
}

async fn overseer_recv(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
) -> AllMessages {
	let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough to receive messages", TIMEOUT));

	log::trace!("Received message:\n{:?}", &msg);

	msg
}

async fn overseer_recv_with_timeout(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	timeout: Duration,
) -> Option<AllMessages> {
	log::trace!("Waiting for message...");
	overseer
		.recv()
		.timeout(timeout)
		.await
}

async fn overseer_signal(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	signal: OverseerSignal,
) {
	overseer
		.send(FromOverseer::Signal(signal))
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough for sending signals.", TIMEOUT));
}

#[test]
fn store_chunk_works() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	test_harness(PruningConfig::default(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let relay_parent = Hash::repeat_byte(32);
		let candidate_hash = Hash::repeat_byte(33);
		let validator_index = 5;

		let chunk = ErasureChunk {
			chunk: vec![1, 2, 3],
			index: validator_index,
			proof: vec![vec![3, 4, 5]],
		};

		let (tx, rx) = oneshot::channel();

		let chunk_msg = AvailabilityStoreMessage::StoreChunk {
			candidate_hash,
			relay_parent,
			validator_index,
			chunk: chunk.clone(),
			tx,
		};

		overseer_send(&mut virtual_overseer, chunk_msg.into()).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::BlockNumber(
				hash,
				tx,
			)) => {
				assert_eq!(hash, relay_parent);
				tx.send(Ok(Some(4))).unwrap();
			}
		);

		assert_eq!(rx.await.unwrap(), Ok(()));

		let (tx, rx) = oneshot::channel();
		let query_chunk = AvailabilityStoreMessage::QueryChunk(
			candidate_hash,
			validator_index,
			tx,
		);

		overseer_send(&mut virtual_overseer, query_chunk.into()).await;

		assert_eq!(rx.await.unwrap().unwrap(), chunk);
	});
}

#[test]
fn store_block_works() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();
	test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let candidate_hash = Hash::from([1; 32]);
		let validator_index = 5;
		let n_validators = 10;

		let pov = PoV {
			block_data: BlockData(vec![4, 5, 6]),
		};

		let available_data = AvailableData {
			pov,
			validation_data: test_state.persisted_validation_data,
		};


		let (tx, rx) = oneshot::channel();
		let block_msg = AvailabilityStoreMessage::StoreAvailableData(
			candidate_hash,
			Some(validator_index),
			n_validators,
			available_data.clone(),
			tx,
		);

		virtual_overseer.send(FromOverseer::Communication{ msg: block_msg }).await;
		assert_eq!(rx.await.unwrap(), Ok(()));

		let pov = query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap();
		assert_eq!(pov, available_data);

		let chunk = query_chunk(&mut virtual_overseer, candidate_hash, validator_index).await.unwrap();

		let chunks = erasure::obtain_chunks_v1(10, &available_data).unwrap();

		let mut branches = erasure::branches(chunks.as_ref());

		let branch = branches.nth(5).unwrap();
		let expected_chunk = ErasureChunk {
			chunk: branch.1.to_vec(),
			index: 5,
			proof: branch.0,
		};

		assert_eq!(chunk, expected_chunk);
	});
}


#[test]
fn store_pov_and_query_chunk_works() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();

	test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let candidate_hash = Hash::from([1; 32]);
		let n_validators = 10;

		let pov = PoV {
			block_data: BlockData(vec![4, 5, 6]),
		};

		let available_data = AvailableData {
			pov,
			validation_data: test_state.persisted_validation_data,
		};

		let no_metrics = Metrics(None);
		let chunks_expected = get_chunks(&available_data, n_validators as usize, &no_metrics).unwrap();

		let (tx, rx) = oneshot::channel();
		let block_msg = AvailabilityStoreMessage::StoreAvailableData(
			candidate_hash,
			None,
			n_validators,
			available_data,
			tx,
		);

		virtual_overseer.send(FromOverseer::Communication{ msg: block_msg }).await;

		assert_eq!(rx.await.unwrap(), Ok(()));

		for validator_index in 0..n_validators {
			let chunk = query_chunk(&mut virtual_overseer, candidate_hash, validator_index).await.unwrap();

			assert_eq!(chunk, chunks_expected[validator_index as usize]);
		}
	});
}

#[test]
fn stored_but_not_included_chunk_is_pruned() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();

	test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let candidate_hash = Hash::repeat_byte(1);
		let relay_parent = Hash::repeat_byte(2);
		let validator_index = 5;

		let chunk = ErasureChunk {
			chunk: vec![1, 2, 3],
			index: validator_index,
			proof: vec![vec![3, 4, 5]],
		};

		let (tx, rx) = oneshot::channel();
		let chunk_msg = AvailabilityStoreMessage::StoreChunk {
			candidate_hash,
			relay_parent,
			validator_index,
			chunk: chunk.clone(),
			tx,
		};

		overseer_send(&mut virtual_overseer, chunk_msg.into()).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::BlockNumber(
				hash,
				tx,
			)) => {
				assert_eq!(hash, relay_parent);
				tx.send(Ok(Some(4))).unwrap();
			}
		);

		rx.await.unwrap().unwrap();

		// At this point data should be in the store.
		assert_eq!(
			query_chunk(&mut virtual_overseer, candidate_hash, validator_index).await.unwrap(),
			chunk,
		);

		// Wait for twice as long as the stored block kept for.
		Delay::new(test_state.pruning_config.keep_stored_block_for * 2).await;

		// The block was not included by this point so it should be pruned now.
		assert!(query_chunk(&mut virtual_overseer, candidate_hash, validator_index).await.is_none());
	});
}

#[test]
fn stored_but_not_included_data_is_pruned() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();

	test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let candidate_hash = Hash::repeat_byte(1);
		let n_validators = 10;

		let pov = PoV {
			block_data: BlockData(vec![4, 5, 6]),
		};

		let available_data = AvailableData {
			pov,
			validation_data: test_state.persisted_validation_data,
		};

		let (tx, rx) = oneshot::channel();
		let block_msg = AvailabilityStoreMessage::StoreAvailableData(
			candidate_hash,
			None,
			n_validators,
			available_data.clone(),
			tx,
		);

		virtual_overseer.send(FromOverseer::Communication{ msg: block_msg }).await;

		rx.await.unwrap().unwrap();

		// At this point data should be in the store.
		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
			available_data,
		);

		// Wait for twice as long as the stored block kept for.
		Delay::new(test_state.pruning_config.keep_stored_block_for * 2).await;

		// The block was not included by this point so it should be pruned now.
		assert!(query_available_data(&mut virtual_overseer, candidate_hash).await.is_none());
	});
}

#[test]
fn stored_data_kept_until_finalized() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();

	test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let n_validators = 10;

		let pov = PoV {
			block_data: BlockData(vec![4, 5, 6]),
		};

		let pov_hash = pov.hash();

		let candidate = TestCandidateBuilder {
			pov_hash,
			..Default::default()
		}.build();

		let candidate_hash = candidate.hash();

		let available_data = AvailableData {
			pov,
			validation_data: test_state.persisted_validation_data,
		};

		let (tx, rx) = oneshot::channel();
		let block_msg = AvailabilityStoreMessage::StoreAvailableData(
			candidate_hash,
			None,
			n_validators,
			available_data.clone(),
			tx,
		);

		virtual_overseer.send(FromOverseer::Communication{ msg: block_msg }).await;

		rx.await.unwrap().unwrap();

		// At this point data should be in the store.
		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
			available_data,
		);

		let new_leaf = Hash::repeat_byte(2);
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![new_leaf.clone()],
				deactivated: smallvec![],
			}),
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::CandidateEvents(tx),
			)) => {
				assert_eq!(relay_parent, new_leaf);
				tx.send(Ok(vec![
					CandidateEvent::CandidateIncluded(candidate, HeadData::default()),
				])).unwrap();
			}
		);

		Delay::new(test_state.pruning_config.keep_stored_block_for * 10).await;

		// At this point data should _still_ be in the store.
		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
			available_data,
		);

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::BlockFinalized(new_leaf)
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::BlockNumber(
				hash,
				tx,
			)) => {
				assert_eq!(hash, new_leaf);
				tx.send(Ok(Some(10))).unwrap();
			}
		);

		// Wait for a half of the time finalized data should be available for
		Delay::new(test_state.pruning_config.keep_finalized_block_for / 2).await;

		// At this point data should _still_ be in the store.
		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
			available_data,
		);

		// Wait until it is should be gone.
		Delay::new(test_state.pruning_config.keep_finalized_block_for).await;

		// At this point data should be gone from the store.
		assert!(
			query_available_data(&mut virtual_overseer, candidate_hash).await.is_none(),
		);
	});
}

#[test]
fn stored_chunk_kept_until_finalized() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();

	test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let relay_parent = Hash::repeat_byte(2);
		let validator_index = 5;
		let candidate = TestCandidateBuilder {
			..Default::default()
		}.build();
		let candidate_hash = candidate.hash();

		let chunk = ErasureChunk {
			chunk: vec![1, 2, 3],
			index: validator_index,
			proof: vec![vec![3, 4, 5]],
		};

		let (tx, rx) = oneshot::channel();
		let chunk_msg = AvailabilityStoreMessage::StoreChunk {
			candidate_hash,
			relay_parent,
			validator_index,
			chunk: chunk.clone(),
			tx,
		};

		overseer_send(&mut virtual_overseer, chunk_msg.into()).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::BlockNumber(
				hash,
				tx,
			)) => {
				assert_eq!(hash, relay_parent);
				tx.send(Ok(Some(4))).unwrap();
			}
		);

		rx.await.unwrap().unwrap();

		// At this point data should be in the store.
		assert_eq!(
			query_chunk(&mut virtual_overseer, candidate_hash, validator_index).await.unwrap(),
			chunk,
		);

		let new_leaf = Hash::repeat_byte(2);
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![new_leaf.clone()],
				deactivated: smallvec![],
			}),
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::CandidateEvents(tx),
			)) => {
				assert_eq!(relay_parent, new_leaf);
				tx.send(Ok(vec![
					CandidateEvent::CandidateIncluded(candidate, HeadData::default()),
				])).unwrap();
			}
		);

		Delay::new(test_state.pruning_config.keep_stored_block_for * 10).await;

		// At this point data should _still_ be in the store.
		assert_eq!(
			query_chunk(&mut virtual_overseer, candidate_hash, validator_index).await.unwrap(),
			chunk,
		);

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::BlockFinalized(new_leaf)
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::BlockNumber(
				hash,
				tx,
			)) => {
				assert_eq!(hash, new_leaf);
				tx.send(Ok(Some(10))).unwrap();
			}
		);

		// Wait for a half of the time finalized data should be available for
		Delay::new(test_state.pruning_config.keep_finalized_block_for / 2).await;

		// At this point data should _still_ be in the store.
		assert_eq!(
			query_chunk(&mut virtual_overseer, candidate_hash, validator_index).await.unwrap(),
			chunk,
		);

		// Wait until it is should be gone.
		Delay::new(test_state.pruning_config.keep_finalized_chunk_for).await;

		// At this point data should be gone from the store.
		assert!(
			query_available_data(&mut virtual_overseer, candidate_hash).await.is_none(),
		);
	});
}

#[test]
fn forkfullness_works() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();

	test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let n_validators = 10;

		let pov_1 = PoV {
			block_data: BlockData(vec![1, 2, 3]),
		};

		let pov_1_hash = pov_1.hash();

		let pov_2 = PoV {
			block_data: BlockData(vec![4, 5, 6]),
		};

		let pov_2_hash = pov_2.hash();

		let candidate_1 = TestCandidateBuilder {
			pov_hash: pov_1_hash,
			..Default::default()
		}.build();

		let candidate_1_hash = candidate_1.hash();

		let candidate_2 = TestCandidateBuilder {
			pov_hash: pov_2_hash,
			..Default::default()
		}.build();

		let candidate_2_hash = candidate_2.hash();

		let available_data_1 = AvailableData {
			pov: pov_1,
			validation_data: test_state.persisted_validation_data.clone(),
		};

		let available_data_2 = AvailableData {
			pov: pov_2,
			validation_data: test_state.persisted_validation_data,
		};

		let (tx, rx) = oneshot::channel();
		let msg = AvailabilityStoreMessage::StoreAvailableData(
			candidate_1_hash,
			None,
			n_validators,
			available_data_1.clone(),
			tx,
		);

		virtual_overseer.send(FromOverseer::Communication{ msg }).await;

		rx.await.unwrap().unwrap();

		let (tx, rx) = oneshot::channel();
		let msg = AvailabilityStoreMessage::StoreAvailableData(
			candidate_2_hash,
			None,
			n_validators,
			available_data_2.clone(),
			tx,
		);

		virtual_overseer.send(FromOverseer::Communication{ msg }).await;

		rx.await.unwrap().unwrap();

		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_1_hash).await.unwrap(),
			available_data_1,
		);

		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_2_hash).await.unwrap(),
			available_data_2,
		);


		let new_leaf_1 = Hash::repeat_byte(2);
		let new_leaf_2 = Hash::repeat_byte(3);

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![new_leaf_1.clone(), new_leaf_2.clone()],
				deactivated: smallvec![],
			}),
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				leaf,
				RuntimeApiRequest::CandidateEvents(tx),
			)) => {
				assert_eq!(leaf, new_leaf_1);
				tx.send(Ok(vec![
					CandidateEvent::CandidateIncluded(candidate_1, HeadData::default()),
				])).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				leaf,
				RuntimeApiRequest::CandidateEvents(tx),
			)) => {
				assert_eq!(leaf, new_leaf_2);
				tx.send(Ok(vec![
					CandidateEvent::CandidateIncluded(candidate_2, HeadData::default()),
				])).unwrap();
			}
		);

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::BlockFinalized(new_leaf_1)
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::BlockNumber(
				hash,
				tx,
			)) => {
				assert_eq!(hash, new_leaf_1);
				tx.send(Ok(Some(5))).unwrap();
			}
		);


		// Data of both candidates should be still present in the DB.
		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_1_hash).await.unwrap(),
			available_data_1,
		);

		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_2_hash).await.unwrap(),
			available_data_2,
		);
		// Wait for longer than finalized blocks should be kept for
		Delay::new(test_state.pruning_config.keep_finalized_block_for + Duration::from_secs(1)).await;

		// Data of both candidates should be gone now.
		assert!(
			query_available_data(&mut virtual_overseer, candidate_1_hash).await.is_none(),
		);

		assert!(
			query_available_data(&mut virtual_overseer, candidate_2_hash).await.is_none(),
		);
	});
}

async fn query_available_data(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	candidate_hash: Hash,
) -> Option<AvailableData> {
	let (tx, rx) = oneshot::channel();

	let query = AvailabilityStoreMessage::QueryAvailableData(candidate_hash, tx);
	virtual_overseer.send(FromOverseer::Communication{ msg: query }).await;

	rx.await.unwrap()
}

async fn query_chunk(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	candidate_hash: Hash,
	index: u32,
) -> Option<ErasureChunk> {
	let (tx, rx) = oneshot::channel();

	let query = AvailabilityStoreMessage::QueryChunk(candidate_hash, index, tx);
	virtual_overseer.send(FromOverseer::Communication{ msg: query }).await;

	rx.await.unwrap()
}
