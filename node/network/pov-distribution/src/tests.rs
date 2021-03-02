// Copyright 2020-2021 Parity Technologies (UK) Ltd.
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

use std::{time::Duration, sync::Arc};

use assert_matches::assert_matches;
use futures::executor;
use tracing::trace;

use sp_keyring::Sr25519Keyring;

use polkadot_primitives::v1::{
	AuthorityDiscoveryId, BlockData, CoreState, GroupRotationInfo, Id as ParaId,
	ScheduledCore, ValidatorIndex, SessionIndex, SessionInfo,
};
use polkadot_subsystem::{messages::{RuntimeApiMessage, RuntimeApiRequest}, jaeger};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_node_network_protocol::{view, our_view};

fn make_pov(data: Vec<u8>) -> PoV {
	PoV { block_data: BlockData(data) }
}

fn make_peer_state(awaited: Vec<(Hash, Vec<Hash>)>)
	-> PeerState
{
	PeerState {
		awaited: awaited.into_iter().map(|(rp, h)| (rp, h.into_iter().collect())).collect()
	}
}

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

fn validator_authority_id(val_ids: &[Sr25519Keyring]) -> Vec<AuthorityDiscoveryId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<PoVDistributionMessage>;

struct TestHarness {
	virtual_overseer: VirtualOverseer,
}

fn test_harness<T: Future<Output = ()>>(
	state: State,
	test: impl FnOnce(TestHarness) -> T,
) {
	let _ = env_logger::builder()
		.is_test(true)
		.filter(
			Some("polkadot_pov_distribution"),
			log::LevelFilter::Trace,
		)
		.filter(
			Some(LOG_TARGET),
			log::LevelFilter::Trace,
		)
		.try_init();

	let pool = sp_core::testing::TaskExecutor::new();

	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = super::PoVDistribution::new(Metrics::default());

	let subsystem = subsystem.run_with_state(context, state);

	let test_fut = test(TestHarness { virtual_overseer });

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	executor::block_on(future::select(test_fut, subsystem));
}

const TIMEOUT: Duration = Duration::from_millis(100);

async fn overseer_send(
	overseer: &mut VirtualOverseer,
	msg: PoVDistributionMessage,
) {
	trace!("Sending message:\n{:?}", &msg);
	overseer
		.send(FromOverseer::Communication { msg })
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough for sending messages.", TIMEOUT));
}

async fn overseer_recv(
	overseer: &mut VirtualOverseer,
) -> AllMessages {
	let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough to receive messages", TIMEOUT));

	trace!("Received message:\n{:?}", &msg);

	msg
}

async fn overseer_recv_with_timeout(
	overseer: &mut VirtualOverseer,
	timeout: Duration,
) -> Option<AllMessages> {
	trace!("Waiting for message...");
	overseer
		.recv()
		.timeout(timeout)
		.await
}

async fn overseer_signal(
	overseer: &mut VirtualOverseer,
	signal: OverseerSignal,
) {
	overseer
		.send(FromOverseer::Signal(signal))
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough for sending signals.", TIMEOUT));
}

#[derive(Clone)]
struct TestState {
	chain_ids: Vec<ParaId>,
	validators: Vec<Sr25519Keyring>,
	validator_public: Vec<ValidatorId>,
	validator_authority_id: Vec<AuthorityDiscoveryId>,
	validator_peer_id: Vec<PeerId>,
	validator_groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo),
	relay_parent: Hash,
	availability_cores: Vec<CoreState>,
	session_index: SessionIndex,
}

impl Default for TestState {
	fn default() -> Self {
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);

		let chain_ids = vec![chain_a, chain_b];

		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
		];

		let validator_public = validator_pubkeys(&validators);
		let validator_authority_id = validator_authority_id(&validators);

		let validator_peer_id = std::iter::repeat_with(|| PeerId::random())
			.take(validator_public.len())
			.collect();

		let validator_groups = vec![vec![2, 0, 4], vec![1], vec![3]]
			.into_iter().map(|g| g.into_iter().map(ValidatorIndex).collect()).collect();
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

		let relay_parent = Hash::repeat_byte(0x05);

		Self {
			chain_ids,
			validators,
			validator_public,
			validator_authority_id,
			validator_peer_id,
			validator_groups,
			relay_parent,
			availability_cores,
			session_index: 1,
		}
	}
}

async fn test_validator_discovery(
	virtual_overseer: &mut VirtualOverseer,
	expected_relay_parent: Hash,
	session_index: SessionIndex,
	validator_ids: &[ValidatorId],
	discovery_ids: &[AuthorityDiscoveryId],
	validator_group: &[ValidatorIndex],
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

			let validators = validator_group.iter()
				.map(|idx| validator_ids[idx.0 as usize].clone())
				.collect();

			let discovery_keys = validator_group.iter()
				.map(|idx| discovery_ids[idx.0 as usize].clone())
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
fn ask_validators_for_povs() {
	let test_state = TestState::default();

	test_harness(State::default(), |test_harness| async move {
		let mut virtual_overseer = test_harness.virtual_overseer;

		let pov_block = PoV {
			block_data: BlockData(vec![42, 43, 44]),
		};

		let pov_hash = pov_block.hash();

		let mut candidate = CandidateDescriptor::default();

		let current = test_state.relay_parent.clone();
		candidate.para_id = test_state.chain_ids[0];
		candidate.pov_hash = pov_hash;
		candidate.relay_parent = test_state.relay_parent;

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: [(test_state.relay_parent, Arc::new(jaeger::Span::Disabled))][..].into(),
				deactivated: [][..].into(),
			}),
		).await;

		// first subsystem will try to obtain validators.
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Validators(tx),
			)) => {
				assert_eq!(relay_parent, current);
				tx.send(Ok(test_state.validator_public.clone())).unwrap();
			}
		);

		let (tx, pov_fetch_result) = oneshot::channel();

		overseer_send(
			&mut virtual_overseer,
			PoVDistributionMessage::FetchPoV(test_state.relay_parent.clone(), candidate, tx),
		).await;

		// obtain the availability cores.
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::AvailabilityCores(tx)
			)) => {
				assert_eq!(relay_parent, current);
				tx.send(Ok(test_state.availability_cores.clone())).unwrap();
			}
		);

		// Obtain the validator groups
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::ValidatorGroups(tx)
			)) => {
				assert_eq!(relay_parent, current);
				tx.send(Ok(test_state.validator_groups.clone())).unwrap();
			}
		);

		// obtain the validators per relay parent
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Validators(tx),
			)) => {
				assert_eq!(relay_parent, current);
				tx.send(Ok(test_state.validator_public.clone())).unwrap();
			}
		);

		test_validator_discovery(
			&mut virtual_overseer,
			current,
			test_state.session_index,
			&test_state.validator_public,
			&test_state.validator_authority_id,
			&test_state.validator_groups.0[0],
		).await;

		// We now should connect to our validator group.
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ConnectToValidators {
					validator_ids,
					mut connected,
					..
				}
			) => {
				assert_eq!(validator_ids.len(), 3);
				assert!(validator_ids.iter().all(|id| test_state.validator_authority_id.contains(id)));

				let result = vec![
					(test_state.validator_authority_id[2].clone(), test_state.validator_peer_id[2].clone()),
					(test_state.validator_authority_id[0].clone(), test_state.validator_peer_id[0].clone()),
					(test_state.validator_authority_id[4].clone(), test_state.validator_peer_id[4].clone()),
				];

				result.into_iter().for_each(|r| connected.try_send(r).unwrap());
			}
		);

		for i in vec![2, 0, 4] {
			overseer_send(
				&mut virtual_overseer,
				PoVDistributionMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerViewChange(
						test_state.validator_peer_id[i].clone(),
						view![current],
					)
				)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
					to_peers,
					payload,
				)) => {
					assert_eq!(to_peers, vec![test_state.validator_peer_id[i].clone()]);
					assert_eq!(payload, awaiting_message(current.clone(), vec![pov_hash.clone()]));
				}
			);
		}

		overseer_send(
			&mut virtual_overseer,
			PoVDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					test_state.validator_peer_id[2].clone(),
					protocol_v1::PoVDistributionMessage::SendPoV(
						current,
						pov_hash,
						protocol_v1::CompressedPoV::compress(&pov_block).unwrap(),
					),
				)
			)
		).await;

		assert_eq!(*pov_fetch_result.await.unwrap(), pov_block);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ReportPeer(id, benefit)) => {
				assert_eq!(benefit, BENEFIT_FRESH_POV);
				assert_eq!(id, test_state.validator_peer_id[2].clone());
			}
		);

		// Now let's test that if some peer is ahead of us we would still
		// send `Await` on `FetchPoV` message to it.
		let next_leaf = Hash::repeat_byte(10);

		// A validator's view changes and now is lets say ahead of us.
		overseer_send(
			&mut virtual_overseer,
			PoVDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerViewChange(
					test_state.validator_peer_id[2].clone(),
					view![next_leaf],
				)
			)
		).await;

		let pov_block = PoV {
			block_data: BlockData(vec![45, 46, 47]),
		};

		let pov_hash = pov_block.hash();

		let candidate = CandidateDescriptor {
			para_id: test_state.chain_ids[0],
			pov_hash,
			relay_parent: next_leaf.clone(),
			..Default::default()
		};

		let (tx, _pov_fetch_result) = oneshot::channel();

		overseer_signal(
			&mut virtual_overseer,
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: [(next_leaf, Arc::new(jaeger::Span::Disabled))][..].into(),
				deactivated: [current.clone()][..].into(),
			})
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Validators(tx),
			)) => {
				assert_eq!(relay_parent, next_leaf);
				tx.send(Ok(test_state.validator_public.clone())).unwrap();
			}
		);

		overseer_send(
			&mut virtual_overseer,
			PoVDistributionMessage::FetchPoV(next_leaf.clone(), candidate, tx),
		).await;

		// Obtain the availability cores.
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::AvailabilityCores(tx)
			)) => {
				assert_eq!(relay_parent, next_leaf);
				tx.send(Ok(test_state.availability_cores.clone())).unwrap();
			}
		);

		// Obtain the validator groups
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::ValidatorGroups(tx)
			)) => {
				assert_eq!(relay_parent, next_leaf);
				tx.send(Ok(test_state.validator_groups.clone())).unwrap();
			}
		);

		// obtain the validators per relay parent
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Validators(tx),
			)) => {
				assert_eq!(relay_parent, next_leaf);
				tx.send(Ok(test_state.validator_public.clone())).unwrap();
			}
		);

		// obtain the validator_id to authority_id mapping
		test_validator_discovery(
			&mut virtual_overseer,
			next_leaf,
			test_state.session_index,
			&test_state.validator_public,
			&test_state.validator_authority_id,
			&test_state.validator_groups.0[0],
		).await;

		// We now should connect to our validator group.
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ConnectToValidators {
					validator_ids,
					mut connected,
					..
				}
			) => {
				assert_eq!(validator_ids.len(), 3);
				assert!(validator_ids.iter().all(|id| test_state.validator_authority_id.contains(id)));

				let result = vec![
					(test_state.validator_authority_id[2].clone(), test_state.validator_peer_id[2].clone()),
					(test_state.validator_authority_id[0].clone(), test_state.validator_peer_id[0].clone()),
					(test_state.validator_authority_id[4].clone(), test_state.validator_peer_id[4].clone()),
				];

				result.into_iter().for_each(|r| connected.try_send(r).unwrap());
			}
		);

		// We already know that the leaf in question in the peer's view so we request
		// a chunk from them right away.
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
				to_peers,
				payload,
			)) => {
				assert_eq!(to_peers, vec![test_state.validator_peer_id[2].clone()]);
				assert_eq!(payload, awaiting_message(next_leaf.clone(), vec![pov_hash.clone()]));
			}
		);
	});
}

#[test]
fn distributes_to_those_awaiting_and_completes_local() {
	let hash_a: Hash = [0; 32].into();
	let hash_b: Hash = [1; 32].into();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();

	let (pov_send, pov_recv) = oneshot::channel();
	let pov = make_pov(vec![1, 2, 3]);
	let pov_hash = pov.hash();

	let state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			let mut b = BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			};

			b.fetching.insert(pov_hash, vec![pov_send]);
			s.insert(hash_a, b);
			s
		},
		peer_state: {
			let mut s = HashMap::new();

			// peer A has hash_a in its view and is awaiting the PoV.
			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_a, vec![pov_hash])]),
			);

			// peer B has hash_a in its view but is not awaiting.
			s.insert(
				peer_b.clone(),
				make_peer_state(vec![(hash_a, vec![])]),
			);

			// peer C doesn't have hash_a in its view but is awaiting the PoV under hash_b.
			s.insert(
				peer_c.clone(),
				make_peer_state(vec![(hash_b, vec![pov_hash])]),
			);

			s
		},
		our_view: our_view![hash_a, hash_b],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov_hash;

	test_harness(state, |test_harness| async move {
		let mut virtual_overseer = test_harness.virtual_overseer;

		overseer_send(
			&mut virtual_overseer,
			PoVDistributionMessage::DistributePoV(
				hash_a,
				descriptor,
				Arc::new(pov.clone())
			)
		).await;

		// Let's assume runtime call failed and we're already connected to the peers.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::AvailabilityCores(tx)
			)) => {
				assert_eq!(relay_parent, hash_a);
				tx.send(Err("nope".to_string().into())).unwrap();
			}
		);

		// our local sender also completed
		assert_eq!(&*pov_recv.await.unwrap(), &pov);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendValidationMessage(peers, message)
			) => {
				assert_eq!(peers, vec![peer_a.clone()]);
				assert_eq!(
					message,
					send_pov_message(hash_a, pov_hash, &protocol_v1::CompressedPoV::compress(&pov).unwrap()),
				);
			}
		)
	});
}


#[test]
fn we_inform_peers_with_same_view_we_are_awaiting() {

	let hash_a: Hash = [0; 32].into();
	let hash_b: Hash = [1; 32].into();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();

	let (pov_send, _) = oneshot::channel();
	let pov = make_pov(vec![1, 2, 3]);
	let pov_hash = pov.hash();

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			let b = BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			};

			s.insert(hash_a, b);
			s
		},
		peer_state: {
			let mut s = HashMap::new();

			// peer A has hash_a in its view.
			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_a, vec![])]),
			);

			// peer B doesn't have hash_a in its view.
			s.insert(
				peer_b.clone(),
				make_peer_state(vec![(hash_b, vec![])]),
			);

			s
		},
		our_view: our_view![hash_a],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);
	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov_hash;

	let para_id_1 = ParaId::from(1);
	let para_id_2 = ParaId::from(2);

	descriptor.para_id = para_id_1;

	let availability_cores = vec![
		CoreState::Scheduled(ScheduledCore {
			para_id: para_id_1,
			collator: None,
		}),
		CoreState::Scheduled(ScheduledCore {
			para_id: para_id_2,
			collator: None,
		}),
	];

	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
	];

	let validator_authority_id = validator_authority_id(&validators);
	let validators = validator_pubkeys(&validators);

	let validator_peer_id: Vec<_> = std::iter::repeat_with(|| PeerId::random())
		.take(validators.len())
		.collect();

	let validator_groups = vec![vec![2, 0, 4], vec![1], vec![3]]
		.into_iter().map(|g| g.into_iter().map(ValidatorIndex).collect()).collect();
	let group_rotation_info = GroupRotationInfo {
		session_start_block: 0,
		group_rotation_frequency: 100,
		now: 1,
	};

	let validator_groups = (validator_groups, group_rotation_info);

	executor::block_on(async move {
		let handle_future = handle_fetch(
			&mut state,
			&mut ctx,
			hash_a,
			descriptor,
			pov_send,
		);

		let check_future = async move {
			//assert_eq!(state.relay_parent_state[&hash_a].fetching[&pov_hash].len(), 1);
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::AvailabilityCores(tx)
				)) => {
					assert_eq!(relay_parent, hash_a);
					tx.send(Ok(availability_cores)).unwrap();
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::ValidatorGroups(tx)
				)) => {
					assert_eq!(relay_parent, hash_a);
					tx.send(Ok(validator_groups.clone())).unwrap();
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::Validators(tx),
				)) => {
					assert_eq!(relay_parent, hash_a);
					tx.send(Ok(validators.clone())).unwrap();
				}
			);

			test_validator_discovery(
				&mut handle,
				hash_a,
				1,
				&validators,
				&validator_authority_id,
				&validator_groups.0[0],
			).await;

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ConnectToValidators {
						validator_ids,
						mut connected,
						..
					}
				) => {
					assert_eq!(validator_ids.len(), 3);
					assert!(validator_ids.iter().all(|id| validator_authority_id.contains(id)));

					let result = vec![
						(validator_authority_id[2].clone(), validator_peer_id[2].clone()),
						(validator_authority_id[0].clone(), validator_peer_id[0].clone()),
						(validator_authority_id[4].clone(), validator_peer_id[4].clone()),
					];

					result.into_iter().for_each(|r| connected.try_send(r).unwrap());
				}
			);

		};

		futures::join!(handle_future, check_future);
	});
}

#[test]
fn peer_view_change_leads_to_us_informing() {
	let hash_a: Hash = [0; 32].into();
	let hash_b: Hash = [1; 32].into();

	let peer_a = PeerId::random();

	let (pov_a_send, _) = oneshot::channel();

	let pov_a = make_pov(vec![1, 2, 3]);
	let pov_a_hash = pov_a.hash();

	let pov_b = make_pov(vec![4, 5, 6]);
	let pov_b_hash = pov_b.hash();

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			let mut b = BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			};

			// pov_a is still being fetched, whereas the fetch of pov_b has already
			// completed, as implied by the empty vector.
			b.fetching.insert(pov_a_hash, vec![pov_a_send]);
			b.fetching.insert(pov_b_hash, vec![]);

			s.insert(hash_a, b);
			s
		},
		peer_state: {
			let mut s = HashMap::new();

			// peer A doesn't yet have hash_a in its view.
			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_b, vec![])]),
			);

			s
		},
		our_view: our_view![hash_a],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	executor::block_on(async move {
		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerViewChange(peer_a.clone(), view![hash_a, hash_b]),
		).await;

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendValidationMessage(peers, message)
			) => {
				assert_eq!(peers, vec![peer_a.clone()]);
				assert_eq!(
					message,
					awaiting_message(hash_a, vec![pov_a_hash]),
				);
			}
		)
	});
}

#[test]
fn peer_complete_fetch_and_is_rewarded() {
	let hash_a: Hash = [0; 32].into();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();

	let (pov_send, pov_recv) = oneshot::channel();

	let pov = make_pov(vec![1, 2, 3]);
	let pov_hash = pov.hash();

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			let mut b = BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			};

			// pov is being fetched.
			b.fetching.insert(pov_hash, vec![pov_send]);

			s.insert(hash_a, b);
			s
		},
		peer_state: {
			let mut s = HashMap::new();

			// peers A and B are functionally the same.
			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_a, vec![])]),
			);

			s.insert(
				peer_b.clone(),
				make_peer_state(vec![(hash_a, vec![])]),
			);

			s
		},
		our_view: our_view![hash_a],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	executor::block_on(async move {
		// Peer A answers our request before peer B.
		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerMessage(
				peer_a.clone(),
				send_pov_message(hash_a, pov_hash, &protocol_v1::CompressedPoV::compress(&pov).unwrap()),
			).focus().unwrap(),
		).await;

		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerMessage(
				peer_b.clone(),
				send_pov_message(hash_a, pov_hash, &protocol_v1::CompressedPoV::compress(&pov).unwrap()),
			).focus().unwrap(),
		).await;

		assert_eq!(&*pov_recv.await.unwrap(), &pov);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, BENEFIT_FRESH_POV);
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, BENEFIT_LATE_POV);
			}
		);
	});
}

#[test]
fn peer_punished_for_sending_bad_pov() {
	let hash_a: Hash = [0; 32].into();

	let peer_a = PeerId::random();

	let (pov_send, _) = oneshot::channel();

	let pov = make_pov(vec![1, 2, 3]);
	let pov_hash = pov.hash();

	let bad_pov = make_pov(vec![6, 6, 6]);

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			let mut b = BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			};

			// pov is being fetched.
			b.fetching.insert(pov_hash, vec![pov_send]);

			s.insert(hash_a, b);
			s
		},
		peer_state: {
			let mut s = HashMap::new();

			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_a, vec![])]),
			);

			s
		},
		our_view: our_view![hash_a],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	executor::block_on(async move {
		// Peer A answers our request: right relay parent, awaited hash, wrong PoV.
		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerMessage(
				peer_a.clone(),
				send_pov_message(hash_a, pov_hash, &protocol_v1::CompressedPoV::compress(&bad_pov).unwrap()),
			).focus().unwrap(),
		).await;

		// didn't complete our sender.
		assert_eq!(state.relay_parent_state[&hash_a].fetching[&pov_hash].len(), 1);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, COST_UNEXPECTED_POV);
			}
		);
	});
}

#[test]
fn peer_punished_for_sending_unexpected_pov() {
	let hash_a: Hash = [0; 32].into();

	let peer_a = PeerId::random();

	let pov = make_pov(vec![1, 2, 3]);
	let pov_hash = pov.hash();

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			let b = BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			};

			s.insert(hash_a, b);
			s
		},
		peer_state: {
			let mut s = HashMap::new();

			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_a, vec![])]),
			);

			s
		},
		our_view: our_view![hash_a],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	executor::block_on(async move {
		// Peer A answers our request: right relay parent, awaited hash, wrong PoV.
		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerMessage(
				peer_a.clone(),
				send_pov_message(hash_a, pov_hash, &protocol_v1::CompressedPoV::compress(&pov).unwrap()),
			).focus().unwrap(),
		).await;

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, COST_UNEXPECTED_POV);
			}
		);
	});
}

#[test]
fn peer_punished_for_sending_pov_out_of_our_view() {
	let hash_a: Hash = [0; 32].into();
	let hash_b: Hash = [1; 32].into();

	let peer_a = PeerId::random();

	let pov = make_pov(vec![1, 2, 3]);
	let pov_hash = pov.hash();

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			let b = BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			};

			s.insert(hash_a, b);
			s
		},
		peer_state: {
			let mut s = HashMap::new();

			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_a, vec![])]),
			);

			s
		},
		our_view: our_view![hash_a],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	executor::block_on(async move {
		// Peer A answers our request: right relay parent, awaited hash, wrong PoV.
		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerMessage(
				peer_a.clone(),
				send_pov_message(hash_b, pov_hash, &protocol_v1::CompressedPoV::compress(&pov).unwrap()),
			).focus().unwrap(),
		).await;

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, COST_UNEXPECTED_POV);
			}
		);
	});
}

#[test]
fn peer_reported_for_awaiting_too_much() {
	let hash_a: Hash = [0; 32].into();

	let peer_a = PeerId::random();
	let n_validators = 10;

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			let b = BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators,
			};

			s.insert(hash_a, b);
			s
		},
		peer_state: {
			let mut s = HashMap::new();

			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_a, vec![])]),
			);

			s
		},
		our_view: our_view![hash_a],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	executor::block_on(async move {
		let max_plausibly_awaited = n_validators * 2;

		// The peer awaits a plausible (albeit unlikely) amount of PoVs.
		for i in 0..max_plausibly_awaited {
			let pov_hash = make_pov(vec![i as u8; 32]).hash();
			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					awaiting_message(hash_a, vec![pov_hash]),
				).focus().unwrap(),
			).await;
		}

		assert_eq!(state.peer_state[&peer_a].awaited[&hash_a].len(), max_plausibly_awaited);

		// The last straw:
		let last_pov_hash = make_pov(vec![max_plausibly_awaited as u8; 32]).hash();
		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerMessage(
				peer_a.clone(),
				awaiting_message(hash_a, vec![last_pov_hash]),
			).focus().unwrap(),
		).await;

		// No more bookkeeping for you!
		assert_eq!(state.peer_state[&peer_a].awaited[&hash_a].len(), max_plausibly_awaited);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, COST_APPARENT_FLOOD);
			}
		);
	});
}

#[test]
fn peer_reported_for_awaiting_outside_their_view() {
	let hash_a: Hash = [0; 32].into();
	let hash_b: Hash = [1; 32].into();

	let peer_a = PeerId::random();

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			s.insert(hash_a, BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			});

			s.insert(hash_b, BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			});

			s
		},
		peer_state: {
			let mut s = HashMap::new();

			// Peer has only hash A in its view.
			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_a, vec![])]),
			);

			s
		},
		our_view: our_view![hash_a, hash_b],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	executor::block_on(async move {
		let pov_hash = make_pov(vec![1, 2, 3]).hash();

		// Hash B is in our view but not the peer's
		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerMessage(
				peer_a.clone(),
				awaiting_message(hash_b, vec![pov_hash]),
			).focus().unwrap(),
		).await;

		assert!(state.peer_state[&peer_a].awaited.get(&hash_b).is_none());

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, COST_AWAITED_NOT_IN_VIEW);
			}
		);
	});
}

#[test]
fn peer_reported_for_awaiting_outside_our_view() {
	let hash_a: Hash = [0; 32].into();
	let hash_b: Hash = [1; 32].into();

	let peer_a = PeerId::random();

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			s.insert(hash_a, BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			});

			s
		},
		peer_state: {
			let mut s = HashMap::new();

			// Peer has hashes A and B in their view.
			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_a, vec![]), (hash_b, vec![])]),
			);

			s
		},
		our_view: our_view![hash_a],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	executor::block_on(async move {
		let pov_hash = make_pov(vec![1, 2, 3]).hash();

		// Hash B is in peer's view but not ours.
		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerMessage(
				peer_a.clone(),
				awaiting_message(hash_b, vec![pov_hash]),
			).focus().unwrap(),
		).await;

		// Illegal `awaited` is ignored.
		assert!(state.peer_state[&peer_a].awaited[&hash_b].is_empty());

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, COST_AWAITED_NOT_IN_VIEW);
			}
		);
	});
}

#[test]
fn peer_complete_fetch_leads_to_us_completing_others() {
	let hash_a: Hash = [0; 32].into();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();

	let (pov_send, pov_recv) = oneshot::channel();

	let pov = make_pov(vec![1, 2, 3]);
	let pov_hash = pov.hash();

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			let mut b = BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			};

			// pov is being fetched.
			b.fetching.insert(pov_hash, vec![pov_send]);

			s.insert(hash_a, b);
			s
		},
		peer_state: {
			let mut s = HashMap::new();

			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_a, vec![])]),
			);

			// peer B is awaiting peer A's request.
			s.insert(
				peer_b.clone(),
				make_peer_state(vec![(hash_a, vec![pov_hash])]),
			);

			s
		},
		our_view: our_view![hash_a],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	executor::block_on(async move {
		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerMessage(
				peer_a.clone(),
				send_pov_message(hash_a, pov_hash, &protocol_v1::CompressedPoV::compress(&pov).unwrap()),
			).focus().unwrap(),
		).await;

		assert_eq!(&*pov_recv.await.unwrap(), &pov);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, BENEFIT_FRESH_POV);
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendValidationMessage(peers, message)
			) => {
				assert_eq!(peers, vec![peer_b.clone()]);
				assert_eq!(
					message,
					send_pov_message(hash_a, pov_hash, &protocol_v1::CompressedPoV::compress(&pov).unwrap()),
				);
			}
		);

		assert!(!state.peer_state[&peer_b].awaited[&hash_a].contains(&pov_hash));
	});
}

#[test]
fn peer_completing_request_no_longer_awaiting() {
	let hash_a: Hash = [0; 32].into();

	let peer_a = PeerId::random();

	let (pov_send, pov_recv) = oneshot::channel();

	let pov = make_pov(vec![1, 2, 3]);
	let pov_hash = pov.hash();

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			let mut b = BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			};

			// pov is being fetched.
			b.fetching.insert(pov_hash, vec![pov_send]);

			s.insert(hash_a, b);
			s
		},
		peer_state: {
			let mut s = HashMap::new();

			// peer A is registered as awaiting.
			s.insert(
				peer_a.clone(),
				make_peer_state(vec![(hash_a, vec![pov_hash])]),
			);

			s
		},
		our_view: our_view![hash_a],
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	executor::block_on(async move {
		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerMessage(
				peer_a.clone(),
				send_pov_message(hash_a, pov_hash, &protocol_v1::CompressedPoV::compress(&pov).unwrap()),
			).focus().unwrap(),
		).await;

		assert_eq!(&*pov_recv.await.unwrap(), &pov);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep)
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, BENEFIT_FRESH_POV);
			}
		);

		// We received the PoV from peer A, so we do not consider it awaited by peer A anymore.
		assert!(!state.peer_state[&peer_a].awaited[&hash_a].contains(&pov_hash));
	});
}
