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

use super::*;
use polkadot_node_subsystem::{
	jaeger,
	messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest},
	ActivatedLeaf, LeafStatus,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt as _;
use sp_consensus_babe::{AllowedSlots, BabeEpochConfiguration, Epoch as BabeEpoch};
use sp_keyring::Sr25519Keyring;
use test_helpers::mock::make_ferdie_keystore;

use assert_matches::assert_matches;
use futures::{executor, future, Future};
use std::{sync::Arc, time::Duration};

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<GossipSupportMessage>;

fn test_harness<T: Future<Output = VirtualOverseer>>(
	mut state: State,
	test_fn: impl FnOnce(VirtualOverseer) -> T,
) -> State {
	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let keystore = make_ferdie_keystore();
	let subsystem = GossipSupport::new(keystore);
	{
		let subsystem = subsystem.run_inner(context, &mut state);

		let test_fut = test_fn(virtual_overseer);

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::join(
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
	}

	state
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

fn authorities() -> Vec<AuthorityDiscoveryId> {
	vec![
		Sr25519Keyring::Alice.public().into(),
		Sr25519Keyring::Bob.public().into(),
		Sr25519Keyring::Charlie.public().into(),
		Sr25519Keyring::Ferdie.public().into(),
		Sr25519Keyring::Eve.public().into(),
		Sr25519Keyring::One.public().into(),
	]
}

fn neighbors() -> Vec<AuthorityDiscoveryId> {
	vec![
		Sr25519Keyring::One.public().into(),
		Sr25519Keyring::Alice.public().into(),
		Sr25519Keyring::Eve.public().into(),
	]
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
			assert_eq!(got, neighbors());
		}
	);
}

#[test]
fn issues_a_connection_request_on_new_session() {
	let hash = Hash::repeat_byte(0xAA);
	let state = test_harness(State::default(), |mut virtual_overseer| async move {
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
				tx.send(Ok(authorities())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ConnectToValidators {
				validator_ids,
				peer_set,
				failed,
			}) => {
				assert_eq!(validator_ids, authorities());
				assert_eq!(peer_set, PeerSet::Validation);
				failed.send(0).unwrap();
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
				tx.send(Ok(authorities())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ConnectToValidators {
				validator_ids,
				peer_set,
				failed,
			}) => {
				assert_eq!(validator_ids, authorities());
				assert_eq!(peer_set, PeerSet::Validation);
				failed.send(0).unwrap();
			}
		);

		test_neighbors(overseer).await;

		virtual_overseer
	});
	assert_eq!(state.last_session_index, Some(2));
	assert!(state.last_failure.is_none());
}

#[test]
fn issues_a_connection_request_when_last_request_was_mostly_unresolved() {
	let hash = Hash::repeat_byte(0xAA);
	let mut state = test_harness(State::default(), |mut virtual_overseer| async move {
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
				tx.send(Ok(authorities())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ConnectToValidators {
				validator_ids,
				peer_set,
				failed,
			}) => {
				assert_eq!(validator_ids, authorities());
				assert_eq!(peer_set, PeerSet::Validation);
				failed.send(2).unwrap();
			}
		);

		test_neighbors(overseer).await;

		virtual_overseer
	});

	assert_eq!(state.last_session_index, Some(1));
	assert!(state.last_failure.is_some());
	state.last_failure = state.last_failure.and_then(|i| i.checked_sub(BACKOFF_DURATION));

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
				tx.send(Ok(authorities())).unwrap();
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ConnectToValidators {
				validator_ids,
				peer_set,
				failed,
			}) => {
				assert_eq!(validator_ids, authorities());
				assert_eq!(peer_set, PeerSet::Validation);
				failed.send(1).unwrap();
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
