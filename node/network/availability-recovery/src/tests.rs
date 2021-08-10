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

use std::{sync::Arc, time::Duration};

use assert_matches::assert_matches;
use futures::{executor, future};
use futures_timer::Delay;

use parity_scale_codec::Encode;

use super::*;

use polkadot_erasure_coding::{branches, obtain_chunks_v1 as obtain_chunks};
use polkadot_node_primitives::{BlockData, PoV};
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_primitives::v1::{AuthorityDiscoveryId, HeadData, PersistedValidationData};
use polkadot_subsystem::{
	jaeger,
	messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest},
	ActivatedLeaf, LeafStatus,
};
use polkadot_subsystem_testhelpers as test_helpers;

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<AvailabilityRecoveryMessage>;

fn test_harness_fast_path<T: Future<Output = VirtualOverseer>>(
	test: impl FnOnce(VirtualOverseer) -> T,
) {
	let _ = env_logger::builder()
		.is_test(true)
		.filter(Some("polkadot_availability_recovery"), log::LevelFilter::Trace)
		.try_init();

	let pool = sp_core::testing::TaskExecutor::new();

	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = AvailabilityRecoverySubsystem::with_fast_path();
	let subsystem = subsystem.run(context);

	let test_fut = test(virtual_overseer);

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	executor::block_on(future::join(
		async move {
			let mut overseer = test_fut.await;
			overseer_signal(&mut overseer, OverseerSignal::Conclude).await;
		},
		subsystem,
	))
	.1
	.unwrap();
}

fn test_harness_chunks_only<T: Future<Output = VirtualOverseer>>(
	test: impl FnOnce(VirtualOverseer) -> T,
) {
	let _ = env_logger::builder()
		.is_test(true)
		.filter(Some("polkadot_availability_recovery"), log::LevelFilter::Trace)
		.try_init();

	let pool = sp_core::testing::TaskExecutor::new();

	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = AvailabilityRecoverySubsystem::with_chunks_only();
	let subsystem = subsystem.run(context);

	let test_fut = test(virtual_overseer);

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	executor::block_on(future::join(
		async move {
			let mut overseer = test_fut.await;
			overseer_signal(&mut overseer, OverseerSignal::Conclude).await;
		},
		subsystem,
	))
	.1
	.unwrap();
}

const TIMEOUT: Duration = Duration::from_millis(100);

macro_rules! delay {
	($delay:expr) => {
		Delay::new(Duration::from_millis($delay)).await;
	};
}

async fn overseer_signal(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityRecoveryMessage>,
	signal: OverseerSignal,
) {
	delay!(50);
	overseer
		.send(FromOverseer::Signal(signal))
		.timeout(TIMEOUT)
		.await
		.expect("10ms is more than enough for sending signals.");
}

async fn overseer_send(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityRecoveryMessage>,
	msg: AvailabilityRecoveryMessage,
) {
	tracing::trace!(msg = ?msg, "sending message");
	overseer
		.send(FromOverseer::Communication { msg })
		.timeout(TIMEOUT)
		.await
		.expect("10ms is more than enough for sending messages.");
}

async fn overseer_recv(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityRecoveryMessage>,
) -> AllMessages {
	tracing::trace!("waiting for message ...");
	let msg = overseer.recv().timeout(TIMEOUT).await.expect("TIMEOUT is enough to recv.");
	tracing::trace!(msg = ?msg, "received message");
	msg
}

use sp_keyring::Sr25519Keyring;

#[derive(Debug)]
enum Has {
	No,
	Yes,
	NetworkError(sc_network::RequestFailure),
}

impl Has {
	fn timeout() -> Self {
		Has::NetworkError(sc_network::RequestFailure::Network(sc_network::OutboundFailure::Timeout))
	}
}

#[derive(Clone)]
struct TestState {
	validators: Vec<Sr25519Keyring>,
	validator_public: Vec<ValidatorId>,
	validator_authority_id: Vec<AuthorityDiscoveryId>,
	current: Hash,
	candidate: CandidateReceipt,
	session_index: SessionIndex,

	persisted_validation_data: PersistedValidationData,

	available_data: AvailableData,
	chunks: Vec<ErasureChunk>,
}

impl TestState {
	fn threshold(&self) -> usize {
		recovery_threshold(self.validators.len()).unwrap()
	}

	fn impossibility_threshold(&self) -> usize {
		self.validators.len() - self.threshold() + 1
	}

	async fn test_runtime_api(&self, virtual_overseer: &mut VirtualOverseer) {
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionInfo(
					session_index,
					tx,
				)
			)) => {
				assert_eq!(relay_parent, self.current);
				assert_eq!(session_index, self.session_index);

				tx.send(Ok(Some(SessionInfo {
					validators: self.validator_public.clone(),
					discovery_keys: self.validator_authority_id.clone(),
					// all validators in the same group.
					validator_groups: vec![(0..self.validators.len()).map(|i| ValidatorIndex(i as _)).collect()],
					..Default::default()
				}))).unwrap();
			}
		);
	}

	async fn respond_to_available_data_query(
		&self,
		virtual_overseer: &mut VirtualOverseer,
		with_data: bool,
	) {
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::AvailabilityStore(
				AvailabilityStoreMessage::QueryAvailableData(_, tx)
			) => {
				let _ = tx.send(if with_data {
					Some(self.available_data.clone())
				} else {
					println!("SENDING NONE");
					None
				});
			}
		)
	}

	async fn respond_to_query_all_request(
		&self,
		virtual_overseer: &mut VirtualOverseer,
		send_chunk: impl Fn(usize) -> bool,
	) {
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::AvailabilityStore(
				AvailabilityStoreMessage::QueryAllChunks(_, tx)
			) => {
				let v = self.chunks.iter()
					.filter(|c| send_chunk(c.index.0 as usize))
					.cloned()
					.collect();

				let _ = tx.send(v);
			}
		)
	}

	async fn test_chunk_requests(
		&self,
		candidate_hash: CandidateHash,
		virtual_overseer: &mut VirtualOverseer,
		n: usize,
		who_has: impl Fn(usize) -> Has,
	) {
		// arbitrary order.
		for _ in 0..n {
			// Receive a request for a chunk.
			assert_matches!(
				overseer_recv(virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendRequests(
						mut requests,
						IfDisconnected::TryConnect,
					)
				) => {
					assert_eq!(requests.len(), 1);

					assert_matches!(
						requests.pop().unwrap(),
						Requests::ChunkFetching(req) => {
							assert_eq!(req.payload.candidate_hash, candidate_hash);

							let validator_index = req.payload.index.0 as usize;
							let available_data = match who_has(validator_index) {
								Has::No => Ok(None),
								Has::Yes => Ok(Some(self.chunks[validator_index].clone().into())),
								Has::NetworkError(e) => Err(e),
							};

							let _ = req.pending_response.send(
								available_data.map(|r|
									req_res::v1::ChunkFetchingResponse::from(r).encode()
								)
							);
						}
					)
				}
			);
		}
	}

	async fn test_full_data_requests(
		&self,
		candidate_hash: CandidateHash,
		virtual_overseer: &mut VirtualOverseer,
		who_has: impl Fn(usize) -> Has,
	) {
		for _ in 0..self.validators.len() {
			// Receive a request for a chunk.
			assert_matches!(
				overseer_recv(virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendRequests(
						mut requests,
						IfDisconnected::TryConnect,
					)
				) => {
					assert_eq!(requests.len(), 1);

					assert_matches!(
						requests.pop().unwrap(),
						Requests::AvailableDataFetching(req) => {
							assert_eq!(req.payload.candidate_hash, candidate_hash);
							let validator_index = self.validator_authority_id
								.iter()
								.position(|a| Recipient::Authority(a.clone()) == req.peer)
								.unwrap();

							let available_data = match who_has(validator_index) {
								Has::No => Ok(None),
								Has::Yes => Ok(Some(self.available_data.clone())),
								Has::NetworkError(e) => Err(e),
							};

							let done = available_data.as_ref().ok().map_or(false, |x| x.is_some());

							let _ = req.pending_response.send(
								available_data.map(|r|
									req_res::v1::AvailableDataFetchingResponse::from(r).encode()
								)
							);

							if done { break }
						}
					)
				}
			);
		}
	}
}

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

fn validator_authority_id(val_ids: &[Sr25519Keyring]) -> Vec<AuthorityDiscoveryId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

fn derive_erasure_chunks_with_proofs_and_root(
	n_validators: usize,
	available_data: &AvailableData,
	alter_chunk: impl Fn(usize, &mut Vec<u8>),
) -> (Vec<ErasureChunk>, Hash) {
	let mut chunks: Vec<Vec<u8>> = obtain_chunks(n_validators, available_data).unwrap();

	for (i, chunk) in chunks.iter_mut().enumerate() {
		alter_chunk(i, chunk)
	}

	// create proofs for each erasure chunk
	let branches = branches(chunks.as_ref());

	let root = branches.root();
	let erasure_chunks = branches
		.enumerate()
		.map(|(index, (proof, chunk))| ErasureChunk {
			chunk: chunk.to_vec(),
			index: ValidatorIndex(index as _),
			proof,
		})
		.collect::<Vec<ErasureChunk>>();

	(erasure_chunks, root)
}

impl Default for TestState {
	fn default() -> Self {
		let validators = vec![
			Sr25519Keyring::Ferdie, // <- this node, role: validator
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
		];

		let validator_public = validator_pubkeys(&validators);
		let validator_authority_id = validator_authority_id(&validators);

		let current = Hash::repeat_byte(1);

		let mut candidate = CandidateReceipt::default();

		let session_index = 10;

		let persisted_validation_data = PersistedValidationData {
			parent_head: HeadData(vec![7, 8, 9]),
			relay_parent_number: Default::default(),
			max_pov_size: 1024,
			relay_parent_storage_root: Default::default(),
		};

		let pov = PoV { block_data: BlockData(vec![42; 64]) };

		let available_data = AvailableData {
			validation_data: persisted_validation_data.clone(),
			pov: Arc::new(pov),
		};

		let (chunks, erasure_root) = derive_erasure_chunks_with_proofs_and_root(
			validators.len(),
			&available_data,
			|_, _| {},
		);

		candidate.descriptor.erasure_root = erasure_root;
		candidate.descriptor.relay_parent = Hash::repeat_byte(10);

		Self {
			validators,
			validator_public,
			validator_authority_id,
			current,
			candidate,
			session_index,
			persisted_validation_data,
			available_data,
			chunks,
		}
	}
}

#[test]
fn availability_is_recovered_from_chunks_if_no_group_provided() {
	let test_state = TestState::default();

	test_harness_fast_path(|mut virtual_overseer| async move {
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: test_state.current.clone(),
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				None,
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		test_state.respond_to_available_data_query(&mut virtual_overseer, false).await;
		test_state.respond_to_query_all_request(&mut virtual_overseer, |_| false).await;

		test_state
			.test_chunk_requests(
				candidate_hash,
				&mut virtual_overseer,
				test_state.threshold(),
				|_| Has::Yes,
			)
			.await;

		// Recovered data should match the original one.
		assert_eq!(rx.await.unwrap().unwrap(), test_state.available_data);

		let (tx, rx) = oneshot::channel();

		// Test another candidate, send no chunks.
		let mut new_candidate = CandidateReceipt::default();

		new_candidate.descriptor.relay_parent = test_state.candidate.descriptor.relay_parent;

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				new_candidate.clone(),
				test_state.session_index,
				None,
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		test_state.respond_to_available_data_query(&mut virtual_overseer, false).await;
		test_state.respond_to_query_all_request(&mut virtual_overseer, |_| false).await;

		test_state
			.test_chunk_requests(
				new_candidate.hash(),
				&mut virtual_overseer,
				test_state.impossibility_threshold(),
				|_| Has::No,
			)
			.await;

		// A request times out with `Unavailable` error.
		assert_eq!(rx.await.unwrap().unwrap_err(), RecoveryError::Unavailable);
		virtual_overseer
	});
}

#[test]
fn availability_is_recovered_from_chunks_even_if_backing_group_supplied_if_chunks_only() {
	let test_state = TestState::default();

	test_harness_chunks_only(|mut virtual_overseer| async move {
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: test_state.current.clone(),
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				Some(GroupIndex(0)),
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		test_state.respond_to_available_data_query(&mut virtual_overseer, false).await;
		test_state.respond_to_query_all_request(&mut virtual_overseer, |_| false).await;

		test_state
			.test_chunk_requests(
				candidate_hash,
				&mut virtual_overseer,
				test_state.threshold(),
				|_| Has::Yes,
			)
			.await;

		// Recovered data should match the original one.
		assert_eq!(rx.await.unwrap().unwrap(), test_state.available_data);

		let (tx, rx) = oneshot::channel();

		// Test another candidate, send no chunks.
		let mut new_candidate = CandidateReceipt::default();

		new_candidate.descriptor.relay_parent = test_state.candidate.descriptor.relay_parent;

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				new_candidate.clone(),
				test_state.session_index,
				None,
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		test_state.respond_to_available_data_query(&mut virtual_overseer, false).await;
		test_state.respond_to_query_all_request(&mut virtual_overseer, |_| false).await;

		test_state
			.test_chunk_requests(
				new_candidate.hash(),
				&mut virtual_overseer,
				test_state.impossibility_threshold(),
				|_| Has::No,
			)
			.await;

		// A request times out with `Unavailable` error.
		assert_eq!(rx.await.unwrap().unwrap_err(), RecoveryError::Unavailable);
		virtual_overseer
	});
}

#[test]
fn bad_merkle_path_leads_to_recovery_error() {
	let mut test_state = TestState::default();

	test_harness_fast_path(|mut virtual_overseer| async move {
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: test_state.current.clone(),
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				None,
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		// Create some faulty chunks.
		test_state.chunks[0].chunk = vec![0; 32];
		test_state.chunks[1].chunk = vec![1; 32];
		test_state.chunks[2].chunk = vec![2; 32];
		test_state.chunks[3].chunk = vec![3; 32];
		test_state.chunks[4].chunk = vec![4; 32];

		test_state.respond_to_available_data_query(&mut virtual_overseer, false).await;
		test_state.respond_to_query_all_request(&mut virtual_overseer, |_| false).await;

		test_state
			.test_chunk_requests(
				candidate_hash,
				&mut virtual_overseer,
				test_state.impossibility_threshold(),
				|_| Has::Yes,
			)
			.await;

		// A request times out with `Unavailable` error.
		assert_eq!(rx.await.unwrap().unwrap_err(), RecoveryError::Unavailable);
		virtual_overseer
	});
}

#[test]
fn wrong_chunk_index_leads_to_recovery_error() {
	let mut test_state = TestState::default();

	test_harness_fast_path(|mut virtual_overseer| async move {
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: test_state.current.clone(),
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				None,
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		// These chunks should fail the index check as they don't have the correct index for validator.
		test_state.chunks[1] = test_state.chunks[0].clone();
		test_state.chunks[2] = test_state.chunks[0].clone();
		test_state.chunks[3] = test_state.chunks[0].clone();
		test_state.chunks[4] = test_state.chunks[0].clone();

		test_state.respond_to_available_data_query(&mut virtual_overseer, false).await;
		test_state.respond_to_query_all_request(&mut virtual_overseer, |_| false).await;

		test_state
			.test_chunk_requests(
				candidate_hash,
				&mut virtual_overseer,
				test_state.impossibility_threshold(),
				|_| Has::No,
			)
			.await;

		// A request times out with `Unavailable` error as there are no good peers.
		assert_eq!(rx.await.unwrap().unwrap_err(), RecoveryError::Unavailable);
		virtual_overseer
	});
}

#[test]
fn invalid_erasure_coding_leads_to_invalid_error() {
	let mut test_state = TestState::default();

	test_harness_fast_path(|mut virtual_overseer| async move {
		let pov = PoV { block_data: BlockData(vec![69; 64]) };

		let (bad_chunks, bad_erasure_root) = derive_erasure_chunks_with_proofs_and_root(
			test_state.chunks.len(),
			&AvailableData {
				validation_data: test_state.persisted_validation_data.clone(),
				pov: Arc::new(pov),
			},
			|i, chunk| *chunk = vec![i as u8; 32],
		);

		test_state.chunks = bad_chunks;
		test_state.candidate.descriptor.erasure_root = bad_erasure_root;

		let candidate_hash = test_state.candidate.hash();

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: test_state.current.clone(),
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				None,
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		test_state.respond_to_available_data_query(&mut virtual_overseer, false).await;
		test_state.respond_to_query_all_request(&mut virtual_overseer, |_| false).await;

		test_state
			.test_chunk_requests(
				candidate_hash,
				&mut virtual_overseer,
				test_state.threshold(),
				|_| Has::Yes,
			)
			.await;

		// f+1 'valid' chunks can't produce correct data.
		assert_eq!(rx.await.unwrap().unwrap_err(), RecoveryError::Invalid);
		virtual_overseer
	});
}

#[test]
fn fast_path_backing_group_recovers() {
	let test_state = TestState::default();

	test_harness_fast_path(|mut virtual_overseer| async move {
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: test_state.current.clone(),
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				Some(GroupIndex(0)),
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		let who_has = |i| match i {
			3 => Has::Yes,
			_ => Has::No,
		};

		test_state.respond_to_available_data_query(&mut virtual_overseer, false).await;

		test_state
			.test_full_data_requests(candidate_hash, &mut virtual_overseer, who_has)
			.await;

		// Recovered data should match the original one.
		assert_eq!(rx.await.unwrap().unwrap(), test_state.available_data);
		virtual_overseer
	});
}

#[test]
fn no_answers_in_fast_path_causes_chunk_requests() {
	let test_state = TestState::default();

	test_harness_fast_path(|mut virtual_overseer| async move {
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: test_state.current.clone(),
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				Some(GroupIndex(0)),
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		// mix of timeout and no.
		let who_has = |i| match i {
			0 | 3 => Has::No,
			_ => Has::timeout(),
		};

		test_state.respond_to_available_data_query(&mut virtual_overseer, false).await;

		test_state
			.test_full_data_requests(candidate_hash, &mut virtual_overseer, who_has)
			.await;

		test_state.respond_to_query_all_request(&mut virtual_overseer, |_| false).await;

		test_state
			.test_chunk_requests(
				candidate_hash,
				&mut virtual_overseer,
				test_state.threshold(),
				|_| Has::Yes,
			)
			.await;

		// Recovered data should match the original one.
		assert_eq!(rx.await.unwrap().unwrap(), test_state.available_data);
		virtual_overseer
	});
}

#[test]
fn task_canceled_when_receivers_dropped() {
	let test_state = TestState::default();

	test_harness_chunks_only(|mut virtual_overseer| async move {
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: test_state.current.clone(),
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let (tx, _) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				None,
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		for _ in 0..test_state.validators.len() {
			match virtual_overseer.recv().timeout(TIMEOUT).await {
				None => return virtual_overseer,
				Some(_) => continue,
			}
		}

		panic!("task requested all validators without concluding")
	});
}

#[test]
fn chunks_retry_until_all_nodes_respond() {
	let test_state = TestState::default();

	test_harness_chunks_only(|mut virtual_overseer| async move {
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: test_state.current.clone(),
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				Some(GroupIndex(0)),
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		test_state.respond_to_available_data_query(&mut virtual_overseer, false).await;
		test_state.respond_to_query_all_request(&mut virtual_overseer, |_| false).await;

		test_state
			.test_chunk_requests(
				candidate_hash,
				&mut virtual_overseer,
				test_state.validators.len(),
				|_| Has::timeout(),
			)
			.await;

		// we get to go another round!

		test_state
			.test_chunk_requests(
				candidate_hash,
				&mut virtual_overseer,
				test_state.impossibility_threshold(),
				|_| Has::No,
			)
			.await;

		// Recovered data should match the original one.
		assert_eq!(rx.await.unwrap().unwrap_err(), RecoveryError::Unavailable);
		virtual_overseer
	});
}

#[test]
fn returns_early_if_we_have_the_data() {
	let test_state = TestState::default();

	test_harness_chunks_only(|mut virtual_overseer| async move {
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: test_state.current.clone(),
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				None,
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;
		test_state.respond_to_available_data_query(&mut virtual_overseer, true).await;

		assert_eq!(rx.await.unwrap().unwrap(), test_state.available_data);
		virtual_overseer
	});
}

#[test]
fn does_not_query_local_validator() {
	let test_state = TestState::default();

	test_harness_chunks_only(|mut virtual_overseer| async move {
		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: test_state.current.clone(),
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			})),
		)
		.await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				None,
				tx,
			),
		)
		.await;

		test_state.test_runtime_api(&mut virtual_overseer).await;
		test_state.respond_to_available_data_query(&mut virtual_overseer, false).await;
		test_state.respond_to_query_all_request(&mut virtual_overseer, |i| i == 0).await;

		let candidate_hash = test_state.candidate.hash();

		test_state
			.test_chunk_requests(
				candidate_hash,
				&mut virtual_overseer,
				test_state.validators.len(),
				|i| if i == 0 { panic!("requested from local validator") } else { Has::timeout() },
			)
			.await;

		// second round, make sure it uses the local chunk.
		test_state
			.test_chunk_requests(
				candidate_hash,
				&mut virtual_overseer,
				test_state.threshold() - 1,
				|i| if i == 0 { panic!("requested from local validator") } else { Has::Yes },
			)
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), test_state.available_data);
		virtual_overseer
	});
}
