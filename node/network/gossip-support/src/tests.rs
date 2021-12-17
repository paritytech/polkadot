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
use sp_consensus_babe::{AllowedSlots, BabeEpochConfiguration, Epoch as BabeEpoch};
use sp_keyring::Sr25519Keyring;

use polkadot_node_subsystem::{
	jaeger,
	messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest},
	ActivatedLeaf, LeafStatus,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt as _;
use test_helpers::mock::make_ferdie_keystore;

use super::*;

lazy_static! {
	static ref MOCK_AUTHORITY_DISCOVERY: MockAuthorityDiscovery = MockAuthorityDiscovery::new();
	static ref AUTHORITIES: Vec<AuthorityDiscoveryId> = {
		let mut authorities = OTHER_AUTHORITIES.clone();
		authorities.push(Sr25519Keyring::Ferdie.public().into());
		authorities
	};
	static ref OTHER_AUTHORITIES: Vec<AuthorityDiscoveryId> = vec![
		Sr25519Keyring::Alice.public().into(),
		Sr25519Keyring::Bob.public().into(),
		Sr25519Keyring::Charlie.public().into(),
		Sr25519Keyring::Eve.public().into(),
		Sr25519Keyring::One.public().into(),
		Sr25519Keyring::Two.public().into(),
	];
	static ref NEIGHBORS: Vec<AuthorityDiscoveryId> = vec![
		Sr25519Keyring::Two.public().into(),
		Sr25519Keyring::Charlie.public().into(),
		Sr25519Keyring::Eve.public().into(),
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
		let authorities: HashMap<_, _> =
			AUTHORITIES.clone().into_iter().map(|a| (PeerId::random(), a)).collect();
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
		authority: polkadot_primitives::v1::AuthorityDiscoveryId,
	) -> Option<HashSet<sc_network::Multiaddr>> {
		self.addrs.get(&authority).cloned()
	}
	async fn get_authority_ids_by_peer_id(
		&mut self,
		peer_id: polkadot_node_network_protocol::PeerId,
	) -> Option<HashSet<polkadot_primitives::v1::AuthorityDiscoveryId>> {
		self.authorities.get(&peer_id).cloned()
	}
}

async fn get_other_authorities_addrs() -> Vec<HashSet<Multiaddr>> {
	let mut addrs = Vec::with_capacity(OTHER_AUTHORITIES.len());
	let mut discovery = MOCK_AUTHORITY_DISCOVERY.clone();
	for authority in OTHER_AUTHORITIES.iter().cloned() {
		if let Some(addr) = discovery.get_addresses_by_authority_id(authority).await {
			addrs.push(addr);
		}
	}
	addrs
}

async fn get_other_authorities_addrs_map() -> HashMap<AuthorityDiscoveryId, HashSet<Multiaddr>> {
	let mut addrs = HashMap::with_capacity(OTHER_AUTHORITIES.len());
	let mut discovery = MOCK_AUTHORITY_DISCOVERY.clone();
	for authority in OTHER_AUTHORITIES.iter().cloned() {
		if let Some(addr) = discovery.get_addresses_by_authority_id(authority.clone()).await {
			addrs.insert(authority, addr);
		}
	}
	addrs
}

fn make_subsystem() -> GossipSupport<MockAuthorityDiscovery> {
	GossipSupport::new(make_ferdie_keystore(), MOCK_AUTHORITY_DISCOVERY.clone())
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
				.send(FromOverseer::Signal(OverseerSignal::Conclude))
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
		.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			leaf,
		))))
		.timeout(TIMEOUT)
		.await
		.expect("signal send timeout");
}

async fn overseer_recv(overseer: &mut VirtualOverseer) -> AllMessages {
	let msg = overseer.recv().timeout(TIMEOUT).await.expect("msg recv timeout");

	msg
}

async fn test_neighbors(overseer: &mut VirtualOverseer) {
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
		AllMessages::NetworkBridge(NetworkBridgeMessage::NewGossipTopology {
			our_neighbors,
		}) => {
			let mut got: Vec<_> = our_neighbors.into_iter().collect();
			got.sort();
			assert_eq!(got, NEIGHBORS.clone());
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
				RuntimeApiRequest::Authorities(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(AUTHORITIES.clone())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ConnectToResolvedValidators {
				validator_addrs,
				peer_set,
			}) => {
				assert_eq!(validator_addrs, get_other_authorities_addrs().await);
				assert_eq!(peer_set, PeerSet::Validation);
			}
		);

		test_neighbors(overseer).await;

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
				RuntimeApiRequest::Authorities(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(AUTHORITIES.clone())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ConnectToResolvedValidators {
				validator_addrs,
				peer_set,
			}) => {
				assert_eq!(validator_addrs, get_other_authorities_addrs().await);
				assert_eq!(peer_set, PeerSet::Validation);
			}
		);

		test_neighbors(overseer).await;

		virtual_overseer
	});
	assert_eq!(state.last_session_index, Some(2));
	assert!(state.last_failure.is_none());
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
	tracing::debug!(
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
					RuntimeApiRequest::Authorities(tx),
				)) => {
					assert_eq!(relay_parent, hash);
					tx.send(Ok(AUTHORITIES.clone())).unwrap();
				}
			);

			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::ConnectToResolvedValidators {
					validator_addrs,
					peer_set,
				}) => {
					let mut expected = get_other_authorities_addrs_map().await;
					expected.remove(&alice);
					expected.remove(&bob);
					let expected: HashSet<Multiaddr> = expected.into_iter().map(|(_,v)| v.into_iter()).flatten().collect();
					assert_eq!(validator_addrs.into_iter().map(|v| v.into_iter()).flatten().collect::<HashSet<_>>(), expected);
					assert_eq!(peer_set, PeerSet::Validation);
				}
			);

			test_neighbors(overseer).await;

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
				RuntimeApiRequest::Authorities(tx),
			)) => {
				assert_eq!(relay_parent, hash);
				tx.send(Ok(AUTHORITIES.clone())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ConnectToResolvedValidators {
				validator_addrs,
				peer_set,
			}) => {
				let mut expected = get_other_authorities_addrs_map().await;
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

#[test]
fn test_matrix_neighbors() {
	for (our_index, len, expected) in vec![
		(0usize, 1usize, vec![]),
		(1, 2, vec![0usize]),
		(0, 9, vec![1, 2, 3, 6]),
		(9, 10, vec![0, 3, 6]),
		(10, 11, vec![1, 4, 7, 9]),
		(7, 11, vec![1, 4, 6, 8, 10]),
	]
	.into_iter()
	{
		let mut result: Vec<_> = matrix_neighbors(our_index, len).collect();
		result.sort();
		assert_eq!(result, expected);
	}
}
