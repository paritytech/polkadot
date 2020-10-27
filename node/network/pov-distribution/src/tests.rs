use super::*;
use futures::executor;
use polkadot_primitives::v1::BlockData;
use assert_matches::assert_matches;

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
		our_view: View(vec![hash_a, hash_b]),
		metrics: Default::default(),
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
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);
	let mut descriptor = CandidateDescriptor::default();
	descriptor.pov_hash = pov_hash;

	executor::block_on(async move {
		handle_fetch(
			&mut state,
			&mut ctx,
			hash_a,
			descriptor,
			pov_send,
		).await.unwrap();

		assert_eq!(state.relay_parent_state[&hash_a].fetching[&pov_hash].len(), 1);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendValidationMessage(peers, message)
			) => {
				assert_eq!(peers, vec![peer_a.clone()]);
				assert_eq!(
					message,
					awaiting_message(hash_a, vec![pov_hash]),
				);
			}
		)
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
		our_view: View(vec![hash_a]),
		metrics: Default::default(),
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
		our_view: View(vec![hash_a]),
		metrics: Default::default(),
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
		our_view: View(vec![hash_a]),
		metrics: Default::default(),
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
		our_view: View(vec![hash_a]),
		metrics: Default::default(),
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
		our_view: View(vec![hash_a]),
		metrics: Default::default(),
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
		our_view: View(vec![hash_a]),
		metrics: Default::default(),
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
