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

use std::time::Duration;
use std::sync::Arc;

use futures::{executor, future};
use futures_timer::Delay;
use assert_matches::assert_matches;
use smallvec::smallvec;

use super::*;

use polkadot_primitives::v1::{
	AuthorityDiscoveryId, PersistedValidationData, PoV, BlockData, HeadData,
};
use polkadot_erasure_coding::{branches, obtain_chunks_v1 as obtain_chunks};
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_subsystem_testhelpers as test_helpers;
use polkadot_subsystem::{messages::{RuntimeApiMessage, RuntimeApiRequest, NetworkBridgeEvent}, jaeger};

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<AvailabilityRecoveryMessage>;

struct TestHarness {
	virtual_overseer: VirtualOverseer,
}

fn test_harness_fast_path<T: Future<Output = ()>>(
	test: impl FnOnce(TestHarness) -> T,
) {
	let _ = env_logger::builder()
		.is_test(true)
		.filter(
			Some("polkadot_availability_recovery"),
			log::LevelFilter::Trace,
		)
		.try_init();

	let pool = sp_core::testing::TaskExecutor::new();

	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = AvailabilityRecoverySubsystem::with_fast_path();
	let subsystem = subsystem.run(context);

	let test_fut = test(TestHarness { virtual_overseer });

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	executor::block_on(future::select(test_fut, subsystem));
}

fn test_harness_chunks_only<T: Future<Output = ()>>(
	test: impl FnOnce(TestHarness) -> T,
) {
	let _ = env_logger::builder()
		.is_test(true)
		.filter(
			Some("polkadot_availability_recovery"),
			log::LevelFilter::Trace,
		)
		.try_init();

	let pool = sp_core::testing::TaskExecutor::new();

	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = AvailabilityRecoverySubsystem::with_chunks_only();
	let subsystem = subsystem.run(context);

	let test_fut = test(TestHarness { virtual_overseer });

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	executor::block_on(future::select(test_fut, subsystem));
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
	let msg = overseer
		.recv()
		.timeout(TIMEOUT)
		.await
		.expect("TIMEOUT is enough to recv.");
	tracing::trace!(msg = ?msg, "received message");
	msg
}


use sp_keyring::Sr25519Keyring;

#[derive(Debug, Clone)]
enum HasAvailableData {
	No,
	Yes,
	Timeout,
	Other(AvailableData),
}

#[derive(Clone)]
struct TestState {
	validators: Vec<Sr25519Keyring>,
	validator_public: Vec<ValidatorId>,
	validator_authority_id: Vec<AuthorityDiscoveryId>,
	validator_peer_id: Vec<PeerId>,
	current: Hash,
	candidate: CandidateReceipt,
	session_index: SessionIndex,


	persisted_validation_data: PersistedValidationData,

	available_data: AvailableData,
	chunks: Vec<ErasureChunk>,
}

impl TestState {
	async fn test_runtime_api(
		&self,
		virtual_overseer: &mut VirtualOverseer,
	) {
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

	async fn test_connect_to_all_validators(
		&self,
		virtual_overseer: &mut VirtualOverseer,
	) {
		self.test_connect_to_validators(virtual_overseer, self.validator_public.len()).await;
	}

	async fn test_connect_to_validators(
		&self,
		virtual_overseer: &mut VirtualOverseer,
		n: usize,
	) {
		// Channels by AuthorityDiscoveryId to send results to.
		// Gather them here and send in batch after the loop not to race.
		let mut results = HashMap::new();

		for _ in 0..n {
			// Connect to shuffled validators one by one.
			assert_matches!(
				overseer_recv(virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ConnectToValidators {
						validator_ids,
						connected,
						..
					}
				) => {
					for validator_id in validator_ids {
						let idx = self.validator_authority_id
							.iter()
							.position(|x| *x == validator_id)
							.unwrap();

						results.insert(
							(
								self.validator_authority_id[idx].clone(),
								self.validator_peer_id[idx].clone(),
							),
							connected.clone(),
						);
					}
				}
			);
		}

		for (k, mut v) in results.into_iter() {
			v.send(k).await.unwrap();
		}
	}

	async fn test_chunk_requests(
		&self,
		candidate_hash: CandidateHash,
		virtual_overseer: &mut VirtualOverseer,
	) {
		for _ in 0..self.validator_public.len() {
			// Receive a request for a chunk.
			assert_matches!(
				overseer_recv(virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendValidationMessage(
						_peers,
						protocol_v1::ValidationProtocol::AvailabilityRecovery(wire_message),
					)
				) => {
					let (request_id, validator_index) = assert_matches!(
						wire_message,
						protocol_v1::AvailabilityRecoveryMessage::RequestChunk(
							request_id,
							candidate_hash_recvd,
							validator_index,
						) => {
							assert_eq!(candidate_hash_recvd, candidate_hash);
							(request_id, validator_index)
						}
					);

					overseer_send(
						virtual_overseer,
						AvailabilityRecoveryMessage::NetworkBridgeUpdateV1(
							NetworkBridgeEvent::PeerMessage(
								self.validator_peer_id[validator_index.0 as usize].clone(),
								protocol_v1::AvailabilityRecoveryMessage::Chunk(
									request_id,
									Some(self.chunks[validator_index.0 as usize].clone()),
								)
							)
						)
					).await;
				}
			);
		}
	}

	async fn test_faulty_chunk_requests(
		&self,
		candidate_hash: CandidateHash,
		virtual_overseer: &mut VirtualOverseer,
		faulty: &[bool],
	) {
		for _ in 0..self.validator_public.len() {
			// Receive a request for a chunk.
			assert_matches!(
				overseer_recv(virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendValidationMessage(
						_peers,
						protocol_v1::ValidationProtocol::AvailabilityRecovery(wire_message),
					)
				) => {
					let (request_id, validator_index) = assert_matches!(
						wire_message,
						protocol_v1::AvailabilityRecoveryMessage::RequestChunk(
							request_id,
							candidate_hash_recvd,
							validator_index,
						) => {
							assert_eq!(candidate_hash_recvd, candidate_hash);
							(request_id, validator_index)
						}
					);

					overseer_send(
						virtual_overseer,
						AvailabilityRecoveryMessage::NetworkBridgeUpdateV1(
							NetworkBridgeEvent::PeerMessage(
								self.validator_peer_id[validator_index.0 as usize].clone(),
								protocol_v1::AvailabilityRecoveryMessage::Chunk(
									request_id,
									Some(self.chunks[validator_index.0 as usize].clone()),
								)
							)
						)
					).await;
				}
			);
		}

		for i in 0..self.validator_public.len() {
			if faulty[i] {
				assert_matches!(
					overseer_recv(virtual_overseer).await,
					AllMessages::NetworkBridge(
						NetworkBridgeMessage::ReportPeer(
							peer,
							rep,
						)
					) => {
						assert_eq!(rep, COST_MERKLE_PROOF_INVALID);

						// These may arrive in any order since the interaction implementation
						// uses `FuturesUnordered`.
						assert!(self.validator_peer_id.iter().find(|p| **p == peer).is_some());
					}
				);
			}
		}
	}

	async fn test_full_data_requests(
		&self,
		candidate_hash: CandidateHash,
		virtual_overseer: &mut VirtualOverseer,
		who_has: &[HasAvailableData],
	) {
		for _ in 0..self.validator_public.len() {
			self.test_connect_to_validators(virtual_overseer, 1).await;

			// Receive a request for a chunk.
			assert_matches!(
				overseer_recv(virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendValidationMessage(
						peers,
						protocol_v1::ValidationProtocol::AvailabilityRecovery(wire_message),
					)
				) => {
					let (request_id, validator_index) = assert_matches!(
						wire_message,
						protocol_v1::AvailabilityRecoveryMessage::RequestFullData(
							request_id,
							candidate_hash_recvd,
						) => {
							assert_eq!(candidate_hash_recvd, candidate_hash);
							assert_eq!(peers.len(), 1);

							let validator_index = self.validator_peer_id.iter().position(|p| p == &peers[0]).unwrap();
							(request_id, validator_index)
						}
					);

					let available_data = match who_has[validator_index] {
						HasAvailableData::No => Some(None),
						HasAvailableData::Yes => Some(Some(self.available_data.clone())),
						HasAvailableData::Timeout => None,
						HasAvailableData::Other(ref other) => Some(Some(other.clone())),
					};

					if let Some(maybe_data) = available_data {
						overseer_send(
							virtual_overseer,
							AvailabilityRecoveryMessage::NetworkBridgeUpdateV1(
								NetworkBridgeEvent::PeerMessage(
									self.validator_peer_id[validator_index].clone(),
									protocol_v1::AvailabilityRecoveryMessage::FullData(
										request_id,
										maybe_data,
									)
								)
							)
						).await;
					}

					match who_has[validator_index] {
						HasAvailableData::Yes => break, // done
						HasAvailableData::No => {}
						HasAvailableData::Timeout => { Delay::new(FULL_DATA_REQUEST_TIMEOUT).await }
						HasAvailableData::Other(_) => {
							assert_matches!(
								overseer_recv(virtual_overseer).await,
								AllMessages::NetworkBridge(
									NetworkBridgeMessage::ReportPeer(
										p,
										rep,
									)
								) => {
									assert_eq!(p, self.validator_peer_id[validator_index]);
									assert_eq!(rep, COST_INVALID_AVAILABLE_DATA);
								}
							);
						}
					}
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
		let validator_peer_id = std::iter::repeat_with(|| PeerId::random())
			.take(validator_public.len())
			.collect();

		let current = Hash::repeat_byte(1);

		let mut candidate = CandidateReceipt::default();

		let session_index = 10;

		let persisted_validation_data = PersistedValidationData {
			parent_head: HeadData(vec![7, 8, 9]),
			relay_parent_number: Default::default(),
			max_pov_size: 1024,
			relay_parent_storage_root: Default::default(),
		};

		let pov = PoV {
			block_data: BlockData(vec![42; 64]),
		};

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
			validator_peer_id,
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

	test_harness_fast_path(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![(test_state.current.clone(), Arc::new(jaeger::Span::Disabled))],
				deactivated: smallvec![],
			}),
		).await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				None,
				tx,
			)
		).await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		test_state.test_connect_to_all_validators(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		test_state.test_chunk_requests(candidate_hash, &mut virtual_overseer).await;

		// Recovered data should match the original one.
		assert_eq!(rx.await.unwrap().unwrap(), test_state.available_data);

		let (tx, rx) = oneshot::channel();

		// Test another candidate, send no chunks.
		let mut new_candidate = CandidateReceipt::default();

		new_candidate.descriptor.relay_parent = test_state.candidate.descriptor.relay_parent;

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				new_candidate,
				test_state.session_index,
				None,
				tx,
			)
		).await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		test_state.test_connect_to_all_validators(&mut virtual_overseer).await;

		// A request times out with `Unavailable` error.
		assert_eq!(rx.await.unwrap().unwrap_err(), RecoveryError::Unavailable);
	});
}

#[test]
fn availability_is_recovered_from_chunks_even_if_backing_group_supplied_if_chunks_only() {
	let test_state = TestState::default();

	test_harness_chunks_only(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![(test_state.current.clone(), Arc::new(jaeger::Span::Disabled))],
				deactivated: smallvec![],
			}),
		).await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				Some(GroupIndex(0)),
				tx,
			)
		).await;

		test_state.test_runtime_api(&mut virtual_overseer).await;
		test_state.test_connect_to_all_validators(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		test_state.test_chunk_requests(candidate_hash, &mut virtual_overseer).await;

		// Recovered data should match the original one.
		assert_eq!(rx.await.unwrap().unwrap(), test_state.available_data);

		let (tx, rx) = oneshot::channel();

		// Test another candidate, send no chunks.
		let mut new_candidate = CandidateReceipt::default();

		new_candidate.descriptor.relay_parent = test_state.candidate.descriptor.relay_parent;

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				new_candidate,
				test_state.session_index,
				None,
				tx,
			)
		).await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		test_state.test_connect_to_all_validators(&mut virtual_overseer).await;

		// A request times out with `Unavailable` error.
		assert_eq!(rx.await.unwrap().unwrap_err(), RecoveryError::Unavailable);
	});
}

#[test]
fn bad_merkle_path_leads_to_recovery_error() {
	let mut test_state = TestState::default();

	test_harness_fast_path(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![(test_state.current.clone(), Arc::new(jaeger::Span::Disabled))],
				deactivated: smallvec![],
			}),
		).await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				None,
				tx,
			)
		).await;

		test_state.test_runtime_api(&mut virtual_overseer).await;
		test_state.test_connect_to_all_validators(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		// Create some faulty chunks.
		test_state.chunks[0].chunk = vec![0; 32];
		test_state.chunks[1].chunk = vec![1; 32];
		test_state.chunks[2].chunk = vec![2; 32];
		test_state.chunks[3].chunk = vec![3; 32];

		let mut faulty = vec![false; test_state.chunks.len()];
		faulty[0] = true;
		faulty[1] = true;
		faulty[2] = true;
		faulty[3] = true;

		test_state.test_faulty_chunk_requests(
			candidate_hash,
			&mut virtual_overseer,
			&faulty,
		).await;

		// A request times out with `Unavailable` error.
		assert_eq!(rx.await.unwrap().unwrap_err(), RecoveryError::Unavailable);
	});
}

#[test]
fn wrong_chunk_index_leads_to_recovery_error() {
	let mut test_state = TestState::default();

	test_harness_fast_path(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![(test_state.current.clone(), Arc::new(jaeger::Span::Disabled))],
				deactivated: smallvec![],
			}),
		).await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				None,
				tx,
			)
		).await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		test_state.test_connect_to_all_validators(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		// These chunks should fail the index check as they don't have the correct index for validator.
		test_state.chunks[1] = test_state.chunks[0].clone();
		test_state.chunks[2] = test_state.chunks[0].clone();
		test_state.chunks[3] = test_state.chunks[0].clone();
		test_state.chunks[4] = test_state.chunks[0].clone();

		let mut faulty = vec![true; test_state.chunks.len()];
		faulty[0] = false;

		test_state.test_faulty_chunk_requests(
			candidate_hash,
			&mut virtual_overseer,
			&faulty,
		).await;

		// A request times out with `Unavailable` error as there are no good peers.
		assert_eq!(rx.await.unwrap().unwrap_err(), RecoveryError::Unavailable);
	});
}

#[test]
fn invalid_erasure_coding_leads_to_invalid_error() {
	let mut test_state = TestState::default();

	test_harness_fast_path(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		let pov = PoV {
			block_data: BlockData(vec![69; 64]),
		};

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
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![(test_state.current.clone(), Arc::new(jaeger::Span::Disabled))],
				deactivated: smallvec![],
			}),
		).await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				None,
				tx,
			)
		).await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		test_state.test_connect_to_all_validators(&mut virtual_overseer).await;

		test_state.test_chunk_requests(
			candidate_hash,
			&mut virtual_overseer,
		).await;

		// A request times out with `Unavailable` error as there are no good peers.
		assert_eq!(rx.await.unwrap().unwrap_err(), RecoveryError::Invalid);
	});
}

#[test]
fn fast_path_backing_group_recovers() {
	let test_state = TestState::default();

	test_harness_fast_path(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![(test_state.current.clone(), Arc::new(jaeger::Span::Disabled))],
				deactivated: smallvec![],
			}),
		).await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				Some(GroupIndex(0)),
				tx,
			)
		).await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		let mut who_has: Vec<_> = (0..test_state.validators.len()).map(|_| HasAvailableData::No).collect();
		who_has[3] = HasAvailableData::Yes;

		test_state.test_full_data_requests(
			candidate_hash,
			&mut virtual_overseer,
			&who_has,
		).await;

		// Recovered data should match the original one.
		assert_eq!(rx.await.unwrap().unwrap(), test_state.available_data);
	});
}

#[test]
fn wrong_data_from_fast_path_peer_leads_to_punishment() {
	let test_state = TestState::default();

	test_harness_fast_path(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![(test_state.current.clone(), Arc::new(jaeger::Span::Disabled))],
				deactivated: smallvec![],
			}),
		).await;

		let (tx, _rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				Some(GroupIndex(0)),
				tx,
			)
		).await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		let mut a = test_state.available_data.clone();
		a.pov = Arc::new(PoV { block_data: BlockData(vec![69; 420]) });

		let who_has: Vec<_> = (0..test_state.validators.len()).map(|_| HasAvailableData::Other(a.clone())).collect();

		// This function implicitly punishes.
		test_state.test_full_data_requests(
			candidate_hash,
			&mut virtual_overseer,
			&who_has,
		).await;
	});
}

#[test]
fn no_answers_in_fast_path_causes_chunk_requests() {
	let test_state = TestState::default();

	test_harness_fast_path(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![(test_state.current.clone(), Arc::new(jaeger::Span::Disabled))],
				deactivated: smallvec![],
			}),
		).await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				test_state.session_index,
				Some(GroupIndex(0)),
				tx,
			)
		).await;

		test_state.test_runtime_api(&mut virtual_overseer).await;

		let candidate_hash = test_state.candidate.hash();

		// mix of timeout and no.
		let mut who_has: Vec<_> = (0..test_state.validators.len()).map(|_| HasAvailableData::Timeout).collect();
		who_has[0] = HasAvailableData::No;
		who_has[3] = HasAvailableData::No;

		test_state.test_full_data_requests(
			candidate_hash,
			&mut virtual_overseer,
			&who_has,
		).await;

		test_state.test_connect_to_all_validators(&mut virtual_overseer).await;
		test_state.test_chunk_requests(candidate_hash, &mut virtual_overseer).await;

		// Recovered data should match the original one.
		assert_eq!(rx.await.unwrap().unwrap(), test_state.available_data);
	});
}
