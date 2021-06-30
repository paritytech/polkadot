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
//

//! Subsystem unit tests

use std::collections::HashSet;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use assert_matches::assert_matches;
use futures::future::poll_fn;
use futures::pin_mut;
use futures::{Future, channel::mpsc};

use futures_timer::Delay;
use parity_scale_codec::Encode;

use polkadot_node_primitives::{CandidateVotes, UncheckedDisputeMessage};
use polkadot_subsystem::messages::DisputeCoordinatorMessage;
use smallvec::SmallVec;
use sp_keyring::{Sr25519Keyring};

use polkadot_node_network_protocol::{IfDisconnected, request_response::{Recipient, Requests, v1::DisputeResponse}};
use polkadot_primitives::v1::{AuthorityDiscoveryId, CandidateHash, Hash, SessionIndex, SessionInfo};
use polkadot_subsystem::{ActivatedLeaf, ActiveLeavesUpdate, FromOverseer, LeafStatus, OverseerSignal, Span, messages::{AllMessages, DisputeDistributionMessage, NetworkBridgeMessage, RuntimeApiMessage, RuntimeApiRequest}};
use polkadot_subsystem_testhelpers::{TestSubsystemContextHandle, mock::make_ferdie_keystore, subsystem_test_harness};

use crate::{DisputeDistributionSubsystem, LOG_TARGET};
use self::mock::{
	ALICE_INDEX, FERDIE_INDEX, make_candidate_receipt, make_dispute_message,
	MOCK_AUTHORITY_DISCOVERY, MOCK_SESSION_INDEX, MOCK_SESSION_INFO, MOCK_NEXT_SESSION_INDEX,
	MOCK_NEXT_SESSION_INFO, FERDIE_DISCOVERY_KEY,
};

/// Useful mock providers.
pub mod mock;

#[test]
fn send_dispute_sends_dispute() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>|
		async move {

			let (_, _) = handle_subsystem_startup(&mut handle, None).await;

			let relay_parent = Hash::random();
			let candidate = make_candidate_receipt(relay_parent);
			let message =
				make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX,).await;
			handle.send(
				FromOverseer::Communication {
					msg: DisputeDistributionMessage::SendDispute(message.clone())
				}
			).await;
			// Requests needed session info:
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(
						hash,
						RuntimeApiRequest::SessionInfo(session_index, tx)
					)
				) => {
					assert_eq!(session_index, MOCK_SESSION_INDEX);
					assert_eq!(
						hash,
						message.candidate_receipt().descriptor.relay_parent
					);
					tx.send(Ok(Some(MOCK_SESSION_INFO.clone()))).expect("Receiver should stay alive.");
				}
			);

			let expected_receivers = {
				let info = &MOCK_SESSION_INFO;
				info.discovery_keys
					.clone()
					.into_iter()
					.filter(|a| a != &Sr25519Keyring::Ferdie.public().into())
					.collect()
				// All validators are also authorities in the first session, so we are
				// done here.
			};
			check_sent_requests(&mut handle, expected_receivers, true).await;

			conclude(&mut handle).await;
	};
	test_harness(test);
}

#[test]
fn disputes_are_recovered_at_startup() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>|
		async move {

			let relay_parent = Hash::random();
			let candidate = make_candidate_receipt(relay_parent);

			let (_, _) = handle_subsystem_startup(&mut handle, Some(candidate.hash())).await;

			let message =
				make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX,).await;
			// Requests needed session info:
			assert_matches!(
				handle.recv().await,
				AllMessages::DisputeCoordinator(
					DisputeCoordinatorMessage::QueryCandidateVotes(
						session_index,
						candidate_hash,
						tx,
					)
				) => {
					assert_eq!(session_index, MOCK_SESSION_INDEX);
					assert_eq!(candidate_hash, candidate.hash());
					let unchecked: UncheckedDisputeMessage = message.into();
					tx.send(Some(CandidateVotes {
						candidate_receipt: candidate,
						valid: vec![(
							unchecked.valid_vote.kind,
							unchecked.valid_vote.validator_index,
							unchecked.valid_vote.signature
						)],
						invalid: vec![(
							unchecked.invalid_vote.kind,
							unchecked.invalid_vote.validator_index,
							unchecked.invalid_vote.signature
						)],
					}))
					.expect("Receiver should stay alive.");
				}
			);

			let expected_receivers = {
				let info = &MOCK_SESSION_INFO;
				info.discovery_keys
					.clone()
					.into_iter()
					.filter(|a| a != &Sr25519Keyring::Ferdie.public().into())
					.collect()
				// All validators are also authorities in the first session, so we are
				// done here.
			};
			check_sent_requests(&mut handle, expected_receivers, true).await;

			conclude(&mut handle).await;
	};
	test_harness(test);
}

#[test]
fn send_dispute_gets_cleaned_up() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>|
		async move {

			let (old_head, _) = handle_subsystem_startup(&mut handle, None).await;

			let relay_parent = Hash::random();
			let candidate = make_candidate_receipt(relay_parent);
			let message =
				make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX,).await;
			handle.send(
				FromOverseer::Communication {
					msg: DisputeDistributionMessage::SendDispute(message.clone())
				}
			).await;
			// Requests needed session info:
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(
						hash,
						RuntimeApiRequest::SessionInfo(session_index, tx)
					)
				) => {
					assert_eq!(session_index, MOCK_SESSION_INDEX);
					assert_eq!(
						hash,
						message.candidate_receipt().descriptor.relay_parent
					);
					tx.send(Ok(Some(MOCK_SESSION_INFO.clone()))).expect("Receiver should stay alive.");
				}
			);

			let expected_receivers = {
				let info = &MOCK_SESSION_INFO;
				info.discovery_keys
					.clone()
					.into_iter()
					.filter(|a| a != &Sr25519Keyring::Ferdie.public().into())
					.collect()
				// All validators are also authorities in the first session, so we are
				// done here.
			};
			check_sent_requests(&mut handle, expected_receivers, false).await;

			// Give tasks a chance to finish:
			Delay::new(Duration::from_millis(20)).await;

			activate_leaf(
				&mut handle,
				Hash::random(),
				Some(old_head),
				MOCK_SESSION_INDEX,
				None,
				// No disputes any more:
				Vec::new(),
			).await;

			// Yield, so subsystem can make progess:
			Delay::new(Duration::from_millis(2)).await;

			conclude(&mut handle).await;
	};
	test_harness(test);
}

#[test]
fn dispute_retries_and_works_across_session_boundaries() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>|
		async move {

			let (old_head, req_tx) = handle_subsystem_startup(&mut handle, None).await;

			let relay_parent = Hash::random();
			let candidate = make_candidate_receipt(relay_parent);
			let message =
				make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX,).await;
			handle.send(
				FromOverseer::Communication {
					msg: DisputeDistributionMessage::SendDispute(message.clone())
				}
			).await;
			// Requests needed session info:
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(
					RuntimeApiMessage::Request(
						hash,
						RuntimeApiRequest::SessionInfo(session_index, tx)
					)
				) => {
					assert_eq!(session_index, MOCK_SESSION_INDEX);
					assert_eq!(
						hash,
						message.candidate_receipt().descriptor.relay_parent
					);
					tx.send(Ok(Some(MOCK_SESSION_INFO.clone()))).expect("Receiver should stay alive.");
				}
			);

			let expected_receivers: HashSet<_> = {
				let info = &MOCK_SESSION_INFO;
				info.discovery_keys
					.clone()
					.into_iter()
					.filter(|a| a != &Sr25519Keyring::Ferdie.public().into())
					.collect()
				// All validators are also authorities in the first session, so we are
				// done here.
			};
			// Requests don't get confirmed - dispute is carried over to next session.
			check_sent_requests(&mut handle, expected_receivers.clone(), false).await;

			// Give tasks a chance to finish:
			Delay::new(Duration::from_millis(20)).await;

			// Trigger retry:
			let old_head2 = Hash::random();
			activate_leaf(
				&mut handle,
				old_head2,
				Some(old_head),
				MOCK_SESSION_INDEX,
				None,
				vec![(MOCK_SESSION_INDEX, candidate.hash())]
			).await;

			check_sent_requests(&mut handle, expected_receivers.clone(), false).await;
			// Give tasks a chance to finish:
			Delay::new(Duration::from_millis(20)).await;

			// Session change:
			activate_leaf(
				&mut handle,
				Hash::random(),
				Some(old_head2),
				MOCK_NEXT_SESSION_INDEX,
				Some(MOCK_NEXT_SESSION_INFO.clone()),
				vec![(MOCK_SESSION_INDEX, candidate.hash())]
			).await;

			let expected_receivers = {
				let validator_count = MOCK_SESSION_INFO.validators.len();
				let old_validators = MOCK_SESSION_INFO
					.discovery_keys
					.clone()
					.into_iter()
					.take(validator_count)
					.filter(|a| *a != *FERDIE_DISCOVERY_KEY);

				MOCK_NEXT_SESSION_INFO
					.discovery_keys
					.clone()
					.into_iter()
					.filter(|a| *a != *FERDIE_DISCOVERY_KEY)
					.chain(old_validators)
					.collect()
			};
			check_sent_requests(&mut handle, expected_receivers, true).await;

			conclude(&mut handle).await;
	};
	test_harness(test);
}

async fn conclude(
	handle: &mut TestSubsystemContextHandle<DisputeDistributionMessage>,
) {
	// No more messages should be in the queue:
	poll_fn(|ctx| {
		let fut = handle.recv();
		pin_mut!(fut);
		// No requests should be inititated, as there is no longer any dispute active:
		assert_matches!(
			fut.poll(ctx),
			Poll::Pending,
			"No requests expected"
			);
		Poll::Ready(())
	}).await;

	handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
}

/// Pass a `new_session` if you expect the subsystem to retrieve `SessionInfo` when given the
/// `session_index`.
async fn activate_leaf(
	handle: &mut TestSubsystemContextHandle<DisputeDistributionMessage>,
	activate: Hash,
	deactivate: Option<Hash>,
	session_index: SessionIndex,
	// New session if we expect the subsystem to request it.
	new_session: Option<SessionInfo>,
	// Currently active disputes to send to the subsystem.
	active_disputes: Vec<(SessionIndex, CandidateHash)>,
) {
	let has_active_disputes = !active_disputes.is_empty();
	handle.send(FromOverseer::Signal(
		OverseerSignal::ActiveLeaves(
			ActiveLeavesUpdate {
				activated: [ActivatedLeaf {
						hash: activate,
						number: 10,
						status: LeafStatus::Fresh,
						span: Arc::new(Span::Disabled),
					}][..]
				   .into(),
				deactivated: deactivate.into_iter().collect(),
			}

	)))
	.await;
	assert_matches!(
		handle.recv().await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			h,
			RuntimeApiRequest::SessionIndexForChild(tx)
		)) => {
			assert_eq!(h, activate);
			tx.send(Ok(session_index)).expect("Receiver should stay alive.");
		}
	);
	assert_matches!(
		handle.recv().await,
		AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::ActiveDisputes(tx)) => {
			tx.send(active_disputes).expect("Receiver should stay alive.");
		}
	);

	let new_session = match (new_session, has_active_disputes) {
		(Some(new_session), true) => new_session,
		_ => return,
	};

	assert_matches!(
		handle.recv().await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				h,
				RuntimeApiRequest::SessionInfo(i, tx)
		)) => {
			assert_eq!(h, activate);
			assert_eq!(i, session_index);
			tx.send(Ok(Some(new_session))).expect("Receiver should stay alive.");
		}
	);
}

/// Check whether sent network bridge requests match the expectation.
async fn check_sent_requests(
	handle: &mut TestSubsystemContextHandle<DisputeDistributionMessage>,
	expected_receivers: HashSet<AuthorityDiscoveryId>,
	confirm_receive: bool,
) {
	let expected_receivers: HashSet<_> =
		expected_receivers
			.into_iter()
			.map(Recipient::Authority)
			.collect();

	// Sends to concerned validators:
	assert_matches!(
		handle.recv().await,
		AllMessages::NetworkBridge(
			NetworkBridgeMessage::SendRequests(reqs, IfDisconnected::TryConnect)
		) => {
			let reqs: Vec<_> = reqs.into_iter().map(|r|
				assert_matches!(
					r,
					Requests::DisputeSending(req) => {req}
				)
			)
			.collect();

			let receivers_raw: Vec<_> = reqs.iter().map(|r| r.peer.clone()).collect();
			let receivers: HashSet<_> = receivers_raw.clone().clone().into_iter().collect();
			assert_eq!(receivers_raw.len(), receivers.len(), "No duplicates are expected.");
			assert_eq!(receivers.len(), expected_receivers.len());
			assert_eq!(receivers, expected_receivers);
			if confirm_receive {
				for req in reqs {
					req.pending_response.send(
						Ok(DisputeResponse::Confirmed.encode())
					)
					.expect("Subsystem should be listening for a response.");
				}
			}
		}
	);
}

/// Initialize subsystem and return request sender needed for sending incoming requests to the
/// subsystem.
async fn handle_subsystem_startup(
	handle: &mut TestSubsystemContextHandle<DisputeDistributionMessage>,
	ongoing_dispute: Option<CandidateHash>,
) -> (Hash, mpsc::Sender<sc_network::config::IncomingRequest>) {
	let (request_tx, request_rx) = mpsc::channel(5);
	handle.send(
		FromOverseer::Communication {
			msg: DisputeDistributionMessage::DisputeSendingReceiver(request_rx),
		}
	).await;

	assert_matches!(
		handle.recv().await,
		AllMessages::NetworkBridge(
			NetworkBridgeMessage::GetAuthorityDiscoveryService(tx)
		) => {
			tx
				.send(Box::new(MOCK_AUTHORITY_DISCOVERY.clone()))
				.expect("Receiver should still be alive.");
		}
	);
	let relay_parent = Hash::random();
	activate_leaf(
		handle,
		relay_parent,
		None,
		MOCK_SESSION_INDEX,
		Some(MOCK_SESSION_INFO.clone()),
		ongoing_dispute.into_iter().map(|c| (MOCK_SESSION_INDEX, c)).collect()
	).await;
	(relay_parent, request_tx)
}


/// Launch subsystem and provided test function
///
/// which simulates the overseer.
fn test_harness<TestFn, Fut>(test: TestFn)
where
	TestFn: FnOnce(TestSubsystemContextHandle<DisputeDistributionMessage>) -> Fut,
	Fut: Future<Output = ()>
{
	// Uncomment this for debugging:
	sp_tracing::try_init_simple();
	let keystore = make_ferdie_keystore();

	let subsystem = DisputeDistributionSubsystem::new(keystore);

	let subsystem = |ctx| async {
		match subsystem.run(ctx).await {
			Ok(()) => {},
			Err(fatal) => {
				tracing::debug!(
					target: LOG_TARGET,
					?fatal,
					"Dispute distribution exited with fatal error."
				);
			}
		}
	};
	subsystem_test_harness(test, subsystem);
}

