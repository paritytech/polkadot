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
use futures::{channel::oneshot, executor, future, Future};

use ::test_helpers::TestCandidateBuilder;
use parking_lot::Mutex;
use polkadot_node_primitives::{AvailableData, BlockData, PoV, Proof};
use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	jaeger,
	messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest},
	ActivatedLeaf, ActiveLeavesUpdate, LeafStatus,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::{database::Database, TimeoutExt};
use polkadot_primitives::v2::{
	CandidateHash, CandidateReceipt, CoreIndex, GroupIndex, HeadData, Header,
	PersistedValidationData, ValidatorId,
};
use sp_keyring::Sr25519Keyring;

mod columns {
	pub const DATA: u32 = 0;
	pub const META: u32 = 1;
	pub const NUM_COLUMNS: u32 = 2;
}

const TEST_CONFIG: Config = Config { col_data: columns::DATA, col_meta: columns::META };

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>;

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

		let clock = TestClock { inner: Arc::new(Mutex::new(Duration::from_secs(0))) };

		Self { persisted_validation_data, pruning_config, clock }
	}
}

fn test_harness<T: Future<Output = VirtualOverseer>>(
	state: TestState,
	store: Arc<dyn Database>,
	test: impl FnOnce(VirtualOverseer) -> T,
) {
	let _ = env_logger::builder()
		.is_test(true)
		.filter(Some("polkadot_node_core_av_store"), log::LevelFilter::Trace)
		.filter(Some(LOG_TARGET), log::LevelFilter::Trace)
		.try_init();

	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = AvailabilityStoreSubsystem::with_pruning_config_and_clock(
		store,
		TEST_CONFIG,
		state.pruning_config.clone(),
		Box::new(state.clock),
		Metrics::default(),
	);

	let subsystem = run(subsystem, context);

	let test_fut = test(virtual_overseer);

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	executor::block_on(future::join(
		async move {
			let mut overseer = test_fut.await;
			overseer_signal(&mut overseer, OverseerSignal::Conclude).await;
		},
		subsystem,
	));
}

const TIMEOUT: Duration = Duration::from_millis(100);

async fn overseer_send(overseer: &mut VirtualOverseer, msg: AvailabilityStoreMessage) {
	gum::trace!(meg = ?msg, "sending message");
	overseer
		.send(FromOrchestra::Communication { msg })
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough for sending messages.", TIMEOUT));
}

async fn overseer_recv(overseer: &mut VirtualOverseer) -> AllMessages {
	let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough to receive messages", TIMEOUT));

	gum::trace!(msg = ?msg, "received message");

	msg
}

async fn overseer_recv_with_timeout(
	overseer: &mut VirtualOverseer,
	timeout: Duration,
) -> Option<AllMessages> {
	gum::trace!("waiting for message...");
	overseer.recv().timeout(timeout).await
}

async fn overseer_signal(overseer: &mut VirtualOverseer, signal: OverseerSignal) {
	overseer
		.send(FromOrchestra::Signal(signal))
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough for sending signals.", TIMEOUT));
}

fn with_tx(db: &Arc<impl Database + ?Sized>, f: impl FnOnce(&mut DBTransaction)) {
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

#[cfg(test)]
fn test_store() -> Arc<dyn Database> {
	let db = kvdb_memorydb::create(columns::NUM_COLUMNS);
	let db =
		polkadot_node_subsystem_util::database::kvdb_impl::DbAdapter::new(db, &[columns::META]);
	Arc::new(db)
}

#[test]
fn runtime_api_error_does_not_stop_the_subsystem() {
	let store = test_store();

	test_harness(TestState::default(), store, |mut virtual_overseer| async move {
		let new_leaf = Hash::repeat_byte(0x01);

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: new_leaf,
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let header = Header {
			parent_hash: Hash::zero(),
			number: 1,
			state_root: Hash::zero(),
			extrinsics_root: Hash::zero(),
			digest: Default::default(),
		};

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::BlockHeader(
				relay_parent,
				tx,
			)) => {
				assert_eq!(relay_parent, new_leaf);
				tx.send(Ok(Some(header))).unwrap();
			}
		);

		// runtime API call fails
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::CandidateEvents(tx),
			)) => {
				assert_eq!(relay_parent, new_leaf);
				#[derive(Debug)]
				struct FauxError;
				impl std::error::Error for FauxError {}
				impl std::fmt::Display for FauxError {
					fn fmt(&self, _f: &mut std::fmt::Formatter) -> std::fmt::Result {
						Ok(())
					}
				}
				tx.send(Err(RuntimeApiError::Execution {
					runtime_api_name: "faux",
					source: Arc::new(FauxError),
				})).unwrap();
			}
		);

		// but that's fine, we're still alive
		let (tx, rx) = oneshot::channel();
		let candidate_hash = CandidateHash(Hash::repeat_byte(33));
		let validator_index = ValidatorIndex(5);
		let query_chunk = AvailabilityStoreMessage::QueryChunk(candidate_hash, validator_index, tx);

		overseer_send(&mut virtual_overseer, query_chunk.into()).await;

		assert!(rx.await.unwrap().is_none());
		virtual_overseer
	});
}

#[test]
fn store_chunk_works() {
	let store = test_store();

	test_harness(TestState::default(), store.clone(), |mut virtual_overseer| async move {
		let candidate_hash = CandidateHash(Hash::repeat_byte(33));
		let validator_index = ValidatorIndex(5);
		let n_validators = 10;

		let chunk = ErasureChunk {
			chunk: vec![1, 2, 3],
			index: validator_index,
			proof: Proof::try_from(vec![vec![3, 4, 5]]).unwrap(),
		};

		// Ensure an entry already exists. In reality this would come from watching
		// chain events.
		with_tx(&store, |tx| {
			super::write_meta(
				tx,
				&TEST_CONFIG,
				&candidate_hash,
				&CandidateMeta {
					data_available: false,
					chunks_stored: bitvec::bitvec![u8, BitOrderLsb0; 0; n_validators],
					state: State::Unavailable(BETimestamp(0)),
				},
			);
		});

		let (tx, rx) = oneshot::channel();

		let chunk_msg =
			AvailabilityStoreMessage::StoreChunk { candidate_hash, chunk: chunk.clone(), tx };

		overseer_send(&mut virtual_overseer, chunk_msg).await;
		assert_eq!(rx.await.unwrap(), Ok(()));

		let (tx, rx) = oneshot::channel();
		let query_chunk = AvailabilityStoreMessage::QueryChunk(candidate_hash, validator_index, tx);

		overseer_send(&mut virtual_overseer, query_chunk).await;

		assert_eq!(rx.await.unwrap().unwrap(), chunk);
		virtual_overseer
	});
}

#[test]
fn store_chunk_does_nothing_if_no_entry_already() {
	let store = test_store();

	test_harness(TestState::default(), store.clone(), |mut virtual_overseer| async move {
		let candidate_hash = CandidateHash(Hash::repeat_byte(33));
		let validator_index = ValidatorIndex(5);

		let chunk = ErasureChunk {
			chunk: vec![1, 2, 3],
			index: validator_index,
			proof: Proof::try_from(vec![vec![3, 4, 5]]).unwrap(),
		};

		let (tx, rx) = oneshot::channel();

		let chunk_msg =
			AvailabilityStoreMessage::StoreChunk { candidate_hash, chunk: chunk.clone(), tx };

		overseer_send(&mut virtual_overseer, chunk_msg).await;
		assert_eq!(rx.await.unwrap(), Err(()));

		let (tx, rx) = oneshot::channel();
		let query_chunk = AvailabilityStoreMessage::QueryChunk(candidate_hash, validator_index, tx);

		overseer_send(&mut virtual_overseer, query_chunk).await;

		assert!(rx.await.unwrap().is_none());
		virtual_overseer
	});
}

#[test]
fn query_chunk_checks_meta() {
	let store = test_store();

	test_harness(TestState::default(), store.clone(), |mut virtual_overseer| async move {
		let candidate_hash = CandidateHash(Hash::repeat_byte(33));
		let validator_index = ValidatorIndex(5);
		let n_validators = 10;

		// Ensure an entry already exists. In reality this would come from watching
		// chain events.
		with_tx(&store, |tx| {
			super::write_meta(
				tx,
				&TEST_CONFIG,
				&candidate_hash,
				&CandidateMeta {
					data_available: false,
					chunks_stored: {
						let mut v = bitvec::bitvec![u8, BitOrderLsb0; 0; n_validators];
						v.set(validator_index.0 as usize, true);
						v
					},
					state: State::Unavailable(BETimestamp(0)),
				},
			);
		});

		let (tx, rx) = oneshot::channel();
		let query_chunk =
			AvailabilityStoreMessage::QueryChunkAvailability(candidate_hash, validator_index, tx);

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
		virtual_overseer
	});
}

#[test]
fn store_block_works() {
	let store = test_store();
	let test_state = TestState::default();
	test_harness(test_state.clone(), store.clone(), |mut virtual_overseer| async move {
		let candidate_hash = CandidateHash(Hash::repeat_byte(1));
		let validator_index = ValidatorIndex(5);
		let n_validators = 10;

		let pov = PoV { block_data: BlockData(vec![4, 5, 6]) };

		let available_data = AvailableData {
			pov: Arc::new(pov),
			validation_data: test_state.persisted_validation_data.clone(),
		};

		let (tx, rx) = oneshot::channel();
		let block_msg = AvailabilityStoreMessage::StoreAvailableData {
			candidate_hash,
			n_validators,
			available_data: available_data.clone(),
			tx,
		};

		virtual_overseer.send(FromOrchestra::Communication { msg: block_msg }).await;
		assert_eq!(rx.await.unwrap(), Ok(()));

		let pov = query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap();
		assert_eq!(pov, available_data);

		let chunk = query_chunk(&mut virtual_overseer, candidate_hash, validator_index)
			.await
			.unwrap();

		let chunks = erasure::obtain_chunks_v1(10, &available_data).unwrap();

		let mut branches = erasure::branches(chunks.as_ref());

		let branch = branches.nth(5).unwrap();
		let expected_chunk = ErasureChunk {
			chunk: branch.1.to_vec(),
			index: ValidatorIndex(5),
			proof: Proof::try_from(branch.0).unwrap(),
		};

		assert_eq!(chunk, expected_chunk);
		virtual_overseer
	});
}

#[test]
fn store_pov_and_query_chunk_works() {
	let store = test_store();
	let test_state = TestState::default();

	test_harness(test_state.clone(), store.clone(), |mut virtual_overseer| async move {
		let candidate_hash = CandidateHash(Hash::repeat_byte(1));
		let n_validators = 10;

		let pov = PoV { block_data: BlockData(vec![4, 5, 6]) };

		let available_data = AvailableData {
			pov: Arc::new(pov),
			validation_data: test_state.persisted_validation_data.clone(),
		};

		let chunks_expected =
			erasure::obtain_chunks_v1(n_validators as _, &available_data).unwrap();

		let (tx, rx) = oneshot::channel();
		let block_msg = AvailabilityStoreMessage::StoreAvailableData {
			candidate_hash,
			n_validators,
			available_data,
			tx,
		};

		virtual_overseer.send(FromOrchestra::Communication { msg: block_msg }).await;

		assert_eq!(rx.await.unwrap(), Ok(()));

		for i in 0..n_validators {
			let chunk = query_chunk(&mut virtual_overseer, candidate_hash, ValidatorIndex(i as _))
				.await
				.unwrap();

			assert_eq!(chunk.chunk, chunks_expected[i as usize]);
		}
		virtual_overseer
	});
}

#[test]
fn query_all_chunks_works() {
	let store = test_store();
	let test_state = TestState::default();

	test_harness(test_state.clone(), store.clone(), |mut virtual_overseer| async move {
		// all chunks for hash 1.
		// 1 chunk for hash 2.
		// 0 chunks for hash 3.
		let candidate_hash_1 = CandidateHash(Hash::repeat_byte(1));
		let candidate_hash_2 = CandidateHash(Hash::repeat_byte(2));
		let candidate_hash_3 = CandidateHash(Hash::repeat_byte(3));

		let n_validators = 10;

		let pov = PoV { block_data: BlockData(vec![4, 5, 6]) };

		let available_data = AvailableData {
			pov: Arc::new(pov),
			validation_data: test_state.persisted_validation_data.clone(),
		};

		{
			let (tx, rx) = oneshot::channel();
			let block_msg = AvailabilityStoreMessage::StoreAvailableData {
				candidate_hash: candidate_hash_1,
				n_validators,
				available_data,
				tx,
			};

			virtual_overseer.send(FromOrchestra::Communication { msg: block_msg }).await;
			assert_eq!(rx.await.unwrap(), Ok(()));
		}

		{
			with_tx(&store, |tx| {
				super::write_meta(
					tx,
					&TEST_CONFIG,
					&candidate_hash_2,
					&CandidateMeta {
						data_available: false,
						chunks_stored: bitvec::bitvec![u8, BitOrderLsb0; 0; n_validators as _],
						state: State::Unavailable(BETimestamp(0)),
					},
				);
			});

			let chunk = ErasureChunk {
				chunk: vec![1, 2, 3],
				index: ValidatorIndex(1),
				proof: Proof::try_from(vec![vec![3, 4, 5]]).unwrap(),
			};

			let (tx, rx) = oneshot::channel();
			let store_chunk_msg = AvailabilityStoreMessage::StoreChunk {
				candidate_hash: candidate_hash_2,
				chunk,
				tx,
			};

			virtual_overseer
				.send(FromOrchestra::Communication { msg: store_chunk_msg })
				.await;
			assert_eq!(rx.await.unwrap(), Ok(()));
		}

		{
			let (tx, rx) = oneshot::channel();

			let msg = AvailabilityStoreMessage::QueryAllChunks(candidate_hash_1, tx);
			virtual_overseer.send(FromOrchestra::Communication { msg }).await;
			assert_eq!(rx.await.unwrap().len(), n_validators as usize);
		}

		{
			let (tx, rx) = oneshot::channel();

			let msg = AvailabilityStoreMessage::QueryAllChunks(candidate_hash_2, tx);
			virtual_overseer.send(FromOrchestra::Communication { msg }).await;
			assert_eq!(rx.await.unwrap().len(), 1);
		}

		{
			let (tx, rx) = oneshot::channel();

			let msg = AvailabilityStoreMessage::QueryAllChunks(candidate_hash_3, tx);
			virtual_overseer.send(FromOrchestra::Communication { msg }).await;
			assert_eq!(rx.await.unwrap().len(), 0);
		}
		virtual_overseer
	});
}

#[test]
fn stored_but_not_included_data_is_pruned() {
	let store = test_store();
	let test_state = TestState::default();

	test_harness(test_state.clone(), store.clone(), |mut virtual_overseer| async move {
		let candidate_hash = CandidateHash(Hash::repeat_byte(1));
		let n_validators = 10;

		let pov = PoV { block_data: BlockData(vec![4, 5, 6]) };

		let available_data = AvailableData {
			pov: Arc::new(pov),
			validation_data: test_state.persisted_validation_data.clone(),
		};

		let (tx, rx) = oneshot::channel();
		let block_msg = AvailabilityStoreMessage::StoreAvailableData {
			candidate_hash,
			n_validators,
			available_data: available_data.clone(),
			tx,
		};

		virtual_overseer.send(FromOrchestra::Communication { msg: block_msg }).await;

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
		virtual_overseer
	});
}

#[test]
fn stored_data_kept_until_finalized() {
	let store = test_store();
	let test_state = TestState::default();

	test_harness(test_state.clone(), store.clone(), |mut virtual_overseer| async move {
		let n_validators = 10;

		let pov = PoV { block_data: BlockData(vec![4, 5, 6]) };

		let pov_hash = pov.hash();

		let candidate = TestCandidateBuilder { pov_hash, ..Default::default() }.build();

		let candidate_hash = candidate.hash();

		let available_data = AvailableData {
			pov: Arc::new(pov),
			validation_data: test_state.persisted_validation_data.clone(),
		};

		let parent = Hash::repeat_byte(2);
		let block_number = 10;

		let (tx, rx) = oneshot::channel();
		let block_msg = AvailabilityStoreMessage::StoreAvailableData {
			candidate_hash,
			n_validators,
			available_data: available_data.clone(),
			tx,
		};

		virtual_overseer.send(FromOrchestra::Communication { msg: block_msg }).await;

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
		)
		.await;

		// Wait until unavailable data would definitely be pruned.
		test_state.clock.inc(test_state.pruning_config.keep_unavailable_for * 10);
		test_state.wait_for_pruning().await;

		// At this point data should _still_ be in the store.
		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
			available_data,
		);

		assert!(has_all_chunks(&mut virtual_overseer, candidate_hash, n_validators, true).await);

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::BlockFinalized(new_leaf, block_number),
		)
		.await;

		// Wait until unavailable data would definitely be pruned.
		test_state.clock.inc(test_state.pruning_config.keep_finalized_for / 2);
		test_state.wait_for_pruning().await;

		// At this point data should _still_ be in the store.
		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
			available_data,
		);

		assert!(has_all_chunks(&mut virtual_overseer, candidate_hash, n_validators, true).await);

		// Wait until it definitely should be gone.
		test_state.clock.inc(test_state.pruning_config.keep_finalized_for);
		test_state.wait_for_pruning().await;

		// At this point data should be gone from the store.
		assert!(query_available_data(&mut virtual_overseer, candidate_hash).await.is_none());

		assert!(has_all_chunks(&mut virtual_overseer, candidate_hash, n_validators, false).await);
		virtual_overseer
	});
}

#[test]
fn we_dont_miss_anything_if_import_notifications_are_missed() {
	let store = test_store();
	let test_state = TestState::default();

	test_harness(test_state.clone(), store.clone(), |mut virtual_overseer| async move {
		overseer_signal(&mut virtual_overseer, OverseerSignal::BlockFinalized(Hash::zero(), 1))
			.await;

		let header = Header {
			parent_hash: Hash::repeat_byte(3),
			number: 4,
			state_root: Hash::zero(),
			extrinsics_root: Hash::zero(),
			digest: Default::default(),
		};
		let new_leaf = Hash::repeat_byte(4);

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: new_leaf,
				number: 4,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::BlockHeader(
				relay_parent,
				tx,
			)) => {
				assert_eq!(relay_parent, new_leaf);
				tx.send(Ok(Some(header))).unwrap();
			}
		);

		let new_heads = vec![
			(Hash::repeat_byte(2), Hash::repeat_byte(1)),
			(Hash::repeat_byte(3), Hash::repeat_byte(2)),
			(Hash::repeat_byte(4), Hash::repeat_byte(3)),
		];

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::Ancestors {
				hash,
				k,
				response_channel: tx,
			}) => {
				assert_eq!(hash, new_leaf);
				assert_eq!(k, 2);
				let _ = tx.send(Ok(vec![
					Hash::repeat_byte(3),
					Hash::repeat_byte(2),
				]));
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::BlockHeader(
				relay_parent,
				tx,
			)) => {
				assert_eq!(relay_parent, Hash::repeat_byte(3));
				tx.send(Ok(Some(Header {
					parent_hash: Hash::repeat_byte(2),
					number: 3,
					state_root: Hash::zero(),
					extrinsics_root: Hash::zero(),
					digest: Default::default(),
				}))).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::BlockHeader(
				relay_parent,
				tx,
			)) => {
				assert_eq!(relay_parent, Hash::repeat_byte(2));
				tx.send(Ok(Some(Header {
					parent_hash: Hash::repeat_byte(1),
					number: 2,
					state_root: Hash::zero(),
					extrinsics_root: Hash::zero(),
					digest: Default::default(),
				}))).unwrap();
			}
		);

		for (head, parent) in new_heads {
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::CandidateEvents(tx),
				)) => {
					assert_eq!(relay_parent, head);
					tx.send(Ok(Vec::new())).unwrap();
				}
			);

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::Validators(tx),
				)) => {
					assert_eq!(relay_parent, parent);
					tx.send(Ok(Vec::new())).unwrap();
				}
			);
		}

		virtual_overseer
	});
}

#[test]
fn forkfullness_works() {
	let store = test_store();
	let test_state = TestState::default();

	test_harness(test_state.clone(), store.clone(), |mut virtual_overseer| async move {
		let n_validators = 10;
		let block_number_1 = 5;
		let block_number_2 = 5;
		let validators: Vec<_> =
			(0..n_validators).map(|_| Sr25519Keyring::Alice.public().into()).collect();
		let parent_1 = Hash::repeat_byte(3);
		let parent_2 = Hash::repeat_byte(4);

		let pov_1 = PoV { block_data: BlockData(vec![1, 2, 3]) };

		let pov_1_hash = pov_1.hash();

		let pov_2 = PoV { block_data: BlockData(vec![4, 5, 6]) };

		let pov_2_hash = pov_2.hash();

		let candidate_1 =
			TestCandidateBuilder { pov_hash: pov_1_hash, ..Default::default() }.build();

		let candidate_1_hash = candidate_1.hash();

		let candidate_2 =
			TestCandidateBuilder { pov_hash: pov_2_hash, ..Default::default() }.build();

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
		let msg = AvailabilityStoreMessage::StoreAvailableData {
			candidate_hash: candidate_1_hash,
			n_validators,
			available_data: available_data_1.clone(),
			tx,
		};

		virtual_overseer.send(FromOrchestra::Communication { msg }).await;

		rx.await.unwrap().unwrap();

		let (tx, rx) = oneshot::channel();
		let msg = AvailabilityStoreMessage::StoreAvailableData {
			candidate_hash: candidate_2_hash,
			n_validators,
			available_data: available_data_2.clone(),
			tx,
		};

		virtual_overseer.send(FromOrchestra::Communication { msg }).await;

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
		)
		.await;

		let _new_leaf_2 = import_leaf(
			&mut virtual_overseer,
			parent_2,
			block_number_2,
			vec![candidate_included(candidate_2)],
			validators.clone(),
		)
		.await;

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::BlockFinalized(new_leaf_1, block_number_1),
		)
		.await;

		// Data of both candidates should be still present in the DB.
		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_1_hash).await.unwrap(),
			available_data_1,
		);

		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_2_hash).await.unwrap(),
			available_data_2,
		);

		assert!(has_all_chunks(&mut virtual_overseer, candidate_1_hash, n_validators, true).await);

		assert!(has_all_chunks(&mut virtual_overseer, candidate_2_hash, n_validators, true).await);

		// Candidate 2 should now be considered unavailable and will be pruned.
		test_state.clock.inc(test_state.pruning_config.keep_unavailable_for);
		test_state.wait_for_pruning().await;

		assert_eq!(
			query_available_data(&mut virtual_overseer, candidate_1_hash).await.unwrap(),
			available_data_1,
		);

		assert!(query_available_data(&mut virtual_overseer, candidate_2_hash).await.is_none());

		assert!(has_all_chunks(&mut virtual_overseer, candidate_1_hash, n_validators, true).await);

		assert!(has_all_chunks(&mut virtual_overseer, candidate_2_hash, n_validators, false).await);

		// Wait for longer than finalized blocks should be kept for
		test_state.clock.inc(test_state.pruning_config.keep_finalized_for);
		test_state.wait_for_pruning().await;

		// Everything should be pruned now.
		assert!(query_available_data(&mut virtual_overseer, candidate_1_hash).await.is_none());

		assert!(query_available_data(&mut virtual_overseer, candidate_2_hash).await.is_none());

		assert!(has_all_chunks(&mut virtual_overseer, candidate_1_hash, n_validators, false).await);

		assert!(has_all_chunks(&mut virtual_overseer, candidate_2_hash, n_validators, false).await);
		virtual_overseer
	});
}

async fn query_available_data(
	virtual_overseer: &mut VirtualOverseer,
	candidate_hash: CandidateHash,
) -> Option<AvailableData> {
	let (tx, rx) = oneshot::channel();

	let query = AvailabilityStoreMessage::QueryAvailableData(candidate_hash, tx);
	virtual_overseer.send(FromOrchestra::Communication { msg: query }).await;

	rx.await.unwrap()
}

async fn query_chunk(
	virtual_overseer: &mut VirtualOverseer,
	candidate_hash: CandidateHash,
	index: ValidatorIndex,
) -> Option<ErasureChunk> {
	let (tx, rx) = oneshot::channel();

	let query = AvailabilityStoreMessage::QueryChunk(candidate_hash, index, tx);
	virtual_overseer.send(FromOrchestra::Communication { msg: query }).await;

	rx.await.unwrap()
}

async fn has_all_chunks(
	virtual_overseer: &mut VirtualOverseer,
	candidate_hash: CandidateHash,
	n_validators: u32,
	expect_present: bool,
) -> bool {
	for i in 0..n_validators {
		if query_chunk(virtual_overseer, candidate_hash, ValidatorIndex(i)).await.is_some() !=
			expect_present
		{
			return false
		}
	}
	true
}

async fn import_leaf(
	virtual_overseer: &mut VirtualOverseer,
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
		OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
			hash: new_leaf,
			number: 1,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		})),
	)
	.await;

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
			RuntimeApiRequest::CandidateEvents(tx),
		)) => {
			assert_eq!(relay_parent, new_leaf);
			tx.send(Ok(events)).unwrap();
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
