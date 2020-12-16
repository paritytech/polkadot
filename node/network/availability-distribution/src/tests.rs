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
use polkadot_node_network_protocol::ObservedRole;
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_primitives::v1::{
	AvailableData, BlockData, CandidateCommitments, CandidateDescriptor, GroupIndex,
	GroupRotationInfo, HeadData, OccupiedCore, PersistedValidationData, PoV, ScheduledCore,
	ValidatorId,
};
use polkadot_subsystem_testhelpers as test_helpers;

use futures::{executor, future, Future};
use sc_keystore::LocalKeystore;
use sp_application_crypto::AppKey;
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
use sp_keyring::Sr25519Keyring;
use std::{sync::Arc, time::Duration};
use maplit::{hashset, hashmap};

macro_rules! view {
	( $( $hash:expr ),* $(,)? ) => {
		View(vec![ $( $hash.clone() ),* ])
	};
	[ $( $hash:expr ),* $(,)? ] => {
		View(vec![ $( $hash.clone() ),* ])
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
	mut state: ProtocolState,
	keystore: SyncCryptoStorePtr,
	test_fx: impl FnOnce(TestHarness) -> T,
) -> ProtocolState {
	sp_tracing::try_init_simple();

	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = AvailabilityDistributionSubsystem::new(keystore, Default::default());
	{
		let subsystem = subsystem.run_inner(context, &mut state);

		let test_fut = test_fx(TestHarness { virtual_overseer });

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}
	state
}

const TIMEOUT: Duration = Duration::from_millis(100);

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

fn dummy_occupied_core(para: ParaId) -> CoreState {
	CoreState::Occupied(OccupiedCore {
		para_id: para,
		next_up_on_available: None,
		occupied_since: 0,
		time_out_at: 5,
		next_up_on_time_out: None,
		availability: Default::default(),
		group_responsible: GroupIndex::from(0),
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
			Hash::repeat_byte(0x11),
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

fn make_valid_availability_gossip(
	test: &TestState,
	candidate_hash: CandidateHash,
	erasure_chunk_index: u32,
	pov: PoV,
) -> AvailabilityGossipMessage {
	let available_data = make_available_data(test.persisted_validation_data.clone(), pov);

	let erasure_chunks = derive_erasure_chunks_with_proofs(test.validators.len(), &available_data);

	let erasure_chunk: ErasureChunk = erasure_chunks
		.get(erasure_chunk_index as usize)
		.expect("Must be valid or input is oob")
		.clone();

	AvailabilityGossipMessage {
		candidate_hash,
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
		test_state.candidates[0].hash(),
		2,
		test_state.pov_blocks[0].clone(),
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

async fn change_our_view(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	view: View,
	validator_public: &[ValidatorId],
	ancestors: Vec<Hash>,
	session_per_relay_parent: HashMap<Hash, SessionIndex>,
	availability_cores_per_relay_parent: HashMap<Hash, Vec<CoreState>>,
	candidate_pending_availabilities_per_relay_parent: HashMap<Hash, Vec<CommittedCandidateReceipt>>,
	data_availability: HashMap<CandidateHash, bool>,
	chunk_data_per_candidate: HashMap<CandidateHash, (PoV, PersistedValidationData)>,
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
		let relay_parent = assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::AvailabilityCores(tx)
			)) => {
				let cores = availability_cores_per_relay_parent.get(&relay_parent)
					.expect(&format!("Availability core for relay parent {:?} does not exist", relay_parent));

				tx.send(Ok(cores.clone())).unwrap();
				relay_parent
			}
		);

		let pending_availability = candidate_pending_availabilities_per_relay_parent.get(&relay_parent)
			.expect(&format!("Candidate pending availability for relay parent {:?} does not exist", relay_parent));

		for _ in 0..pending_availability.len() {
			assert_matches!(
				overseer_recv(virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					hash,
					RuntimeApiRequest::CandidatePendingAvailability(para, tx)
				)) => {
					assert_eq!(relay_parent, hash);

					let candidate = pending_availability.iter()
						.find(|c| c.descriptor.para_id == para)
						.expect(&format!("Pending candidate for para {} does not exist", para));

					tx.send(Ok(Some(candidate.clone()))).unwrap();
				}
			);
		}
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
			let avail_data = make_available_data(persisted.clone(), pov.clone());
			let chunks = derive_erasure_chunks_with_proofs(validator_public.len(), &avail_data);

			for _ in 0..chunks.len() {
				assert_matches!(
					overseer_recv(virtual_overseer).await,
					AllMessages::AvailabilityStore(
						AvailabilityStoreMessage::QueryChunk(
							candidate_hash,
							index,
							tx,
						)
					) => {
						tracing::trace!("Query chunk {} for candidate {:?}", index, candidate_hash);
						tx.send(Some(chunks[index as usize].clone())).unwrap();
					}
				)
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

	let state = test_harness(ProtocolState::default(), keystore, move |test_harness| async move {
		let mut virtual_overseer = test_harness.virtual_overseer;

		let TestState {
			chain_ids,
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
				ancestors[0] => vec![dummy_occupied_core(chain_ids[0]), dummy_occupied_core(chain_ids[1])],
				current => vec![
					CoreState::Occupied(OccupiedCore {
						para_id: chain_ids[0].clone(),
						next_up_on_available: None,
						occupied_since: 0,
						time_out_at: 10,
						next_up_on_time_out: None,
						availability: Default::default(),
						group_responsible: GroupIndex::from(0),
					}),
					CoreState::Free,
					CoreState::Free,
					CoreState::Occupied(OccupiedCore {
						para_id: chain_ids[1].clone(),
						next_up_on_available: None,
						occupied_since: 1,
						time_out_at: 7,
						next_up_on_time_out: None,
						availability: Default::default(),
						group_responsible: GroupIndex::from(0),
					}),
					CoreState::Free,
					CoreState::Free,
				]
			},
			hashmap! {
				ancestors[0] => vec![candidates[0].clone(), candidates[1].clone()],
				current => vec![candidates[0].clone(), candidates[1].clone()],
			},
			hashmap! {
				candidates[0].hash() => true,
				candidates[1].hash() => false,
			},
			hashmap! {
				candidates[0].hash() => (pov_blocks[0].clone(), test_state.persisted_validation_data.clone()),
			}
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

	test_harness(ProtocolState::default(), keystore, move |test_harness| async move {
		let mut virtual_overseer = test_harness.virtual_overseer;

		let TestState {
			relay_parent: current,
			validator_public,
			ancestors,
			candidates,
			pov_blocks,
			..
		} = test_state.clone();

		let valid: AvailabilityGossipMessage = make_valid_availability_gossip(
			&test_state,
			candidates[1].hash(),
			2,
			pov_blocks[1].clone(),
		);

		change_our_view(
			&mut virtual_overseer,
			view![current],
			&validator_public,
			vec![ancestors[0]],
			hashmap! { current => 1 },
			hashmap! {
				current => vec![
					dummy_occupied_core(candidates[0].descriptor.para_id),
					dummy_occupied_core(candidates[1].descriptor.para_id),
				],
			},
			hashmap! {
				current => vec![
					candidates[0].clone(),
					candidates[1].clone(),
				],
			},
			hashmap! {
				candidates[0].hash() => true,
				candidates[1].hash() => false
			},
			hashmap! {
				candidates[0].hash() => (pov_blocks[0].clone(), test_state.persisted_validation_data.clone()),
			},
		).await;

		{
			// valid (first, from b)
			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						chunk_protocol_message(valid.clone()),
					),
				),
			)
			.await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(
						peer,
						rep
					)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, BENEFIT_VALID_MESSAGE_FIRST);
				}
			);
		}

		{
			// valid (duplicate, from b)
			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						chunk_protocol_message(valid.clone()),
					),
				),
			)
			.await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(
						peer,
						rep
					)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_PEER_DUPLICATE_MESSAGE);
				}
			);
		}

		{
			// valid (second, from a)
			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_a.clone(),
						chunk_protocol_message(valid.clone()),
					),
				),
			)
			.await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(
						peer,
						rep
					)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, BENEFIT_VALID_MESSAGE);
				}
			);
		}

		// peer a is not interested in anything anymore
		overseer_send(
			&mut virtual_overseer,
			AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerViewChange(peer_a.clone(), view![]),
			),
		)
		.await;

		{
			// send the a message again, so we should detect the duplicate
			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_a.clone(),
						chunk_protocol_message(valid.clone()),
					),
				),
			)
			.await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(
						peer,
						rep
					)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, COST_PEER_DUPLICATE_MESSAGE);
				}
			);
		}

		// peer b sends a message before we have the view
		// setup peer a with interest in parent x
		overseer_send(
			&mut virtual_overseer,
			AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerDisconnected(peer_b.clone()),
			),
		)
		.await;

		overseer_send(
			&mut virtual_overseer,
			AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(peer_b.clone(), ObservedRole::Full),
			),
		)
		.await;
	});
}

#[test]
fn reputation_multiple_peers_same_chunk() {
	let test_state = TestState::default();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(&peer_a, &peer_b);

	let keystore = test_state.keystore.clone();

	test_harness(ProtocolState::default(), keystore, move |test_harness| async move {
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
			hashmap! {
				current => 1,
			},
			hashmap! {
				current => vec![
					dummy_occupied_core(candidates[0].descriptor.para_id),
					dummy_occupied_core(candidates[1].descriptor.para_id)
				],
			},
			hashmap! {
				current => vec![
					candidates[0].clone(),
					candidates[1].clone(),
				],
			},
			hashmap! {
				candidates[0].hash() => false,
				candidates[1].hash() => false,
			},
			hashmap! {
			},
		).await;

		let valid = make_valid_availability_gossip(
			&test_state,
			candidates[1].hash(),
			2,
			pov_blocks[1].clone(),
		);

		// Make peer a and b listen on `current`
		overseer_send(
			&mut virtual_overseer,
			AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerViewChange(peer_a.clone(), view![current]),
			),
		)
		.await;

		overseer_send(
			&mut virtual_overseer,
			AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![current]),
			),
		)
		.await;

		overseer_send(
			&mut virtual_overseer,
			AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					chunk_protocol_message(valid.clone()),
				),
			),
		)
		.await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(
					peer,
					rep
				)
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, BENEFIT_VALID_MESSAGE_FIRST);
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendValidationMessage(
					peers,
					protocol_v1::ValidationProtocol::AvailabilityDistribution(
						protocol_v1::AvailabilityDistributionMessage::Chunk(hash, chunk),
					),
				)
			) => {
				assert_eq!(1, peers.len());
				assert_eq!(peers[0], peer_b);
				assert_eq!(valid.candidate_hash, hash);
				assert_eq!(candidates[1].hash(), hash);
				assert_eq!(valid.erasure_chunk, chunk);
			}
		);

		// Let peer B send the same message as peer A just did
		overseer_send(
			&mut virtual_overseer,
			AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					chunk_protocol_message(valid.clone()),
				),
			),
		)
		.await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(
					peer,
					rep
				)
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, BENEFIT_VALID_MESSAGE);
			}
		);

		// There shouldn't be any other message.
		assert!(virtual_overseer.recv().timeout(TIMEOUT).await.is_none());

	});
}

#[test]
fn not_a_live_candidate_is_detected() {
	let test_state = TestState::default();

	let peer_a = PeerId::random();

	let keystore = test_state.keystore.clone();

	test_harness(ProtocolState::default(), keystore, move |test_harness| async move {
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
					dummy_occupied_core(candidates[0].descriptor.para_id),
				],
			},
			hashmap! { current => vec![candidates[0].clone()] },
			hashmap! { candidates[0].hash() => true },
			hashmap! { candidates[0].hash() => (pov_blocks[0].clone(), test_state.persisted_validation_data.clone())},
		).await;

		{
			let valid2 = make_valid_availability_gossip(
				&test_state,
				candidates[1].hash(),
				1,
				pov_blocks[1].clone(),
			);

			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(peer_a.clone(), chunk_protocol_message(valid2)),
				),
			)
			.await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(
						peer,
						rep
					)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, COST_NOT_A_LIVE_CANDIDATE);
				}
			);
		}
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

	let hash_a = Hash::repeat_byte(0x00);
	let hash_b = Hash::repeat_byte(0x01);
	let hash_c = Hash::repeat_byte(0x02);
	let hash_d = Hash::repeat_byte(0x03);

	state.receipts.insert(hash_a, HashSet::new());
	state.receipts.insert(hash_b, HashSet::new());
	state.receipts.insert(hash_c, HashSet::new());
	state.receipts.insert(hash_d, HashSet::new());

	state.per_relay_parent.insert(hash_a, PerRelayParent {
		ancestors: vec![hash_b],
		live_candidates: HashSet::new(),
	});

	state.per_relay_parent.insert(hash_c, PerRelayParent::default());

	state.clean_up_receipts_cache();

	assert_eq!(state.receipts.len(), 3);
	assert!(state.receipts.contains_key(&hash_a));
	assert!(state.receipts.contains_key(&hash_b));
	assert!(state.receipts.contains_key(&hash_c));
	assert!(!state.receipts.contains_key(&hash_d));
}

#[test]
fn remove_relay_parent_only_removes_per_candidate_if_final() {
	let mut state = ProtocolState::default();

	let hash_a = Hash::repeat_byte(0);
	let hash_b = Hash::repeat_byte(1);

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
	let relay_parent = Hash::repeat_byte(0x00);

	let mut state = ProtocolState::default();

	let ancestor_a = Hash::repeat_byte(1);

	let candidate_hash_a = CandidateHash(Hash::repeat_byte(10));
	let candidate_hash_b = CandidateHash(Hash::repeat_byte(11));

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
	let hash_a = Hash::repeat_byte(0u8);
	let hash_b = Hash::repeat_byte(1u8);

	let para_a = ParaId::from(1);
	let para_b = ParaId::from(2);
	let para_c = ParaId::from(3);

	let make_candidate = |para_id| {
		let mut candidate = CommittedCandidateReceipt::default();
		candidate.descriptor.para_id = para_id;
		candidate.descriptor.relay_parent = Hash::repeat_byte(69u8);
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
						para_id: para_b,
						next_up_on_available: None,
						occupied_since: 0,
						time_out_at: 0,
						next_up_on_time_out: None,
						availability: Default::default(),
						group_responsible: GroupIndex::from(0),
					}),
					CoreState::Occupied(OccupiedCore {
						para_id: para_c,
						next_up_on_available: None,
						occupied_since: 0,
						time_out_at: 0,
						next_up_on_time_out: None,
						availability: Default::default(),
						group_responsible: GroupIndex::from(0),
					}),
				]));
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(
					r,
					RuntimeApiRequest::CandidatePendingAvailability(p, tx),
				)
			) if r == hash_b && p == para_b => {
				let _ = tx.send(Ok(Some(candidate_b)));
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(
					r,
					RuntimeApiRequest::CandidatePendingAvailability(p, tx),
				)
			) if r == hash_b && p == para_c => {
				let _ = tx.send(Ok(Some(candidate_c)));
			}
		);
	};

	futures::pin_mut!(test_fut);
	futures::pin_mut!(answer);

	executor::block_on(future::join(test_fut, answer));
}


#[test]
fn candidates_overlapping() {
	let test_state = TestState::default();

	// use the same PoV, nobody cares for the test
	let pov_block = PoV {
		block_data: BlockData(vec![48, 49, 50]),
	};

	let pov_hash = pov_block.hash();

	// 4 ancestors, allows us to create two overlapping sets of ancestors
	// of size 3
	let ancestors = &[
		Hash::repeat_byte(0xA0),
		Hash::repeat_byte(0xA1),
		Hash::repeat_byte(0xA2),
		Hash::repeat_byte(0xA3),
	];

	let mut state = ProtocolState {
		view: view![],
		per_candidate: hashmap!{},
		per_relay_parent: hashmap!{},
		.. Default::default()
	};

	let validators = validator_pubkeys(&test_state.validators);
	let validator_index = Some(0 as ValidatorIndex);
	let relay_parent = test_state.relay_parent;

	let create_candidate = |ancestor: Hash| {
		TestCandidateBuilder {
			para_id: test_state.chain_ids[1],
			relay_parent: ancestor,
			pov_hash,
			erasure_root: make_erasure_root(test_state.persisted_validation_data.clone(), test_state.validators.len(), pov_block.clone()),
			head_data: test_state
				.head_data
				.get(&test_state.chain_ids[1])
				.unwrap()
				.clone(),
			..Default::default()
		}
		.build()
	};

	// create the same candidate for all ancestors
	let candidate_rp = create_candidate(relay_parent);
	let candidate_a0 = create_candidate(ancestors[0]);
	let candidate_a1 = create_candidate(ancestors[1]);
	let candidate_a2 = create_candidate(ancestors[2]);
	let candidate_a3 = create_candidate(ancestors[3]);

	let candidate_set_rp = hashmap!{
		candidate_rp.hash() => FetchedLiveCandidate::Fresh(candidate_rp.descriptor.clone()),
		candidate_a0.hash() => FetchedLiveCandidate::Fresh(candidate_a0.descriptor.clone()),
		candidate_a1.hash() => FetchedLiveCandidate::Fresh(candidate_a1.descriptor.clone()),
		candidate_a2.hash() => FetchedLiveCandidate::Fresh(candidate_a2.descriptor.clone()),
	};
	let candidate_set_a0 = hashmap!{
		candidate_a0.hash() => FetchedLiveCandidate::Fresh(candidate_a0.descriptor.clone()),
		candidate_a1.hash() => FetchedLiveCandidate::Fresh(candidate_a1.descriptor.clone()),
		candidate_a2.hash() => FetchedLiveCandidate::Fresh(candidate_a2.descriptor.clone()),
		candidate_a3.hash() => FetchedLiveCandidate::Fresh(candidate_a3.descriptor.clone()),
	};

	state.add_relay_parent(relay_parent, validators.clone(), validator_index, candidate_set_rp, ancestors.to_vec());

	assert!(state.per_candidate.get(&candidate_rp.hash()).is_some());
	assert!(state.per_candidate.get(&candidate_a0.hash()).is_some());
	assert!(state.per_candidate.get(&candidate_a1.hash()).is_some());
	assert!(state.per_candidate.get(&candidate_a2.hash()).is_some());
	assert!(state.per_candidate.get(&candidate_a3.hash()).is_none());

	state.add_relay_parent(ancestors[0], validators.clone(), validator_index, candidate_set_a0, ancestors.to_vec());


	assert!(state.per_candidate.get(&candidate_rp.hash()).unwrap().live_in.contains(&relay_parent));
	assert!(state.per_candidate.get(&candidate_a0.hash()).unwrap().live_in.contains(&relay_parent));

	// replacing the view is part of `handle_our_view_change, so the view does not change
	// here

	assert!(state.per_candidate.get(&candidate_rp.hash()).is_some());
	assert!(state.per_candidate.get(&candidate_a0.hash()).is_some());
	assert!(state.per_candidate.get(&candidate_a1.hash()).is_some());
	assert!(state.per_candidate.get(&candidate_a2.hash()).is_some());
	assert!(state.per_candidate.get(&candidate_a3.hash()).is_some());

	state.remove_relay_parent(&relay_parent);


	assert!(state.per_candidate.get(&candidate_rp.hash()).is_none());
	assert!(state.per_candidate.get(&candidate_a0.hash()).is_some());
	assert!(state.per_candidate.get(&candidate_a1.hash()).is_some());
	assert!(state.per_candidate.get(&candidate_a2.hash()).is_some());
	assert!(state.per_candidate.get(&candidate_a3.hash()).is_some());

	state.remove_relay_parent(&ancestors[0]);

	assert!(state.per_candidate.get(&candidate_rp.hash()).is_none());
	assert!(state.per_candidate.get(&candidate_a0.hash()).is_none());
	assert!(state.per_candidate.get(&candidate_a1.hash()).is_none());
	assert!(state.per_candidate.get(&candidate_a2.hash()).is_none());
	assert!(state.per_candidate.get(&candidate_a3.hash()).is_none());

	assert_matches!(state.per_candidate.get(&candidate_rp.hash()), None => {});
	assert_matches!(state.per_candidate.get(&candidate_a0.hash()), None => {});
}

#[test]
fn view_setup_w_overlapping_ancestors_teardown() {

	let validator_index = Some(0 as ValidatorIndex);

	let test_state = TestState::default();

	let pov_block_a = PoV {
		block_data: BlockData(vec![42, 43, 44]),
	};

	let pov_block_b = PoV {
		block_data: BlockData(vec![45, 46, 47]),
	};

	let pov_hash_a = pov_block_a.hash();
	let pov_hash_b = pov_block_b.hash();

	let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0])
		.unwrap();

	// two candidates for different relay parents
	let candidates = vec![
		TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash: pov_hash_a,
			erasure_root: make_erasure_root(test_state.persisted_validation_data.clone(), test_state.validators.len(), pov_block_a),
			..Default::default()
		}
		.build(),
		TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.ancestors[0],
			pov_hash: pov_hash_b,
			erasure_root: make_erasure_root(test_state.persisted_validation_data.clone(), test_state.validators.len(), pov_block_b.clone()),
			head_data: expected_head_data.clone(),
			..Default::default()
		}
		.build(),
	];

	let candidate_hash_a = candidates[0].hash();
	let candidate_hash_b = candidates[1].hash();

	let state = ProtocolState {
		view: view![test_state.relay_parent, test_state.ancestors[0]],
		receipts: hashmap!{
			test_state.relay_parent => hashset!{
				candidate_hash_a,
				candidate_hash_b,
			},
			test_state.ancestors[0] => hashset!{
				candidate_hash_a,
				candidate_hash_b,
			},
		},
		per_candidate : hashmap!{
			candidate_hash_a => PerCandidate {
				descriptor: candidates[0].descriptor.clone(),
				validators: test_state.validator_public.clone(),
				validator_index: validator_index.clone(),
				live_in: hashset!{
					test_state.relay_parent,
					test_state.ancestors[0],
				},
				.. Default::default()
			},
			candidate_hash_b => PerCandidate {
				descriptor: candidates[1].descriptor.clone(),
				validators: test_state.validator_public.clone(),
				validator_index: validator_index.clone(),
				live_in: hashset!{
					test_state.relay_parent,
					test_state.ancestors[0],
				},
				.. Default::default()
			},
		},
		per_relay_parent: hashmap!{
			test_state.relay_parent => PerRelayParent {
				ancestors: vec![
					test_state.ancestors[0],
					test_state.ancestors[1],
					test_state.ancestors[2],
				],
				live_candidates: hashset!{
					candidate_hash_a,
					candidate_hash_b,
				},
			},
			test_state.ancestors[0] => PerRelayParent {
				ancestors: vec![
					test_state.ancestors[1],
					test_state.ancestors[2],
				],
				live_candidates: hashset!{
					candidate_hash_a,
					candidate_hash_b,
				},
			},
		},
		.. Default::default()
	};

	let keystore = test_state.keystore.clone();
	let state = test_harness(state, keystore, move |test_harness| async move {

		let TestHarness {
			mut virtual_overseer,
			// keystore,
		} = test_harness;

		{
			// Clear our view
			overseer_send(
				&mut virtual_overseer,
				AvailabilityDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(view![]),
				),
			)
			.await;


			// There shouldn't be any other message.
			assert!(virtual_overseer.recv().timeout(TIMEOUT).await.is_none());
		}
	});

	assert_matches!(state, ProtocolState {
		per_candidate,
		per_relay_parent,
		..
	} => {
		assert!(per_candidate.is_empty());
		assert!(per_relay_parent.is_empty());
	});

}

#[test]
fn peers_with_empty_views() {

	let test_state = TestState::default();

	// two peers with random views each
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(&peer_a, &peer_b);

	let pov_block_a = PoV {
		block_data: BlockData(vec![42, 43, 44]),
	};

	let pov_block_b = PoV {
		block_data: BlockData(vec![45, 46, 47]),
	};

	let make_candidate = |relay_parent: Hash, pov: &PoV| {
		TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent,
			pov_hash: pov.hash(),
			erasure_root: make_erasure_root(test_state.persisted_validation_data.clone(), test_state.validators.len(), pov.clone()),
			head_data: test_state
				.head_data
				.get(&test_state.chain_ids[0])
				.unwrap()
				.clone(),
			..Default::default()
		}
		.build()
	};

	let candidate_a = make_candidate(test_state.relay_parent, &pov_block_a);
	let candidate_b = make_candidate(test_state.ancestors[0], &pov_block_b);

	let candidate_hash_a = candidate_a.hash();
	let candidate_hash_b = candidate_b.hash();

	let validator_index = Some(0 as ValidatorIndex);

	let mut state = ProtocolState {
		peer_views: hashmap!{
				peer_b.clone() => view![
					// empty view of peer b
				],
				peer_a.clone() => view![
					// empty view of peer a
				],
			},
		view: view![test_state.relay_parent],
		receipts: hashmap!{
			test_state.ancestors[0] => hashset!{
				candidate_hash_a,
				candidate_hash_b,
			},
			test_state.relay_parent => hashset!{
				candidate_hash_a,
				candidate_hash_b,
			},
		},
		per_candidate : hashmap!{
			candidate_hash_a => PerCandidate {
				descriptor: candidate_a.descriptor.clone(),
				validators: test_state.validator_public.clone(),
				validator_index: validator_index.clone(),
				live_in: hashset!{test_state.relay_parent, test_state.ancestors[0]},
				.. Default::default()
			},
			candidate_hash_b => PerCandidate {
				descriptor: candidate_b.descriptor.clone(),
				validators: test_state.validator_public.clone(),
				validator_index: validator_index.clone(),
				live_in: hashset!{test_state.relay_parent, test_state.ancestors[0]},
				.. Default::default()
			},
		},
		per_relay_parent: hashmap!{
			test_state.relay_parent => PerRelayParent {
				ancestors: vec![
					test_state.ancestors[0],
					test_state.ancestors[1],
					test_state.ancestors[2],
				],
				live_candidates: hashset!{ candidate_hash_a, candidate_hash_b },
			},
			test_state.ancestors[0] => PerRelayParent {
				ancestors: vec![
					test_state.ancestors[1],
					test_state.ancestors[2],
					test_state.ancestors[3],
				],
				live_candidates: hashset!{ candidate_hash_a, candidate_hash_b },
			},
		}
	};

	// receipts has an initial entry for `relay_parent`.
	// let mut receipts = HashMap::new();
	// receipts.insert(test_state.relay_parent, vec![candidate_hash_a, candidate_hash_b].into_iter().collect());

	let pool = sp_core::testing::TaskExecutor::new();

	let (mut ctx, mut virtual_overseer) =
		test_helpers::make_subsystem_context::<AvailabilityDistributionMessage, _>(pool);

	let metrics = Metrics::default();

	let ancestors = test_state.ancestors.clone();

	let test_fut = {
		let peer_a = peer_a.clone();
		let peer_b = peer_b.clone();
		let test_state = test_state.clone();
		async move {
			// make sure to store the chunk, we are the associated validator for this one
			let erasure_chunk_index = validator_index.unwrap();

			let gossip = make_valid_availability_gossip(&test_state, candidate_hash_b, erasure_chunk_index, pov_block_b.clone());
			process_incoming_peer_message(&mut ctx, &mut state, peer_a.clone(),  gossip.clone(), &metrics).await.unwrap();

			// no peer is interested, yet this has to appear in the vault
			assert_eq!(state.per_candidate.get(&candidate_hash_b).unwrap().message_vault.get(&erasure_chunk_index), Some(&gossip));


			// a different chunk from peer b
			let erasure_chunk_index = 1_u32;
			let gossip = make_valid_availability_gossip(&test_state, candidate_hash_b, erasure_chunk_index, pov_block_b.clone());
			process_incoming_peer_message(&mut ctx, &mut state, peer_b.clone(),  gossip.clone(), &metrics).await.unwrap();

			// no peer is interested, yet this has to appear in the vault
			assert_eq!(state.per_candidate.get(&candidate_hash_b).unwrap().message_vault.get(&erasure_chunk_index), Some(&gossip));


			// the same chunk from peer a
			let erasure_chunk_index = 1_u32;
			let gossip = make_valid_availability_gossip(&test_state, candidate_hash_b, erasure_chunk_index, pov_block_b.clone());
			process_incoming_peer_message(&mut ctx, &mut state, peer_a.clone(),  gossip.clone(), &metrics).await.unwrap();

			// no peer is interested, yet this has to appear in the vault
			assert_eq!(state.per_candidate.get(&candidate_hash_b).unwrap().message_vault.get(&erasure_chunk_index), Some(&gossip));
		}
	};

	let overseer = async move {
		// hash_a should be answered out of cache, so we should just have
		// queried for hash_b.
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, BENEFIT_VALID_MESSAGE_FIRST);
			}
		);

		// only if the erasure chunk matches with our validator index
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::AvailabilityStore(
				AvailabilityStoreMessage::StoreChunk{
					candidate_hash,
					relay_parent,
					tx,
					..
				}
			) => {
				assert_eq!(candidate_hash, candidate_hash_b);
				assert_eq!(relay_parent, ancestors[0]);
				tx.send(Ok(())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_b.clone());
				assert_eq!(rep, BENEFIT_VALID_MESSAGE_FIRST);
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_a.clone());
				assert_eq!(rep, BENEFIT_VALID_MESSAGE);
			}
		);
	};

	futures::pin_mut!(test_fut);
	futures::pin_mut!(overseer);

	executor::block_on(future::join(test_fut, overseer));
}
