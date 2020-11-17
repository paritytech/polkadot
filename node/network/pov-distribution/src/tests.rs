use super::*;

use std::time::Duration;

use assert_matches::assert_matches;
use futures::executor;
use log::trace;
use smallvec::smallvec;

use sp_keyring::Sr25519Keyring;

use polkadot_primitives::v1::{
	AuthorityDiscoveryId, BlockData, CoreState, GroupRotationInfo, Id as ParaId,
	ScheduledCore, ValidatorIndex,
};
use polkadot_subsystem::messages::{RuntimeApiMessage, RuntimeApiRequest};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt;

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

struct TestHarness {
	virtual_overseer: test_helpers::TestSubsystemContextHandle<PoVDistributionMessage>,
}

fn test_harness<T: Future<Output = ()>>(
	test: impl FnOnce(TestHarness) -> T,
) {
	let _ = env_logger::builder()
		.is_test(true)
		.filter(
			Some("polkadot_collator_protocol"),
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

	let subsystem = subsystem.run(context);

	let test_fut = test(TestHarness { virtual_overseer });

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	executor::block_on(future::select(test_fut, subsystem));
}

const TIMEOUT: Duration = Duration::from_millis(100);

async fn overseer_send(
	overseer: &mut test_helpers::TestSubsystemContextHandle<PoVDistributionMessage>,
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
	overseer: &mut test_helpers::TestSubsystemContextHandle<PoVDistributionMessage>,
) -> AllMessages {
	let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
		.await
		.expect(&format!("{:?} is more than enough to receive messages", TIMEOUT));

	trace!("Received message:\n{:?}", &msg);

	msg
}

async fn overseer_recv_with_timeout(
	overseer: &mut test_helpers::TestSubsystemContextHandle<PoVDistributionMessage>,
	timeout: Duration,
) -> Option<AllMessages> {
	trace!("Waiting for message...");
	overseer
		.recv()
		.timeout(timeout)
		.await
}

async fn overseer_signal(
	overseer: &mut test_helpers::TestSubsystemContextHandle<PoVDistributionMessage>,
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
		}
	}
}

#[test]
fn ask_newly_connected_validators_for_povs() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
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
				activated: smallvec![test_state.relay_parent.clone()],
				deactivated: smallvec![],
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

		// obtain the validator_id to authority_id mapping
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::ValidatorDiscovery(validators, tx),
			)) => {
				assert_eq!(relay_parent, current);
				assert_eq!(validators.len(), 3);
				assert!(validators.iter().all(|v| test_state.validator_public.contains(&v)));

				let result = vec![
					Some(test_state.validator_authority_id[2].clone()),
					Some(test_state.validator_authority_id[0].clone()),
					Some(test_state.validator_authority_id[4].clone()),
				];
				tx.send(Ok(result)).unwrap();
			}
		);

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
						View(vec![current]),
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
					protocol_v1::PoVDistributionMessage::SendPoV(current, pov_hash, pov_block.clone()),
				)
			)
		).await;

		assert_eq!(*pov_fetch_result.await.unwrap(), pov_block);
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

	let mut state = State {
		relay_parent_state: {
			let mut s = HashMap::new();
			let mut b = BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: 10,
			};

			let per_fetched_pov = PerFetchedPoV {
				send_to: vec![pov_send],
			};
			b.fetching.insert(pov_hash, per_fetched_pov);
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
		our_view: View(vec![hash_a, hash_b]),
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);
	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov_hash;

	executor::block_on(async move {
		handle_distribute(
			&mut state,
			&mut ctx,
			hash_a,
			descriptor,
			Arc::new(pov.clone()),
		).await.unwrap();

		assert!(!state.peer_state[&peer_a].awaited[&hash_a].contains(&pov_hash));
		assert!(state.peer_state[&peer_c].awaited[&hash_b].contains(&pov_hash));

		// our local sender also completed
		assert_eq!(&*pov_recv.await.unwrap(), &pov);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendValidationMessage(peers, message)
			) => {
				assert_eq!(peers, vec![peer_a.clone()]);
				assert_eq!(
					message,
					send_pov_message(hash_a, pov_hash, pov.clone()),
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
		our_view: View(vec![hash_a]),
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

	let validator_groups = vec![vec![2, 0, 4], vec![1], vec![3]];
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
					tx.send(Ok(validator_groups)).unwrap();
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

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::ValidatorDiscovery(validators_res, tx),
				)) => {
					assert_eq!(relay_parent, hash_a);
					assert_eq!(validators_res.len(), 3);
					assert!(validators_res.iter().all(|v| validators.contains(&v)));

					let result = vec![
						Some(validator_authority_id[2].clone()),
						Some(validator_authority_id[0].clone()),
						Some(validator_authority_id[4].clone()),
					];

					tx.send(Ok(result)).unwrap();
				}
			);

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

		futures::join!(handle_future, check_future).0.unwrap();
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
			let per_fetched_a = PerFetchedPoV {
				send_to: vec![pov_a_send],
			};

			let per_fetched_b = PerFetchedPoV {
				send_to: vec![],
			};

			b.fetching.insert(pov_a_hash, per_fetched_a);
			b.fetching.insert(pov_b_hash, per_fetched_b);

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
		our_view: View(vec![hash_a]),
		metrics: Default::default(),
		connection_requests: Default::default(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	executor::block_on(async move {
		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerViewChange(peer_a.clone(), View(vec![hash_a, hash_b])),
		).await.unwrap();

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
			let per_fetched = PerFetchedPoV {
				send_to: vec![pov_send],
			};
			b.fetching.insert(pov_hash, per_fetched);

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
		our_view: View(vec![hash_a]),
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
				send_pov_message(hash_a, pov_hash, pov.clone()),
			).focus().unwrap(),
		).await.unwrap();

		handle_network_update(
			&mut state,
			&mut ctx,
			NetworkBridgeEvent::PeerMessage(
				peer_b.clone(),
				send_pov_message(hash_a, pov_hash, pov.clone()),
			).focus().unwrap(),
		).await.unwrap();

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
			let per_fetched = PerFetchedPoV {
				send_to: vec![pov_send],
			};
			b.fetching.insert(pov_hash, per_fetched);

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
		our_view: View(vec![hash_a]),
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
				send_pov_message(hash_a, pov_hash, bad_pov.clone()),
			).focus().unwrap(),
		).await.unwrap();

		// didn't complete our sender.
		assert_eq!(state.relay_parent_state[&hash_a].fetching[&pov_hash].send_to.len(), 1);

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
		our_view: View(vec![hash_a]),
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
				send_pov_message(hash_a, pov_hash, pov.clone()),
			).focus().unwrap(),
		).await.unwrap();

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
		our_view: View(vec![hash_a]),
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
				send_pov_message(hash_b, pov_hash, pov.clone()),
			).focus().unwrap(),
		).await.unwrap();

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
		our_view: View(vec![hash_a]),
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
			).await.unwrap();
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
		).await.unwrap();

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
		our_view: View(vec![hash_a, hash_b]),
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
		).await.unwrap();

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
		our_view: View(vec![hash_a]),
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
		).await.unwrap();

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

			let per_fetched_pov = PerFetchedPoV {
				send_to: vec![pov_send],
			};
			// pov is being fetched.
			b.fetching.insert(pov_hash, per_fetched_pov);

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
		our_view: View(vec![hash_a]),
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
				send_pov_message(hash_a, pov_hash, pov.clone()),
			).focus().unwrap(),
		).await.unwrap();

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
					send_pov_message(hash_a, pov_hash, pov.clone()),
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

			let per_fetched_pov = PerFetchedPoV {
				send_to: vec![pov_send],
			};
			// pov is being fetched.
			b.fetching.insert(pov_hash, per_fetched_pov);

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
		our_view: View(vec![hash_a]),
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
				send_pov_message(hash_a, pov_hash, pov.clone()),
			).focus().unwrap(),
		).await.unwrap();

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
