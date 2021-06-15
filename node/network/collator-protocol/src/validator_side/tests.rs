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
use std::{iter, time::Duration};
use std::sync::Arc;
use futures::{executor, future, Future};
use sp_core::{crypto::Pair, Encode};
use sp_keystore::SyncCryptoStore;
use sp_keystore::testing::KeyStore as TestKeyStore;
use sp_keyring::Sr25519Keyring;
use assert_matches::assert_matches;

use polkadot_primitives::v1::{
	CollatorPair, ValidatorId, ValidatorIndex, CoreState, CandidateDescriptor,
	GroupRotationInfo, ScheduledCore, OccupiedCore, GroupIndex,
};
use polkadot_node_primitives::BlockData;
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_subsystem_testhelpers as test_helpers;
use polkadot_subsystem::messages::{RuntimeApiMessage, RuntimeApiRequest};
use polkadot_node_network_protocol::{our_view, ObservedRole,
	request_response::Requests
};

const ACTIVITY_TIMEOUT: Duration = Duration::from_millis(50);
const DECLARE_TIMEOUT: Duration = Duration::from_millis(25);

#[derive(Clone)]
struct TestState {
	chain_ids: Vec<ParaId>,
	relay_parent: Hash,
	collators: Vec<CollatorPair>,
	validators: Vec<Sr25519Keyring>,
	validator_public: Vec<ValidatorId>,
	validator_groups: Vec<Vec<ValidatorIndex>>,
	group_rotation_info: GroupRotationInfo,
	cores: Vec<CoreState>,
}

impl Default for TestState {
	fn default() -> Self {
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);

		let chain_ids = vec![chain_a, chain_b];
		let relay_parent = Hash::repeat_byte(0x05);
		let collators = iter::repeat(())
			.map(|_| CollatorPair::generate().0)
			.take(4)
			.collect();

		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Eve,
		];

		let validator_public = validators.iter().map(|k| k.public().into()).collect();
		let validator_groups = vec![
			vec![ValidatorIndex(0), ValidatorIndex(1)],
			vec![ValidatorIndex(2), ValidatorIndex(3)],
			vec![ValidatorIndex(4)],
		];

		let group_rotation_info = GroupRotationInfo {
			session_start_block: 0,
			group_rotation_frequency: 1,
			now: 0,
		};

		let cores = vec![
			CoreState::Scheduled(ScheduledCore {
				para_id: chain_ids[0],
				collator: None,
			}),
			CoreState::Free,
			CoreState::Occupied(OccupiedCore {
				next_up_on_available: None,
				occupied_since: 0,
				time_out_at: 1,
				next_up_on_time_out: None,
				availability: Default::default(),
				group_responsible: GroupIndex(0),
				candidate_hash: Default::default(),
				candidate_descriptor: {
					let mut d = CandidateDescriptor::default();
					d.para_id = chain_ids[1];

					d
				},
			}),
		];

		Self {
			chain_ids,
			relay_parent,
			collators,
			validators,
			validator_public,
			validator_groups,
			group_rotation_info,
			cores,
		}
	}
}

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>;

struct TestHarness {
	virtual_overseer: VirtualOverseer,
}

fn test_harness<T: Future<Output = VirtualOverseer>>(test: impl FnOnce(TestHarness) -> T) {
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

	let keystore = TestKeyStore::new();
	keystore.sr25519_generate_new(
		polkadot_primitives::v1::PARACHAIN_KEY_TYPE_ID,
		Some(&Sr25519Keyring::Alice.to_seed()),
	).unwrap();

	let subsystem = run(
		context,
		Arc::new(keystore),
		crate::CollatorEvictionPolicy {
			inactive_collator: ACTIVITY_TIMEOUT,
			undeclared: DECLARE_TIMEOUT,
		},
		Metrics::default(),
	);

	let test_fut = test(TestHarness { virtual_overseer });

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	executor::block_on(future::join(async move {
		let mut overseer = test_fut.await;
		overseer_signal(&mut overseer, OverseerSignal::Conclude).await;
	}, subsystem)).1.unwrap();
}

const TIMEOUT: Duration = Duration::from_millis(200);

async fn overseer_send(
	overseer: &mut VirtualOverseer,
	msg: CollatorProtocolMessage,
) {
	tracing::trace!("Sending message:\n{:?}", &msg);
	overseer
		.send(FromOverseer::Communication { msg })
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is enough for sending messages.", TIMEOUT));
}

async fn overseer_recv(
	overseer: &mut VirtualOverseer,
) -> AllMessages {
	let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
		.await
		.expect(&format!("{:?} is enough to receive messages.", TIMEOUT));

	tracing::trace!("Received message:\n{:?}", &msg);

	msg
}

async fn overseer_recv_with_timeout(
	overseer: &mut VirtualOverseer,
	timeout: Duration,
) -> Option<AllMessages> {
	tracing::trace!("Waiting for message...");
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

async fn respond_to_core_info_queries(
	virtual_overseer: &mut VirtualOverseer,
	test_state: &TestState,
) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			_,
			RuntimeApiRequest::Validators(tx),
		)) => {
			let _ = tx.send(Ok(test_state.validator_public.clone()));
		}
	);

	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			_,
			RuntimeApiRequest::ValidatorGroups(tx),
		)) => {
			let _ = tx.send(Ok((
				test_state.validator_groups.clone(),
				test_state.group_rotation_info.clone(),
			)));
		}
	);

	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			_,
			RuntimeApiRequest::AvailabilityCores(tx),
		)) => {
			let _ = tx.send(Ok(test_state.cores.clone()));
		}
	);
}

// As we receive a relevant advertisement act on it and issue a collation request.
#[test]
fn act_on_advertisement() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
		} = test_harness;

		let pair = CollatorPair::generate().0;
		tracing::trace!("activating");

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
			)
		).await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_b,
					ObservedRole::Full,
					None,
				),
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						pair.public(),
						test_state.chain_ids[0],
						pair.sign(&protocol_v1::declare_signature_payload(&peer_b)),
					)
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
						test_state.relay_parent,
					)
				)
			)
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
		) => {
			let req = reqs.into_iter().next()
				.expect("There should be exactly one request");
			match req {
				Requests::CollationFetching(req) => {
					let payload = req.payload;
					assert_eq!(payload.relay_parent, test_state.relay_parent);
					assert_eq!(payload.para_id, test_state.chain_ids[0]);
				}
				_ => panic!("Unexpected request"),
			}
		});

		virtual_overseer
	});
}

// Test that other subsystems may modify collators' reputations.
#[test]
fn collator_reporting_works() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
		} = test_harness;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
			)
		).await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();
		let peer_c = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_b,
					ObservedRole::Full,
					None,
				),
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_c,
					ObservedRole::Full,
					None,
				),
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						test_state.collators[0].public(),
						test_state.chain_ids[0],
						test_state.collators[0].sign(&protocol_v1::declare_signature_payload(&peer_b)),
					),
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_c.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						test_state.collators[1].public(),
						test_state.chain_ids[0],
						test_state.collators[1].sign(&protocol_v1::declare_signature_payload(&peer_c)),
					),
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::ReportCollator(test_state.collators[0].public()),
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep),
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, COST_REPORT_BAD);
			}
		);

		virtual_overseer
	});
}

// Test that we verify the signatures on `Declare` and `AdvertiseCollation` messages.
#[test]
fn collator_authentication_verification_works() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
		} = test_harness;

		let peer_b = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_b,
					ObservedRole::Full,
					None,
				),
			)
		).await;

		// the peer sends a declare message but sign the wrong payload
		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(NetworkBridgeEvent::PeerMessage(
				peer_b.clone(),
				protocol_v1::CollatorProtocolMessage::Declare(
					test_state.collators[0].public(),
					test_state.chain_ids[0],
					test_state.collators[0].sign(&[42]),
				),
			)),
		)
		.await;

		// it should be reported for sending a message with an invalid signature
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::ReportPeer(peer, rep),
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, COST_INVALID_SIGNATURE);
			}
		);
		virtual_overseer
	});
}

// A test scenario that takes the following steps
//  - Two collators connect, declare themselves and advertise a collation relevant to
//	our view.
//  - This results subsystem acting upon these advertisements and issuing two messages to
//	the CandidateBacking subsystem.
//  - CandidateBacking requests both of the collations.
//  - Collation protocol requests these collations.
//  - The collations are sent to it.
//  - Collations are fetched correctly.
#[test]
fn fetch_collations_works() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
		} = test_harness;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
			),
		).await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();
		let peer_c = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_b,
					ObservedRole::Full,
					None,
				),
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_c,
					ObservedRole::Full,
					None,
				),
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						test_state.collators[0].public(),
						test_state.chain_ids[0],
						test_state.collators[0].sign(&protocol_v1::declare_signature_payload(&peer_b)),
					)
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_c.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						test_state.collators[1].public(),
						test_state.chain_ids[0],
						test_state.collators[1].sign(&protocol_v1::declare_signature_payload(&peer_c)),
					)
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
						test_state.relay_parent,
					)
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_c.clone(),
					protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
						test_state.relay_parent,
					)
				)
			)
		).await;

		let response_channel = assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
		) => {
			let req = reqs.into_iter().next()
				.expect("There should be exactly one request");
			match req {
				Requests::CollationFetching(req) => {
					let payload = req.payload;
					assert_eq!(payload.relay_parent, test_state.relay_parent);
					assert_eq!(payload.para_id, test_state.chain_ids[0]);
					req.pending_response
				}
				_ => panic!("Unexpected request"),
			}
		});

		let mut candidate_a = CandidateReceipt::default();
		candidate_a.descriptor.para_id = test_state.chain_ids[0];
		candidate_a.descriptor.relay_parent = test_state.relay_parent;
		response_channel.send(Ok(
			CollationFetchingResponse::Collation(
				candidate_a.clone(),
				PoV {
					block_data: BlockData(vec![]),
				},
			).encode()
		)).expect("Sending response should succeed");

		let _ = assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
		) => {
			let req = reqs.into_iter().next()
				.expect("There should be exactly one request");
			match req {
				Requests::CollationFetching(req) => {
					let payload = req.payload;
					assert_eq!(payload.relay_parent, test_state.relay_parent);
					assert_eq!(payload.para_id, test_state.chain_ids[0]);
					req.pending_response
				}
				_ => panic!("Unexpected request"),
			}
		});

		virtual_overseer
	});
}

#[test]
fn inactive_disconnected() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
		} = test_harness;

		let pair = CollatorPair::generate().0;

		let hash_a = test_state.relay_parent;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::OurViewChange(our_view![hash_a])
			)
		).await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_b.clone(),
					ObservedRole::Full,
					None,
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						pair.public(),
						test_state.chain_ids[0],
						pair.sign(&protocol_v1::declare_signature_payload(&peer_b)),
					)
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
						test_state.relay_parent,
					)
				)
			)
		).await;

		let _ = assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
		) => {
			let req = reqs.into_iter().next()
				.expect("There should be exactly one request");
			match req {
				Requests::CollationFetching(req) => {
					let payload = req.payload;
					assert_eq!(payload.relay_parent, test_state.relay_parent);
					assert_eq!(payload.para_id, test_state.chain_ids[0]);
					req.pending_response
				}
				_ => panic!("Unexpected request"),
			}
		});

		Delay::new(ACTIVITY_TIMEOUT * 3).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ReportPeer(
				peer,
				rep,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, COST_REQUEST_TIMED_OUT);
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(
				peer,
				peer_set,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(peer_set, PeerSet::Collation);
			}
		);
		virtual_overseer
	});
}

#[test]
fn activity_extends_life() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
		} = test_harness;

		let pair = CollatorPair::generate().0;

		let hash_a = test_state.relay_parent;
		let hash_b = Hash::repeat_byte(1);
		let hash_c = Hash::repeat_byte(2);

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::OurViewChange(our_view![hash_a, hash_b, hash_c])
			)
		).await;

		// 3 heads, 3 times.
		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;
		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;
		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_b.clone(),
					ObservedRole::Full,
					None,
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						pair.public(),
						test_state.chain_ids[0],
						pair.sign(&protocol_v1::declare_signature_payload(&peer_b)),
					)
				)
			)
		).await;

		Delay::new(ACTIVITY_TIMEOUT * 2 / 3).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
						hash_a,
					)
				)
			)
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
		) => {
			let req = reqs.into_iter().next()
				.expect("There should be exactly one request");
			match req {
				Requests::CollationFetching(req) => {
					let payload = req.payload;
					assert_eq!(payload.relay_parent, hash_a);
					assert_eq!(payload.para_id, test_state.chain_ids[0]);
				}
				_ => panic!("Unexpected request"),
			}
		});

		Delay::new(ACTIVITY_TIMEOUT * 2 / 3).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
						hash_b
					)
				)
			)
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ReportPeer(
				peer,
				rep,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, COST_REQUEST_TIMED_OUT);
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
		) => {
			let req = reqs.into_iter().next()
				.expect("There should be exactly one request");
			match req {
				Requests::CollationFetching(req) => {
					let payload = req.payload;
					assert_eq!(payload.relay_parent, hash_b);
					assert_eq!(payload.para_id, test_state.chain_ids[0]);
				}
				_ => panic!("Unexpected request"),
			}
		});

		Delay::new(ACTIVITY_TIMEOUT * 2 / 3).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
						hash_c,
					)
				)
			)
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ReportPeer(
				peer,
				rep,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, COST_REQUEST_TIMED_OUT);
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
		) => {
			let req = reqs.into_iter().next()
				.expect("There should be exactly one request");
			match req {
				Requests::CollationFetching(req) => {
					let payload = req.payload;
					assert_eq!(payload.relay_parent, hash_c);
					assert_eq!(payload.para_id, test_state.chain_ids[0]);
				}
				_ => panic!("Unexpected request"),
			}
		});

		Delay::new(ACTIVITY_TIMEOUT * 3 / 2).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ReportPeer(
				peer,
				rep,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, COST_REQUEST_TIMED_OUT);
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(
				peer,
				peer_set,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(peer_set, PeerSet::Collation);
			}
		);
		virtual_overseer
	});
}

#[test]
fn disconnect_if_no_declare() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
		} = test_harness;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
			)
		).await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_b.clone(),
					ObservedRole::Full,
					None,
				)
			)
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(
				peer,
				peer_set,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(peer_set, PeerSet::Collation);
			}
		);
		virtual_overseer
	})
}

#[test]
fn disconnect_if_wrong_declare() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
		} = test_harness;

		let pair = CollatorPair::generate().0;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
			)
		).await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_b.clone(),
					ObservedRole::Full,
					None,
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						pair.public(),
						ParaId::from(69),
						pair.sign(&protocol_v1::declare_signature_payload(&peer_b)),
					)
				)
			)
		).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::ReportPeer(
				peer,
				rep,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, COST_UNNEEDED_COLLATOR);
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(
				peer,
				peer_set,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(peer_set, PeerSet::Collation);
			}
		);
		virtual_overseer
	})
}

#[test]
fn view_change_clears_old_collators() {
	let mut test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
		} = test_harness;

		let pair = CollatorPair::generate().0;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
			)
		).await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_b.clone(),
					ObservedRole::Full,
					None,
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						pair.public(),
						test_state.chain_ids[0],
						pair.sign(&protocol_v1::declare_signature_payload(&peer_b)),
					)
				)
			)
		).await;

		let hash_b = Hash::repeat_byte(69);

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::OurViewChange(our_view![hash_b])
			)
		).await;

		test_state.group_rotation_info = test_state.group_rotation_info.bump_rotation();
		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(
				peer,
				peer_set,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(peer_set, PeerSet::Collation);
			}
		);
		virtual_overseer
	})
}

// A test scenario that takes the following steps
//  - Two collators connect, declare themselves and advertise a collation relevant to
//	our view.
//  - This results subsystem acting upon these advertisements and issuing two messages to
//	the CandidateBacking subsystem.
//  - CandidateBacking requests both of the collations.
//  - Collation protocol requests these collations.
//  - The collations are sent to it.
//  - Collations are fetched correctly.
#[test]
fn seconding_works() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness {
			mut virtual_overseer,
		} = test_harness;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::OurViewChange(our_view![test_state.relay_parent])
			),
		).await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();
		let peer_c = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_b,
					ObservedRole::Full,
					None,
				),
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerConnected(
					peer_c,
					ObservedRole::Full,
					None,
				),
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						test_state.collators[0].public(),
						test_state.chain_ids[0],
						test_state.collators[0].sign(&protocol_v1::declare_signature_payload(&peer_b)),
					)
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_c.clone(),
					protocol_v1::CollatorProtocolMessage::Declare(
						test_state.collators[1].public(),
						test_state.chain_ids[0],
						test_state.collators[1].sign(&protocol_v1::declare_signature_payload(&peer_c)),
					)
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
						test_state.relay_parent,
					)
				)
			)
		).await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerMessage(
					peer_c.clone(),
					protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
						test_state.relay_parent,
					)
				)
			)
		).await;

		let response_channel = assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
		) => {
			let req = reqs.into_iter().next()
				.expect("There should be exactly one request");
			match req {
				Requests::CollationFetching(req) => {
					let payload = req.payload;
					assert_eq!(payload.relay_parent, test_state.relay_parent);
					assert_eq!(payload.para_id, test_state.chain_ids[0]);
					req.pending_response
				}
				_ => panic!("Unexpected request"),
			}
		});

		let mut candidate_a = CandidateReceipt::default();
		// Memoize PoV data to ensure we receive the right one
		let pov = PoV {
			block_data: BlockData(vec![1, 2, 3, 4, 5]),
		};
		candidate_a.descriptor.para_id = test_state.chain_ids[0];
		candidate_a.descriptor.relay_parent = test_state.relay_parent;
		response_channel.send(Ok(
			CollationFetchingResponse::Collation(
				candidate_a.clone(),
				pov.clone(),
			).encode()
		)).expect("Sending response should succeed");

		let response_channel = assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
		) => {
			let req = reqs.into_iter().next()
				.expect("There should be exactly one request");
			match req {
				Requests::CollationFetching(req) => {
					let payload = req.payload;
					assert_eq!(payload.relay_parent, test_state.relay_parent);
					assert_eq!(payload.para_id, test_state.chain_ids[0]);
					req.pending_response
				}
				_ => panic!("Unexpected request"),
			}
		});

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::CandidateBacking(CandidateBackingMessage::Second(relay_parent, candidate_receipt, incoming_pov)
		) => {
					assert_eq!(relay_parent, test_state.relay_parent);
					assert_eq!(candidate_receipt.descriptor.para_id, test_state.chain_ids[0]);
					assert_eq!(incoming_pov, pov);
		});

		let mut candidate_b = CandidateReceipt::default();
		candidate_b.descriptor.para_id = test_state.chain_ids[0];
		candidate_b.descriptor.relay_parent = test_state.relay_parent;

		// Send second collation to ensure first collation gets seconded
		response_channel.send(Ok(
			CollationFetchingResponse::Collation(
				candidate_b.clone(),
				PoV {
					block_data: BlockData(vec![]),
				},
			).encode()
		)).expect("Sending response should succeed after seconding");

		// Ensure we don't receive any message related to candidate backing
		// All Peers should get disconnected after successful Candidate Backing Message
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::DisconnectPeer(_, _)
		) => {});

		virtual_overseer
	});
}
