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

use std::{
	collections::HashSet,
	sync::Arc,
	task::Poll,
	time::{Duration, Instant},
};

use assert_matches::assert_matches;
use futures::{
	channel::{mpsc, oneshot},
	future::{poll_fn, ready},
	pin_mut, Future, SinkExt,
};
use futures_timer::Delay;
use parity_scale_codec::{Decode, Encode};

use sc_network::config::RequestResponseConfig;

use polkadot_node_network_protocol::{
	request_response::{v1::DisputeRequest, IncomingRequest, ReqProtocolNames},
	PeerId,
};
use sp_keyring::Sr25519Keyring;

use polkadot_node_network_protocol::{
	request_response::{v1::DisputeResponse, Recipient, Requests},
	IfDisconnected,
};
use polkadot_node_primitives::{CandidateVotes, DisputeStatus, UncheckedDisputeMessage};
use polkadot_node_subsystem::{
	messages::{
		AllMessages, DisputeCoordinatorMessage, DisputeDistributionMessage, ImportStatementsResult,
		NetworkBridgeTxMessage, RuntimeApiMessage, RuntimeApiRequest,
	},
	ActivatedLeaf, ActiveLeavesUpdate, FromOrchestra, LeafStatus, OverseerSignal, Span,
};
use polkadot_node_subsystem_test_helpers::{
	mock::make_ferdie_keystore, subsystem_test_harness, TestSubsystemContextHandle,
};
use polkadot_primitives::v2::{
	AuthorityDiscoveryId, CandidateHash, CandidateReceipt, Hash, SessionIndex, SessionInfo,
};

use self::mock::{
	make_candidate_receipt, make_dispute_message, ALICE_INDEX, FERDIE_DISCOVERY_KEY, FERDIE_INDEX,
	MOCK_AUTHORITY_DISCOVERY, MOCK_NEXT_SESSION_INDEX, MOCK_NEXT_SESSION_INFO, MOCK_SESSION_INDEX,
	MOCK_SESSION_INFO,
};
use crate::{
	receiver::BATCH_COLLECTING_INTERVAL,
	tests::mock::{BOB_INDEX, CHARLIE_INDEX},
	DisputeDistributionSubsystem, Metrics, LOG_TARGET, SEND_RATE_LIMIT,
};

/// Useful mock providers.
pub mod mock;

#[test]
fn send_dispute_sends_dispute() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>, _req_cfg| async move {
		let _ = handle_subsystem_startup(&mut handle, None).await;

		let relay_parent = Hash::random();
		let candidate = make_candidate_receipt(relay_parent);
		send_dispute(&mut handle, candidate, true).await;
		conclude(&mut handle).await;
	};
	test_harness(test);
}

#[test]
fn send_honors_rate_limit() {
	sp_tracing::try_init_simple();
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>, _req_cfg| async move {
		let _ = handle_subsystem_startup(&mut handle, None).await;

		let relay_parent = Hash::random();
		let candidate = make_candidate_receipt(relay_parent);
		let before_request = Instant::now();
		send_dispute(&mut handle, candidate, true).await;
		// First send should not be rate limited:
		gum::trace!("Passed time: {:#?}", Instant::now().saturating_duration_since(before_request));
		// This test would likely be flaky on CI:
		//assert!(Instant::now().saturating_duration_since(before_request) < SEND_RATE_LIMIT);

		let relay_parent = Hash::random();
		let candidate = make_candidate_receipt(relay_parent);
		send_dispute(&mut handle, candidate, false).await;
		// Second send should be rate limited:
		gum::trace!(
			"Passed time for send_dispute: {:#?}",
			Instant::now().saturating_duration_since(before_request)
		);
		assert!(Instant::now() - before_request >= SEND_RATE_LIMIT);
		conclude(&mut handle).await;
	};
	test_harness(test);
}

/// Helper for sending a new dispute to dispute-distribution sender and handling resulting messages.
async fn send_dispute(
	handle: &mut TestSubsystemContextHandle<DisputeDistributionMessage>,
	candidate: CandidateReceipt,
	needs_session_info: bool,
) {
	let before_request = Instant::now();
	let message = make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX).await;
	gum::trace!(
		"Passed time for making message: {:#?}",
		Instant::now().saturating_duration_since(before_request)
	);
	let before_request = Instant::now();
	handle
		.send(FromOrchestra::Communication {
			msg: DisputeDistributionMessage::SendDispute(message.clone()),
		})
		.await;
	gum::trace!(
		"Passed time for sending message: {:#?}",
		Instant::now().saturating_duration_since(before_request)
	);
	if needs_session_info {
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
	}

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
	check_sent_requests(handle, expected_receivers, true).await;
}

// Things to test:
// x Request triggers import
// x Subsequent imports get batched
// x Batch gets flushed.
// x Batch gets renewed.
// x Non authority requests get dropped.
// x Sending rate limit is honored.
// x Receiving rate limit is honored.
// x Duplicate requests on batch are dropped

#[test]
fn received_non_authorities_are_dropped() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>,
	            mut req_cfg: RequestResponseConfig| async move {
		let req_tx = req_cfg.inbound_queue.as_mut().unwrap();
		let _ = handle_subsystem_startup(&mut handle, None).await;

		let relay_parent = Hash::random();
		let candidate = make_candidate_receipt(relay_parent);
		let message = make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX).await;

		// Non validator request should get dropped:
		let rx_response =
			send_network_dispute_request(req_tx, PeerId::random(), message.clone().into()).await;

		assert_matches!(
			rx_response.await,
			Ok(resp) => {
				let sc_network::config::OutgoingResponse {
					result: _,
					reputation_changes,
					sent_feedback: _,
				} = resp;
				// Peer should get punished:
				assert_eq!(reputation_changes.len(), 1);
			}
		);
		conclude(&mut handle).await;
	};
	test_harness(test);
}

#[test]
fn received_request_triggers_import() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>,
	            mut req_cfg: RequestResponseConfig| async move {
		let req_tx = req_cfg.inbound_queue.as_mut().unwrap();
		let _ = handle_subsystem_startup(&mut handle, None).await;

		let relay_parent = Hash::random();
		let candidate = make_candidate_receipt(relay_parent);
		let message = make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX).await;

		nested_network_dispute_request(
			&mut handle,
			req_tx,
			MOCK_AUTHORITY_DISCOVERY.get_peer_id_by_authority(Sr25519Keyring::Alice),
			message.clone().into(),
			ImportStatementsResult::ValidImport,
			true,
			move |_handle, _req_tx, _message| ready(()),
		)
		.await;

		gum::trace!(target: LOG_TARGET, "Concluding.");
		conclude(&mut handle).await;
	};
	test_harness(test);
}

#[test]
fn batching_works() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>,
	            mut req_cfg: RequestResponseConfig| async move {
		let req_tx = req_cfg.inbound_queue.as_mut().unwrap();
		let _ = handle_subsystem_startup(&mut handle, None).await;

		let relay_parent = Hash::random();
		let candidate = make_candidate_receipt(relay_parent);
		let message = make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX).await;

		// Initial request should get forwarded immediately:
		nested_network_dispute_request(
			&mut handle,
			req_tx,
			MOCK_AUTHORITY_DISCOVERY.get_peer_id_by_authority(Sr25519Keyring::Alice),
			message.clone().into(),
			ImportStatementsResult::ValidImport,
			true,
			move |_handle, _req_tx, _message| ready(()),
		)
		.await;

		let mut rx_responses = Vec::new();

		let message = make_dispute_message(candidate.clone(), BOB_INDEX, FERDIE_INDEX).await;
		let peer = MOCK_AUTHORITY_DISCOVERY.get_peer_id_by_authority(Sr25519Keyring::Bob);
		rx_responses.push(send_network_dispute_request(req_tx, peer, message.clone().into()).await);

		let message = make_dispute_message(candidate.clone(), CHARLIE_INDEX, FERDIE_INDEX).await;
		let peer = MOCK_AUTHORITY_DISCOVERY.get_peer_id_by_authority(Sr25519Keyring::Charlie);
		rx_responses.push(send_network_dispute_request(req_tx, peer, message.clone().into()).await);
		gum::trace!("Imported 3 votes into batch");

		Delay::new(BATCH_COLLECTING_INTERVAL).await;
		gum::trace!("Batch should still be alive");
		// Batch should still be alive (2 new votes):
		// Let's import two more votes, but fully duplicates - should not extend batch live.
		gum::trace!("Importing duplicate votes");
		let mut rx_responses_duplicate = Vec::new();
		let message = make_dispute_message(candidate.clone(), BOB_INDEX, FERDIE_INDEX).await;
		let peer = MOCK_AUTHORITY_DISCOVERY.get_peer_id_by_authority(Sr25519Keyring::Bob);
		rx_responses_duplicate
			.push(send_network_dispute_request(req_tx, peer, message.clone().into()).await);

		let message = make_dispute_message(candidate.clone(), CHARLIE_INDEX, FERDIE_INDEX).await;
		let peer = MOCK_AUTHORITY_DISCOVERY.get_peer_id_by_authority(Sr25519Keyring::Charlie);
		rx_responses_duplicate
			.push(send_network_dispute_request(req_tx, peer, message.clone().into()).await);

		for rx_response in rx_responses_duplicate {
			assert_matches!(
				rx_response.await,
				Ok(resp) => {
					let sc_network::config::OutgoingResponse {
						result,
						reputation_changes,
						sent_feedback: _,
					} = resp;
					gum::trace!(
						target: LOG_TARGET,
						?reputation_changes,
						"Received reputation changes."
					);
					// We don't punish on that.
					assert_eq!(reputation_changes.len(), 0);

					assert_matches!(result, Err(()));
				}
			);
		}

		Delay::new(BATCH_COLLECTING_INTERVAL).await;
		gum::trace!("Batch should be ready now (only duplicates have been added)");

		let pending_confirmation = assert_matches!(
			handle.recv().await,
			AllMessages::DisputeCoordinator(
				DisputeCoordinatorMessage::ImportStatements {
					candidate_receipt: _,
					session,
					statements,
					pending_confirmation: Some(pending_confirmation),
				}
			) => {
				assert_eq!(session, MOCK_SESSION_INDEX);
				assert_eq!(statements.len(), 3);
				pending_confirmation
			}
		);
		pending_confirmation.send(ImportStatementsResult::ValidImport).unwrap();

		for rx_response in rx_responses {
			assert_matches!(
				rx_response.await,
				Ok(resp) => {
					let sc_network::config::OutgoingResponse {
						result,
						reputation_changes: _,
						sent_feedback,
					} = resp;

					let result = result.unwrap();
					let decoded =
						<DisputeResponse as Decode>::decode(&mut result.as_slice()).unwrap();

					assert!(decoded == DisputeResponse::Confirmed);
					if let Some(sent_feedback) = sent_feedback {
						sent_feedback.send(()).unwrap();
					}
					gum::trace!(
						target: LOG_TARGET,
						"Valid import happened."
						);

				}
			);
		}

		gum::trace!(target: LOG_TARGET, "Concluding.");
		conclude(&mut handle).await;
	};
	test_harness(test);
}

#[test]
fn receive_rate_limit_is_enforced() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>,
	            mut req_cfg: RequestResponseConfig| async move {
		let req_tx = req_cfg.inbound_queue.as_mut().unwrap();
		let _ = handle_subsystem_startup(&mut handle, None).await;

		let relay_parent = Hash::random();
		let candidate = make_candidate_receipt(relay_parent);
		let message = make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX).await;

		// Initial request should get forwarded immediately:
		nested_network_dispute_request(
			&mut handle,
			req_tx,
			MOCK_AUTHORITY_DISCOVERY.get_peer_id_by_authority(Sr25519Keyring::Alice),
			message.clone().into(),
			ImportStatementsResult::ValidImport,
			true,
			move |_handle, _req_tx, _message| ready(()),
		)
		.await;

		let mut rx_responses = Vec::new();

		let peer = MOCK_AUTHORITY_DISCOVERY.get_peer_id_by_authority(Sr25519Keyring::Bob);

		let message = make_dispute_message(candidate.clone(), BOB_INDEX, FERDIE_INDEX).await;
		rx_responses.push(send_network_dispute_request(req_tx, peer, message.clone().into()).await);

		let message = make_dispute_message(candidate.clone(), CHARLIE_INDEX, FERDIE_INDEX).await;
		rx_responses.push(send_network_dispute_request(req_tx, peer, message.clone().into()).await);

		gum::trace!("Import one too much:");

		let message = make_dispute_message(candidate.clone(), CHARLIE_INDEX, ALICE_INDEX).await;
		let rx_response_flood =
			send_network_dispute_request(req_tx, peer, message.clone().into()).await;

		assert_matches!(
			rx_response_flood.await,
			Ok(resp) => {
				let sc_network::config::OutgoingResponse {
					result: _,
					reputation_changes,
					sent_feedback: _,
				} = resp;
				gum::trace!(
					target: LOG_TARGET,
					?reputation_changes,
					"Received reputation changes."
				);
				// Received punishment for flood:
				assert_eq!(reputation_changes.len(), 1);
			}
		);
		gum::trace!("Need to wait 2 patch intervals:");
		Delay::new(BATCH_COLLECTING_INTERVAL).await;
		Delay::new(BATCH_COLLECTING_INTERVAL).await;

		gum::trace!("Batch should be ready now");

		let pending_confirmation = assert_matches!(
			handle.recv().await,
			AllMessages::DisputeCoordinator(
				DisputeCoordinatorMessage::ImportStatements {
					candidate_receipt: _,
					session,
					statements,
					pending_confirmation: Some(pending_confirmation),
				}
			) => {
				assert_eq!(session, MOCK_SESSION_INDEX);
				// Only 3 as fourth was flood:
				assert_eq!(statements.len(), 3);
				pending_confirmation
			}
		);
		pending_confirmation.send(ImportStatementsResult::ValidImport).unwrap();

		for rx_response in rx_responses {
			assert_matches!(
				rx_response.await,
				Ok(resp) => {
					let sc_network::config::OutgoingResponse {
						result,
						reputation_changes: _,
						sent_feedback,
					} = resp;

					let result = result.unwrap();
					let decoded =
						<DisputeResponse as Decode>::decode(&mut result.as_slice()).unwrap();

					assert!(decoded == DisputeResponse::Confirmed);
					if let Some(sent_feedback) = sent_feedback {
						sent_feedback.send(()).unwrap();
					}
					gum::trace!(
						target: LOG_TARGET,
						"Valid import happened."
						);

				}
			);
		}

		gum::trace!(target: LOG_TARGET, "Concluding.");
		conclude(&mut handle).await;
	};
	test_harness(test);
}

#[test]
fn disputes_are_recovered_at_startup() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>, _| async move {
		let relay_parent = Hash::random();
		let candidate = make_candidate_receipt(relay_parent);

		let _ = handle_subsystem_startup(&mut handle, Some(candidate.hash())).await;

		let message = make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX).await;
		// Requests needed session info:
		assert_matches!(
			handle.recv().await,
			AllMessages::DisputeCoordinator(
				DisputeCoordinatorMessage::QueryCandidateVotes(
					query,
					tx,
				)
			) => {
				let (session_index, candidate_hash) = query.get(0).unwrap().clone();
				assert_eq!(session_index, MOCK_SESSION_INDEX);
				assert_eq!(candidate_hash, candidate.hash());
				let unchecked: UncheckedDisputeMessage = message.into();
				tx.send(vec![(session_index, candidate_hash, CandidateVotes {
					candidate_receipt: candidate,
					valid: [(
						unchecked.valid_vote.validator_index,
						(unchecked.valid_vote.kind,
						unchecked.valid_vote.signature
						),
					)].into_iter().collect(),
					invalid: [(
						unchecked.invalid_vote.validator_index,
						(
						unchecked.invalid_vote.kind,
						unchecked.invalid_vote.signature
						),
					)].into_iter().collect(),
				})])
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
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>, _| async move {
		let old_head = handle_subsystem_startup(&mut handle, None).await;

		let relay_parent = Hash::random();
		let candidate = make_candidate_receipt(relay_parent);
		let message = make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX).await;
		handle
			.send(FromOrchestra::Communication {
				msg: DisputeDistributionMessage::SendDispute(message.clone()),
			})
			.await;
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
		)
		.await;

		// Yield, so subsystem can make progress:
		Delay::new(Duration::from_millis(2)).await;

		conclude(&mut handle).await;
	};
	test_harness(test);
}

#[test]
fn dispute_retries_and_works_across_session_boundaries() {
	let test = |mut handle: TestSubsystemContextHandle<DisputeDistributionMessage>, _| async move {
		let old_head = handle_subsystem_startup(&mut handle, None).await;

		let relay_parent = Hash::random();
		let candidate = make_candidate_receipt(relay_parent);
		let message = make_dispute_message(candidate.clone(), ALICE_INDEX, FERDIE_INDEX).await;
		handle
			.send(FromOrchestra::Communication {
				msg: DisputeDistributionMessage::SendDispute(message.clone()),
			})
			.await;
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
			vec![(MOCK_SESSION_INDEX, candidate.hash(), DisputeStatus::Active)],
		)
		.await;

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
			vec![(MOCK_SESSION_INDEX, candidate.hash(), DisputeStatus::Active)],
		)
		.await;

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

async fn send_network_dispute_request(
	req_tx: &mut mpsc::Sender<sc_network::config::IncomingRequest>,
	peer: PeerId,
	message: DisputeRequest,
) -> oneshot::Receiver<sc_network::config::OutgoingResponse> {
	let (pending_response, rx_response) = oneshot::channel();
	let req =
		sc_network::config::IncomingRequest { peer, payload: message.encode(), pending_response };
	req_tx.feed(req).await.unwrap();
	rx_response
}

/// Send request and handle its reactions.
///
/// Passed in function will be called while votes are still being imported.
async fn nested_network_dispute_request<'a, F, O>(
	handle: &'a mut TestSubsystemContextHandle<DisputeDistributionMessage>,
	req_tx: &'a mut mpsc::Sender<sc_network::config::IncomingRequest>,
	peer: PeerId,
	message: DisputeRequest,
	import_result: ImportStatementsResult,
	need_session_info: bool,
	inner: F,
) where
	F: FnOnce(
			&'a mut TestSubsystemContextHandle<DisputeDistributionMessage>,
			&'a mut mpsc::Sender<sc_network::config::IncomingRequest>,
			DisputeRequest,
		) -> O
		+ 'a,
	O: Future<Output = ()> + 'a,
{
	let rx_response = send_network_dispute_request(req_tx, peer, message.clone().into()).await;

	if need_session_info {
		// Subsystem might need `SessionInfo` for determining indices:
		match handle.recv().await {
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_,
				RuntimeApiRequest::SessionInfo(_, tx),
			)) => {
				tx.send(Ok(Some(MOCK_SESSION_INFO.clone())))
					.expect("Receiver should stay alive.");
			},
			unexpected => panic!("Unexpected message {:?}", unexpected),
		}
	}

	// Import should get initiated:
	let pending_confirmation = assert_matches!(
		handle.recv().await,
		AllMessages::DisputeCoordinator(
			DisputeCoordinatorMessage::ImportStatements {
				candidate_receipt,
				session,
				statements,
				pending_confirmation: Some(pending_confirmation),
			}
		) => {
			let candidate_hash = candidate_receipt.hash();
			assert_eq!(session, MOCK_SESSION_INDEX);
			assert_eq!(candidate_hash, message.0.candidate_receipt.hash());
			assert_eq!(statements.len(), 2);
			pending_confirmation
		}
	);

	// Do the inner thing:
	inner(handle, req_tx, message).await;

	// Confirm import
	pending_confirmation.send(import_result).unwrap();

	assert_matches!(
		rx_response.await,
		Ok(resp) => {
			let sc_network::config::OutgoingResponse {
				result,
				reputation_changes,
				sent_feedback,
			} = resp;

			match import_result {
				ImportStatementsResult::ValidImport => {
					let result = result.unwrap();
					let decoded =
						<DisputeResponse as Decode>::decode(&mut result.as_slice()).unwrap();

					assert!(decoded == DisputeResponse::Confirmed);
					if let Some(sent_feedback) = sent_feedback {
						sent_feedback.send(()).unwrap();
					}
					gum::trace!(
						target: LOG_TARGET,
						"Valid import happened."
					);

				}
				ImportStatementsResult::InvalidImport => {
					// Peer should get punished:
					assert_eq!(reputation_changes.len(), 1);
				}
			}
		}
	);
}

async fn conclude(handle: &mut TestSubsystemContextHandle<DisputeDistributionMessage>) {
	// No more messages should be in the queue:
	poll_fn(|ctx| {
		let fut = handle.recv();
		pin_mut!(fut);
		// No requests should be initiated, as there is no longer any dispute active:
		assert_matches!(fut.poll(ctx), Poll::Pending, "No requests expected");
		Poll::Ready(())
	})
	.await;

	handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
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
	active_disputes: Vec<(SessionIndex, CandidateHash, DisputeStatus)>,
) {
	let has_active_disputes = !active_disputes.is_empty();
	handle
		.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
			activated: Some(ActivatedLeaf {
				hash: activate,
				number: 10,
				status: LeafStatus::Fresh,
				span: Arc::new(Span::Disabled),
			}),
			deactivated: deactivate.into_iter().collect(),
		})))
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
		expected_receivers.into_iter().map(Recipient::Authority).collect();

	// Sends to concerned validators:
	assert_matches!(
		handle.recv().await,
		AllMessages::NetworkBridgeTx(
			NetworkBridgeTxMessage::SendRequests(reqs, IfDisconnected::ImmediateError)
		) => {
			let reqs: Vec<_> = reqs.into_iter().map(|r|
				assert_matches!(
					r,
					Requests::DisputeSendingV1(req) => {req}
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
) -> Hash {
	let relay_parent = Hash::random();
	activate_leaf(
		handle,
		relay_parent,
		None,
		MOCK_SESSION_INDEX,
		Some(MOCK_SESSION_INFO.clone()),
		ongoing_dispute
			.into_iter()
			.map(|c| (MOCK_SESSION_INDEX, c, DisputeStatus::Active))
			.collect(),
	)
	.await;
	relay_parent
}

/// Launch subsystem and provided test function
///
/// which simulates the overseer.
fn test_harness<TestFn, Fut>(test: TestFn)
where
	TestFn: FnOnce(
		TestSubsystemContextHandle<DisputeDistributionMessage>,
		RequestResponseConfig,
	) -> Fut,
	Fut: Future<Output = ()>,
{
	sp_tracing::try_init_simple();
	let keystore = make_ferdie_keystore();

	let genesis_hash = Hash::repeat_byte(0xff);
	let req_protocol_names = ReqProtocolNames::new(&genesis_hash, None);
	let (req_receiver, req_cfg) = IncomingRequest::get_config_receiver(&req_protocol_names);
	let subsystem = DisputeDistributionSubsystem::new(
		keystore,
		req_receiver,
		MOCK_AUTHORITY_DISCOVERY.clone(),
		Metrics::new_dummy(),
	);

	let subsystem = |ctx| async {
		match subsystem.run(ctx).await {
			Ok(()) => {},
			Err(fatal) => {
				gum::debug!(
					target: LOG_TARGET,
					?fatal,
					"Dispute distribution exited with fatal error."
				);
			},
		}
	};
	subsystem_test_harness(|handle| test(handle, req_cfg), subsystem);
}
