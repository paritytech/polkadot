// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Unit tests for Gossip Support Subsystem.

use std::{collections::HashSet, sync::Arc, time::Duration};

use assert_matches::assert_matches;
use async_trait::async_trait;
use futures::{executor, future, Future};
use lazy_static::lazy_static;

use sc_network::multiaddr::Protocol;
use sp_authority_discovery::AuthorityPair as AuthorityDiscoveryPair;
use sp_consensus_babe::{AllowedSlots, BabeEpochConfiguration, Epoch as BabeEpoch};
use sp_core::crypto::Pair as PairT;
use sp_keyring::Sr25519Keyring;

use polkadot_node_network_protocol::grid_topology::{SessionGridTopology, TopologyPeerInfo};
use polkadot_node_subsystem::{
	jaeger,
	messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest},
	ActivatedLeaf, LeafStatus,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt as _;
use polkadot_primitives::v2::{GroupIndex, IndexedVec};
use test_helpers::mock::make_ferdie_keystore;

use super::*;

const AUTHORITY_KEYRINGS: &[Sr25519Keyring] = &[
	Sr25519Keyring::Alice,
	Sr25519Keyring::Bob,
	Sr25519Keyring::Charlie,
	Sr25519Keyring::Eve,
	Sr25519Keyring::One,
	Sr25519Keyring::Two,
	Sr25519Keyring::Ferdie,
];

lazy_static! {
	static ref MOCK_AUTHORITY_DISCOVERY: MockAuthorityDiscovery = MockAuthorityDiscovery::new();
	static ref AUTHORITIES: Vec<AuthorityDiscoveryId> =
		AUTHORITY_KEYRINGS.iter().map(|k| k.public().into()).collect();

	static ref AUTHORITIES_WITHOUT_US: Vec<AuthorityDiscoveryId> = {
		let mut a = AUTHORITIES.clone();
		a.pop(); // remove FERDIE.
		a
	};

	static ref PAST_PRESENT_FUTURE_AUTHORITIES: Vec<AuthorityDiscoveryId> = {
		(0..50)
			.map(|_| AuthorityDiscoveryPair::generate().0.public())
			.chain(AUTHORITIES.clone())
			.collect()
	};

	// [2 6]
	// [4 5]
	// [1 3]
	// [0  ]

	static ref EXPECTED_SHUFFLING: Vec<usize> = vec![6, 4, 0, 5, 2, 3, 1];

	static ref ROW_NEIGHBORS: Vec<ValidatorIndex> = vec![
		ValidatorIndex::from(2),
	];

	static ref COLUMN_NEIGHBORS: Vec<ValidatorIndex> = vec![
		ValidatorIndex::from(3),
		ValidatorIndex::from(5),
	];
}

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<GossipSupportMessage>;

#[derive(Debug, Clone)]
struct MockAuthorityDiscovery {
	addrs: HashMap<AuthorityDiscoveryId, HashSet<Multiaddr>>,
	authorities: HashMap<PeerId, HashSet<AuthorityDiscoveryId>>,
}

impl MockAuthorityDiscovery {
	fn new() -> Self {
		let authorities: HashMap<_, _> = PAST_PRESENT_FUTURE_AUTHORITIES
			.clone()
			.into_iter()
			.map(|a| (PeerId::random(), a))
			.collect();
		let addrs = authorities
			.clone()
			.into_iter()
			.map(|(p, a)| {
				let multiaddr = Multiaddr::empty().with(Protocol::P2p(p.into()));
				(a, HashSet::from([multiaddr]))
			})
			.collect();
		Self {
			addrs,
			authorities: authorities.into_iter().map(|(p, a)| (p, HashSet::from([a]))).collect(),
		}
	}
}

#[async_trait]
impl AuthorityDiscovery for MockAuthorityDiscovery {
	async fn get_addresses_by_authority_id(
		&mut self,
		authority: polkadot_primitives::v2::AuthorityDiscoveryId,
	) -> Option<HashSet<sc_network::Multiaddr>> {
		self.addrs.get(&authority).cloned()
	}
	async fn get_authority_ids_by_peer_id(
		&mut self,
		peer_id: polkadot_node_network_protocol::PeerId,
	) -> Option<HashSet<polkadot_primitives::v2::AuthorityDiscoveryId>> {
		self.authorities.get(&peer_id).cloned()
	}
}

async fn get_multiaddrs(authorities: Vec<AuthorityDiscoveryId>) -> Vec<HashSet<Multiaddr>> {
	let mut addrs = Vec::with_capacity(authorities.len());
	let mut discovery = MOCK_AUTHORITY_DISCOVERY.clone();
	for authority in authorities.into_iter() {
		if let Some(addr) = discovery.get_addresses_by_authority_id(authority).await {
			addrs.push(addr);
		}
	}
	addrs
}

async fn get_address_map(
	authorities: Vec<AuthorityDiscoveryId>,
) -> HashMap<AuthorityDiscoveryId, HashSet<Multiaddr>> {
	let mut addrs = HashMap::with_capacity(authorities.len());
	let mut discovery = MOCK_AUTHORITY_DISCOVERY.clone();
	for authority in authorities.into_iter() {
		if let Some(addr) = discovery.get_addresses_by_authority_id(authority.clone()).await {
			addrs.insert(authority, addr);
		}
	}
	addrs
}

fn make_subsystem() -> GossipSupport<MockAuthorityDiscovery> {
	GossipSupport::new(
		make_ferdie_keystore(),
		MOCK_AUTHORITY_DISCOVERY.clone(),
		Metrics::new_dummy(),
	)
}

fn test_harness<T: Future<Output = VirtualOverseer>, AD: AuthorityDiscovery>(
	subsystem: GossipSupport<AD>,
	test_fn: impl FnOnce(VirtualOverseer) -> T,
) -> GossipSupport<AD> {
	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = subsystem.run(context);

	let test_fut = test_fn(virtual_overseer);

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	let (_, subsystem) = executor::block_on(future::join(
		async move {
			let mut overseer = test_fut.await;
			overseer
				.send(FromOrchestra::Signal(OverseerSignal::Conclude))
				.timeout(TIMEOUT)
				.await
				.expect("Conclude send timeout");
		},
		subsystem,
	));
	subsystem
}

const TIMEOUT: Duration = Duration::from_millis(100);

async fn overseer_signal_active_leaves(overseer: &mut VirtualOverseer, leaf: Hash) {
	let leaf = ActivatedLeaf {
		hash: leaf,
		number: 0xdeadcafe,
		status: LeafStatus::Fresh,
		span: Arc::new(jaeger::Span::Disabled),
	};
	overseer
		.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			leaf,
		))))
		.timeout(TIMEOUT)
		.await
		.expect("signal send timeout");
}

fn make_session_info() -> SessionInfo {
	let all_validator_indices: Vec<_> = (0..6).map(ValidatorIndex::from).collect();
	SessionInfo {
		active_validator_indices: all_validator_indices.clone(),
		random_seed: [0; 32],
		dispute_period: 6,
		validators: AUTHORITY_KEYRINGS.iter().map(|k| k.public().into()).collect(),
		discovery_keys: AUTHORITIES.clone(),
		assignment_keys: AUTHORITY_KEYRINGS.iter().map(|k| k.public().into()).collect(),
		validator_groups: IndexedVec::<GroupIndex, Vec<ValidatorIndex>>::from(vec![
			all_validator_indices,
		]),
		n_cores: 1,
		zeroth_delay_tranche_width: 1,
		relay_vrf_modulo_samples: 1,
		n_delay_tranches: 1,
		no_show_slots: 1,
		needed_approvals: 1,
	}
}

async fn overseer_recv(overseer: &mut VirtualOverseer) -> AllMessages {
	let msg = overseer.recv().timeout(TIMEOUT).await.expect("msg recv timeout");

	msg
}

async fn test_neighbors(overseer: &mut VirtualOverseer, expected_session: SessionIndex) {
	assert_matches!(
		overseer_recv(overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			_,
			RuntimeApiRequest::CurrentBabeEpoch(tx),
		)) => {
			let _ = tx.send(Ok(BabeEpoch {
				epoch_index: 2 as _,
				start_slot: 0.into(),
				duration: 200,
				authorities: vec![(Sr25519Keyring::Alice.public().into(), 1)],
				randomness: [0u8; 32],
				config: BabeEpochConfiguration {
					c: (1, 4),
					allowed_slots: AllowedSlots::PrimarySlots,
				},
			})).unwrap();
		}
	);

	assert_matches!(
		overseer_recv(overseer).await,
		AllMessages::NetworkBridgeRx(NetworkBridgeRxMessage::NewGossipTopology {
			session: got_session,
			local_index,
			canonical_shuffling,
			shuffled_indices,
		}) => {
			assert_eq!(expected_session, got_session);
			assert_eq!(local_index, Some(ValidatorIndex(6)));
			assert_eq!(shuffled_indices, EXPECTED_SHUFFLING.clone());

			let grid_topology = SessionGridTopology::new(
				shuffled_indices,
				canonical_shuffling.into_iter()
					.map(|(a, v)| TopologyPeerInfo {
						validator_index: v,
						discovery_id: a,
						peer_ids: Vec::new(),
					})
					.collect(),
			);

			let grid_neighbors = grid_topology
				.compute_grid_neighbors_for(local_index.unwrap())
				.unwrap();

			let mut got_row: Vec<_> = grid_neighbors.validator_indices_x.into_iter().collect();
			let mut got_column: Vec<_> = grid_neighbors.validator_indices_y.into_iter().collect();
			got_row.sort();
			got_column.sort();
			assert_eq!(got_row, ROW_NEIGHBORS.clone());
			assert_eq!(got_column, COLUMN_NEIGHBORS.clone());
		}
	);
}

#[test]
fn issues_a_connection_request_on_new_session() {
	let hash = Hash::repeat_byte(0xAA);
	let state = test_harness(make_subsystem(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		overseer_signal_active_leaves(overseer, hash).await;
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionIndexForChild(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(1)).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionInfo(s, tx),
			)) => {
				assert_eq!(relay_parent, hash);
				assert_eq!(s, 1);
				tx.send(Ok(Some(make_session_info()))).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Authorities(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(AUTHORITIES.clone())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ConnectToResolvedValidators {
				validator_addrs,
				peer_set,
			}) => {
				assert_eq!(validator_addrs, get_multiaddrs(AUTHORITIES_WITHOUT_US.clone()).await);
				assert_eq!(peer_set, PeerSet::Validation);
			}
		);

		test_neighbors(overseer, 1).await;

		virtual_overseer
	});

	assert_eq!(state.last_session_index, Some(1));
	assert!(state.last_failure.is_none());

	// does not issue on the same session
	let hash = Hash::repeat_byte(0xBB);
	let state = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		overseer_signal_active_leaves(overseer, hash).await;
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionIndexForChild(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(1)).unwrap();
			}
		);
		virtual_overseer
	});

	assert_eq!(state.last_session_index, Some(1));
	assert!(state.last_failure.is_none());

	// does on the new one
	let hash = Hash::repeat_byte(0xCC);
	let state = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		overseer_signal_active_leaves(overseer, hash).await;
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionIndexForChild(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(2)).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionInfo(s, tx),
			)) => {
				assert_eq!(relay_parent, hash);
				assert_eq!(s, 2);
				tx.send(Ok(Some(make_session_info()))).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Authorities(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(AUTHORITIES.clone())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ConnectToResolvedValidators {
				validator_addrs,
				peer_set,
			}) => {
				assert_eq!(validator_addrs, get_multiaddrs(AUTHORITIES_WITHOUT_US.clone()).await);
				assert_eq!(peer_set, PeerSet::Validation);
			}
		);

		test_neighbors(overseer, 2).await;

		virtual_overseer
	});
	assert_eq!(state.last_session_index, Some(2));
	assert!(state.last_failure.is_none());
}

#[test]
fn issues_connection_request_to_past_present_future() {
	let hash = Hash::repeat_byte(0xAA);
	test_harness(make_subsystem(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		overseer_signal_active_leaves(overseer, hash).await;
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionIndexForChild(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(1)).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionInfo(s, tx),
			)) => {
				assert_eq!(relay_parent, hash);
				assert_eq!(s, 1);
				tx.send(Ok(Some(make_session_info()))).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Authorities(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(PAST_PRESENT_FUTURE_AUTHORITIES.clone())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ConnectToResolvedValidators {
				validator_addrs,
				peer_set,
			}) => {
				let all_without_ferdie: Vec<_> = PAST_PRESENT_FUTURE_AUTHORITIES
					.iter()
					.cloned()
					.filter(|p| p != &Sr25519Keyring::Ferdie.public().into())
					.collect();

				let addrs = get_multiaddrs(all_without_ferdie).await;

				assert_eq!(validator_addrs, addrs);
				assert_eq!(peer_set, PeerSet::Validation);
			}
		);

		// Ensure neighbors are unaffected
		test_neighbors(overseer, 1).await;

		virtual_overseer
	});
}

#[test]
fn disconnect_when_not_in_past_present_future() {
	sp_tracing::try_init_simple();
	let hash = Hash::repeat_byte(0xAA);
	test_harness(make_subsystem(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		overseer_signal_active_leaves(overseer, hash).await;
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionIndexForChild(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(1)).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionInfo(s, tx),
			)) => {
				assert_eq!(relay_parent, hash);
				assert_eq!(s, 1);
				let mut heute_leider_nicht = make_session_info();
				heute_leider_nicht.discovery_keys = AUTHORITIES_WITHOUT_US.clone();
				tx.send(Ok(Some(heute_leider_nicht))).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Authorities(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(AUTHORITIES_WITHOUT_US.clone())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ConnectToResolvedValidators {
				validator_addrs,
				peer_set,
			}) => {
				assert!(validator_addrs.is_empty());
				assert_eq!(peer_set, PeerSet::Validation);
			}
		);

		virtual_overseer
	});
}

#[test]
fn test_log_output() {
	sp_tracing::try_init_simple();
	let alice: AuthorityDiscoveryId = Sr25519Keyring::Alice.public().into();
	let bob = Sr25519Keyring::Bob.public().into();
	let unconnected_authorities = {
		let mut m = HashMap::new();
		let peer_id = PeerId::random();
		let addr = Multiaddr::empty().with(Protocol::P2p(peer_id.into()));
		let addrs = HashSet::from([addr.clone(), addr]);
		m.insert(alice, addrs);
		let peer_id = PeerId::random();
		let addr = Multiaddr::empty().with(Protocol::P2p(peer_id.into()));
		let addrs = HashSet::from([addr.clone(), addr]);
		m.insert(bob, addrs);
		m
	};
	let pretty = PrettyAuthorities(unconnected_authorities.iter());
	gum::debug!(
		target: LOG_TARGET,
		unconnected_authorities = %pretty,
		"Connectivity Report"
	);
}

#[test]
fn issues_a_connection_request_when_last_request_was_mostly_unresolved() {
	let hash = Hash::repeat_byte(0xAA);
	let mut state = make_subsystem();
	// There will be two lookup failures:
	let alice = Sr25519Keyring::Alice.public().into();
	let bob = Sr25519Keyring::Bob.public().into();
	let alice_addr = state.authority_discovery.addrs.remove(&alice);
	state.authority_discovery.addrs.remove(&bob);

	let mut state = {
		let alice = alice.clone();
		let bob = bob.clone();

		test_harness(state, |mut virtual_overseer| async move {
			let overseer = &mut virtual_overseer;
			overseer_signal_active_leaves(overseer, hash).await;
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::SessionIndexForChild(tx),
				)) => {
					assert_eq!(relay_parent, hash);
					tx.send(Ok(1)).unwrap();
				}
			);

			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::SessionInfo(s, tx),
				)) => {
					assert_eq!(relay_parent, hash);
					assert_eq!(s, 1);
					tx.send(Ok(Some(make_session_info()))).unwrap();
				}
			);

			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::Authorities(tx),
				)) => {
					assert_eq!(relay_parent, hash);
					tx.send(Ok(AUTHORITIES.clone())).unwrap();
				}
			);

			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ConnectToResolvedValidators {
					validator_addrs,
					peer_set,
				}) => {
					let mut expected = get_address_map(AUTHORITIES_WITHOUT_US.clone()).await;
					expected.remove(&alice);
					expected.remove(&bob);
					let expected: HashSet<Multiaddr> = expected.into_iter().map(|(_,v)| v.into_iter()).flatten().collect();
					assert_eq!(validator_addrs.into_iter().map(|v| v.into_iter()).flatten().collect::<HashSet<_>>(), expected);
					assert_eq!(peer_set, PeerSet::Validation);
				}
			);

			test_neighbors(overseer, 1).await;

			virtual_overseer
		})
	};

	assert_eq!(state.last_session_index, Some(1));
	assert!(state.last_failure.is_some());
	state.last_failure = state.last_failure.and_then(|i| i.checked_sub(BACKOFF_DURATION));
	// One error less:
	state.authority_discovery.addrs.insert(alice, alice_addr.unwrap());

	let hash = Hash::repeat_byte(0xBB);
	let state = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		overseer_signal_active_leaves(overseer, hash).await;
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionIndexForChild(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(1)).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionInfo(s, tx),
			)) => {
				assert_eq!(relay_parent, hash);
				assert_eq!(s, 1);
				tx.send(Ok(Some(make_session_info()))).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Authorities(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(AUTHORITIES.clone())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ConnectToResolvedValidators {
				validator_addrs,
				peer_set,
			}) => {
				let mut expected = get_address_map(AUTHORITIES_WITHOUT_US.clone()).await;
				expected.remove(&bob);
				let expected: HashSet<Multiaddr> = expected.into_iter().map(|(_,v)| v.into_iter()).flatten().collect();
				assert_eq!(validator_addrs.into_iter().map(|v| v.into_iter()).flatten().collect::<HashSet<_>>(), expected);
				assert_eq!(peer_set, PeerSet::Validation);
			}
		);

		virtual_overseer
	});

	assert_eq!(state.last_session_index, Some(1));
	assert!(state.last_failure.is_none());
}
