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
use futures::{executor, future, Future};
use sp_core::{crypto::Pair, Encode};
use sp_keyring::Sr25519Keyring;
use sp_keystore::{testing::KeyStore as TestKeyStore, SyncCryptoStore};
use std::{iter, sync::Arc, task::Poll, time::Duration};

use polkadot_node_network_protocol::{
	our_view,
	peer_set::CollationVersion,
	request_response::{Requests, ResponseSender},
	ObservedRole,
};
use polkadot_node_primitives::BlockData;
use polkadot_node_subsystem::messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_primitives::v2::{
	CollatorPair, CoreState, GroupIndex, GroupRotationInfo, OccupiedCore, ScheduledCore,
	ValidatorId, ValidatorIndex,
};
use polkadot_primitives_test_helpers::{
	dummy_candidate_descriptor, dummy_candidate_receipt_bad_sig, dummy_hash,
};

const ACTIVITY_TIMEOUT: Duration = Duration::from_millis(500);
const DECLARE_TIMEOUT: Duration = Duration::from_millis(25);

#[derive(Clone)]
struct TestState {
	chain_ids: Vec<ParaId>,
	relay_parent: Hash,
	collators: Vec<CollatorPair>,
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
		let collators = iter::repeat(()).map(|_| CollatorPair::generate().0).take(5).collect();

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

		let group_rotation_info =
			GroupRotationInfo { session_start_block: 0, group_rotation_frequency: 1, now: 0 };

		let cores = vec![
			CoreState::Scheduled(ScheduledCore { para_id: chain_ids[0], collator: None }),
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
					let mut d = dummy_candidate_descriptor(dummy_hash());
					d.para_id = chain_ids[1];

					d
				},
			}),
		];

		Self {
			chain_ids,
			relay_parent,
			collators,
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
		.filter(Some("polkadot_collator_protocol"), log::LevelFilter::Trace)
		.filter(Some(LOG_TARGET), log::LevelFilter::Trace)
		.try_init();

	let pool = sp_core::testing::TaskExecutor::new();

	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let keystore = TestKeyStore::new();
	keystore
		.sr25519_generate_new(
			polkadot_primitives::v2::PARACHAIN_KEY_TYPE_ID,
			Some(&Sr25519Keyring::Alice.to_seed()),
		)
		.unwrap();

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

	executor::block_on(future::join(
		async move {
			let mut overseer = test_fut.await;
			overseer_signal(&mut overseer, OverseerSignal::Conclude).await;
		},
		subsystem,
	))
	.1
	.unwrap();
}

const TIMEOUT: Duration = Duration::from_millis(200);

async fn overseer_send(overseer: &mut VirtualOverseer, msg: CollatorProtocolMessage) {
	gum::trace!("Sending message:\n{:?}", &msg);
	overseer
		.send(FromOrchestra::Communication { msg })
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is enough for sending messages.", TIMEOUT));
}

async fn overseer_recv(overseer: &mut VirtualOverseer) -> AllMessages {
	let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
		.await
		.expect(&format!("{:?} is enough to receive messages.", TIMEOUT));

	gum::trace!("Received message:\n{:?}", &msg);

	msg
}

async fn overseer_recv_with_timeout(
	overseer: &mut VirtualOverseer,
	timeout: Duration,
) -> Option<AllMessages> {
	gum::trace!("Waiting for message...");
	overseer.recv().timeout(timeout).await
}

async fn overseer_signal(overseer: &mut VirtualOverseer, signal: OverseerSignal) {
	overseer
		.send(FromOrchestra::Signal(signal))
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

/// Assert that the next message is a `CandidateBacking(Second())`.
async fn assert_candidate_backing_second(
	virtual_overseer: &mut VirtualOverseer,
	expected_relay_parent: Hash,
	expected_para_id: ParaId,
	expected_pov: &PoV,
) -> CandidateReceipt {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::CandidateBacking(CandidateBackingMessage::Second(relay_parent, candidate_receipt, incoming_pov)
	) => {
		assert_eq!(expected_relay_parent, relay_parent);
		assert_eq!(expected_para_id, candidate_receipt.descriptor.para_id);
		assert_eq!(*expected_pov, incoming_pov);
		candidate_receipt
	})
}

/// Assert that a collator got disconnected.
async fn assert_collator_disconnect(virtual_overseer: &mut VirtualOverseer, expected_peer: PeerId) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::DisconnectPeer(
			peer,
			peer_set,
		)) => {
			assert_eq!(expected_peer, peer);
			assert_eq!(PeerSet::Collation, peer_set);
		}
	);
}

/// Assert that a fetch collation request was send.
async fn assert_fetch_collation_request(
	virtual_overseer: &mut VirtualOverseer,
	relay_parent: Hash,
	para_id: ParaId,
) -> ResponseSender {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
	) => {
		let req = reqs.into_iter().next()
			.expect("There should be exactly one request");
		match req {
			Requests::CollationFetchingV1(req) => {
				let payload = req.payload;
				assert_eq!(payload.relay_parent, relay_parent);
				assert_eq!(payload.para_id, para_id);
				req.pending_response
			}
			_ => panic!("Unexpected request"),
		}
	})
}

/// Connect and declare a collator
async fn connect_and_declare_collator(
	virtual_overseer: &mut VirtualOverseer,
	peer: PeerId,
	collator: CollatorPair,
	para_id: ParaId,
) {
	overseer_send(
		virtual_overseer,
		CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerConnected(
			peer.clone(),
			ObservedRole::Full,
			CollationVersion::V1.into(),
			None,
		)),
	)
	.await;

	overseer_send(
		virtual_overseer,
		CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerMessage(
			peer.clone(),
			Versioned::V1(protocol_v1::CollatorProtocolMessage::Declare(
				collator.public(),
				para_id,
				collator.sign(&protocol_v1::declare_signature_payload(&peer)),
			)),
		)),
	)
	.await;
}

/// Advertise a collation.
async fn advertise_collation(
	virtual_overseer: &mut VirtualOverseer,
	peer: PeerId,
	relay_parent: Hash,
) {
	overseer_send(
		virtual_overseer,
		CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerMessage(
			peer,
			Versioned::V1(protocol_v1::CollatorProtocolMessage::AdvertiseCollation(relay_parent)),
		)),
	)
	.await;
}

// As we receive a relevant advertisement act on it and issue a collation request.
#[test]
fn act_on_advertisement() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		let pair = CollatorPair::generate().0;
		gum::trace!("activating");

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![test_state.relay_parent],
			)),
		)
		.await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_b.clone(),
			pair.clone(),
			test_state.chain_ids[0],
		)
		.await;

		advertise_collation(&mut virtual_overseer, peer_b.clone(), test_state.relay_parent).await;

		assert_fetch_collation_request(
			&mut virtual_overseer,
			test_state.relay_parent,
			test_state.chain_ids[0],
		)
		.await;

		virtual_overseer
	});
}

// Test that other subsystems may modify collators' reputations.
#[test]
fn collator_reporting_works() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![test_state.relay_parent],
			)),
		)
		.await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();
		let peer_c = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_b.clone(),
			test_state.collators[0].clone(),
			test_state.chain_ids[0].clone(),
		)
		.await;

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_c.clone(),
			test_state.collators[1].clone(),
			test_state.chain_ids[0].clone(),
		)
		.await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::ReportCollator(test_state.collators[0].public()),
		)
		.await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(peer, rep),
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
		let TestHarness { mut virtual_overseer } = test_harness;

		let peer_b = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerConnected(
				peer_b,
				ObservedRole::Full,
				CollationVersion::V1.into(),
				None,
			)),
		)
		.await;

		// the peer sends a declare message but sign the wrong payload
		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerMessage(
				peer_b.clone(),
				Versioned::V1(protocol_v1::CollatorProtocolMessage::Declare(
					test_state.collators[0].public(),
					test_state.chain_ids[0],
					test_state.collators[0].sign(&[42]),
				)),
			)),
		)
		.await;

		// it should be reported for sending a message with an invalid signature
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(peer, rep),
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, COST_INVALID_SIGNATURE);
			}
		);
		virtual_overseer
	});
}

/// Tests that a validator fetches only one collation at any moment of time
/// per relay parent and ignores other advertisements once a candidate gets
/// seconded.
#[test]
fn fetch_one_collation_at_a_time() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		let second = Hash::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![test_state.relay_parent, second],
			)),
		)
		.await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;
		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();
		let peer_c = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_b.clone(),
			test_state.collators[0].clone(),
			test_state.chain_ids[0].clone(),
		)
		.await;

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_c.clone(),
			test_state.collators[1].clone(),
			test_state.chain_ids[0].clone(),
		)
		.await;

		advertise_collation(&mut virtual_overseer, peer_b.clone(), test_state.relay_parent).await;
		advertise_collation(&mut virtual_overseer, peer_c.clone(), test_state.relay_parent).await;

		let response_channel = assert_fetch_collation_request(
			&mut virtual_overseer,
			test_state.relay_parent,
			test_state.chain_ids[0],
		)
		.await;

		assert!(
			overseer_recv_with_timeout(&mut &mut virtual_overseer, Duration::from_millis(30)).await.is_none(),
			"There should not be sent any other PoV request while the first one wasn't finished or timed out.",
		);

		let pov = PoV { block_data: BlockData(vec![]) };
		let mut candidate_a =
			dummy_candidate_receipt_bad_sig(dummy_hash(), Some(Default::default()));
		candidate_a.descriptor.para_id = test_state.chain_ids[0];
		candidate_a.descriptor.relay_parent = test_state.relay_parent;
		response_channel
			.send(Ok(
				CollationFetchingResponse::Collation(candidate_a.clone(), pov.clone()).encode()
			))
			.expect("Sending response should succeed");

		assert_candidate_backing_second(
			&mut virtual_overseer,
			test_state.relay_parent,
			test_state.chain_ids[0],
			&pov,
		)
		.await;

		// Ensure the subsystem is polled.
		test_helpers::Yield::new().await;

		// Second collation is not requested since there's already seconded one.
		assert_matches!(futures::poll!(virtual_overseer.recv().boxed()), Poll::Pending);

		virtual_overseer
	})
}

/// Tests that a validator starts fetching next queued collations on [`MAX_UNSHARED_DOWNLOAD_TIME`]
/// timeout and in case of an error.
#[test]
fn fetches_next_collation() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		let second = Hash::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![test_state.relay_parent, second],
			)),
		)
		.await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;
		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();
		let peer_c = PeerId::random();
		let peer_d = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_b.clone(),
			test_state.collators[2].clone(),
			test_state.chain_ids[0].clone(),
		)
		.await;

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_c.clone(),
			test_state.collators[3].clone(),
			test_state.chain_ids[0].clone(),
		)
		.await;

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_d.clone(),
			test_state.collators[4].clone(),
			test_state.chain_ids[0].clone(),
		)
		.await;

		advertise_collation(&mut virtual_overseer, peer_b.clone(), second).await;
		advertise_collation(&mut virtual_overseer, peer_c.clone(), second).await;
		advertise_collation(&mut virtual_overseer, peer_d.clone(), second).await;

		// Dropping the response channel should lead to fetching the second collation.
		assert_fetch_collation_request(&mut virtual_overseer, second, test_state.chain_ids[0])
			.await;

		let response_channel_non_exclusive =
			assert_fetch_collation_request(&mut virtual_overseer, second, test_state.chain_ids[0])
				.await;

		// Third collator should receive response after that timeout:
		Delay::new(MAX_UNSHARED_DOWNLOAD_TIME + Duration::from_millis(50)).await;

		let response_channel =
			assert_fetch_collation_request(&mut virtual_overseer, second, test_state.chain_ids[0])
				.await;

		let pov = PoV { block_data: BlockData(vec![1]) };
		let mut candidate_a =
			dummy_candidate_receipt_bad_sig(dummy_hash(), Some(Default::default()));
		candidate_a.descriptor.para_id = test_state.chain_ids[0];
		candidate_a.descriptor.relay_parent = second;

		// First request finishes now:
		response_channel_non_exclusive
			.send(Ok(
				CollationFetchingResponse::Collation(candidate_a.clone(), pov.clone()).encode()
			))
			.expect("Sending response should succeed");

		response_channel
			.send(Ok(
				CollationFetchingResponse::Collation(candidate_a.clone(), pov.clone()).encode()
			))
			.expect("Sending response should succeed");

		assert_candidate_backing_second(
			&mut virtual_overseer,
			second,
			test_state.chain_ids[0],
			&pov,
		)
		.await;

		virtual_overseer
	});
}

#[test]
fn reject_connection_to_next_group() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![test_state.relay_parent],
			)),
		)
		.await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_b.clone(),
			test_state.collators[0].clone(),
			test_state.chain_ids[1].clone(), // next, not current `para_id`
		)
		.await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(
				peer,
				rep,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, COST_UNNEEDED_COLLATOR);
			}
		);

		assert_collator_disconnect(&mut virtual_overseer, peer_b).await;

		virtual_overseer
	})
}

// Ensure that we fetch a second collation, after the first checked collation was found to be invalid.
#[test]
fn fetch_next_collation_on_invalid_collation() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		let second = Hash::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![test_state.relay_parent, second],
			)),
		)
		.await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;
		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();
		let peer_c = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_b.clone(),
			test_state.collators[0].clone(),
			test_state.chain_ids[0].clone(),
		)
		.await;

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_c.clone(),
			test_state.collators[1].clone(),
			test_state.chain_ids[0].clone(),
		)
		.await;

		advertise_collation(&mut virtual_overseer, peer_b.clone(), test_state.relay_parent).await;
		advertise_collation(&mut virtual_overseer, peer_c.clone(), test_state.relay_parent).await;

		let response_channel = assert_fetch_collation_request(
			&mut virtual_overseer,
			test_state.relay_parent,
			test_state.chain_ids[0],
		)
		.await;

		let pov = PoV { block_data: BlockData(vec![]) };
		let mut candidate_a =
			dummy_candidate_receipt_bad_sig(dummy_hash(), Some(Default::default()));
		candidate_a.descriptor.para_id = test_state.chain_ids[0];
		candidate_a.descriptor.relay_parent = test_state.relay_parent;
		response_channel
			.send(Ok(
				CollationFetchingResponse::Collation(candidate_a.clone(), pov.clone()).encode()
			))
			.expect("Sending response should succeed");

		let receipt = assert_candidate_backing_second(
			&mut virtual_overseer,
			test_state.relay_parent,
			test_state.chain_ids[0],
			&pov,
		)
		.await;

		// Inform that the candidate was invalid.
		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::Invalid(test_state.relay_parent, receipt),
		)
		.await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(
				peer,
				rep,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, COST_REPORT_BAD);
			}
		);

		// We should see a request for another collation.
		assert_fetch_collation_request(
			&mut virtual_overseer,
			test_state.relay_parent,
			test_state.chain_ids[0],
		)
		.await;

		virtual_overseer
	});
}

#[test]
fn inactive_disconnected() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		let pair = CollatorPair::generate().0;

		let hash_a = test_state.relay_parent;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![hash_a],
			)),
		)
		.await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_b.clone(),
			pair.clone(),
			test_state.chain_ids[0],
		)
		.await;
		advertise_collation(&mut virtual_overseer, peer_b.clone(), test_state.relay_parent).await;

		assert_fetch_collation_request(
			&mut virtual_overseer,
			test_state.relay_parent,
			test_state.chain_ids[0],
		)
		.await;

		Delay::new(ACTIVITY_TIMEOUT * 3).await;

		assert_collator_disconnect(&mut virtual_overseer, peer_b.clone()).await;
		virtual_overseer
	});
}

#[test]
fn activity_extends_life() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		let pair = CollatorPair::generate().0;

		let hash_a = test_state.relay_parent;
		let hash_b = Hash::repeat_byte(1);
		let hash_c = Hash::repeat_byte(2);

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![hash_a, hash_b, hash_c],
			)),
		)
		.await;

		// 3 heads, 3 times.
		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;
		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;
		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_b.clone(),
			pair.clone(),
			test_state.chain_ids[0],
		)
		.await;

		Delay::new(ACTIVITY_TIMEOUT * 2 / 3).await;

		advertise_collation(&mut virtual_overseer, peer_b.clone(), hash_a).await;

		assert_fetch_collation_request(&mut virtual_overseer, hash_a, test_state.chain_ids[0])
			.await;

		Delay::new(ACTIVITY_TIMEOUT * 2 / 3).await;

		advertise_collation(&mut virtual_overseer, peer_b.clone(), hash_b).await;

		assert_fetch_collation_request(&mut virtual_overseer, hash_b, test_state.chain_ids[0])
			.await;

		Delay::new(ACTIVITY_TIMEOUT * 2 / 3).await;

		advertise_collation(&mut virtual_overseer, peer_b.clone(), hash_c).await;

		assert_fetch_collation_request(&mut virtual_overseer, hash_c, test_state.chain_ids[0])
			.await;

		Delay::new(ACTIVITY_TIMEOUT * 3 / 2).await;

		assert_collator_disconnect(&mut virtual_overseer, peer_b.clone()).await;

		virtual_overseer
	});
}

#[test]
fn disconnect_if_no_declare() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![test_state.relay_parent],
			)),
		)
		.await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerConnected(
				peer_b.clone(),
				ObservedRole::Full,
				CollationVersion::V1.into(),
				None,
			)),
		)
		.await;

		assert_collator_disconnect(&mut virtual_overseer, peer_b.clone()).await;

		virtual_overseer
	})
}

#[test]
fn disconnect_if_wrong_declare() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		let pair = CollatorPair::generate().0;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![test_state.relay_parent],
			)),
		)
		.await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerConnected(
				peer_b.clone(),
				ObservedRole::Full,
				CollationVersion::V1.into(),
				None,
			)),
		)
		.await;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerMessage(
				peer_b.clone(),
				Versioned::V1(protocol_v1::CollatorProtocolMessage::Declare(
					pair.public(),
					ParaId::from(69),
					pair.sign(&protocol_v1::declare_signature_payload(&peer_b)),
				)),
			)),
		)
		.await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(
				peer,
				rep,
			)) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep, COST_UNNEEDED_COLLATOR);
			}
		);

		assert_collator_disconnect(&mut virtual_overseer, peer_b.clone()).await;

		virtual_overseer
	})
}

#[test]
fn view_change_clears_old_collators() {
	let mut test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer } = test_harness;

		let pair = CollatorPair::generate().0;

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![test_state.relay_parent],
			)),
		)
		.await;

		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		let peer_b = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_b.clone(),
			pair.clone(),
			test_state.chain_ids[0],
		)
		.await;

		let hash_b = Hash::repeat_byte(69);

		overseer_send(
			&mut virtual_overseer,
			CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![hash_b],
			)),
		)
		.await;

		test_state.group_rotation_info = test_state.group_rotation_info.bump_rotation();
		respond_to_core_info_queries(&mut virtual_overseer, &test_state).await;

		assert_collator_disconnect(&mut virtual_overseer, peer_b.clone()).await;

		virtual_overseer
	})
}
