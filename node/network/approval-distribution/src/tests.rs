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

use std::time::Duration;
use futures::{future, Future, executor};
use assert_matches::assert_matches;
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt as _;
use polkadot_node_network_protocol::{view, ObservedRole};
use polkadot_node_primitives::approval::{
	AssignmentCertKind, RELAY_VRF_MODULO_CONTEXT, VRFOutput, VRFProof,
};
use super::*;

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<ApprovalDistributionMessage>;

fn test_harness<T: Future<Output = ()>>(
	mut state: State,
	test_fn: impl FnOnce(VirtualOverseer) -> T,
) -> State {
	let _ = env_logger::builder()
		.is_test(true)
		.filter(
			Some(LOG_TARGET),
			log::LevelFilter::Trace,
		)
		.try_init();

	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = ApprovalDistribution::new(Default::default());
	{
		let subsystem = subsystem.run_inner(context, &mut state);

		let test_fut = test_fn(virtual_overseer);

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}

	state
}

const TIMEOUT: Duration = Duration::from_millis(100);

async fn overseer_send(
	overseer: &mut VirtualOverseer,
	msg: ApprovalDistributionMessage,
) {
	tracing::trace!(msg = ?msg, "Sending message");
	overseer
		.send(FromOverseer::Communication { msg })
		.timeout(TIMEOUT)
		.await
		.expect("msg send timeout");
}

async fn overseer_signal_block_finalized(
	overseer: &mut VirtualOverseer,
	number: BlockNumber,
) {
	tracing::trace!(
		?number,
		"Sending a finalized signal",
	);
	// we don't care about the block hash
	overseer
		.send(FromOverseer::Signal(OverseerSignal::BlockFinalized(Hash::zero(), number)))
		.timeout(TIMEOUT)
		.await
		.expect("signal send timeout");
}

async fn overseer_recv(
	overseer: &mut VirtualOverseer,
) -> AllMessages {
	tracing::trace!("Waiting for a message");
	let msg = overseer
		.recv()
		.timeout(TIMEOUT)
		.await
		.expect("msg recv timeout");

	tracing::trace!(msg = ?msg, "Received message");

	msg
}

async fn setup_peer_with_view(
	virtual_overseer: &mut VirtualOverseer,
	peer_id: &PeerId,
	view: View,
) {
	overseer_send(
		virtual_overseer,
		ApprovalDistributionMessage::NetworkBridgeUpdateV1(
			NetworkBridgeEvent::PeerConnected(peer_id.clone(), ObservedRole::Full)
		)
	).await;
	overseer_send(
		virtual_overseer,
		ApprovalDistributionMessage::NetworkBridgeUpdateV1(
			NetworkBridgeEvent::PeerViewChange(peer_id.clone(), view)
		)
	).await;
}

async fn send_message_from_peer(
	virtual_overseer: &mut VirtualOverseer,
	peer_id: &PeerId,
	msg: protocol_v1::ApprovalDistributionMessage,
) {
	overseer_send(
		virtual_overseer,
		ApprovalDistributionMessage::NetworkBridgeUpdateV1(
			NetworkBridgeEvent::PeerMessage(peer_id.clone(), msg)
		)
	).await;
}

fn fake_assignment_cert(
	block_hash: Hash,
	validator: ValidatorIndex,
) -> IndirectAssignmentCert {
	let ctx = schnorrkel::signing_context(RELAY_VRF_MODULO_CONTEXT);
	let msg = b"WhenParachains?";
	let mut prng = rand_core::OsRng;
	let keypair = schnorrkel::Keypair::generate_with(&mut prng);
	let (inout, proof, _) = keypair.vrf_sign(ctx.bytes(msg));
	let out = inout.to_output();

	IndirectAssignmentCert {
		block_hash,
		validator,
		cert: AssignmentCert {
			kind: AssignmentCertKind::RelayVRFModulo {
				sample: 1,
			},
			vrf: (VRFOutput(out), VRFProof(proof)),
		}
	}
}

async fn expect_reputation_change(
	virtual_overseer: &mut VirtualOverseer,
	peer_id: &PeerId,
	expected_reputation_change: Rep,
) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::NetworkBridge(
			NetworkBridgeMessage::ReportPeer(
				rep_peer,
				rep,
			)
		) => {
			assert_eq!(peer_id, &rep_peer);
			assert_eq!(expected_reputation_change, rep);
		}
	);
}


/// import an assignment
/// connect a new peer
/// the new peer sends us the same assignment
#[test]
fn try_import_the_same_assignment() {
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();
	let peer_d = PeerId::random();
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		// setup peers
		setup_peer_with_view(overseer, &peer_a, view![]).await;
		setup_peer_with_view(overseer, &peer_b, view![hash]).await;
		setup_peer_with_view(overseer, &peer_c, view![hash]).await;

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 2,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
		};
		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		// send the assignment related to `hash`
		let validator_index = ValidatorIndex(0);
		let cert = fake_assignment_cert(hash, validator_index);
		let assignments = vec![(cert.clone(), 0u32)];

		let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments.clone());
		send_message_from_peer(overseer, &peer_a, msg).await;

		expect_reputation_change(overseer, &peer_a, COST_UNEXPECTED_MESSAGE).await;

		// send an `Accept` message from the Approval Voting subsystem
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportAssignment(
				assignment,
				0u32,
				tx,
			)) => {
				assert_eq!(assignment, cert);
				tx.send(AssignmentCheckResult::Accepted).unwrap();
			}
		);

		expect_reputation_change(overseer, &peer_a, BENEFIT_VALID_MESSAGE_FIRST).await;

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
				peers,
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments)
				)
			)) => {
				assert_eq!(peers.len(), 2);
				assert_eq!(assignments.len(), 1);
			}
		);

		// setup new peer
		setup_peer_with_view(overseer, &peer_d, view![]).await;

		// send the same assignment from peer_d
		let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments);
		send_message_from_peer(overseer, &peer_d, msg).await;

		expect_reputation_change(overseer, &peer_d, COST_UNEXPECTED_MESSAGE).await;
		expect_reputation_change(overseer, &peer_d, BENEFIT_VALID_MESSAGE).await;

		assert!(overseer
			.recv()
			.timeout(TIMEOUT)
			.await
			.is_none(),
			"no message should be sent",
		);
	});
}

/// https://github.com/paritytech/polkadot/pull/2160#discussion_r547594835
///
/// 1. Send a view update that removes block B from their view.
/// 2. Send a message from B that they incur COST_UNEXPECTED_MESSAGE for,
///    but then they receive BENEFIT_VALID_MESSAGE.
/// 3. Send all other messages related to B.
#[test]
fn spam_attack_results_in_negative_reputation_change() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let peer_a = PeerId::random();
	let hash_b = Hash::repeat_byte(0xBB);

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		let peer = &peer_a;
		setup_peer_with_view(overseer, peer, view![]).await;

		// new block `hash_b` with 20 candidates
		let candidates_count = 20;
		let meta = BlockApprovalMeta {
			hash: hash_b.clone(),
			parent_hash,
			number: 2,
			candidates: vec![Default::default(); candidates_count],
			slot: 1.into(),
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		// send 20 assignments related to `hash_b`
		// to populate our knowledge
		let assignments: Vec<_> = (0..candidates_count)
			.map(|candidate_index| {
				let validator_index = ValidatorIndex(candidate_index as u32);
				let cert = fake_assignment_cert(hash_b, validator_index);
				(cert, candidate_index as u32)
			}).collect();

		let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments.clone());
		send_message_from_peer(overseer, peer, msg.clone()).await;

		for i in 0..candidates_count {
			expect_reputation_change(overseer, peer, COST_UNEXPECTED_MESSAGE).await;

			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportAssignment(
					assignment,
					claimed_candidate_index,
					tx,
				)) => {
					assert_eq!(assignment, assignments[i].0);
					assert_eq!(claimed_candidate_index, assignments[i].1);
					tx.send(AssignmentCheckResult::Accepted).unwrap();
				}
			);

			expect_reputation_change(overseer, peer, BENEFIT_VALID_MESSAGE_FIRST).await;
		}

		// send a view update that removes block B from peer's view by bumping the finalized_number
		overseer_send(
			overseer,
			ApprovalDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), View::with_finalized(2))
			)
		).await;

		// send the assignments again
		send_message_from_peer(overseer, peer, msg.clone()).await;

		// each of them will incur `COST_UNEXPECTED_MESSAGE`, not only the first one
		for _ in 0..candidates_count {
			expect_reputation_change(overseer, peer, COST_UNEXPECTED_MESSAGE).await;
			expect_reputation_change(overseer, peer, BENEFIT_VALID_MESSAGE).await;
		}
	});
}

#[test]
fn import_approval_happy_path() {
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		// setup peers
		setup_peer_with_view(overseer, &peer_a, view![]).await;
		setup_peer_with_view(overseer, &peer_b, view![hash]).await;
		setup_peer_with_view(overseer, &peer_c, view![hash]).await;

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
		};
		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		// import an assignment related to `hash` locally
		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;
		let cert = fake_assignment_cert(hash, validator_index);
		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert, candidate_index)
		).await;

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
				peers,
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments)
				)
			)) => {
				assert_eq!(peers.len(), 2);
				assert_eq!(assignments.len(), 1);
			}
		);

		// send the an approval from peer_b
		let approval = IndirectSignedApprovalVote {
			block_hash: hash,
			candidate_index,
			validator: validator_index,
			signature: Default::default(),
		};
		let msg = protocol_v1::ApprovalDistributionMessage::Approvals(vec![approval.clone()]);
		send_message_from_peer(overseer, &peer_b, msg).await;

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportApproval(
				vote,
				tx,
			)) => {
				assert_eq!(vote, approval);
				tx.send(ApprovalCheckResult::Accepted).unwrap();
			}
		);

		expect_reputation_change(overseer, &peer_b, BENEFIT_VALID_MESSAGE_FIRST).await;

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
				peers,
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(approvals)
				)
			)) => {
				assert_eq!(peers.len(), 1);
				assert_eq!(approvals.len(), 1);
			}
		);
	});
}

#[test]
fn import_approval_bad() {
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		// setup peers
		setup_peer_with_view(overseer, &peer_a, view![]).await;
		setup_peer_with_view(overseer, &peer_b, view![hash]).await;

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
		};
		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;
		let cert = fake_assignment_cert(hash, validator_index);

		// send the an approval from peer_b, we don't have an assignment yet
		let approval = IndirectSignedApprovalVote {
			block_hash: hash,
			candidate_index,
			validator: validator_index,
			signature: Default::default(),
		};
		let msg = protocol_v1::ApprovalDistributionMessage::Approvals(vec![approval.clone()]);
		send_message_from_peer(overseer, &peer_b, msg).await;

		expect_reputation_change(overseer, &peer_b, COST_UNEXPECTED_MESSAGE).await;

		// now import an assignment from peer_b
		let assignments = vec![(cert.clone(), candidate_index)];
		let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments);
		send_message_from_peer(overseer, &peer_b, msg).await;

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportAssignment(
				assignment,
				i,
				tx,
			)) => {
				assert_eq!(assignment, cert);
				assert_eq!(i, candidate_index);
				tx.send(AssignmentCheckResult::Accepted).unwrap();
			}
		);

		expect_reputation_change(overseer, &peer_b, BENEFIT_VALID_MESSAGE_FIRST).await;

		// and try again
		let msg = protocol_v1::ApprovalDistributionMessage::Approvals(vec![approval.clone()]);
		send_message_from_peer(overseer, &peer_b, msg).await;

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportApproval(
				vote,
				tx,
			)) => {
				assert_eq!(vote, approval);
				tx.send(ApprovalCheckResult::Bad).unwrap();
			}
		);

		expect_reputation_change(overseer, &peer_b, COST_INVALID_MESSAGE).await;
	});
}

/// make sure we clean up the state on block finalized
#[test]
fn update_our_view() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash_a = Hash::repeat_byte(0xAA);
	let hash_b = Hash::repeat_byte(0xBB);
	let hash_c = Hash::repeat_byte(0xCC);

	let state = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		// new block `hash_a` with 1 candidates
		let meta_a = BlockApprovalMeta {
			hash: hash_a,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
		};
		let meta_b = BlockApprovalMeta {
			hash: hash_b,
			parent_hash: hash_a,
			number: 2,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
		};
		let meta_c = BlockApprovalMeta {
			hash: hash_c,
			parent_hash: hash_b,
			number: 3,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta_a, meta_b, meta_c]);
		overseer_send(overseer, msg).await;
	});

	assert!(state.blocks_by_number.get(&1).is_some());
	assert!(state.blocks_by_number.get(&2).is_some());
	assert!(state.blocks_by_number.get(&3).is_some());
	assert!(state.blocks.get(&hash_a).is_some());
	assert!(state.blocks.get(&hash_b).is_some());
	assert!(state.blocks.get(&hash_c).is_some());

	let state = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		// finalize a block
		overseer_signal_block_finalized(overseer, 2).await;
	});

	assert!(state.blocks_by_number.get(&1).is_none());
	assert!(state.blocks_by_number.get(&2).is_none());
	assert!(state.blocks_by_number.get(&3).is_some());
	assert!(state.blocks.get(&hash_a).is_none());
	assert!(state.blocks.get(&hash_b).is_none());
	assert!(state.blocks.get(&hash_c).is_some());

	let state = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		// finalize a very high block
		overseer_signal_block_finalized(overseer, 4_000_000_000).await;
	});

	assert!(state.blocks_by_number.get(&3).is_none());
	assert!(state.blocks.get(&hash_c).is_none());
}

/// make sure we unify with peers and clean up the state
#[test]
fn update_peer_view() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash_a = Hash::repeat_byte(0xAA);
	let hash_b = Hash::repeat_byte(0xBB);
	let hash_c = Hash::repeat_byte(0xCC);
	let hash_d = Hash::repeat_byte(0xDD);
	let peer_a = PeerId::random();
	let peer = &peer_a;

	let state = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		// new block `hash_a` with 1 candidates
		let meta_a = BlockApprovalMeta {
			hash: hash_a,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
		};
		let meta_b = BlockApprovalMeta {
			hash: hash_b,
			parent_hash: hash_a,
			number: 2,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
		};
		let meta_c = BlockApprovalMeta {
			hash: hash_c,
			parent_hash: hash_b,
			number: 3,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta_a, meta_b, meta_c]);
		overseer_send(overseer, msg).await;

		let cert_a = fake_assignment_cert(hash_a, ValidatorIndex(0));
		let cert_b = fake_assignment_cert(hash_b, ValidatorIndex(0));

		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert_a, 0)
		).await;

		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert_b, 0)
		).await;

		// connect a peer
		setup_peer_with_view(overseer, peer, view![hash_a]).await;

		// we should send relevant assignments to the peer
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
				peers,
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments)
				)
			)) => {
				assert_eq!(peers.len(), 1);
				assert_eq!(assignments.len(), 1);
			}
		);
	});

	assert_eq!(state.peer_views.get(peer).map(|v| v.finalized_number), Some(0));
	assert_eq!(
		state.blocks
			.get(&hash_a)
			.unwrap()
			.known_by
			.get(peer)
			.unwrap()
			.known_messages
			.len(),
		1,
	);

	let state = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		// update peer's view
		overseer_send(
			overseer,
			ApprovalDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), View::new(vec![hash_b, hash_c, hash_d], 2))
			)
		).await;

		let cert_c = fake_assignment_cert(hash_c, ValidatorIndex(0));

		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert_c.clone(), 0)
		).await;

		// we should send relevant assignments to the peer
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
				peers,
				protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments)
				)
			)) => {
				assert_eq!(peers.len(), 1);
				assert_eq!(assignments.len(), 1);
				assert_eq!(assignments[0].0, cert_c);
			}
		);
	});

	assert_eq!(state.peer_views.get(peer).map(|v| v.finalized_number), Some(2));
	assert_eq!(
		state.blocks
			.get(&hash_c)
			.unwrap()
			.known_by
			.get(peer)
			.unwrap()
			.known_messages
			.len(),
		1,
	);

	let finalized_number = 4_000_000_000;
	let state = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		// update peer's view
		overseer_send(
			overseer,
			ApprovalDistributionMessage::NetworkBridgeUpdateV1(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), View::with_finalized(finalized_number))
			)
		).await;
	});

	assert_eq!(state.peer_views.get(peer).map(|v| v.finalized_number), Some(finalized_number));
	assert!(
		state.blocks
			.get(&hash_c)
			.unwrap()
			.known_by
			.get(peer)
			.is_none()
	);
}

#[test]
fn import_remotely_then_locally() {
	let peer_a = PeerId::random();
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);
	let peer = &peer_a;

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		// setup the peer
		setup_peer_with_view(overseer, peer, view![hash]).await;

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
		};
		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		// import the assignment remotely first
		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;
		let cert = fake_assignment_cert(hash, validator_index);
		let assignments = vec![(cert.clone(), candidate_index)];
		let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments.clone());
		send_message_from_peer(overseer, peer, msg).await;

		// send an `Accept` message from the Approval Voting subsystem
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportAssignment(
				assignment,
				i,
				tx,
			)) => {
				assert_eq!(assignment, cert);
				assert_eq!(i, candidate_index);
				tx.send(AssignmentCheckResult::Accepted).unwrap();
			}
		);

		expect_reputation_change(overseer, peer, BENEFIT_VALID_MESSAGE_FIRST).await;

		// import the same assignment locally
		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert, candidate_index)
		).await;

		assert!(overseer
			.recv()
			.timeout(TIMEOUT)
			.await
			.is_none(),
			"no message should be sent",
		);

		// send the approval remotely
		let approval = IndirectSignedApprovalVote {
			block_hash: hash,
			candidate_index,
			validator: validator_index,
			signature: Default::default(),
		};
		let msg = protocol_v1::ApprovalDistributionMessage::Approvals(vec![approval.clone()]);
		send_message_from_peer(overseer, peer, msg).await;

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportApproval(
				vote,
				tx,
			)) => {
				assert_eq!(vote, approval);
				tx.send(ApprovalCheckResult::Accepted).unwrap();
			}
		);
		expect_reputation_change(overseer, peer, BENEFIT_VALID_MESSAGE_FIRST).await;

		// import the same approval locally
		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeApproval(approval)
		).await;

		assert!(overseer
			.recv()
			.timeout(TIMEOUT)
			.await
			.is_none(),
			"no message should be sent",
		);
	});
}
