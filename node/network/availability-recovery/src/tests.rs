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
use polkadot_subsystem::messages::{RuntimeApiMessage, RuntimeApiRequest};

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<AvailabilityRecoveryMessage>;

struct TestHarness {
	virtual_overseer: VirtualOverseer,
}

fn test_harness<T: Future<Output = ()>>(
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

	let subsystem = AvailabilityRecoverySubsystem::new();
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

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

fn validator_authority_id(val_ids: &[Sr25519Keyring]) -> Vec<AuthorityDiscoveryId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

fn derive_erasure_chunks_with_proofs_and_root(
	n_validators: usize,
	available_data: &AvailableData,
) -> (Vec<ErasureChunk>, Hash) {
	let chunks: Vec<Vec<u8>> = obtain_chunks(n_validators, available_data).unwrap();

	// create proofs for each erasure chunk
	let branches = branches(chunks.as_ref());

	let root = branches.root();
	let erasure_chunks = branches
		.enumerate()
		.map(|(index, (proof, chunk))| ErasureChunk {
			chunk: chunk.to_vec(),
			index: index as _,
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
			block_number: Default::default(),
			hrmp_mqc_heads: Vec::new(),
			dmq_mqc_head: Default::default(),
			max_pov_size: 1024,
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
		);

		candidate.descriptor.erasure_root = erasure_root;

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

async fn test_validator_discovery(
	virtual_overseer: &mut VirtualOverseer,
	expected_relay_parent: Hash,
	session_index: SessionIndex,
	validator_ids: &[ValidatorId],
	discovery_ids: &[AuthorityDiscoveryId],
) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			relay_parent,
			RuntimeApiRequest::SessionIndexForChild(tx),
		)) => {
			assert_eq!(relay_parent, expected_relay_parent);
			tx.send(Ok(session_index)).unwrap();
		}
	);

	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			relay_parent,
			RuntimeApiRequest::SessionInfo(index, tx),
		)) => {
			assert_eq!(relay_parent, expected_relay_parent);
			assert_eq!(index, session_index);

			let validators = validator_ids
				.iter()
				.cloned()
				.collect();

			let discovery_keys = discovery_ids
				.iter()
				.cloned()
				.collect();

			tx.send(Ok(Some(SessionInfo {
				validators,
				discovery_keys,
				..Default::default()
			}))).unwrap();
		}
	);
}

#[test]
fn availability_is_recovered() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: smallvec![test_state.current.clone()],
				deactivated: smallvec![],
			}),
		).await;

		let (tx, rx) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			AvailabilityRecoveryMessage::RecoverAvailableData(
				test_state.candidate.clone(),
				tx,
			)
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionIndexForChild(tx),
			)) => {
				assert_eq!(relay_parent, test_state.current);
				tx.send(Ok(test_state.session_index)).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionInfo(
					session_index,
					tx,
				)
			)) => {
				assert_eq!(relay_parent, test_state.current);
				assert_eq!(session_index, test_state.session_index);

				tx.send(Ok(Some(SessionInfo {
					validators: test_state.validator_public.clone(),
					..Default::default()
				}))).unwrap();
			}
		);

		// Indexes of validators subsystem has attempted to connect to.
		let mut attempted_to_connect_to = Vec::new();

		for _ in 0..test_state.validator_public.len() {
			test_validator_discovery(
				&mut virtual_overseer,
				test_state.current,
				test_state.session_index,
				&test_state.validator_public,
				&test_state.validator_authority_id,
			).await;

			// Connect to shuffled validators one by one.
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ConnectToValidators {
						validator_ids,
						mut connected,
						..
					}
				) => {
					for validator_id in validator_ids {
						let idx = test_state.validator_authority_id
							.iter()
							.position(|x| *x == validator_id)
							.unwrap();

						attempted_to_connect_to.push(idx);

						let result = (
							test_state.validator_authority_id[idx].clone(),
							test_state.validator_peer_id[idx].clone(),
						);

						connected.try_send(result).unwrap();
					}
				}
			);
		}

		for _ in 0..test_state.validator_public.len() {
			// Receive a request for a chunk.
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
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
							candidate_hash,
							validator_index,
						) => {
							assert_eq!(candidate_hash, test_state.candidate.hash());
							(request_id, validator_index)
						}
					);

					overseer_send(
						&mut virtual_overseer,
						AvailabilityRecoveryMessage::NetworkBridgeUpdateV1(
							NetworkBridgeEvent::PeerMessage(
								test_state.validator_peer_id[validator_index as usize].clone(),
								protocol_v1::AvailabilityRecoveryMessage::Chunk(
									request_id,
									Some(test_state.chunks[validator_index as usize].clone()),
								)
							)
						)
					).await;
				}
			);
		}

		// Recovered data should match the original one.
		assert_eq!(rx.await.unwrap().unwrap(), test_state.available_data);
	});
}
