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
use polkadot_node_network_protocol::{
	grid_topology::{SessionGridTopology, TopologyPeerInfo},
	our_view,
	peer_set::ValidationVersion,
	view, ObservedRole,
};
use polkadot_node_primitives::approval::{
	AssignmentCertKind, VRFOutput, VRFProof, RELAY_VRF_MODULO_CONTEXT,
};
use polkadot_node_subsystem::messages::{network_bridge_event, AllMessages, ApprovalCheckError};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_util::TimeoutExt as _;
use polkadot_primitives::v2::{AuthorityDiscoveryId, BlakeTwo256, HashT};
use polkadot_primitives_test_helpers::dummy_signature;
use rand::SeedableRng;
use sp_authority_discovery::AuthorityPair as AuthorityDiscoveryPair;
use sp_core::crypto::Pair as PairT;
use std::time::Duration;

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<ApprovalDistributionMessage>;

fn test_harness<T: Future<Output = VirtualOverseer>>(
	mut state: State,
	test_fn: impl FnOnce(VirtualOverseer) -> T,
) -> State {
	let _ = env_logger::builder()
		.is_test(true)
		.filter(Some(LOG_TARGET), log::LevelFilter::Trace)
		.try_init();

	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = ApprovalDistribution::new(Default::default());
	{
		let mut rng = rand_chacha::ChaCha12Rng::seed_from_u64(12345);

		let subsystem = subsystem.run_inner(context, &mut state, &mut rng);

		let test_fut = test_fn(virtual_overseer);

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::join(
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
	}

	state
}

const TIMEOUT: Duration = Duration::from_millis(200);

async fn overseer_send(overseer: &mut VirtualOverseer, msg: ApprovalDistributionMessage) {
	gum::trace!(msg = ?msg, "Sending message");
	overseer
		.send(FromOrchestra::Communication { msg })
		.timeout(TIMEOUT)
		.await
		.expect("msg send timeout");
}

async fn overseer_signal_block_finalized(overseer: &mut VirtualOverseer, number: BlockNumber) {
	gum::trace!(?number, "Sending a finalized signal");
	// we don't care about the block hash
	overseer
		.send(FromOrchestra::Signal(OverseerSignal::BlockFinalized(Hash::zero(), number)))
		.timeout(TIMEOUT)
		.await
		.expect("signal send timeout");
}

async fn overseer_recv(overseer: &mut VirtualOverseer) -> AllMessages {
	gum::trace!("Waiting for a message");
	let msg = overseer.recv().timeout(TIMEOUT).await.expect("msg recv timeout");

	gum::trace!(msg = ?msg, "Received message");

	msg
}

fn make_peers_and_authority_ids(n: usize) -> Vec<(PeerId, AuthorityDiscoveryId)> {
	(0..n)
		.map(|_| {
			let peer_id = PeerId::random();
			let authority_id = AuthorityDiscoveryPair::generate().0.public();

			(peer_id, authority_id)
		})
		.collect()
}

fn make_gossip_topology(
	session: SessionIndex,
	all_peers: &[(PeerId, AuthorityDiscoveryId)],
	neighbors_x: &[usize],
	neighbors_y: &[usize],
) -> network_bridge_event::NewGossipTopology {
	// This builds a grid topology which is a square matrix.
	// The local validator occupies the top left-hand corner.
	// The X peers occupy the same row and the Y peers occupy
	// the same column.

	let local_index = 1;

	assert_eq!(
		neighbors_x.len(),
		neighbors_y.len(),
		"mocking grid topology only implemented for squares",
	);

	let d = neighbors_x.len() + 1;

	let grid_size = d * d;
	assert!(grid_size > 0);
	assert!(all_peers.len() >= grid_size);

	let peer_info = |i: usize| TopologyPeerInfo {
		peer_ids: vec![all_peers[i].0.clone()],
		validator_index: ValidatorIndex::from(i as u32),
		discovery_id: all_peers[i].1.clone(),
	};

	let mut canonical_shuffling: Vec<_> = (0..)
		.filter(|i| local_index != *i)
		.filter(|i| !neighbors_x.contains(i))
		.filter(|i| !neighbors_y.contains(i))
		.take(grid_size)
		.map(peer_info)
		.collect();

	// filled with junk except for own.
	let mut shuffled_indices = vec![d + 1; grid_size];
	shuffled_indices[local_index] = 0;
	canonical_shuffling[0] = peer_info(local_index);

	for (x_pos, v) in neighbors_x.iter().enumerate() {
		let pos = 1 + x_pos;
		canonical_shuffling[pos] = peer_info(*v);
	}

	for (y_pos, v) in neighbors_y.iter().enumerate() {
		let pos = d * (1 + y_pos);
		canonical_shuffling[pos] = peer_info(*v);
	}

	let topology = SessionGridTopology::new(shuffled_indices, canonical_shuffling);

	// sanity check.
	{
		let g_n = topology
			.compute_grid_neighbors_for(ValidatorIndex(local_index as _))
			.expect("topology just constructed with this validator index");

		assert_eq!(g_n.validator_indices_x.len(), neighbors_x.len());
		assert_eq!(g_n.validator_indices_y.len(), neighbors_y.len());

		for i in neighbors_x {
			assert!(g_n.validator_indices_x.contains(&ValidatorIndex(*i as _)));
		}

		for i in neighbors_y {
			assert!(g_n.validator_indices_y.contains(&ValidatorIndex(*i as _)));
		}
	}

	network_bridge_event::NewGossipTopology {
		session,
		topology,
		local_index: Some(ValidatorIndex(local_index as _)),
	}
}

async fn setup_gossip_topology(
	virtual_overseer: &mut VirtualOverseer,
	gossip_topology: network_bridge_event::NewGossipTopology,
) {
	overseer_send(
		virtual_overseer,
		ApprovalDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::NewGossipTopology(
			gossip_topology,
		)),
	)
	.await;
}

async fn setup_peer_with_view(
	virtual_overseer: &mut VirtualOverseer,
	peer_id: &PeerId,
	view: View,
) {
	overseer_send(
		virtual_overseer,
		ApprovalDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerConnected(
			peer_id.clone(),
			ObservedRole::Full,
			ValidationVersion::V1.into(),
			None,
		)),
	)
	.await;
	overseer_send(
		virtual_overseer,
		ApprovalDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerViewChange(
			peer_id.clone(),
			view,
		)),
	)
	.await;
}

async fn send_message_from_peer(
	virtual_overseer: &mut VirtualOverseer,
	peer_id: &PeerId,
	msg: protocol_v1::ApprovalDistributionMessage,
) {
	overseer_send(
		virtual_overseer,
		ApprovalDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerMessage(
			peer_id.clone(),
			Versioned::V1(msg),
		)),
	)
	.await;
}

fn fake_assignment_cert(block_hash: Hash, validator: ValidatorIndex) -> IndirectAssignmentCert {
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
			kind: AssignmentCertKind::RelayVRFModulo { sample: 1 },
			vrf: (VRFOutput(out), VRFProof(proof)),
		},
	}
}

async fn expect_reputation_change(
	virtual_overseer: &mut VirtualOverseer,
	peer_id: &PeerId,
	expected_reputation_change: Rep,
) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::NetworkBridgeTx(
			NetworkBridgeTxMessage::ReportPeer(
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
			session: 1,
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
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments)
				))
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

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");
		virtual_overseer
	});
}

/// <https://github.com/paritytech/polkadot/pull/2160#discussion_r547594835>
///
/// 1. Send a view update that removes block B from their view.
/// 2. Send a message from B that they incur `COST_UNEXPECTED_MESSAGE` for,
///    but then they receive `BENEFIT_VALID_MESSAGE`.
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
			session: 1,
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
			})
			.collect();

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
			ApprovalDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerViewChange(
				peer.clone(),
				View::with_finalized(2),
			)),
		)
		.await;

		// send the assignments again
		send_message_from_peer(overseer, peer, msg.clone()).await;

		// each of them will incur `COST_UNEXPECTED_MESSAGE`, not only the first one
		for _ in 0..candidates_count {
			expect_reputation_change(overseer, peer, COST_UNEXPECTED_MESSAGE).await;
			expect_reputation_change(overseer, peer, BENEFIT_VALID_MESSAGE).await;
		}
		virtual_overseer
	});
}

/// Imagine we send a message to peer A and peer B.
/// Upon receiving them, they both will try to send the message each other.
/// This test makes sure they will not punish each other for such duplicate messages.
///
/// See <https://github.com/paritytech/polkadot/issues/2499>.
#[test]
fn peer_sending_us_the_same_we_just_sent_them_is_ok() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let peer_a = PeerId::random();
	let hash = Hash::repeat_byte(0xAA);

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		let peer = &peer_a;
		setup_peer_with_view(overseer, peer, view![]).await;

		// new block `hash` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};
		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		// import an assignment related to `hash` locally
		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;
		let cert = fake_assignment_cert(hash, validator_index);
		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert.clone(), candidate_index),
		)
		.await;

		// update peer view to include the hash
		overseer_send(
			overseer,
			ApprovalDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerViewChange(
				peer.clone(),
				view![hash],
			)),
		)
		.await;

		// we should send them the assignment
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments)
				))
			)) => {
				assert_eq!(peers.len(), 1);
				assert_eq!(assignments.len(), 1);
			}
		);

		// but if someone else is sending it the same assignment
		// the peer could send us it as well
		let assignments = vec![(cert, candidate_index)];
		let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments);
		send_message_from_peer(overseer, peer, msg.clone()).await;

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "we should not punish the peer");

		// send the assignments again
		send_message_from_peer(overseer, peer, msg).await;

		// now we should
		expect_reputation_change(overseer, peer, COST_DUPLICATE_MESSAGE).await;
		virtual_overseer
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
			session: 1,
		};
		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		// import an assignment related to `hash` locally
		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;
		let cert = fake_assignment_cert(hash, validator_index);
		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert, candidate_index),
		)
		.await;

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments)
				))
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
			signature: dummy_signature(),
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
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(approvals)
				))
			)) => {
				assert_eq!(peers.len(), 1);
				assert_eq!(approvals.len(), 1);
			}
		);
		virtual_overseer
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
			session: 1,
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
			signature: dummy_signature(),
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
				tx.send(ApprovalCheckResult::Bad(ApprovalCheckError::UnknownBlock(hash))).unwrap();
			}
		);

		expect_reputation_change(overseer, &peer_b, COST_INVALID_MESSAGE).await;
		virtual_overseer
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
			session: 1,
		};
		let meta_b = BlockApprovalMeta {
			hash: hash_b,
			parent_hash: hash_a,
			number: 2,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};
		let meta_c = BlockApprovalMeta {
			hash: hash_c,
			parent_hash: hash_b,
			number: 3,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta_a, meta_b, meta_c]);
		overseer_send(overseer, msg).await;
		virtual_overseer
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
		virtual_overseer
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
		virtual_overseer
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
			session: 1,
		};
		let meta_b = BlockApprovalMeta {
			hash: hash_b,
			parent_hash: hash_a,
			number: 2,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};
		let meta_c = BlockApprovalMeta {
			hash: hash_c,
			parent_hash: hash_b,
			number: 3,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta_a, meta_b, meta_c]);
		overseer_send(overseer, msg).await;

		let cert_a = fake_assignment_cert(hash_a, ValidatorIndex(0));
		let cert_b = fake_assignment_cert(hash_b, ValidatorIndex(0));

		overseer_send(overseer, ApprovalDistributionMessage::DistributeAssignment(cert_a, 0)).await;

		overseer_send(overseer, ApprovalDistributionMessage::DistributeAssignment(cert_b, 0)).await;

		// connect a peer
		setup_peer_with_view(overseer, peer, view![hash_a]).await;

		// we should send relevant assignments to the peer
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments)
				))
			)) => {
				assert_eq!(peers.len(), 1);
				assert_eq!(assignments.len(), 1);
			}
		);
		virtual_overseer
	});

	assert_eq!(state.peer_views.get(peer).map(|v| v.finalized_number), Some(0));
	assert_eq!(
		state
			.blocks
			.get(&hash_a)
			.unwrap()
			.known_by
			.get(peer)
			.unwrap()
			.sent
			.known_messages
			.len(),
		1,
	);

	let state = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		// update peer's view
		overseer_send(
			overseer,
			ApprovalDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerViewChange(
				peer.clone(),
				View::new(vec![hash_b, hash_c, hash_d], 2),
			)),
		)
		.await;

		let cert_c = fake_assignment_cert(hash_c, ValidatorIndex(0));

		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert_c.clone(), 0),
		)
		.await;

		// we should send relevant assignments to the peer
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(assignments)
				))
			)) => {
				assert_eq!(peers.len(), 1);
				assert_eq!(assignments.len(), 1);
				assert_eq!(assignments[0].0, cert_c);
			}
		);
		virtual_overseer
	});

	assert_eq!(state.peer_views.get(peer).map(|v| v.finalized_number), Some(2));
	assert_eq!(
		state
			.blocks
			.get(&hash_c)
			.unwrap()
			.known_by
			.get(peer)
			.unwrap()
			.sent
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
			ApprovalDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerViewChange(
				peer.clone(),
				View::with_finalized(finalized_number),
			)),
		)
		.await;
		virtual_overseer
	});

	assert_eq!(state.peer_views.get(peer).map(|v| v.finalized_number), Some(finalized_number));
	assert!(state.blocks.get(&hash_c).unwrap().known_by.get(peer).is_none());
}

/// E.g. if someone copies the keys...
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
			session: 1,
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
			ApprovalDistributionMessage::DistributeAssignment(cert, candidate_index),
		)
		.await;

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");

		// send the approval remotely
		let approval = IndirectSignedApprovalVote {
			block_hash: hash,
			candidate_index,
			validator: validator_index,
			signature: dummy_signature(),
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
		overseer_send(overseer, ApprovalDistributionMessage::DistributeApproval(approval)).await;

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");
		virtual_overseer
	});
}

#[test]
fn sends_assignments_even_when_state_is_approved() {
	let peer_a = PeerId::random();
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);
	let peer = &peer_a;

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};
		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;

		// import an assignment and approval locally.
		let cert = fake_assignment_cert(hash, validator_index);
		let approval = IndirectSignedApprovalVote {
			block_hash: hash,
			candidate_index,
			validator: validator_index,
			signature: dummy_signature(),
		};

		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert.clone(), candidate_index),
		)
		.await;

		overseer_send(overseer, ApprovalDistributionMessage::DistributeApproval(approval.clone()))
			.await;

		// connect the peer.
		setup_peer_with_view(overseer, peer, view![hash]).await;

		let assignments = vec![(cert.clone(), candidate_index)];
		let approvals = vec![approval.clone()];

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
				))
			)) => {
				assert_eq!(peers, vec![peer.clone()]);
				assert_eq!(sent_assignments, assignments);
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(sent_approvals)
				))
			)) => {
				assert_eq!(peers, vec![peer.clone()]);
				assert_eq!(sent_approvals, approvals);
			}
		);

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");
		virtual_overseer
	});
}

/// <https://github.com/paritytech/polkadot/pull/5089>
///
/// 1. Receive remote peer view update with an unknown head
/// 2. Receive assignments for that unknown head
/// 3. Update our view and import the new block
/// 4. Expect that no reputation with `COST_UNEXPECTED_MESSAGE` is applied
#[test]
fn race_condition_in_local_vs_remote_view_update() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let peer_a = PeerId::random();
	let hash_b = Hash::repeat_byte(0xBB);

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;
		let peer = &peer_a;

		// Test a small number of candidates
		let candidates_count = 1;
		let meta = BlockApprovalMeta {
			hash: hash_b.clone(),
			parent_hash,
			number: 2,
			candidates: vec![Default::default(); candidates_count],
			slot: 1.into(),
			session: 1,
		};

		// This will send a peer view that is ahead of our view
		setup_peer_with_view(overseer, peer, view![hash_b]).await;

		// Send our view update to include a new head
		overseer_send(
			overseer,
			ApprovalDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(
				our_view![hash_b],
			)),
		)
		.await;

		// send assignments related to `hash_b` but they will come to the MessagesPending
		let assignments: Vec<_> = (0..candidates_count)
			.map(|candidate_index| {
				let validator_index = ValidatorIndex(candidate_index as u32);
				let cert = fake_assignment_cert(hash_b, validator_index);
				(cert, candidate_index as u32)
			})
			.collect();

		let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments.clone());
		send_message_from_peer(overseer, peer, msg.clone()).await;

		// This will handle pending messages being processed
		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		for i in 0..candidates_count {
			// Previously, this has caused out-of-view assignments/approvals
			//expect_reputation_change(overseer, peer, COST_UNEXPECTED_MESSAGE).await;

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

			// Since we have a valid statement pending, this should always occur
			expect_reputation_change(overseer, peer, BENEFIT_VALID_MESSAGE_FIRST).await;
		}
		virtual_overseer
	});
}

// Tests that local messages propagate to both dimensions.
#[test]
fn propagates_locally_generated_assignment_to_both_dimensions() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);

	let peers = make_peers_and_authority_ids(100);

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;

		// Connect all peers.
		for (peer, _) in &peers {
			setup_peer_with_view(overseer, peer, view![hash]).await;
		}

		// Set up a gossip topology.
		setup_gossip_topology(
			overseer,
			make_gossip_topology(1, &peers, &[0, 10, 20, 30], &[50, 51, 52, 53]),
		)
		.await;

		let expected_indices = [
			// Both dimensions in the gossip topology
			0, 10, 20, 30, 50, 51, 52, 53,
		];

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;

		// import an assignment and approval locally.
		let cert = fake_assignment_cert(hash, validator_index);
		let approval = IndirectSignedApprovalVote {
			block_hash: hash,
			candidate_index,
			validator: validator_index,
			signature: dummy_signature(),
		};

		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert.clone(), candidate_index),
		)
		.await;

		overseer_send(overseer, ApprovalDistributionMessage::DistributeApproval(approval.clone()))
			.await;

		let assignments = vec![(cert.clone(), candidate_index)];
		let approvals = vec![approval.clone()];

		let assignment_sent_peers = assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				sent_peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
				))
			)) => {
				assert_eq!(sent_peers.len(), expected_indices.len() + 4);
				for &i in &expected_indices {
					assert!(
						sent_peers.contains(&peers[i].0),
						"Message not sent to expected peer {}",
						i,
					);
				}
				assert_eq!(sent_assignments, assignments);
				sent_peers
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				sent_peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(sent_approvals)
				))
			)) => {
				// Random sampling is reused from the assignment.
				assert_eq!(sent_peers, assignment_sent_peers);
				assert_eq!(sent_approvals, approvals);
			}
		);

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");
		virtual_overseer
	});
}

// Tests that messages propagate to the unshared dimension.
#[test]
fn propagates_assignments_along_unshared_dimension() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);

	let peers = make_peers_and_authority_ids(100);

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;

		// Connect all peers.
		for (peer, _) in &peers {
			setup_peer_with_view(overseer, peer, view![hash]).await;
		}

		// Set up a gossip topology.
		setup_gossip_topology(
			overseer,
			make_gossip_topology(1, &peers, &[0, 10, 20, 30], &[50, 51, 52, 53]),
		)
		.await;

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		// Test messages from X direction go to Y peers
		{
			let validator_index = ValidatorIndex(0);
			let candidate_index = 0u32;

			// import an assignment and approval locally.
			let cert = fake_assignment_cert(hash, validator_index);
			let assignments = vec![(cert.clone(), candidate_index)];

			let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments.clone());

			// Issuer of the message is important, not the peer we receive from.
			// 99 deliberately chosen because it's not in X or Y.
			send_message_from_peer(overseer, &peers[99].0, msg).await;
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportAssignment(
					_,
					_,
					tx,
				)) => {
					tx.send(AssignmentCheckResult::Accepted).unwrap();
				}
			);
			expect_reputation_change(overseer, &peers[99].0, BENEFIT_VALID_MESSAGE_FIRST).await;

			let expected_y = [50, 51, 52, 53];

			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					sent_peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
					))
				)) => {
					assert_eq!(sent_peers.len(), expected_y.len() + 4);
					for &i in &expected_y {
						assert!(
							sent_peers.contains(&peers[i].0),
							"Message not sent to expected peer {}",
							i,
						);
					}
					assert_eq!(sent_assignments, assignments);
				}
			);
		};

		// Test messages from X direction go to Y peers
		{
			let validator_index = ValidatorIndex(50);
			let candidate_index = 0u32;

			// import an assignment and approval locally.
			let cert = fake_assignment_cert(hash, validator_index);
			let assignments = vec![(cert.clone(), candidate_index)];

			let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments.clone());

			// Issuer of the message is important, not the peer we receive from.
			// 99 deliberately chosen because it's not in X or Y.
			send_message_from_peer(overseer, &peers[99].0, msg).await;
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportAssignment(
					_,
					_,
					tx,
				)) => {
					tx.send(AssignmentCheckResult::Accepted).unwrap();
				}
			);
			expect_reputation_change(overseer, &peers[99].0, BENEFIT_VALID_MESSAGE_FIRST).await;

			let expected_x = [0, 10, 20, 30];

			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					sent_peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
					))
				)) => {
					assert_eq!(sent_peers.len(), expected_x.len() + 4);
					for &i in &expected_x {
						assert!(
							sent_peers.contains(&peers[i].0),
							"Message not sent to expected peer {}",
							i,
						);
					}
					assert_eq!(sent_assignments, assignments);
				}
			);
		};

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");
		virtual_overseer
	});
}

// tests that messages are propagated to necessary peers after they connect
#[test]
fn propagates_to_required_after_connect() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);

	let peers = make_peers_and_authority_ids(100);

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;

		let omitted = [0, 10, 50, 51];

		// Connect all peers except omitted.
		for (i, (peer, _)) in peers.iter().enumerate() {
			if !omitted.contains(&i) {
				setup_peer_with_view(overseer, peer, view![hash]).await;
			}
		}

		// Set up a gossip topology.
		setup_gossip_topology(
			overseer,
			make_gossip_topology(1, &peers, &[0, 10, 20, 30], &[50, 51, 52, 53]),
		)
		.await;

		let expected_indices = [
			// Both dimensions in the gossip topology, minus omitted.
			20, 30, 52, 53,
		];

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;

		// import an assignment and approval locally.
		let cert = fake_assignment_cert(hash, validator_index);
		let approval = IndirectSignedApprovalVote {
			block_hash: hash,
			candidate_index,
			validator: validator_index,
			signature: dummy_signature(),
		};

		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert.clone(), candidate_index),
		)
		.await;

		overseer_send(overseer, ApprovalDistributionMessage::DistributeApproval(approval.clone()))
			.await;

		let assignments = vec![(cert.clone(), candidate_index)];
		let approvals = vec![approval.clone()];

		let assignment_sent_peers = assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				sent_peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
				))
			)) => {
				assert_eq!(sent_peers.len(), expected_indices.len() + 4);
				for &i in &expected_indices {
					assert!(
						sent_peers.contains(&peers[i].0),
						"Message not sent to expected peer {}",
						i,
					);
				}
				assert_eq!(sent_assignments, assignments);
				sent_peers
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				sent_peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(sent_approvals)
				))
			)) => {
				// Random sampling is reused from the assignment.
				assert_eq!(sent_peers, assignment_sent_peers);
				assert_eq!(sent_approvals, approvals);
			}
		);

		for i in omitted.iter().copied() {
			setup_peer_with_view(overseer, &peers[i].0, view![hash]).await;

			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					sent_peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
					))
				)) => {
					assert_eq!(sent_peers.len(), 1);
					assert_eq!(&sent_peers[0], &peers[i].0);
					assert_eq!(sent_assignments, assignments);
				}
			);

			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					sent_peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Approvals(sent_approvals)
					))
				)) => {
					assert_eq!(sent_peers.len(), 1);
					assert_eq!(&sent_peers[0], &peers[i].0);
					assert_eq!(sent_approvals, approvals);
				}
			);
		}

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");
		virtual_overseer
	});
}

// test that new gossip topology triggers send of messages.
#[test]
fn sends_to_more_peers_after_getting_topology() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);

	let peers = make_peers_and_authority_ids(100);

	let _ = test_harness(State::default(), |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;

		// Connect all peers except omitted.
		for (peer, _) in &peers {
			setup_peer_with_view(overseer, peer, view![hash]).await;
		}

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;

		// import an assignment and approval locally.
		let cert = fake_assignment_cert(hash, validator_index);
		let approval = IndirectSignedApprovalVote {
			block_hash: hash,
			candidate_index,
			validator: validator_index,
			signature: dummy_signature(),
		};

		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert.clone(), candidate_index),
		)
		.await;

		overseer_send(overseer, ApprovalDistributionMessage::DistributeApproval(approval.clone()))
			.await;

		let assignments = vec![(cert.clone(), candidate_index)];
		let approvals = vec![approval.clone()];

		let mut expected_indices = vec![0, 10, 20, 30, 50, 51, 52, 53];
		let assignment_sent_peers = assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				sent_peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
				))
			)) => {
				// Only sends to random peers.
				assert_eq!(sent_peers.len(), 4);
				for peer in &sent_peers {
					let i = peers.iter().position(|p| peer == &p.0).unwrap();
					// Random gossip before topology can send to topology-targeted peers.
					// Remove them from the expected indices so we don't expect
					// them to get the messages again after the assignment.
					expected_indices.retain(|&i2| i2 != i);
				}
				assert_eq!(sent_assignments, assignments);
				sent_peers
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				sent_peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(sent_approvals)
				))
			)) => {
				// Random sampling is reused from the assignment.
				assert_eq!(sent_peers, assignment_sent_peers);
				assert_eq!(sent_approvals, approvals);
			}
		);

		// Set up a gossip topology.
		setup_gossip_topology(
			overseer,
			make_gossip_topology(1, &peers, &[0, 10, 20, 30], &[50, 51, 52, 53]),
		)
		.await;

		let mut expected_indices_assignments = expected_indices.clone();
		let mut expected_indices_approvals = expected_indices.clone();

		for _ in 0..expected_indices_assignments.len() {
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					sent_peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
					))
				)) => {
					// Sends to all expected peers.
					assert_eq!(sent_peers.len(), 1);
					assert_eq!(sent_assignments, assignments);

					let pos = expected_indices_assignments.iter()
						.position(|i| &peers[*i].0 == &sent_peers[0])
						.unwrap();
					expected_indices_assignments.remove(pos);
				}
			);
		}

		for _ in 0..expected_indices_approvals.len() {
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					sent_peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Approvals(sent_approvals)
					))
				)) => {
					// Sends to all expected peers.
					assert_eq!(sent_peers.len(), 1);
					assert_eq!(sent_approvals, approvals);

					let pos = expected_indices_approvals.iter()
						.position(|i| &peers[*i].0 == &sent_peers[0])
						.unwrap();

					expected_indices_approvals.remove(pos);
				}
			);
		}

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");
		virtual_overseer
	});
}

// test aggression L1
#[test]
fn originator_aggression_l1() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);

	let peers = make_peers_and_authority_ids(100);

	let mut state = State::default();
	state.aggression_config.resend_unfinalized_period = None;
	let aggression_l1_threshold = state.aggression_config.l1_threshold.clone().unwrap();

	let _ = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;

		// Connect all peers except omitted.
		for (peer, _) in &peers {
			setup_peer_with_view(overseer, peer, view![hash]).await;
		}

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;

		// import an assignment and approval locally.
		let cert = fake_assignment_cert(hash, validator_index);
		let approval = IndirectSignedApprovalVote {
			block_hash: hash,
			candidate_index,
			validator: validator_index,
			signature: dummy_signature(),
		};

		// Set up a gossip topology.
		setup_gossip_topology(
			overseer,
			make_gossip_topology(1, &peers, &[0, 10, 20, 30], &[50, 51, 52, 53]),
		)
		.await;

		overseer_send(
			overseer,
			ApprovalDistributionMessage::DistributeAssignment(cert.clone(), candidate_index),
		)
		.await;

		overseer_send(overseer, ApprovalDistributionMessage::DistributeApproval(approval.clone()))
			.await;

		let assignments = vec![(cert.clone(), candidate_index)];
		let approvals = vec![approval.clone()];

		let prev_sent_indices = assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				sent_peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(_)
				))
			)) => {
				sent_peers.into_iter()
					.filter_map(|sp| peers.iter().position(|p| &p.0 == &sp))
					.collect::<Vec<_>>()
			}
		);

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				_,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Approvals(_)
				))
			)) => { }
		);

		// Add blocks until aggression L1 is triggered.
		{
			let mut parent_hash = hash;
			for level in 0..aggression_l1_threshold {
				let number = 1 + level + 1; // first block had number 1
				let hash = BlakeTwo256::hash_of(&(parent_hash, number));
				let meta = BlockApprovalMeta {
					hash,
					parent_hash,
					number,
					candidates: vec![],
					slot: (level as u64).into(),
					session: 1,
				};

				let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
				overseer_send(overseer, msg).await;

				parent_hash = hash;
			}
		}

		let unsent_indices =
			(0..peers.len()).filter(|i| !prev_sent_indices.contains(&i)).collect::<Vec<_>>();

		for _ in 0..unsent_indices.len() {
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					sent_peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
					))
				)) => {
					// Sends to all expected peers.
					assert_eq!(sent_peers.len(), 1);
					assert_eq!(sent_assignments, assignments);

					assert!(unsent_indices.iter()
						.find(|i| &peers[**i].0 == &sent_peers[0])
						.is_some());
				}
			);
		}

		for _ in 0..unsent_indices.len() {
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					sent_peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Approvals(sent_approvals)
					))
				)) => {
					// Sends to all expected peers.
					assert_eq!(sent_peers.len(), 1);
					assert_eq!(sent_approvals, approvals);

					assert!(unsent_indices.iter()
						.find(|i| &peers[**i].0 == &sent_peers[0])
						.is_some());
				}
			);
		}

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");
		virtual_overseer
	});
}

// test aggression L1
#[test]
fn non_originator_aggression_l1() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);

	let peers = make_peers_and_authority_ids(100);

	let mut state = State::default();
	state.aggression_config.resend_unfinalized_period = None;
	let aggression_l1_threshold = state.aggression_config.l1_threshold.clone().unwrap();

	let _ = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;

		// Connect all peers except omitted.
		for (peer, _) in &peers {
			setup_peer_with_view(overseer, peer, view![hash]).await;
		}

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;

		// import an assignment and approval locally.
		let cert = fake_assignment_cert(hash, validator_index);

		// Set up a gossip topology.
		setup_gossip_topology(
			overseer,
			make_gossip_topology(1, &peers, &[0, 10, 20, 30], &[50, 51, 52, 53]),
		)
		.await;

		let assignments = vec![(cert.clone(), candidate_index)];
		let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments.clone());

		// Issuer of the message is important, not the peer we receive from.
		// 99 deliberately chosen because it's not in X or Y.
		send_message_from_peer(overseer, &peers[99].0, msg).await;
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportAssignment(
				_,
				_,
				tx,
			)) => {
				tx.send(AssignmentCheckResult::Accepted).unwrap();
			}
		);

		expect_reputation_change(overseer, &peers[99].0, BENEFIT_VALID_MESSAGE_FIRST).await;

		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				_,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(_)
				))
			)) => { }
		);

		// Add blocks until aggression L1 is triggered.
		{
			let mut parent_hash = hash;
			for level in 0..aggression_l1_threshold {
				let number = 1 + level + 1; // first block had number 1
				let hash = BlakeTwo256::hash_of(&(parent_hash, number));
				let meta = BlockApprovalMeta {
					hash,
					parent_hash,
					number,
					candidates: vec![],
					slot: (level as u64).into(),
					session: 1,
				};

				let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
				overseer_send(overseer, msg).await;

				parent_hash = hash;
			}
		}

		// No-op on non-originator

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");
		virtual_overseer
	});
}

// test aggression L2 on non-originator
#[test]
fn non_originator_aggression_l2() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);

	let peers = make_peers_and_authority_ids(100);

	let mut state = State::default();
	state.aggression_config.resend_unfinalized_period = None;

	let aggression_l1_threshold = state.aggression_config.l1_threshold.clone().unwrap();
	let aggression_l2_threshold = state.aggression_config.l2_threshold.clone().unwrap();
	let _ = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;

		// Connect all peers except omitted.
		for (peer, _) in &peers {
			setup_peer_with_view(overseer, peer, view![hash]).await;
		}

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;

		// import an assignment and approval locally.
		let cert = fake_assignment_cert(hash, validator_index);

		// Set up a gossip topology.
		setup_gossip_topology(
			overseer,
			make_gossip_topology(1, &peers, &[0, 10, 20, 30], &[50, 51, 52, 53]),
		)
		.await;

		let assignments = vec![(cert.clone(), candidate_index)];
		let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments.clone());

		// Issuer of the message is important, not the peer we receive from.
		// 99 deliberately chosen because it's not in X or Y.
		send_message_from_peer(overseer, &peers[99].0, msg).await;
		assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportAssignment(
				_,
				_,
				tx,
			)) => {
				tx.send(AssignmentCheckResult::Accepted).unwrap();
			}
		);

		expect_reputation_change(overseer, &peers[99].0, BENEFIT_VALID_MESSAGE_FIRST).await;

		let prev_sent_indices = assert_matches!(
			overseer_recv(overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				sent_peers,
				Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
					protocol_v1::ApprovalDistributionMessage::Assignments(_)
				))
			)) => {
				sent_peers.into_iter()
					.filter_map(|sp| peers.iter().position(|p| &p.0 == &sp))
					.collect::<Vec<_>>()
			}
		);

		// Add blocks until aggression L1 is triggered.
		let chain_head = {
			let mut parent_hash = hash;
			for level in 0..aggression_l1_threshold {
				let number = 1 + level + 1; // first block had number 1
				let hash = BlakeTwo256::hash_of(&(parent_hash, number));
				let meta = BlockApprovalMeta {
					hash,
					parent_hash,
					number,
					candidates: vec![],
					slot: (level as u64).into(),
					session: 1,
				};

				let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
				overseer_send(overseer, msg).await;

				parent_hash = hash;
			}

			parent_hash
		};

		// No-op on non-originator

		// Add blocks until aggression L2 is triggered.
		{
			let mut parent_hash = chain_head;
			for level in 0..aggression_l2_threshold - aggression_l1_threshold {
				let number = aggression_l1_threshold + level + 1 + 1; // first block had number 1
				let hash = BlakeTwo256::hash_of(&(parent_hash, number));
				let meta = BlockApprovalMeta {
					hash,
					parent_hash,
					number,
					candidates: vec![],
					slot: (level as u64).into(),
					session: 1,
				};

				let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
				overseer_send(overseer, msg).await;

				parent_hash = hash;
			}
		}

		// XY dimension - previously sent.
		let unsent_indices = [0, 10, 20, 30, 50, 51, 52, 53]
			.iter()
			.cloned()
			.filter(|i| !prev_sent_indices.contains(&i))
			.collect::<Vec<_>>();

		for _ in 0..unsent_indices.len() {
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					sent_peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
					))
				)) => {
					// Sends to all expected peers.
					assert_eq!(sent_peers.len(), 1);
					assert_eq!(sent_assignments, assignments);

					assert!(unsent_indices.iter()
						.find(|i| &peers[**i].0 == &sent_peers[0])
						.is_some());
				}
			);
		}

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");
		virtual_overseer
	});
}

// Tests that messages propagate to the unshared dimension.
#[test]
fn resends_messages_periodically() {
	let parent_hash = Hash::repeat_byte(0xFF);
	let hash = Hash::repeat_byte(0xAA);

	let peers = make_peers_and_authority_ids(100);

	let mut state = State::default();
	state.aggression_config.l1_threshold = None;
	state.aggression_config.l2_threshold = None;
	state.aggression_config.resend_unfinalized_period = Some(2);
	let _ = test_harness(state, |mut virtual_overseer| async move {
		let overseer = &mut virtual_overseer;

		// Connect all peers.
		for (peer, _) in &peers {
			setup_peer_with_view(overseer, peer, view![hash]).await;
		}

		// Set up a gossip topology.
		setup_gossip_topology(
			overseer,
			make_gossip_topology(1, &peers, &[0, 10, 20, 30], &[50, 51, 52, 53]),
		)
		.await;

		// new block `hash_a` with 1 candidates
		let meta = BlockApprovalMeta {
			hash,
			parent_hash,
			number: 1,
			candidates: vec![Default::default(); 1],
			slot: 1.into(),
			session: 1,
		};

		let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
		overseer_send(overseer, msg).await;

		let validator_index = ValidatorIndex(0);
		let candidate_index = 0u32;

		// import an assignment and approval locally.
		let cert = fake_assignment_cert(hash, validator_index);
		let assignments = vec![(cert.clone(), candidate_index)];

		{
			let msg = protocol_v1::ApprovalDistributionMessage::Assignments(assignments.clone());

			// Issuer of the message is important, not the peer we receive from.
			// 99 deliberately chosen because it's not in X or Y.
			send_message_from_peer(overseer, &peers[99].0, msg).await;
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::ApprovalVoting(ApprovalVotingMessage::CheckAndImportAssignment(
					_,
					_,
					tx,
				)) => {
					tx.send(AssignmentCheckResult::Accepted).unwrap();
				}
			);
			expect_reputation_change(overseer, &peers[99].0, BENEFIT_VALID_MESSAGE_FIRST).await;

			let expected_y = [50, 51, 52, 53];

			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					sent_peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
					))
				)) => {
					assert_eq!(sent_peers.len(), expected_y.len() + 4);
					for &i in &expected_y {
						assert!(
							sent_peers.contains(&peers[i].0),
							"Message not sent to expected peer {}",
							i,
						);
					}
					assert_eq!(sent_assignments, assignments);
				}
			);
		};

		let mut number = 1;
		for _ in 0..10 {
			// Add blocks until resend is done.
			{
				let mut parent_hash = hash;
				for level in 0..2 {
					number = number + 1;
					let hash = BlakeTwo256::hash_of(&(parent_hash, number));
					let meta = BlockApprovalMeta {
						hash,
						parent_hash,
						number,
						candidates: vec![],
						slot: (level as u64).into(),
						session: 1,
					};

					let msg = ApprovalDistributionMessage::NewBlocks(vec![meta]);
					overseer_send(overseer, msg).await;

					parent_hash = hash;
				}
			}

			let mut expected_y = vec![50, 51, 52, 53];

			// Expect messages sent only to topology peers, one by one.
			for _ in 0..expected_y.len() {
				assert_matches!(
					overseer_recv(overseer).await,
					AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
						sent_peers,
						Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
							protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
						))
					)) => {
						assert_eq!(sent_peers.len(), 1);
						let expected_pos = expected_y.iter()
							.position(|&i| &peers[i].0 == &sent_peers[0])
							.unwrap();

						expected_y.remove(expected_pos);
						assert_eq!(sent_assignments, assignments);
					}
				);
			}
		}

		assert!(overseer.recv().timeout(TIMEOUT).await.is_none(), "no message should be sent");
		virtual_overseer
	});
}

fn batch_test_round(message_count: usize) {
	use polkadot_node_subsystem::SubsystemContext;
	let pool = sp_core::testing::TaskExecutor::new();
	let mut state = State::default();

	let (mut context, mut virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());
	let subsystem = ApprovalDistribution::new(Default::default());
	let mut rng = rand_chacha::ChaCha12Rng::seed_from_u64(12345);
	let mut sender = context.sender().clone();
	let subsystem = subsystem.run_inner(context, &mut state, &mut rng);

	let test_fut = async move {
		let overseer = &mut virtual_overseer;
		let validators = 0..message_count;
		let assignments: Vec<_> = validators
			.clone()
			.map(|index| (fake_assignment_cert(Hash::zero(), ValidatorIndex(index as u32)), 0))
			.collect();

		let approvals: Vec<_> = validators
			.map(|index| IndirectSignedApprovalVote {
				block_hash: Hash::zero(),
				candidate_index: 0,
				validator: ValidatorIndex(index as u32),
				signature: dummy_signature(),
			})
			.collect();

		let peer = PeerId::random();
		send_assignments_batched(&mut sender, assignments.clone(), peer).await;
		send_approvals_batched(&mut sender, approvals.clone(), peer).await;

		// Check expected assignments batches.
		for assignment_index in (0..assignments.len()).step_by(super::MAX_ASSIGNMENT_BATCH_SIZE) {
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Assignments(sent_assignments)
					))
				)) => {
					// Last batch should cover all remaining messages.
					if sent_assignments.len() < super::MAX_ASSIGNMENT_BATCH_SIZE {
						assert_eq!(sent_assignments.len() + assignment_index, assignments.len());
					} else {
						assert_eq!(sent_assignments.len(), super::MAX_ASSIGNMENT_BATCH_SIZE);
					}

					assert_eq!(peers.len(), 1);

					for (message_index,  assignment) in sent_assignments.iter().enumerate() {
						assert_eq!(assignment.0, assignments[assignment_index + message_index].0);
						assert_eq!(assignment.1, 0);
					}
				}
			);
		}

		// Check approval vote batching.
		for approval_index in (0..approvals.len()).step_by(super::MAX_APPROVAL_BATCH_SIZE) {
			assert_matches!(
				overseer_recv(overseer).await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					peers,
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Approvals(sent_approvals)
					))
				)) => {
					// Last batch should cover all remaining messages.
					if sent_approvals.len() < super::MAX_APPROVAL_BATCH_SIZE {
						assert_eq!(sent_approvals.len() + approval_index, approvals.len());
					} else {
						assert_eq!(sent_approvals.len(), super::MAX_APPROVAL_BATCH_SIZE);
					}

					assert_eq!(peers.len(), 1);

					for (message_index,  approval) in sent_approvals.iter().enumerate() {
						assert_eq!(approval, &approvals[approval_index + message_index]);
					}
				}
			);
		}
		virtual_overseer
	};

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	executor::block_on(future::join(
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
}

#[test]
fn batch_sending_1_msg() {
	batch_test_round(1);
}

#[test]
fn batch_sending_exactly_one_batch() {
	batch_test_round(super::MAX_APPROVAL_BATCH_SIZE);
	batch_test_round(super::MAX_ASSIGNMENT_BATCH_SIZE);
}

#[test]
fn batch_sending_partial_batch() {
	batch_test_round(super::MAX_APPROVAL_BATCH_SIZE * 2 + 4);
	batch_test_round(super::MAX_ASSIGNMENT_BATCH_SIZE * 2 + 4);
}

#[test]
fn batch_sending_multiple_same_len() {
	batch_test_round(super::MAX_APPROVAL_BATCH_SIZE * 10);
	batch_test_round(super::MAX_ASSIGNMENT_BATCH_SIZE * 10);
}

#[test]
fn batch_sending_half_batch() {
	batch_test_round(super::MAX_APPROVAL_BATCH_SIZE / 2);
	batch_test_round(super::MAX_ASSIGNMENT_BATCH_SIZE / 2);
}

#[test]
#[should_panic]
fn const_batch_size_panics_if_zero() {
	crate::ensure_size_not_zero(0);
}

#[test]
fn const_ensure_size_not_zero() {
	crate::ensure_size_not_zero(super::MAX_ASSIGNMENT_BATCH_SIZE);
	crate::ensure_size_not_zero(super::MAX_APPROVAL_BATCH_SIZE);
}
