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

use assert_matches::assert_matches;
use futures::{Future, channel::mpsc};

use parity_scale_codec::Encode;

use polkadot_subsystem::messages::DisputeCoordinatorMessage;
use smallvec::SmallVec;
use sp_keyring::{Sr25519Keyring};

use polkadot_node_network_protocol::{IfDisconnected, request_response::{Recipient, Requests, v1::DisputeResponse}};
use polkadot_primitives::v1::{AuthorityDiscoveryId, CandidateHash, Hash, SessionIndex, SessionInfo};
use polkadot_subsystem::{ActivatedLeaf, ActiveLeavesUpdate, FromOverseer, LeafStatus, OverseerSignal, Span, messages::{AllMessages, DisputeDistributionMessage, NetworkBridgeMessage, RuntimeApiMessage, RuntimeApiRequest}};
use polkadot_subsystem_testhelpers::{TestSubsystemContextHandle, mock::make_ferdie_keystore, subsystem_test_harness};

use crate::{DisputeDistributionSubsystem, LOG_TARGET, tests::mock::{MOCK_SESSION_INDEX, make_session_info}};

use self::mock::{ALICE_INDEX, FERDIE_INDEX, make_candidate_receipt, make_dispute_message, MockAuthorityDiscovery};

/// Useful mock providers.
pub mod mock;

#[test]
fn send_dispute_sends_dispute() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>|
		async move {

			let (req_tx, authority_discovery) = handle_subsystem_startup(&mut handle).await;

			let relay_parent = Hash::random();
			let candidate = make_candidate_receipt(relay_parent);
			let message =
				make_dispute_message(candidate, ALICE_INDEX, FERDIE_INDEX,).await;
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
					tx.send(Ok(Some(make_session_info()))).expect("Receiver should stay alive.");
					tracing::trace!(
						target: LOG_TARGET,
						"After tx.send"
					);
				}
			);

			let expected_receivers = {
				let info = make_session_info();
				info.discovery_keys
					.clone()
					.into_iter()
					.filter(|a| a != &Sr25519Keyring::Ferdie.public().into())
					.collect()
				// All validators are also authorities in the first session, so we are
				// done here.
			};
			check_sent_requests(&mut handle, expected_receivers, true).await;

			// println!("Received: {:?}", handle.recv().await);
			handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
	};
	test_harness(test);
}

/// Pass a `new_session` if you expect the subsystem to retrieve `SessionInfo` when given the
/// `session_index`.
async fn activate_leaf(
	handle: &mut TestSubsystemContextHandle<DisputeDistributionMessage>,
	activate: Hash,
	session_index: SessionIndex,
	// New session if we expect the subsystem to request it.
	new_session: Option<SessionInfo>,
	// Currently active disputes to send to the subsystem.
	active_disputes: Vec<(SessionIndex, CandidateHash)>,
) {
	handle.send(FromOverseer::Signal(
		OverseerSignal::ActiveLeaves(
			ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: activate,
				number: 10,
				status: LeafStatus::Fresh,
				span: Arc::new(Span::Disabled),
			}
	))))
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

	// panic!("Got: {:?}", handle.recv().await);
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
	// assert_matches!(
	match handle.recv().await {
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
		_ => {
			unreachable!("Unexpected message received.");
		}
	};
}

/// Initialize subsystem and return request sender needed for sending incoming requests to the
/// subsystem.
async fn handle_subsystem_startup(
	handle: &mut TestSubsystemContextHandle<DisputeDistributionMessage>
) -> (mpsc::Sender<sc_network::config::IncomingRequest>, MockAuthorityDiscovery) {
	let (request_tx, request_rx) = mpsc::channel(5);
	handle.send(
		FromOverseer::Communication {
			msg: DisputeDistributionMessage::DisputeSendingReceiver(request_rx),
		}
	).await;

	let authority_discovery = MockAuthorityDiscovery::new();

	assert_matches!(
		handle.recv().await,
		AllMessages::NetworkBridge(
			NetworkBridgeMessage::GetAuthorityDiscoveryService(tx)
		) => {
			tx
				.send(Box::new(authority_discovery.clone()))
				.expect("Receiver shoult still be alive.");
		}
	);
	activate_leaf(handle, Hash::random(), MOCK_SESSION_INDEX, Some(make_session_info()), Vec::new()).await;
	(request_tx, authority_discovery)
}


/// Launch subsystem and provided test function
///
/// which simulates the overseer.
fn test_harness<TestFn, Fut>(test: TestFn)
where
	TestFn: FnOnce(TestSubsystemContextHandle<DisputeDistributionMessage>) -> Fut,
	Fut: Future<Output = ()>
{
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

