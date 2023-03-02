// Copyright 2023 Parity Technologies (UK) Ltd.
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

use bitvec::order::Lsb0;
use parity_scale_codec::{Decode, Encode};
use polkadot_node_network_protocol::{
	grid_topology::TopologyPeerInfo, request_response::vstaging as request_vstaging,
	vstaging::BackedCandidateManifest,
};
use polkadot_primitives_test_helpers::make_candidate;
use sc_network::config::{
	IncomingRequest as RawIncomingRequest, OutgoingResponse as RawOutgoingResponse,
};

#[test]
fn cluster_peer_allowed_to_send_incomplete_statements() {
	let group_size = 3;
	let config = TestConfig {
		validator_count: 20,
		group_size,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();

	test_harness(config, |mut state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);
		let peer_id = PeerId::random();

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			local_para,
			vec![1, 2, 3].into(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

		let test_leaf = TestLeaf {
			number: 1,
			hash: relay_parent,
			parent_hash: Hash::repeat_byte(0),
			session: 1,
			availability_cores: state.make_availability_cores(|i| {
				CoreState::Scheduled(ScheduledCore {
					para_id: ParaId::from(i as u32),
					collator: None,
				})
			}),
			para_data: (0..state.session_info.validator_groups.len())
				.map(|i| {
					(
						ParaId::from(i as u32),
						PerParaData { min_relay_parent: 1, head_data: vec![1, 2, 3].into() },
					)
				})
				.collect(),
		};

		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];
		let v_b = other_group_validators[1];

		// peer A is in group, has relay parent in view.
		// peer B is in group, has no relay parent in view.
		// peer C is not in group, has relay parent in view.
		{
			connect_peer(
				&mut overseer,
				peer_a.clone(),
				Some(vec![state.discovery_id(other_group_validators[0])].into_iter().collect()),
			)
			.await;

			connect_peer(
				&mut overseer,
				peer_b.clone(),
				Some(vec![state.discovery_id(other_group_validators[1])].into_iter().collect()),
			)
			.await;

			connect_peer(&mut overseer, peer_c.clone(), None).await;

			send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
			send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;
		}

		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Peer in cluster sends a statement, triggering a request.
		{
			let a_seconded = state
				.sign_statement(
					v_a,
					CompactStatement::Seconded(candidate_hash),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
				)
				.as_unchecked()
				.clone();

			send_peer_message(
				&mut overseer,
				peer_a.clone(),
				protocol_vstaging::StatementDistributionMessage::Statement(
					relay_parent,
					a_seconded,
				),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST => { }
			);
		}

		// Send a request to peer and mock its response to include just one statement.
		{
			let b_seconded = state
				.sign_statement(
					v_b,
					CompactStatement::Seconded(candidate_hash),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
				)
				.as_unchecked()
				.clone();
			let statements = vec![b_seconded.clone()];
			// `1` indicates statements NOT to request.
			let mask = StatementFilter::blank(group_size);
			let req = assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(mut requests, IfDisconnected::ImmediateError)) => {
					assert_eq!(requests.len(), 1);
					assert_matches!(
						requests.pop().unwrap(),
						Requests::AttestedCandidateVStaging(mut outgoing) => {
							assert_eq!(outgoing.peer, Recipient::Peer(peer_a.clone()));
							assert_eq!(outgoing.payload.candidate_hash, candidate_hash);
							assert_eq!(outgoing.payload.mask, mask);

							let res = AttestedCandidateResponse {
								candidate_receipt: candidate.clone(),
								persisted_validation_data: pvd.clone(),
								statements,
							};
							outgoing.pending_response.send(Ok(res.encode()));
						}
					);
				}
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT => { }
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == BENEFIT_VALID_RESPONSE => { }
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages:: NetworkBridgeTx(
					NetworkBridgeTxMessage::SendValidationMessage(
						peers,
						Versioned::VStaging(
							protocol_vstaging::ValidationProtocol::StatementDistribution(
								protocol_vstaging::StatementDistributionMessage::Statement(hash, statement),
							),
						),
					)
				) => {
					assert_eq!(peers, vec![peer_a]);
					assert_eq!(hash, relay_parent);
					assert_eq!(statement, b_seconded);
				}
			);
		}

		answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;

		overseer
	});
}

// TODO [now]: peer reported for providing statements meant to be masked out

// Peer reported for not providing enough statements, request retried.
#[test]
fn peer_reported_for_not_enough_statements() {
	let validator_count = 6;
	let group_size = 3;
	let config = TestConfig {
		validator_count,
		group_size,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_c = PeerId::random();
	let peer_d = PeerId::random();
	let peer_e = PeerId::random();

	test_harness(config, |mut state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();

		let other_group =
			next_group_index(local_validator.group_index, validator_count, group_size);
		let other_para = ParaId::from(other_group.0);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			other_para,
			vec![1, 2, 3].into(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

		let test_leaf = TestLeaf {
			number: 1,
			hash: relay_parent,
			parent_hash: Hash::repeat_byte(0),
			session: 1,
			availability_cores: state.make_availability_cores(|i| {
				CoreState::Scheduled(ScheduledCore {
					para_id: ParaId::from(i as u32),
					collator: None,
				})
			}),
			para_data: (0..state.session_info.validator_groups.len())
				.map(|i| {
					(
						ParaId::from(i as u32),
						PerParaData { min_relay_parent: 1, head_data: vec![1, 2, 3].into() },
					)
				})
				.collect(),
		};

		let target_group_validators = state.group_validators(other_group, true);
		let v_c = target_group_validators[0];
		let v_d = target_group_validators[1];
		let v_e = target_group_validators[2];

		// Connect C, D, E
		{
			connect_peer(
				&mut overseer,
				peer_c.clone(),
				Some(vec![state.discovery_id(v_c)].into_iter().collect()),
			)
			.await;

			connect_peer(
				&mut overseer,
				peer_d.clone(),
				Some(vec![state.discovery_id(v_d)].into_iter().collect()),
			)
			.await;

			connect_peer(
				&mut overseer,
				peer_e.clone(),
				Some(vec![state.discovery_id(v_e)].into_iter().collect()),
			)
			.await;

			send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;
			send_peer_view_change(&mut overseer, peer_d.clone(), view![relay_parent]).await;
			send_peer_view_change(&mut overseer, peer_e.clone(), view![relay_parent]).await;
		}

		activate_leaf(&mut overseer, other_para, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		{
			let topology = NewGossipTopology {
				session: 1,
				topology: SessionGridTopology::new(
					(0..validator_count).collect(),
					(0..validator_count)
						.map(|i| TopologyPeerInfo {
							peer_ids: Vec::new(),
							validator_index: ValidatorIndex(i as u32),
							discovery_id: AuthorityDiscoveryPair::generate().0.public(),
						})
						.collect(),
				),
				local_index: Some(state.local.as_ref().unwrap().validator_index),
			};
			send_new_topology(&mut overseer, topology).await;
		}

		let manifest = BackedCandidateManifest {
			relay_parent,
			candidate_hash,
			group_index: other_group,
			para_id: other_para,
			parent_head_data_hash: pvd.parent_head.hash(),
			statement_knowledge: StatementFilter {
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 1],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 0],
			},
		};

		// Peer sends an announcement.
		send_peer_message(
			&mut overseer,
			peer_c.clone(),
			protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
				manifest.clone(),
			),
		)
		.await;
		let c_seconded = state
			.sign_statement(
				v_c,
				CompactStatement::Seconded(candidate_hash),
				&SigningContext { parent_hash: relay_parent, session_index: 1 },
			)
			.as_unchecked()
			.clone();
		let statements = vec![c_seconded.clone()];
		// `1` indicates statements NOT to request.
		let mask = StatementFilter {
			seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
			validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
		};

		// We send a request to peer. Mock its response to include just one statement.
		{
			let req = assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(mut requests, IfDisconnected::ImmediateError)) => {
					assert_eq!(requests.len(), 1);
					assert_matches!(
						requests.pop().unwrap(),
						Requests::AttestedCandidateVStaging(mut outgoing) => {
							assert_eq!(outgoing.peer, Recipient::Peer(peer_c.clone()));
							assert_eq!(outgoing.payload.candidate_hash, candidate_hash);
							assert_eq!(outgoing.payload.mask, mask);

							let res = AttestedCandidateResponse {
								candidate_receipt: candidate.clone(),
								persisted_validation_data: pvd.clone(),
								statements: statements.clone(),
							};
							outgoing.pending_response.send(Ok(res.encode()));
						}
					);
				}
			);

			// The peer is reported for only sending one statement.
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_c && r == COST_INVALID_RESPONSE => { }
			);
		}

		// We re-try the request.
		{
			let statements = vec![
				c_seconded,
				state
					.sign_statement(
						v_d,
						CompactStatement::Seconded(candidate_hash),
						&SigningContext { parent_hash: relay_parent, session_index: 1 },
					)
					.as_unchecked()
					.clone(),
				state
					.sign_statement(
						v_e,
						CompactStatement::Seconded(candidate_hash),
						&SigningContext { parent_hash: relay_parent, session_index: 1 },
					)
					.as_unchecked()
					.clone(),
			];
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(mut requests, IfDisconnected::ImmediateError)) => {
					assert_eq!(requests.len(), 1);
					assert_matches!(
						requests.pop().unwrap(),
						Requests::AttestedCandidateVStaging(mut outgoing) => {
							assert_eq!(outgoing.peer, Recipient::Peer(peer_c.clone()));
							assert_eq!(outgoing.payload.candidate_hash, candidate_hash);

							let res = AttestedCandidateResponse {
								candidate_receipt: candidate.clone(),
								persisted_validation_data: pvd.clone(),
								statements,
							};
							outgoing.pending_response.send(Ok(res.encode()));
						}
					);
				}
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_c && r == BENEFIT_VALID_RESPONSE
			);

			answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;
		}

		overseer
	});
}

// Test that a peer answering an `AttestedCandidateRequest` with duplicate statements is punished.
#[test]
fn peer_reported_for_duplicate_statements() {
	let config = TestConfig {
		validator_count: 20,
		group_size: 3,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();

	test_harness(config, |mut state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);
		let peer_id = PeerId::random();

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			local_para,
			vec![1, 2, 3].into(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

		let test_leaf = TestLeaf {
			number: 1,
			hash: relay_parent,
			parent_hash: Hash::repeat_byte(0),
			session: 1,
			availability_cores: state.make_availability_cores(|i| {
				CoreState::Scheduled(ScheduledCore {
					para_id: ParaId::from(i as u32),
					collator: None,
				})
			}),
			para_data: (0..state.session_info.validator_groups.len())
				.map(|i| {
					(
						ParaId::from(i as u32),
						PerParaData { min_relay_parent: 1, head_data: vec![1, 2, 3].into() },
					)
				})
				.collect(),
		};

		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];
		let v_b = other_group_validators[1];

		// peer A is in group, has relay parent in view.
		// peer B is in group, has no relay parent in view.
		// peer C is not in group, has relay parent in view.
		{
			connect_peer(
				&mut overseer,
				peer_a.clone(),
				Some(vec![state.discovery_id(other_group_validators[0])].into_iter().collect()),
			)
			.await;

			connect_peer(
				&mut overseer,
				peer_b.clone(),
				Some(vec![state.discovery_id(other_group_validators[1])].into_iter().collect()),
			)
			.await;

			connect_peer(&mut overseer, peer_c.clone(), None).await;

			send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
			send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;
		}

		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Peer in cluster sends a statement, triggering a request.
		{
			let a_seconded = state
				.sign_statement(
					v_a,
					CompactStatement::Seconded(candidate_hash),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
				)
				.as_unchecked()
				.clone();

			send_peer_message(
				&mut overseer,
				peer_a.clone(),
				protocol_vstaging::StatementDistributionMessage::Statement(
					relay_parent,
					a_seconded,
				),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST => { }
			);
		}

		// Send a request to peer and mock its response to include two identical statements.
		{
			let b_seconded = state
				.sign_statement(
					v_b,
					CompactStatement::Seconded(candidate_hash),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
				)
				.as_unchecked()
				.clone();
			let statements = vec![b_seconded.clone(), b_seconded.clone()];
			let req = assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(mut requests, IfDisconnected::ImmediateError)) => {
					assert_eq!(requests.len(), 1);
					assert_matches!(
						requests.pop().unwrap(),
						Requests::AttestedCandidateVStaging(mut outgoing) => {
							assert_eq!(outgoing.peer, Recipient::Peer(peer_a.clone()));
							assert_eq!(outgoing.payload.candidate_hash, candidate_hash);

							let res = AttestedCandidateResponse {
								candidate_receipt: candidate.clone(),
								persisted_validation_data: pvd.clone(),
								statements,
							};
							outgoing.pending_response.send(Ok(res.encode()));
						}
					);
				}
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT => { }
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == COST_UNREQUESTED_RESPONSE_STATEMENT => { }
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == BENEFIT_VALID_RESPONSE => { }
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages:: NetworkBridgeTx(
					NetworkBridgeTxMessage::SendValidationMessage(
						peers,
						Versioned::VStaging(
							protocol_vstaging::ValidationProtocol::StatementDistribution(
								protocol_vstaging::StatementDistributionMessage::Statement(hash, statement),
							),
						),
					)
				) => {
					assert_eq!(peers, vec![peer_a]);
					assert_eq!(hash, relay_parent);
					assert_eq!(statement, b_seconded);
				}
			);
		}

		answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;

		overseer
	});
}

#[test]
fn peer_reported_for_providing_statements_with_invalid_signatures() {
	let config = TestConfig {
		validator_count: 20,
		group_size: 3,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();

	test_harness(config, |mut state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);
		let peer_id = PeerId::random();

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			local_para,
			vec![1, 2, 3].into(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

		let test_leaf = TestLeaf {
			number: 1,
			hash: relay_parent,
			parent_hash: Hash::repeat_byte(0),
			session: 1,
			availability_cores: state.make_availability_cores(|i| {
				CoreState::Scheduled(ScheduledCore {
					para_id: ParaId::from(i as u32),
					collator: None,
				})
			}),
			para_data: (0..state.session_info.validator_groups.len())
				.map(|i| {
					(
						ParaId::from(i as u32),
						PerParaData { min_relay_parent: 1, head_data: vec![1, 2, 3].into() },
					)
				})
				.collect(),
		};

		let other_group_validators = state.group_validators(local_validator.group_index, true);
		state.group_validators((local_validator.group_index.0 + 1).into(), true);
		let v_a = other_group_validators[0];
		let v_b = other_group_validators[1];

		// peer A is in group, has relay parent in view.
		// peer B is in group, has no relay parent in view.
		// peer C is not in group, has relay parent in view.
		{
			connect_peer(
				&mut overseer,
				peer_a.clone(),
				Some(vec![state.discovery_id(other_group_validators[0])].into_iter().collect()),
			)
			.await;

			connect_peer(
				&mut overseer,
				peer_b.clone(),
				Some(vec![state.discovery_id(other_group_validators[1])].into_iter().collect()),
			)
			.await;

			connect_peer(&mut overseer, peer_c.clone(), None).await;

			send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
			send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;
		}

		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Peer in cluster sends a statement, triggering a request.
		{
			let a_seconded = state
				.sign_statement(
					v_a,
					CompactStatement::Seconded(candidate_hash),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
				)
				.as_unchecked()
				.clone();

			send_peer_message(
				&mut overseer,
				peer_a.clone(),
				protocol_vstaging::StatementDistributionMessage::Statement(
					relay_parent,
					a_seconded,
				),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST => { }
			);
		}

		// Send a request to peer and mock its response to include invalid statements.
		{
			// Sign statement with wrong signing context, leading to bad signature.
			let b_seconded_invalid = state
				.sign_statement(
					v_b,
					CompactStatement::Seconded(candidate_hash),
					&SigningContext { parent_hash: Hash::repeat_byte(42), session_index: 1 },
				)
				.as_unchecked()
				.clone();
			let statements = vec![b_seconded_invalid.clone()];
			let req = assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(mut requests, IfDisconnected::ImmediateError)) => {
					assert_eq!(requests.len(), 1);
					assert_matches!(
						requests.pop().unwrap(),
						Requests::AttestedCandidateVStaging(mut outgoing) => {
							assert_eq!(outgoing.peer, Recipient::Peer(peer_a.clone()));
							assert_eq!(outgoing.payload.candidate_hash, candidate_hash);

							let res = AttestedCandidateResponse {
								candidate_receipt: candidate.clone(),
								persisted_validation_data: pvd.clone(),
								statements,
							};
							outgoing.pending_response.send(Ok(res.encode()));
						}
					);
				}
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == COST_INVALID_SIGNATURE => { }
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == BENEFIT_VALID_RESPONSE => { }
			);

			answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;
		}

		overseer
	});
}

#[test]
fn peer_reported_for_providing_statements_with_wrong_validator_id() {
	let config = TestConfig {
		validator_count: 20,
		group_size: 3,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();

	test_harness(config, |mut state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);
		let peer_id = PeerId::random();

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			local_para,
			vec![1, 2, 3].into(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

		let test_leaf = TestLeaf {
			number: 1,
			hash: relay_parent,
			parent_hash: Hash::repeat_byte(0),
			session: 1,
			availability_cores: state.make_availability_cores(|i| {
				CoreState::Scheduled(ScheduledCore {
					para_id: ParaId::from(i as u32),
					collator: None,
				})
			}),
			para_data: (0..state.session_info.validator_groups.len())
				.map(|i| {
					(
						ParaId::from(i as u32),
						PerParaData { min_relay_parent: 1, head_data: vec![1, 2, 3].into() },
					)
				})
				.collect(),
		};

		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let next_group_validators =
			state.group_validators((local_validator.group_index.0 + 1).into(), true);
		let v_a = other_group_validators[0];
		let v_b = other_group_validators[1];
		let v_c = next_group_validators[0];

		// peer A is in group, has relay parent in view.
		// peer B is in group, has no relay parent in view.
		// peer C is not in group, has relay parent in view.
		{
			connect_peer(
				&mut overseer,
				peer_a.clone(),
				Some(vec![state.discovery_id(other_group_validators[0])].into_iter().collect()),
			)
			.await;

			connect_peer(
				&mut overseer,
				peer_b.clone(),
				Some(vec![state.discovery_id(other_group_validators[1])].into_iter().collect()),
			)
			.await;

			connect_peer(&mut overseer, peer_c.clone(), None).await;

			send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
			send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;
		}

		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Peer in cluster sends a statement, triggering a request.
		{
			let a_seconded = state
				.sign_statement(
					v_a,
					CompactStatement::Seconded(candidate_hash),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
				)
				.as_unchecked()
				.clone();

			send_peer_message(
				&mut overseer,
				peer_a.clone(),
				protocol_vstaging::StatementDistributionMessage::Statement(
					relay_parent,
					a_seconded,
				),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST => { }
			);
		}

		// Send a request to peer and mock its response to include a wrong validator ID.
		{
			let c_seconded_invalid = state
				.sign_statement(
					v_c,
					CompactStatement::Seconded(candidate_hash),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
				)
				.as_unchecked()
				.clone();
			let statements = vec![c_seconded_invalid.clone()];
			let req = assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(mut requests, IfDisconnected::ImmediateError)) => {
					assert_eq!(requests.len(), 1);
					assert_matches!(
						requests.pop().unwrap(),
						Requests::AttestedCandidateVStaging(mut outgoing) => {
							assert_eq!(outgoing.peer, Recipient::Peer(peer_a.clone()));
							assert_eq!(outgoing.payload.candidate_hash, candidate_hash);

							let res = AttestedCandidateResponse {
								candidate_receipt: candidate.clone(),
								persisted_validation_data: pvd.clone(),
								statements,
							};
							outgoing.pending_response.send(Ok(res.encode()));
						}
					);
				}
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == COST_UNREQUESTED_RESPONSE_STATEMENT => { }
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == BENEFIT_VALID_RESPONSE => { }
			);

			answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;
		}

		overseer
	});
}

#[test]
fn local_node_sanity_checks_incoming_requests() {
	let config = TestConfig {
		validator_count: 20,
		group_size: 3,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();
	let peer_d = PeerId::random();

	test_harness(config, |mut state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			local_para,
			vec![1, 2, 3].into(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

		let test_leaf = TestLeaf {
			number: 1,
			hash: relay_parent,
			parent_hash: Hash::repeat_byte(0),
			session: 1,
			availability_cores: state.make_availability_cores(|i| {
				CoreState::Scheduled(ScheduledCore {
					para_id: ParaId::from(i as u32),
					collator: None,
				})
			}),
			para_data: (0..state.session_info.validator_groups.len())
				.map(|i| {
					(
						ParaId::from(i as u32),
						PerParaData { min_relay_parent: 1, head_data: vec![1, 2, 3].into() },
					)
				})
				.collect(),
		};

		// peer A is in group, has relay parent in view.
		// peer B is in group, has no relay parent in view.
		// peer C is not in group, has relay parent in view.
		{
			let other_group_validators = state.group_validators(local_validator.group_index, true);

			connect_peer(
				&mut overseer,
				peer_a.clone(),
				Some(vec![state.discovery_id(other_group_validators[0])].into_iter().collect()),
			)
			.await;

			connect_peer(
				&mut overseer,
				peer_b.clone(),
				Some(vec![state.discovery_id(other_group_validators[1])].into_iter().collect()),
			)
			.await;

			connect_peer(&mut overseer, peer_c.clone(), None).await;

			send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
			send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;
		}

		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		let mask = StatementFilter::blank(state.config.group_size);

		// Should drop requests for unknown candidates.
		{
			let (pending_response, rx) = oneshot::channel();
			state
				.req_sender
				.send(RawIncomingRequest {
					// Request from peer that received manifest.
					peer: peer_c,
					payload: request_vstaging::AttestedCandidateRequest {
						candidate_hash: candidate.hash(),
						mask: mask.clone(),
					}
					.encode(),
					pending_response,
				})
				.await
				.unwrap();

			assert_matches!(rx.await, Err(oneshot::Canceled));
		}

		// Confirm candidate.
		{
			let full_signed = state
				.sign_statement(
					local_validator.validator_index,
					CompactStatement::Seconded(candidate_hash),
					&SigningContext { session_index: 1, parent_hash: relay_parent },
				)
				.convert_to_superpayload(StatementWithPVD::Seconded(candidate.clone(), pvd.clone()))
				.unwrap();

			overseer
				.send(FromOrchestra::Communication {
					msg: StatementDistributionMessage::Share(relay_parent, full_signed),
				})
				.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					peers,
					Versioned::VStaging(protocol_vstaging::ValidationProtocol::StatementDistribution(
						protocol_vstaging::StatementDistributionMessage::Statement(
							r,
							s,
						)
					))
				)) => {
					assert_eq!(peers, vec![peer_a.clone()]);
					assert_eq!(r, relay_parent);
					assert_eq!(s.unchecked_payload(), &CompactStatement::Seconded(candidate_hash));
					assert_eq!(s.unchecked_validator_index(), local_validator.validator_index);
				}
			);

			answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;
		}

		// Should drop requests from unknown peers.
		{
			let (pending_response, rx) = oneshot::channel();
			state
				.req_sender
				.send(RawIncomingRequest {
					// Request from peer that received manifest.
					peer: peer_d,
					payload: request_vstaging::AttestedCandidateRequest {
						candidate_hash: candidate.hash(),
						mask: mask.clone(),
					}
					.encode(),
					pending_response,
				})
				.await
				.unwrap();

			assert_matches!(rx.await, Err(oneshot::Canceled));
		}

		// Should drop requests with bitfields of the wrong size.
		{
			let mask = StatementFilter::blank(state.config.group_size + 1);
			let (pending_response, rx) = oneshot::channel();
			state
				.req_sender
				.send(RawIncomingRequest {
					// Request from peer that received manifest.
					peer: peer_c,
					payload: request_vstaging::AttestedCandidateRequest {
						candidate_hash: candidate.hash(),
						mask,
					}
					.encode(),
					pending_response,
				})
				.await
				.unwrap();

			assert_matches!(
				rx.await,
				Ok(RawOutgoingResponse {
					result,
					reputation_changes,
					sent_feedback
				}) => {
					assert_matches!(result, Err(()));
					assert_eq!(reputation_changes, vec![COST_INVALID_REQUEST_BITFIELD_SIZE.into_base_rep()]);
					assert_matches!(sent_feedback, None);
				}
			);
		}

		// Local node should reject requests if we did not send a manifest to that peer.
		{
			let (pending_response, rx) = oneshot::channel();
			state
				.req_sender
				.send(RawIncomingRequest {
					// Request from peer that received manifest.
					peer: peer_c,
					payload: request_vstaging::AttestedCandidateRequest {
						candidate_hash: candidate.hash(),
						mask: mask.clone(),
					}
					.encode(),
					pending_response,
				})
				.await
				.unwrap();

			// Should get `COST_UNEXPECTED_REQUEST` response.
			assert_matches!(
				rx.await,
				Ok(RawOutgoingResponse {
					result,
					reputation_changes,
					sent_feedback
				}) => {
					assert_matches!(result, Err(()));
					assert_eq!(reputation_changes, vec![COST_UNEXPECTED_REQUEST.into_base_rep()]);
					assert_matches!(sent_feedback, None);
				}
			);
		}

		overseer
	});
}

#[test]
fn local_node_respects_statement_mask() {
	let validator_count = 6;
	let group_size = 3;
	let config = TestConfig {
		validator_count,
		group_size,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();
	let peer_d = PeerId::random();

	test_harness(config, |mut state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			local_para,
			vec![1, 2, 3].into(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

		let test_leaf = TestLeaf {
			number: 1,
			hash: relay_parent,
			parent_hash: Hash::repeat_byte(0),
			session: 1,
			availability_cores: state.make_availability_cores(|i| {
				CoreState::Scheduled(ScheduledCore {
					para_id: ParaId::from(i as u32),
					collator: None,
				})
			}),
			para_data: (0..state.session_info.validator_groups.len())
				.map(|i| {
					(
						ParaId::from(i as u32),
						PerParaData { min_relay_parent: 1, head_data: vec![1, 2, 3].into() },
					)
				})
				.collect(),
		};

		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let target_group_validators =
			state.group_validators((local_validator.group_index.0 + 1).into(), true);
		let v_a = other_group_validators[0];
		let v_b = other_group_validators[1];
		let v_c = target_group_validators[0];
		let v_d = target_group_validators[1];

		// peer A is in group, has relay parent in view.
		// peer B is in group, has no relay parent in view.
		// peer C is not in group, has relay parent in view.
		// peer D is not in group, has no relay parent in view.
		{
			connect_peer(
				&mut overseer,
				peer_a.clone(),
				Some(vec![state.discovery_id(v_a)].into_iter().collect()),
			)
			.await;

			connect_peer(
				&mut overseer,
				peer_b.clone(),
				Some(vec![state.discovery_id(v_b)].into_iter().collect()),
			)
			.await;

			connect_peer(
				&mut overseer,
				peer_c.clone(),
				Some(vec![state.discovery_id(v_c)].into_iter().collect()),
			)
			.await;

			connect_peer(
				&mut overseer,
				peer_d.clone(),
				Some(vec![state.discovery_id(v_d)].into_iter().collect()),
			)
			.await;

			send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
			send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;
		}

		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		{
			let topology = NewGossipTopology {
				session: 1,
				topology: SessionGridTopology::new(
					(0..validator_count).collect(),
					(0..validator_count)
						.map(|i| TopologyPeerInfo {
							peer_ids: Vec::new(),
							validator_index: ValidatorIndex(i as u32),
							discovery_id: AuthorityDiscoveryPair::generate().0.public(),
						})
						.collect(),
				),
				local_index: Some(state.local.as_ref().unwrap().validator_index),
			};
			send_new_topology(&mut overseer, topology).await;
		}

		// Confirm the candidate locally so that we don't send out requests.
		{
			let statement = state
				.sign_full_statement(
					local_validator.validator_index,
					Statement::Seconded(candidate.clone()),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
					pvd.clone(),
				)
				.clone();

			overseer
				.send(FromOrchestra::Communication {
					msg: StatementDistributionMessage::Share(relay_parent, statement),
				})
				.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(peers, _)) if peers == vec![peer_a]
			);

			answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;
		}

		// Send enough statements to make candidate backable, make sure announcements are sent.

		// Send statement from peer A.
		{
			let statement = state
				.sign_statement(
					v_a,
					CompactStatement::Seconded(candidate_hash),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
				)
				.as_unchecked()
				.clone();

			send_peer_message(
				&mut overseer,
				peer_a.clone(),
				protocol_vstaging::StatementDistributionMessage::Statement(relay_parent, statement),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST => { }
			);
		}

		// Send statement from peer B.
		let statement_b = state
			.sign_statement(
				v_b,
				CompactStatement::Seconded(candidate_hash),
				&SigningContext { parent_hash: relay_parent, session_index: 1 },
			)
			.as_unchecked()
			.clone();
		{
			send_peer_message(
				&mut overseer,
				peer_b.clone(),
				protocol_vstaging::StatementDistributionMessage::Statement(
					relay_parent,
					statement_b.clone(),
				),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_b && r == BENEFIT_VALID_STATEMENT_FIRST => { }
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(peers, _)) if peers == vec![peer_a]
			);
		}

		// Send Backed notification.
		{
			overseer
				.send(FromOrchestra::Communication {
					msg: StatementDistributionMessage::Backed(candidate_hash),
				})
				.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages:: NetworkBridgeTx(
					NetworkBridgeTxMessage::SendValidationMessage(
						peers,
						Versioned::VStaging(
							protocol_vstaging::ValidationProtocol::StatementDistribution(
								protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(manifest),
							),
						),
					)
				) => {
					assert_eq!(peers, vec![peer_c]);
					assert_eq!(manifest, BackedCandidateManifest {
						relay_parent,
						candidate_hash,
						group_index: local_validator.group_index,
						para_id: local_para,
						parent_head_data_hash: pvd.parent_head.hash(),
						statement_knowledge: StatementFilter {
							seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 1],
							validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
						},
					});
				}
			);

			answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;
		}

		// `1` indicates statements NOT to request.
		let mask = StatementFilter {
			seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 0, 1],
			validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
		};

		// Incoming request to local node. Local node should send statements, respecting mask.
		{
			let (pending_response, rx) = oneshot::channel();
			state
				.req_sender
				.send(RawIncomingRequest {
					// Request from peer that received manifest.
					peer: peer_c,
					payload: request_vstaging::AttestedCandidateRequest {
						candidate_hash: candidate.hash(),
						mask,
					}
					.encode(),
					pending_response,
				})
				.await
				.unwrap();

			let expected_statements = vec![statement_b];
			assert_matches!(rx.await, Ok(full_response) => {
				// Response is the same for vstaging.
				let request_vstaging::AttestedCandidateResponse { candidate_receipt, persisted_validation_data, statements } =
					request_vstaging::AttestedCandidateResponse::decode(
						&mut full_response.result.expect("We should have a proper answer").as_ref(),
					).expect("Decoding should work");
				assert_eq!(candidate_receipt, candidate);
				assert_eq!(persisted_validation_data, pvd);
				assert_eq!(statements, expected_statements);
			});
		}

		overseer
	});
}
