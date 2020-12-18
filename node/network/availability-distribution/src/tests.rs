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
use polkadot_erasure_coding::{branches, obtain_chunks_v1 as obtain_chunks};
use polkadot_node_network_protocol::{view, ObservedRole};
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_primitives::v1::{
	AvailableData, BlockData, CandidateCommitments, CandidateDescriptor, GroupIndex,
	GroupRotationInfo, HeadData, OccupiedCore, PersistedValidationData, PoV, ScheduledCore, Id as ParaId,
	CommittedCandidateReceipt,
};
use polkadot_subsystem_testhelpers as test_helpers;

use futures::{executor, future, Future};
use sc_keystore::LocalKeystore;
use sp_application_crypto::AppKey;
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
use sp_keyring::Sr25519Keyring;
use std::{sync::Arc, time::Duration};
use maplit::hashmap;

macro_rules! view {
	( $( $hash:expr ),* $(,)? ) => {
		// Finalized number unimportant for availability distribution.
		View { heads: vec![ $( $hash.clone() ),* ], finalized_number: 0 }
	};
}

fn chunk_protocol_message(
	message: AvailabilityGossipMessage,
) -> protocol_v1::AvailabilityDistributionMessage {
	protocol_v1::AvailabilityDistributionMessage::Chunk(
		message.candidate_hash,
		message.erasure_chunk,
	)
}

struct TestHarness {
	virtual_overseer: test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
}

fn test_harness<T: Future<Output = ()>>(
	keystore: SyncCryptoStorePtr,
	test_fx: impl FnOnce(TestHarness) -> T,
) -> ProtocolState {
	sp_tracing::try_init_simple();

	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = AvailabilityDistributionSubsystem::new(keystore, Default::default());
	let mut state = ProtocolState::default();
	{
		let subsystem = subsystem.run_inner(context, &mut state);

		let test_fut = test_fx(TestHarness { virtual_overseer });

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}

	state
}

async fn overseer_send(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	msg: impl Into<AvailabilityDistributionMessage>,
) {
	let msg = msg.into();
	tracing::trace!(msg = ?msg, "sending message");
	overseer.send(FromOverseer::Communication { msg }).await
}

async fn overseer_recv(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
) -> AllMessages {
	tracing::trace!("waiting for message ...");
	let msg = overseer.recv().await;
	tracing::trace!(msg = ?msg, "received message");
	msg
}

fn occupied_core_from_candidate(receipt: &CommittedCandidateReceipt) -> CoreState {
	CoreState::Occupied(OccupiedCore {
		next_up_on_available: None,
		occupied_since: 0,
		time_out_at: 5,
		next_up_on_time_out: None,
		availability: Default::default(),
		group_responsible: GroupIndex::from(0),
		candidate_hash: receipt.hash(),
		candidate_descriptor: receipt.descriptor().clone(),
	})
}

#[derive(Clone)]
struct TestState {
	chain_ids: Vec<ParaId>,
	validators: Vec<Sr25519Keyring>,
	validator_public: Vec<ValidatorId>,
	validator_groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo),
	head_data: HashMap<ParaId, HeadData>,
	keystore: SyncCryptoStorePtr,
	relay_parent: Hash,
	ancestors: Vec<Hash>,
	availability_cores: Vec<CoreState>,
	persisted_validation_data: PersistedValidationData,
	candidates: Vec<CommittedCandidateReceipt>,
	pov_blocks: Vec<PoV>,
}

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

impl Default for TestState {
	fn default() -> Self {
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);

		let chain_ids = vec![chain_a, chain_b];

		let validators = vec![
			Sr25519Keyring::Ferdie, // <- this node, role: validator
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
		];

		let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());

		SyncCryptoStore::sr25519_generate_new(
			&*keystore,
			ValidatorId::ID,
			Some(&validators[0].to_seed()),
		)
		.expect("Insert key into keystore");

		let validator_public = validator_pubkeys(&validators);

		let validator_groups = vec![vec![2, 0, 4], vec![1], vec![3]];
		let group_rotation_info = GroupRotationInfo {
			session_start_block: 0,
			group_rotation_frequency: 100,
			now: 1,
		};
		let validator_groups = (validator_groups, group_rotation_info);

		let availability_cores = vec![
			CoreState::Scheduled(ScheduledCore {
				para_id: chain_ids[0],
				collator: None,
			}),
			CoreState::Scheduled(ScheduledCore {
				para_id: chain_ids[1],
				collator: None,
			}),
		];

		let mut head_data = HashMap::new();
		head_data.insert(chain_a, HeadData(vec![4, 5, 6]));
		head_data.insert(chain_b, HeadData(vec![7, 8, 9]));

		let ancestors = vec![
			Hash::repeat_byte(0x44),
			Hash::repeat_byte(0x33),
			Hash::repeat_byte(0x22),
		];
		let relay_parent = Hash::repeat_byte(0x05);

		let persisted_validation_data = PersistedValidationData {
			parent_head: HeadData(vec![7, 8, 9]),
			block_number: Default::default(),
			hrmp_mqc_heads: Vec::new(),
			dmq_mqc_head: Default::default(),
			max_pov_size: 1024,
		};

		let pov_block_a = PoV {
			block_data: BlockData(vec![42, 43, 44]),
		};

		let pov_block_b = PoV {
			block_data: BlockData(vec![45, 46, 47]),
		};

		let candidates = vec![
			TestCandidateBuilder {
				para_id: chain_ids[0],
				relay_parent: relay_parent,
				pov_hash: pov_block_a.hash(),
				erasure_root: make_erasure_root(persisted_validation_data.clone(), validators.len(), pov_block_a.clone()),
				head_data: head_data.get(&chain_ids[0]).unwrap().clone(),
				..Default::default()
			}
			.build(),
			TestCandidateBuilder {
				para_id: chain_ids[1],
				relay_parent: relay_parent,
				pov_hash: pov_block_b.hash(),
				erasure_root: make_erasure_root(persisted_validation_data.clone(), validators.len(), pov_block_b.clone()),
				head_data: head_data.get(&chain_ids[1]).unwrap().clone(),
				..Default::default()
			}
			.build(),
		];

		let pov_blocks = vec![pov_block_a, pov_block_b];

		Self {
			chain_ids,
			keystore,
			validators,
			validator_public,
			validator_groups,
			availability_cores,
			head_data,
			persisted_validation_data,
			relay_parent,
			ancestors,
			candidates,
			pov_blocks,
		}
	}
}

fn make_available_data(validation_data: PersistedValidationData, pov: PoV) -> AvailableData {
	AvailableData {
		validation_data,
		pov: Arc::new(pov),
	}
}

fn make_erasure_root(peristed: PersistedValidationData, validator_count: usize, pov: PoV) -> Hash {
	let available_data = make_available_data(peristed, pov);

	let chunks = obtain_chunks(validator_count, &available_data).unwrap();
	branches(&chunks).root()
}

fn make_erasure_chunks(peristed: PersistedValidationData, validator_count: usize, pov: PoV) -> Vec<ErasureChunk> {
	let available_data = make_available_data(peristed, pov);

	derive_erasure_chunks_with_proofs(validator_count, &available_data)
}

fn make_valid_availability_gossip(
	test: &TestState,
	candidate: usize,
	erasure_chunk_index: u32,
) -> AvailabilityGossipMessage {
	let erasure_chunks = make_erasure_chunks(
		test.persisted_validation_data.clone(),
		test.validator_public.len(),
		test.pov_blocks[candidate].clone(),
	);

	let erasure_chunk: ErasureChunk = erasure_chunks
		.get(erasure_chunk_index as usize)
		.expect("Must be valid or input is oob")
		.clone();

	AvailabilityGossipMessage {
		candidate_hash: test.candidates[candidate].hash(),
		erasure_chunk,
	}
}

#[derive(Default)]
struct TestCandidateBuilder {
	para_id: ParaId,
	head_data: HeadData,
	pov_hash: Hash,
	relay_parent: Hash,
	erasure_root: Hash,
}

impl TestCandidateBuilder {
	fn build(self) -> CommittedCandidateReceipt {
		CommittedCandidateReceipt {
			descriptor: CandidateDescriptor {
				para_id: self.para_id,
				pov_hash: self.pov_hash,
				relay_parent: self.relay_parent,
				erasure_root: self.erasure_root,
				..Default::default()
			},
			commitments: CandidateCommitments {
				head_data: self.head_data,
				..Default::default()
			},
		}
	}
}

#[test]
fn helper_integrity() {
	let test_state = TestState::default();

	let message = make_valid_availability_gossip(
		&test_state,
		0,
		2,
	);

	let root = &test_state.candidates[0].descriptor.erasure_root;

	let anticipated_hash = branch_hash(
		root,
		&message.erasure_chunk.proof,
		dbg!(message.erasure_chunk.index as usize),
	)
	.expect("Must be able to derive branch hash");
	assert_eq!(
		anticipated_hash,
		BlakeTwo256::hash(&message.erasure_chunk.chunk)
	);
}

fn derive_erasure_chunks_with_proofs(
	n_validators: usize,
	available_data: &AvailableData,
) -> Vec<ErasureChunk> {
	let chunks: Vec<Vec<u8>> = obtain_chunks(n_validators, available_data).unwrap();

	// create proofs for each erasure chunk
	let branches = branches(chunks.as_ref());

	let erasure_chunks = branches
		.enumerate()
		.map(|(index, (proof, chunk))| ErasureChunk {
			chunk: chunk.to_vec(),
			index: index as _,
			proof,
		})
		.collect::<Vec<ErasureChunk>>();

	erasure_chunks
}

async fn expect_chunks_network_message(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	peers: &[PeerId],
	candidates: &[CandidateHash],
	chunks: &[ErasureChunk],
) {
	for _ in 0..chunks.len() {
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendValidationMessage(
					send_peers,
					protocol_v1::ValidationProtocol::AvailabilityDistribution(
						protocol_v1::AvailabilityDistributionMessage::Chunk(send_candidate, send_chunk),
					),
				)
			) => {
				assert!(candidates.contains(&send_candidate), format!("Could not find candidate: {:?}", send_candidate));
				assert!(chunks.iter().any(|c| c == &send_chunk), format!("Could not find chunk: {:?}", send_chunk));
				assert_eq!(peers.len(), send_peers.len());
				assert!(peers.iter().all(|p| send_peers.contains(p)));
			}
		);
	}
}

async fn change_our_view(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	view: View,
	validator_public: &[ValidatorId],
	ancestors: Vec<Hash>,
	session_per_relay_parent: HashMap<Hash, SessionIndex>,
	availability_cores_per_relay_parent: HashMap<Hash, Vec<CoreState>>,
	data_availability: HashMap<CandidateHash, bool>,
	chunk_data_per_candidate: HashMap<CandidateHash, (PoV, PersistedValidationData)>,
	send_chunks_to: HashMap<CandidateHash, Vec<PeerId>>,
) {
	overseer_send(virtual_overseer, NetworkBridgeEvent::OurViewChange(view.clone())).await;

	// obtain the validators per relay parent
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			relay_parent,
			RuntimeApiRequest::Validators(tx),
		)) => {
			assert!(view.contains(&relay_parent));
			tx.send(Ok(validator_public.to_vec())).unwrap();
		}
	);

	// query of k ancestors, we only provide one
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::ChainApi(ChainApiMessage::Ancestors {
			hash: relay_parent,
			k,
			response_channel: tx,
		}) => {
			assert!(view.contains(&relay_parent));
			assert_eq!(k, AvailabilityDistributionSubsystem::K + 1);
			tx.send(Ok(ancestors.clone())).unwrap();
		}
	);

	for _ in 0..session_per_relay_parent.len() {
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionIndexForChild(tx)
			)) => {
				let index = session_per_relay_parent.get(&relay_parent)
					.expect(&format!("Session index for relay parent {:?} does not exist", relay_parent));
				tx.send(Ok(*index)).unwrap();
			}
		);
	}

	for _ in 0..availability_cores_per_relay_parent.len() {
		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::AvailabilityCores(tx)
			)) => {
				let cores = availability_cores_per_relay_parent.get(&relay_parent)
					.expect(&format!("Availability core for relay parent {:?} does not exist", relay_parent));

				tx.send(Ok(cores.clone())).unwrap();
			}
		);
	}

	for _ in 0..data_availability.len() {
		let (available, candidate_hash) = assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::AvailabilityStore(
				AvailabilityStoreMessage::QueryDataAvailability(
					candidate_hash,
					tx,
				)
			) => {
				let available = data_availability.get(&candidate_hash)
					.expect(&format!("No data availability for candidate {:?}", candidate_hash));

				tx.send(*available).unwrap();
				(available, candidate_hash)
			}
		);

		if !available {
			continue;
		}

		if let Some((pov, persisted)) = chunk_data_per_candidate.get(&candidate_hash) {
			let chunks = make_erasure_chunks(persisted.clone(), validator_public.len(), pov.clone());

			for _ in 0..chunks.len() {
				let chunk = assert_matches!(
					overseer_recv(virtual_overseer).await,
					AllMessages::AvailabilityStore(
						AvailabilityStoreMessage::QueryChunk(
							candidate_hash,
							index,
							tx,
						)
					) => {
						tracing::trace!("Query chunk {} for candidate {:?}", index, candidate_hash);
						let chunk = chunks[index as usize].clone();
						tx.send(Some(chunk.clone())).unwrap();
						chunk
					}
				);

				if let Some(peers) = send_chunks_to.get(&candidate_hash) {
					expect_chunks_network_message(virtual_overseer, &peers, &[candidate_hash], &[chunk]).await;
				}
			}
		}
	}
}

async fn setup_peer_with_view(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	peer: PeerId,
	view: View,
) {
	overseer_send(virtual_overseer, NetworkBridgeEvent::PeerConnected(peer.clone(), ObservedRole::Full)).await;

	overseer_send(virtual_overseer, NetworkBridgeEvent::PeerViewChange(peer, view)).await;
}

async fn peer_send_message(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	peer: PeerId,
	message: AvailabilityGossipMessage,
	expected_reputation_change: Rep,
) {
	overseer_send(virtual_overseer, NetworkBridgeEvent::PeerMessage(peer.clone(), chunk_protocol_message(message))).await;

	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::NetworkBridge(
			NetworkBridgeMessage::ReportPeer(
				rep_peer,
				rep,
			)
		) => {
			assert_eq!(peer, rep_peer);
			assert_eq!(expected_reputation_change, rep);
		}
	);
}

#[test]
fn check_views() {
	let test_state = TestState::default();

	let peer_a = PeerId::random();
	let peer_a_2 = peer_a.clone();
	let peer_b = PeerId::random();
	let peer_b_2 = peer_b.clone();
	assert_ne!(&peer_a, &peer_b);

	let keystore = test_state.keystore.clone();
	let current = test_state.relay_parent;
	let ancestors = test_state.ancestors.clone();

	let state = test_harness(keystore, move |test_harness| async move {
		let mut virtual_overseer = test_harness.virtual_overseer;

		let TestState {
			validator_public,
			relay_parent: current,
			ancestors,
			candidates,
			pov_blocks,
			..
		} = test_state.clone();

		let genesis = Hash::repeat_byte(0xAA);
		change_our_view(
			&mut virtual_overseer,
			view![current],
			&validator_public,
			vec![ancestors[0], genesis],
			hashmap! { current => 1, genesis => 1 },
			hashmap! {
				ancestors[0] => vec![
					occupied_core_from_candidate(&candidates[0]),
					occupied_core_from_candidate(&candidates[1]),
				],
				current => vec![
					CoreState::Occupied(OccupiedCore {
						next_up_on_available: None,
						occupied_since: 0,
						time_out_at: 10,
						next_up_on_time_out: None,
						availability: Default::default(),
						group_responsible: GroupIndex::from(0),
						candidate_hash: candidates[0].hash(),
						candidate_descriptor: candidates[0].descriptor().clone(),
					}),
					CoreState::Free,
					CoreState::Free,
					CoreState::Occupied(OccupiedCore {
						next_up_on_available: None,
						occupied_since: 1,
						time_out_at: 7,
						next_up_on_time_out: None,
						availability: Default::default(),
						group_responsible: GroupIndex::from(0),
						candidate_hash: candidates[1].hash(),
						candidate_descriptor: candidates[1].descriptor().clone(),
					}),
					CoreState::Free,
					CoreState::Free,
				]
			},
			hashmap! {
				candidates[0].hash() => true,
				candidates[1].hash() => false,
			},
			hashmap! {
				candidates[0].hash() => (pov_blocks[0].clone(), test_state.persisted_validation_data.clone()),
			},
			hashmap! {},
		).await;

		// setup peer a with interest in current
		setup_peer_with_view(&mut virtual_overseer, peer_a.clone(), view![current]).await;

		// setup peer b with interest in ancestor
		setup_peer_with_view(&mut virtual_overseer, peer_b.clone(), view![ancestors[0]]).await;
	});

	assert_matches! {
		state,
		ProtocolState {
			peer_views,
			view,
			..
		} => {
			assert_eq!(
				peer_views,
				hashmap! {
					peer_a_2 => view![current],
					peer_b_2 => view![ancestors[0]],
				},
			);
			assert_eq!(view, view![current]);
		}
	};
}

#[test]
fn reputation_verification() {
	let test_state = TestState::default();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(&peer_a, &peer_b);

	let keystore = test_state.keystore.clone();

	test_harness(keystore, move |test_harness| async move {
		let mut virtual_overseer = test_harness.virtual_overseer;

		let TestState {
			relay_parent: current,
			validator_public,
			ancestors,
			candidates,
			pov_blocks,
			..
		} = test_state.clone();

		let valid = make_valid_availability_gossip(
			&test_state,
			0,
			2,
		);

		change_our_view(
			&mut virtual_overseer,
			view![current],
			&validator_public,
			vec![ancestors[0]],
			hashmap! { current => 1 },
			hashmap! {
				current => vec![
					occupied_core_from_candidate(&candidates[0]),
					occupied_core_from_candidate(&candidates[1]),
				],
			},
			hashmap! { candidates[0].hash() => true, candidates[1].hash() => false },
			hashmap! { candidates[0].hash() => (pov_blocks[0].clone(), test_state.persisted_validation_data.clone())},
			hashmap! {},
		).await;

		// valid (first, from b)
		peer_send_message(&mut virtual_overseer, peer_b.clone(), valid.clone(), BENEFIT_VALID_MESSAGE).await;

		// valid (duplicate, from b)
		peer_send_message(&mut virtual_overseer, peer_b.clone(), valid.clone(), COST_PEER_DUPLICATE_MESSAGE).await;

		// valid (second, from a)
		peer_send_message(&mut virtual_overseer, peer_a.clone(), valid.clone(), BENEFIT_VALID_MESSAGE).await;

		// send the a message again, so we should detect the duplicate
		peer_send_message(&mut virtual_overseer, peer_a.clone(), valid.clone(), COST_PEER_DUPLICATE_MESSAGE).await;

		// peer b sends a message before we have the view
		// setup peer a with interest in parent x
		overseer_send(&mut virtual_overseer, NetworkBridgeEvent::PeerDisconnected(peer_b.clone())).await;

		overseer_send(&mut virtual_overseer, NetworkBridgeEvent::PeerConnected(peer_b.clone(), ObservedRole::Full)).await;

		{
			// send another message
			let valid = make_valid_availability_gossip(&test_state, 1, 2);

			// Make peer a and b listen on `current`
			overseer_send(&mut virtual_overseer, NetworkBridgeEvent::PeerViewChange(peer_a.clone(), view![current])).await;

			let mut chunks = make_erasure_chunks(
				test_state.persisted_validation_data.clone(),
				validator_public.len(),
				pov_blocks[0].clone(),
			);

			// Both peers send us this chunk already
			chunks.remove(2);

			expect_chunks_network_message(&mut virtual_overseer, &[peer_a.clone()], &[candidates[0].hash()], &chunks).await;

			overseer_send(&mut virtual_overseer, NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![current])).await;

			expect_chunks_network_message(&mut virtual_overseer, &[peer_b.clone()], &[candidates[0].hash()], &chunks).await;

			peer_send_message(&mut virtual_overseer, peer_a.clone(), valid.clone(), BENEFIT_VALID_MESSAGE_FIRST).await;

			expect_chunks_network_message(
				&mut virtual_overseer,
				&[peer_b.clone()],
				&[candidates[1].hash()],
				&[valid.erasure_chunk.clone()],
			).await;

			// Let B send the same message
			peer_send_message(&mut virtual_overseer, peer_b.clone(), valid.clone(), BENEFIT_VALID_MESSAGE).await;
		}
	});
}

#[test]
fn not_a_live_candidate_is_detected() {
	let test_state = TestState::default();

	let peer_a = PeerId::random();

	let keystore = test_state.keystore.clone();

	test_harness(keystore, move |test_harness| async move {
		let mut virtual_overseer = test_harness.virtual_overseer;

		let TestState {
			relay_parent: current,
			validator_public,
			ancestors,
			candidates,
			pov_blocks,
			..
		} = test_state.clone();

		change_our_view(
			&mut virtual_overseer,
			view![current],
			&validator_public,
			vec![ancestors[0]],
			hashmap! { current => 1 },
			hashmap! {
				current => vec![
					occupied_core_from_candidate(&candidates[0]),
				],
			},
			hashmap! { candidates[0].hash() => true },
			hashmap! { candidates[0].hash() => (pov_blocks[0].clone(), test_state.persisted_validation_data.clone())},
			hashmap! {},
		).await;

		let valid = make_valid_availability_gossip(
			&test_state,
			1,
			1,
		);

		peer_send_message(&mut virtual_overseer, peer_a.clone(), valid.clone(), COST_NOT_A_LIVE_CANDIDATE).await;
	});
}

#[test]
fn peer_change_view_before_us() {
	let test_state = TestState::default();

	let peer_a = PeerId::random();

	let keystore = test_state.keystore.clone();

	test_harness(keystore, move |test_harness| async move {
		let mut virtual_overseer = test_harness.virtual_overseer;

		let TestState {
			relay_parent: current,
			validator_public,
			ancestors,
			candidates,
			pov_blocks,
			..
		} = test_state.clone();

		setup_peer_with_view(&mut virtual_overseer, peer_a.clone(), view![current]).await;

		change_our_view(
			&mut virtual_overseer,
			view![current],
			&validator_public,
			vec![ancestors[0]],
			hashmap! { current => 1 },
			hashmap! {
				current => vec![
					occupied_core_from_candidate(&candidates[0]),
				],
			},
			hashmap! { candidates[0].hash() => true },
			hashmap! { candidates[0].hash() => (pov_blocks[0].clone(), test_state.persisted_validation_data.clone())},
			hashmap! { candidates[0].hash() => vec![peer_a.clone()] },
		).await;

		let valid = make_valid_availability_gossip(
			&test_state,
			0,
			0,
		);

		// We send peer a all the chunks of candidate0, so we just benefit him for sending a valid message
		peer_send_message(&mut virtual_overseer, peer_a.clone(), valid.clone(), BENEFIT_VALID_MESSAGE).await;
	});
}

#[test]
fn candidate_chunks_are_put_into_message_vault_when_candidate_is_first_seen() {
	let test_state = TestState::default();

	let peer_a = PeerId::random();

	let keystore = test_state.keystore.clone();

	test_harness(keystore, move |test_harness| async move {
		let mut virtual_overseer = test_harness.virtual_overseer;

		let TestState {
			relay_parent: current,
			validator_public,
			ancestors,
			candidates,
			pov_blocks,
			..
		} = test_state.clone();

		change_our_view(
			&mut virtual_overseer,
			view![ancestors[0]],
			&validator_public,
			vec![ancestors[1]],
			hashmap! { ancestors[0] => 1 },
			hashmap! {
				ancestors[0] => vec![
					occupied_core_from_candidate(&candidates[0]),
				],
			},
			hashmap! { candidates[0].hash() => true },
			hashmap! { candidates[0].hash() => (pov_blocks[0].clone(), test_state.persisted_validation_data.clone())},
			hashmap! {},
		).await;

		change_our_view(
			&mut virtual_overseer,
			view![current],
			&validator_public,
			vec![ancestors[0]],
			hashmap! { current => 1 },
			hashmap! {
				current => vec![
					occupied_core_from_candidate(&candidates[0]),
				],
			},
			hashmap! { candidates[0].hash() => true },
			hashmap! {},
			hashmap! {},
		).await;

		// Let peera connect, we should send him all the chunks of the candidate
		setup_peer_with_view(&mut virtual_overseer, peer_a.clone(), view![current]).await;

		let chunks = make_erasure_chunks(
			test_state.persisted_validation_data.clone(),
			validator_public.len(),
			pov_blocks[0].clone(),
		);
		expect_chunks_network_message(
			&mut virtual_overseer,
			&[peer_a],
			&[candidates[0].hash()],
			&chunks,
		).await;
	});
}

#[test]
fn k_ancestors_in_session() {
	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut virtual_overseer) =
		test_helpers::make_subsystem_context::<AvailabilityDistributionMessage, _>(pool);

	const DATA: &[(Hash, SessionIndex)] = &[
		(Hash::repeat_byte(0x32), 3), // relay parent
		(Hash::repeat_byte(0x31), 3), // grand parent
		(Hash::repeat_byte(0x30), 3), // great ...
		(Hash::repeat_byte(0x20), 2),
		(Hash::repeat_byte(0x12), 1),
		(Hash::repeat_byte(0x11), 1),
		(Hash::repeat_byte(0x10), 1),
	];
	const K: usize = 5;

	const EXPECTED: &[Hash] = &[DATA[1].0, DATA[2].0];

	let test_fut = async move {
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ChainApi(ChainApiMessage::Ancestors {
				hash: relay_parent,
				k,
				response_channel: tx,
			}) => {
				assert_eq!(k, K+1);
				assert_eq!(relay_parent, DATA[0].0);
				tx.send(Ok(DATA[1..=k].into_iter().map(|x| x.0).collect::<Vec<_>>())).unwrap();
			}
		);

		// query the desired session index of the relay parent
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionIndexForChild(tx),
			)) => {
				assert_eq!(relay_parent, DATA[0].0);
				let session: SessionIndex = DATA[0].1;
				tx.send(Ok(session)).unwrap();
			}
		);

		// query ancestors
		for i in 2usize..=(EXPECTED.len() + 1 + 1) {
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::SessionIndexForChild(tx),
				)) => {
					// query is for ancestor_parent
					let x = &DATA[i];
					assert_eq!(relay_parent, x.0);
					// but needs to yield ancestor_parent's child's session index
					let x = &DATA[i-1];
					tx.send(Ok(x.1)).unwrap();
				}
			);
		}
	};

	let sut = async move {
		let ancestors = query_up_to_k_ancestors_in_same_session(&mut ctx, DATA[0].0, K)
			.await
			.unwrap();
		assert_eq!(ancestors, EXPECTED.to_vec());
	};

	futures::pin_mut!(test_fut);
	futures::pin_mut!(sut);

	executor::block_on(future::join(test_fut, sut).timeout(Duration::from_millis(1000)));
}

#[test]
fn clean_up_receipts_cache_unions_ancestors_and_view() {
	let mut state = ProtocolState::default();

	let hash_a = [0u8; 32].into();
	let hash_b = [1u8; 32].into();
	let hash_c = [2u8; 32].into();
	let hash_d = [3u8; 32].into();

	state.live_under.insert(hash_a, HashSet::new());
	state.live_under.insert(hash_b, HashSet::new());
	state.live_under.insert(hash_c, HashSet::new());
	state.live_under.insert(hash_d, HashSet::new());

	state.per_relay_parent.insert(hash_a, PerRelayParent {
		ancestors: vec![hash_b],
		live_candidates: HashSet::new(),
	});

	state.per_relay_parent.insert(hash_c, PerRelayParent::default());

	state.clean_up_live_under_cache();

	assert_eq!(state.live_under.len(), 3);
	assert!(state.live_under.contains_key(&hash_a));
	assert!(state.live_under.contains_key(&hash_b));
	assert!(state.live_under.contains_key(&hash_c));
	assert!(!state.live_under.contains_key(&hash_d));
}

#[test]
fn remove_relay_parent_only_removes_per_candidate_if_final() {
	let mut state = ProtocolState::default();

	let hash_a = [0u8; 32].into();
	let hash_b = [1u8; 32].into();

	let candidate_hash_a = CandidateHash([46u8; 32].into());

	state.per_relay_parent.insert(hash_a, PerRelayParent {
		ancestors: vec![],
		live_candidates: std::iter::once(candidate_hash_a).collect(),
	});

	state.per_relay_parent.insert(hash_b, PerRelayParent {
		ancestors: vec![],
		live_candidates: std::iter::once(candidate_hash_a).collect(),
	});

	state.per_candidate.insert(candidate_hash_a, PerCandidate {
		live_in: vec![hash_a, hash_b].into_iter().collect(),
		..Default::default()
	});

	state.remove_relay_parent(&hash_a);

	assert!(!state.per_relay_parent.contains_key(&hash_a));
	assert!(!state.per_candidate.get(&candidate_hash_a).unwrap().live_in.contains(&hash_a));
	assert!(state.per_candidate.get(&candidate_hash_a).unwrap().live_in.contains(&hash_b));

	state.remove_relay_parent(&hash_b);

	assert!(!state.per_relay_parent.contains_key(&hash_b));
	assert!(!state.per_candidate.contains_key(&candidate_hash_a));
}

#[test]
fn add_relay_parent_includes_all_live_candidates() {
	let relay_parent = [0u8; 32].into();

	let mut state = ProtocolState::default();

	let ancestor_a = [1u8; 32].into();

	let candidate_hash_a = CandidateHash([10u8; 32].into());
	let candidate_hash_b = CandidateHash([11u8; 32].into());

	let candidates = vec![
		(candidate_hash_a, FetchedLiveCandidate::Fresh(Default::default())),
		(candidate_hash_b, FetchedLiveCandidate::Cached),
	].into_iter().collect();

	state.add_relay_parent(
		relay_parent,
		Vec::new(),
		None,
		candidates,
		vec![ancestor_a],
	);

	assert!(
		state.per_candidate.get(&candidate_hash_a).unwrap().live_in.contains(&relay_parent)
	);
	assert!(
		state.per_candidate.get(&candidate_hash_b).unwrap().live_in.contains(&relay_parent)
	);

	let per_relay_parent = state.per_relay_parent.get(&relay_parent).unwrap();

	assert!(per_relay_parent.live_candidates.contains(&candidate_hash_a));
	assert!(per_relay_parent.live_candidates.contains(&candidate_hash_b));
}

#[test]
fn query_pending_availability_at_pulls_from_and_updates_receipts() {
	let hash_a = [0u8; 32].into();
	let hash_b = [1u8; 32].into();

	let para_a = ParaId::from(1);
	let para_b = ParaId::from(2);
	let para_c = ParaId::from(3);

	let make_candidate = |para_id| {
		let mut candidate = CommittedCandidateReceipt::default();
		candidate.descriptor.para_id = para_id;
		candidate.descriptor.relay_parent = [69u8; 32].into();
		candidate
	};

	let candidate_a = make_candidate(para_a);
	let candidate_b = make_candidate(para_b);
	let candidate_c = make_candidate(para_c);

	let candidate_hash_a = candidate_a.hash();
	let candidate_hash_b = candidate_b.hash();
	let candidate_hash_c = candidate_c.hash();

	// receipts has an initial entry for hash_a but not hash_b.
	let mut receipts = HashMap::new();
	receipts.insert(hash_a, vec![candidate_hash_a, candidate_hash_b].into_iter().collect());

	let pool = sp_core::testing::TaskExecutor::new();

	let (mut ctx, mut virtual_overseer) =
		test_helpers::make_subsystem_context::<AvailabilityDistributionMessage, _>(pool);

	let test_fut = async move {
		let live_candidates = query_pending_availability_at(
			&mut ctx,
			vec![hash_a, hash_b],
			&mut receipts,
		).await.unwrap();

		// although 'b' is cached from the perspective of hash_a, it gets overwritten when we query what's happening in
		//
		assert_eq!(live_candidates.len(), 3);
		assert_matches!(live_candidates.get(&candidate_hash_a).unwrap(), FetchedLiveCandidate::Cached);
		assert_matches!(live_candidates.get(&candidate_hash_b).unwrap(), FetchedLiveCandidate::Cached);
		assert_matches!(live_candidates.get(&candidate_hash_c).unwrap(), FetchedLiveCandidate::Fresh(_));

		assert!(receipts.get(&hash_b).unwrap().contains(&candidate_hash_b));
		assert!(receipts.get(&hash_b).unwrap().contains(&candidate_hash_c));
	};

	let answer = async move {
		// hash_a should be answered out of cache, so we should just have
		// queried for hash_b.
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(
					r,
					RuntimeApiRequest::AvailabilityCores(tx),
				)
			) if r == hash_b => {
				let _ = tx.send(Ok(vec![
					CoreState::Occupied(OccupiedCore {
						next_up_on_available: None,
						occupied_since: 0,
						time_out_at: 0,
						next_up_on_time_out: None,
						availability: Default::default(),
						group_responsible: GroupIndex::from(0),
						candidate_hash: candidate_hash_b,
						candidate_descriptor: candidate_b.descriptor.clone(),
					}),
					CoreState::Occupied(OccupiedCore {
						next_up_on_available: None,
						occupied_since: 0,
						time_out_at: 0,
						next_up_on_time_out: None,
						availability: Default::default(),
						group_responsible: GroupIndex::from(0),
						candidate_hash: candidate_hash_c,
						candidate_descriptor: candidate_c.descriptor.clone(),
					}),
				]));
			}
		);
	};

	futures::pin_mut!(test_fut);
	futures::pin_mut!(answer);

	executor::block_on(future::join(test_fut, answer));
}

#[test]
fn new_peer_gets_all_chunks_send() {
	let test_state = TestState::default();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(&peer_a, &peer_b);

	let keystore = test_state.keystore.clone();

	test_harness(keystore, move |test_harness| async move {
		let mut virtual_overseer = test_harness.virtual_overseer;

		let TestState {
			relay_parent: current,
			validator_public,
			ancestors,
			candidates,
			pov_blocks,
			..
		} = test_state.clone();

		let valid = make_valid_availability_gossip(
			&test_state,
			1,
			2,
		);

		change_our_view(
			&mut virtual_overseer,
			view![current],
			&validator_public,
			vec![ancestors[0]],
			hashmap! { current => 1 },
			hashmap! {
				current => vec![
					occupied_core_from_candidate(&candidates[0]),
					occupied_core_from_candidate(&candidates[1])
				],
			},
			hashmap! { candidates[0].hash() => true, candidates[1].hash() => false },
			hashmap! { candidates[0].hash() => (pov_blocks[0].clone(), test_state.persisted_validation_data.clone())},
			hashmap! {},
		).await;

		peer_send_message(&mut virtual_overseer, peer_b.clone(), valid.clone(), BENEFIT_VALID_MESSAGE_FIRST).await;

		setup_peer_with_view(&mut virtual_overseer, peer_a.clone(), view![current]).await;

		let mut chunks = make_erasure_chunks(
			test_state.persisted_validation_data.clone(),
			validator_public.len(),
			pov_blocks[0].clone(),
		);

		chunks.push(valid.erasure_chunk);

		expect_chunks_network_message(
			&mut virtual_overseer,
			&[peer_a],
			&[candidates[0].hash(), candidates[1].hash()],
			&chunks,
		).await;
	});
}
