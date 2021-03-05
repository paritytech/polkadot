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

use polkadot_primitives::v1::{
	AvailableData, BlockData, CandidateDescriptor, CandidateReceipt, HeadData,
	PersistedValidationData, PoV, Id as ParaId, CandidateHash, Header, ValidatorId,
	CoreIndex, GroupIndex,
};
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_subsystem::{
	ActiveLeavesUpdate, errors::RuntimeApiError, jaeger, messages::AllMessages,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use sp_keyring::Sr25519Keyring;
use parking_lot::Mutex;

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

#[derive(Clone)]
struct TestClock {
	inner: Arc<Mutex<Duration>>,
}

impl TestClock {
	fn now(&self) -> Duration {
		self.inner.lock().clone()
	}

	fn inc(&self, by: Duration) {
		*self.inner.lock() += by;
	}
}

impl Clock for TestClock {
	fn now(&self) -> Result<Duration, Error> {
		Ok(TestClock::now(self))
	}
}


#[derive(Clone)]
struct TestState {
	persisted_validation_data: PersistedValidationData,
	pruning_config: PruningConfig,
	clock: TestClock,
}

impl TestState {
	// pruning is only polled periodically, so we sometimes need to delay until
	// we're sure the subsystem has done pruning.
	async fn wait_for_pruning(&self) {
		Delay::new(self.pruning_config.pruning_interval * 2).await
	}
}

impl Default for TestState {
	fn default() -> Self {
		let persisted_validation_data = PersistedValidationData {
			parent_head: HeadData(vec![7, 8, 9]),
			relay_parent_number: 5,
			max_pov_size: 1024,
			relay_parent_storage_root: Default::default(),
		};

		let pruning_config = PruningConfig {
			keep_unavailable_for: Duration::from_secs(1),
			keep_finalized_for: Duration::from_secs(2),
			pruning_interval: Duration::from_millis(250),
		};

		let clock = TestClock {
			inner: Arc::new(Mutex::new(Duration::from_secs(0))),
		};

		Self {
			persisted_validation_data,
			pruning_config,
			clock,
		}
	}
}


fn test_harness<T: Future<Output=()>>(
	state: TestState,
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

	let subsystem = AvailabilityStoreSubsystem::new_in_memory(
		store,
		state.pruning_config.clone(),
		Box::new(state.clock),
	);

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
	tracing::trace!(meg = ?msg, "sending message");
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

	tracing::trace!(msg = ?msg, "received message");

	msg
}

async fn overseer_recv_with_timeout(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	timeout: Duration,
) -> Option<AllMessages> {
	tracing::trace!("waiting for message...");
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

fn with_tx(db: &Arc<impl KeyValueDB>, f: impl FnOnce(&mut DBTransaction)) {
	let mut tx = DBTransaction::new();
	f(&mut tx);
	db.write(tx).unwrap();
}

fn candidate_included(receipt: CandidateReceipt) -> CandidateEvent {
	CandidateEvent::CandidateIncluded(
		receipt,
		HeadData::default(),
		CoreIndex::default(),
		GroupIndex::default(),
	)
}

#[test]
fn runtime_api_error_does_not_stop_the_subsystem() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));

	test_harness(TestState::default(), store, |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let new_leaf = Hash::repeat_byte(0x01);

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: vec![(new_leaf, Arc::new(jaeger::Span::Disabled))].into(),
				deactivated: vec![].into(),
			}),
		).await;

		// runtime api call fails
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::CandidateEvents(tx),
			)) => {
				assert_eq!(relay_parent, new_leaf);
				tx.send(Err(RuntimeApiError::from("oh no".to_string()))).unwrap();
			}
		);

		// but that's fine, we're still alive
		let (tx, rx) = oneshot::channel();
		let candidate_hash = CandidateHash(Hash::repeat_byte(33));
		let validator_index = ValidatorIndex(5);
		let query_chunk = AvailabilityStoreMessage::QueryChunk(
			candidate_hash,
			validator_index,
			tx,
		);

		overseer_send(&mut virtual_overseer, query_chunk.into()).await;

		assert!(rx.await.unwrap().is_none());

	});
}

#[test]
fn store_chunk_works() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	test_harness(TestState::default(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let relay_parent = Hash::repeat_byte(32);
		let candidate_hash = CandidateHash(Hash::repeat_byte(33));
		let validator_index = ValidatorIndex(5);
		let n_validators = 10;

		let chunk = ErasureChunk {
			chunk: vec![1, 2, 3],
			index: validator_index,
			proof: vec![vec![3, 4, 5]],
		};

		// Ensure an entry already exists. In reality this would come from watching
		// chain events.
		with_tx(&store, |tx| {
			super::write_meta(tx, &candidate_hash, &CandidateMeta {
				data_available: false,
				chunks_stored: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
				state: State::Unavailable(BETimestamp(0)),
			});
		});

		let (tx, rx) = oneshot::channel();

		let chunk_msg = AvailabilityStoreMessage::StoreChunk {
			candidate_hash,
			relay_parent,
			chunk: chunk.clone(),
			tx,
		};

		overseer_send(&mut virtual_overseer, chunk_msg.into()).await;
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
fn store_chunk_does_nothing_if_no_entry_already() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	test_harness(TestState::default(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let relay_parent = Hash::repeat_byte(32);
		let candidate_hash = CandidateHash(Hash::repeat_byte(33));
		let validator_index = ValidatorIndex(5);

		let chunk = ErasureChunk {
			chunk: vec![1, 2, 3],
			index: validator_index,
			proof: vec![vec![3, 4, 5]],
		};

		let (tx, rx) = oneshot::channel();

		let chunk_msg = AvailabilityStoreMessage::StoreChunk {
			candidate_hash,
			relay_parent,
			chunk: chunk.clone(),
			tx,
		};

		overseer_send(&mut virtual_overseer, chunk_msg.into()).await;
		assert_eq!(rx.await.unwrap(), Err(()));

		let (tx, rx) = oneshot::channel();
		let query_chunk = AvailabilityStoreMessage::QueryChunk(
			candidate_hash,
			validator_index,
			tx,
		);

		overseer_send(&mut virtual_overseer, query_chunk.into()).await;

		assert!(rx.await.unwrap().is_none());
	});
}

#[test]
fn query_chunk_checks_meta() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	test_harness(TestState::default(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let candidate_hash = CandidateHash(Hash::repeat_byte(33));
		let validator_index = ValidatorIndex(5);
		let n_validators = 10;

		// Ensure an entry already exists. In reality this would come from watching
		// chain events.
		with_tx(&store, |tx| {
			super::write_meta(tx, &candidate_hash, &CandidateMeta {
				data_available: false,
				chunks_stored: {
					let mut v = bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators];
					v.set(validator_index.0 as usize, true);
					v
				},
				state: State::Unavailable(BETimestamp(0)),
			});
		});

		let (tx, rx) = oneshot::channel();
		let query_chunk = AvailabilityStoreMessage::QueryChunkAvailability(
			candidate_hash,
			validator_index,
			tx,
		);

		overseer_send(&mut virtual_overseer, query_chunk.into()).await;
		assert!(rx.await.unwrap());

		let (tx, rx) = oneshot::channel();
		let query_chunk = AvailabilityStoreMessage::QueryChunkAvailability(
			candidate_hash,
			ValidatorIndex(validator_index.0 + 1),
			tx,
		);

		overseer_send(&mut virtual_overseer, query_chunk.into()).await;
		assert!(!rx.await.unwrap());
	});
}

#[test]
fn store_block_works() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();
	test_harness(test_state.clone(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let candidate_hash = CandidateHash(Hash::repeat_byte(1));
		let validator_index = ValidatorIndex(5);
		let n_validators = 10;

		let pov = PoV {
			block_data: BlockData(vec![4, 5, 6]),
		};

		let available_data = AvailableData {
			pov: Arc::new(pov),
			validation_data: test_state.persisted_validation_data.clone(),
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
			index: ValidatorIndex(5),
			proof: branch.0,
		};

		assert_eq!(chunk, expected_chunk);
	});
}

#[test]
fn store_pov_and_query_chunk_works() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();

	test_harness(test_state.clone(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let candidate_hash = CandidateHash(Hash::repeat_byte(1));
		let n_validators = 10;

		let pov = PoV {
			block_data: BlockData(vec![4, 5, 6]),
		};

		let available_data = AvailableData {
			pov: Arc::new(pov),
			validation_data: test_state.persisted_validation_data.clone(),
		};

		let chunks_expected = erasure::obtain_chunks_v1(n_validators as _, &available_data).unwrap();

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

		for i in 0..n_validators {
			let chunk = query_chunk(&mut virtual_overseer, candidate_hash, ValidatorIndex(i as _)).await.unwrap();

			assert_eq!(chunk.chunk, chunks_expected[i as usize]);
		}
	});
}

#[test]
fn stored_but_not_included_data_is_pruned() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();

	test_harness(test_state.clone(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let candidate_hash = CandidateHash(Hash::repeat_byte(1));
		let n_validators = 10;

		let pov = PoV {
			block_data: BlockData(vec![4, 5, 6]),
		};

		let available_data = AvailableData {
			pov: Arc::new(pov),
			validation_data: test_state.persisted_validation_data.clone(),
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

		// Wait until pruning.
		test_state.clock.inc(test_state.pruning_config.keep_unavailable_for);
		test_state.wait_for_pruning().await;

		// The block was not included by this point so it should be pruned now.
		assert!(query_available_data(&mut virtual_overseer, candidate_hash).await.is_none());
	});
}

#[test]
fn stored_data_kept_until_finalized() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();

	test_harness(test_state.clone(), store.clone(), |test_harness| async move {
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
			pov: Arc::new(pov),
			validation_data: test_state.persisted_validation_data.clone(),
		};

		let parent = Hash::repeat_byte(2);
		let block_number = 10;

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

		let new_leaf = import_leaf(
			&mut virtual_overseer,
			parent,
			block_number,
			vec![candidate_included(candidate)],
			(0..n_validators).map(|_| Sr25519Keyring::Alice.public().into()).collect(),
		).await;

		// Wait until unavailable data would definitely be pruned.
		test_state.clock.inc(test_state.pruning_config.keep_unavailable_for * 10);
		test_state.wait_for_pruning().await;

		// At this point data should _still_ be in the store.
		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
			available_data,
		);

		assert!(
			query_all_chunks(&mut virtual_overseer, candidate_hash, n_validators, true).await
		);

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::BlockFinalized(new_leaf, block_number)
		).await;

		// Wait until unavailable data would definitely be pruned.
		test_state.clock.inc(test_state.pruning_config.keep_finalized_for / 2);
		test_state.wait_for_pruning().await;

		// At this point data should _still_ be in the store.
		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
			available_data,
		);

		assert!(
			query_all_chunks(&mut virtual_overseer, candidate_hash, n_validators, true).await
		);

		// Wait until it definitely should be gone.
		test_state.clock.inc(test_state.pruning_config.keep_finalized_for);
		test_state.wait_for_pruning().await;

		// At this point data should be gone from the store.
		assert!(
			query_available_data(&mut virtual_overseer, candidate_hash).await.is_none(),
		);

		assert!(
			query_all_chunks(&mut virtual_overseer, candidate_hash, n_validators, false).await
		);
	});
}

#[test]
fn forkfullness_works() {
	let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
	let test_state = TestState::default();

	test_harness(test_state.clone(), store.clone(), |test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;
		let n_validators = 10;
		let block_number_1 = 5;
		let block_number_2 = 5;
		let validators: Vec<_> = (0..n_validators).map(|_| Sr25519Keyring::Alice.public().into()).collect();
		let parent_1 = Hash::repeat_byte(3);
		let parent_2 = Hash::repeat_byte(4);

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
			pov: Arc::new(pov_1),
			validation_data: test_state.persisted_validation_data.clone(),
		};

		let available_data_2 = AvailableData {
			pov: Arc::new(pov_2),
			validation_data: test_state.persisted_validation_data.clone(),
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

		let new_leaf_1 = import_leaf(
			&mut virtual_overseer,
			parent_1,
			block_number_1,
			vec![candidate_included(candidate_1)],
			validators.clone(),
		).await;

		let _new_leaf_2 = import_leaf(
			&mut virtual_overseer,
			parent_2,
			block_number_2,
			vec![candidate_included(candidate_2)],
			validators.clone(),
		).await;

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::BlockFinalized(new_leaf_1, block_number_1)
		).await;

		// Data of both candidates should be still present in the DB.
		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_1_hash).await.unwrap(),
			available_data_1,
		);

		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_2_hash).await.unwrap(),
			available_data_2,
		);

		assert!(
			query_all_chunks(&mut virtual_overseer, candidate_1_hash, n_validators, true).await,
		);

		assert!(
			query_all_chunks(&mut virtual_overseer, candidate_2_hash, n_validators, true).await,
		);

		// Candidate 2 should now be considered unavailable and will be pruned.
		test_state.clock.inc(test_state.pruning_config.keep_unavailable_for);
		test_state.wait_for_pruning().await;

		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_1_hash).await.unwrap(),
			available_data_1,
		);

		assert!(
			query_available_data(&mut virtual_overseer, candidate_2_hash).await.is_none(),
		);

		assert!(
			query_all_chunks(&mut virtual_overseer, candidate_1_hash, n_validators, true).await,
		);

		assert!(
			query_all_chunks(&mut virtual_overseer, candidate_2_hash, n_validators, false).await,
		);

		// Wait for longer than finalized blocks should be kept for
		test_state.clock.inc(test_state.pruning_config.keep_finalized_for);
		test_state.wait_for_pruning().await;

		// Everything should be pruned now.
		assert!(
			query_available_data(&mut virtual_overseer, candidate_1_hash).await.is_none(),
		);

		assert!(
			query_available_data(&mut virtual_overseer, candidate_2_hash).await.is_none(),
		);

		assert!(
			query_all_chunks(&mut virtual_overseer, candidate_1_hash, n_validators, false).await,
		);

		assert!(
			query_all_chunks(&mut virtual_overseer, candidate_2_hash, n_validators, false).await,
		);
	});
}

async fn query_available_data(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	candidate_hash: CandidateHash,
) -> Option<AvailableData> {
	let (tx, rx) = oneshot::channel();

	let query = AvailabilityStoreMessage::QueryAvailableData(candidate_hash, tx);
	virtual_overseer.send(FromOverseer::Communication{ msg: query }).await;

	rx.await.unwrap()
}

async fn query_chunk(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	candidate_hash: CandidateHash,
	index: ValidatorIndex,
) -> Option<ErasureChunk> {
	let (tx, rx) = oneshot::channel();

	let query = AvailabilityStoreMessage::QueryChunk(candidate_hash, index, tx);
	virtual_overseer.send(FromOverseer::Communication{ msg: query }).await;

	rx.await.unwrap()
}

async fn query_all_chunks(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	candidate_hash: CandidateHash,
	n_validators: u32,
	expect_present: bool,
) -> bool {
	for i in 0..n_validators {
		if query_chunk(virtual_overseer, candidate_hash, ValidatorIndex(i)).await.is_some() != expect_present {
			return false
		}
	}
	true
}

async fn import_leaf(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	parent_hash: Hash,
	block_number: BlockNumber,
	events: Vec<CandidateEvent>,
	validators: Vec<ValidatorId>,
) -> Hash {
	let header = Header {
		parent_hash,
		number: block_number,
		state_root: Hash::zero(),
		extrinsics_root: Hash::zero(),
		digest: Default::default(),
	};
	let new_leaf = header.hash();

	overseer_signal(
		virtual_overseer,
		OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
			activated: vec![(new_leaf, Arc::new(jaeger::Span::Disabled))].into(),
			deactivated: vec![].into(),
		}),
	).await;

	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			relay_parent,
			RuntimeApiRequest::CandidateEvents(tx),
		)) => {
			assert_eq!(relay_parent, new_leaf);
			tx.send(Ok(events)).unwrap();
		}
	);

	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::ChainApi(ChainApiMessage::BlockNumber(
			relay_parent,
			tx,
		)) => {
			assert_eq!(relay_parent, new_leaf);
			tx.send(Ok(Some(block_number))).unwrap();
		}
	);

	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::ChainApi(ChainApiMessage::BlockHeader(
			relay_parent,
			tx,
		)) => {
			assert_eq!(relay_parent, new_leaf);
			tx.send(Ok(Some(header))).unwrap();
		}
	);

	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			relay_parent,
			RuntimeApiRequest::Validators(tx),
		)) => {
			assert_eq!(relay_parent, parent_hash);
			tx.send(Ok(validators)).unwrap();
		}
	);

	new_leaf
}
