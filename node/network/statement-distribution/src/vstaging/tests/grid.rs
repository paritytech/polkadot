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
use polkadot_node_network_protocol::{
	grid_topology::TopologyPeerInfo,
	vstaging::{BackedCandidateAcknowledgement, BackedCandidateManifest},
};
use polkadot_primitives_test_helpers::make_candidate;

// Backed candidate leads to advertisement to relevant validators with relay-parent.
#[test]
fn backed_candidate_leads_to_advertisement() {
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

	test_harness(config, |state, mut overseer| async move {
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
		{
			let statement = state
				.sign_statement(
					v_b,
					CompactStatement::Seconded(candidate_hash),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
				)
				.as_unchecked()
				.clone();

			send_peer_message(
				&mut overseer,
				peer_b.clone(),
				protocol_vstaging::StatementDistributionMessage::Statement(relay_parent, statement),
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

		overseer
	});
}

#[test]
fn received_advertisement_before_confirmation_leads_to_request() {
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

	test_harness(config, |state, mut overseer| async move {
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

		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let target_group_validators = state.group_validators(other_group, true);
		let v_a = other_group_validators[0];
		let v_b = other_group_validators[1];
		let v_c = target_group_validators[0];
		let v_d = target_group_validators[1];

		// peer A is in group, has relay parent in view.
		// peer B is in group, has no relay parent in view.
		// peer C is not in group, has relay parent in view.
		// peer D is not in group, has relay parent in view.
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
			send_peer_view_change(&mut overseer, peer_d.clone(), view![relay_parent]).await;
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

		// Receive an advertisement from C on an unconfirmed candidate.
		{
			let manifest = BackedCandidateManifest {
				relay_parent,
				candidate_hash,
				group_index: other_group,
				para_id: other_para,
				parent_head_data_hash: pvd.parent_head.hash(),
				statement_knowledge: StatementFilter {
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
				},
			};
			send_peer_message(
				&mut overseer,
				peer_c.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(manifest),
			)
			.await;

			let statements = vec![
				state
					.sign_statement(
						v_c,
						CompactStatement::Seconded(candidate_hash),
						&SigningContext { parent_hash: relay_parent, session_index: 1 },
					)
					.as_unchecked()
					.clone(),
				state
					.sign_statement(
						v_d,
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

			// C provided two statements we're seeing for the first time.
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT => { }
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT => { }
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_c && r == BENEFIT_VALID_RESPONSE => { }
			);

			answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;
		}

		overseer
	});
}

// 1. We receive manifest from grid peer, request, pass votes to backing, then receive Backed
// message. Only then should we send an acknowledgement to the grid peer.
//
// 2. (starting from end state of (1)) we receive a manifest about the same candidate from another
// grid peer and instantaneously acknowledge.
//
// Bit more context about this design choice: Statement-distribution doesn't fully emulate the
// statement logic of backing and only focuses on the number of statements. That means that we might
// request a manifest and for some reason the backing subsystem would still not consider the
// candidate as backed. So, in particular, we don't want to advertise such an unbacked candidate
// along the grid & increase load on ourselves and our peers for serving & importing such a
// candidate.
#[test]
fn received_advertisement_after_backing_leads_to_acknowledgement() {
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

	test_harness(config, |state, mut overseer| async move {
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
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
			},
		};

		let statement_c = state
			.sign_statement(
				v_c,
				CompactStatement::Seconded(candidate_hash),
				&SigningContext { parent_hash: relay_parent, session_index: 1 },
			)
			.as_unchecked()
			.clone();
		let statement_d = state
			.sign_statement(
				v_d,
				CompactStatement::Seconded(candidate_hash),
				&SigningContext { parent_hash: relay_parent, session_index: 1 },
			)
			.as_unchecked()
			.clone();

		// Receive an advertisement from C.
		{
			send_peer_message(
				&mut overseer,
				peer_c.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
					manifest.clone(),
				),
			)
			.await;

			// Should send a request to C.
			let statements = vec![
				statement_c.clone(),
				statement_d.clone(),
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

		// Receive Backed message.
		overseer
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::Backed(candidate_hash),
			})
			.await;

		// Should send an acknowledgement back to C.
		{
			assert_matches!(
				overseer.recv().await,
				AllMessages:: NetworkBridgeTx(
					NetworkBridgeTxMessage::SendValidationMessage(
						peers,
						Versioned::VStaging(
							protocol_vstaging::ValidationProtocol::StatementDistribution(
								protocol_vstaging::StatementDistributionMessage::BackedCandidateKnown(ack),
							),
						),
					)
				) => {
					assert_eq!(peers, vec![peer_c]);
					assert_eq!(ack, BackedCandidateAcknowledgement {
						candidate_hash,
						statement_knowledge: StatementFilter {
							seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 1],
							validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
						},
					});
				}
			);

			// TODO: Sends a statement back to C?
			assert_matches!(
				overseer.recv().await,
				AllMessages:: NetworkBridgeTx(
					NetworkBridgeTxMessage::SendValidationMessages(messages)
				) => {
					assert_eq!(messages.len(), 1);
					assert_eq!(messages[0].0, vec![peer_c]);

					assert_matches!(
						&messages[0].1,
						Versioned::VStaging(protocol_vstaging::ValidationProtocol::StatementDistribution(
							protocol_vstaging::StatementDistributionMessage::Statement(
								r,
								s,
							)
						)) if r == &relay_parent
							&& s.unchecked_payload() == &CompactStatement::Seconded(candidate_hash)
					);
				}
			);

			answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;
		}

		// Receive a manifest about the same candidate from peer D.
		{
			send_peer_message(
				&mut overseer,
				peer_d.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
					manifest.clone(),
				),
			)
			.await;

			let expected_ack = BackedCandidateAcknowledgement {
				candidate_hash,
				statement_knowledge: StatementFilter {
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 1],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
				},
			};

			// Instantaneously acknowledge.
			assert_matches!(
				overseer.recv().await,
				AllMessages:: NetworkBridgeTx(
					NetworkBridgeTxMessage::SendValidationMessages(messages)
				) => {
					assert_eq!(messages.len(), 2);
					assert_eq!(messages[0].0, vec![peer_d]);
					assert_eq!(messages[1].0, vec![peer_d]);

					assert_matches!(
						&messages[0].1,
						Versioned::VStaging(protocol_vstaging::ValidationProtocol::StatementDistribution(
							protocol_vstaging::StatementDistributionMessage::BackedCandidateKnown(ack)
						)) if *ack == expected_ack
					);

					assert_matches!(
						&messages[1].1,
						Versioned::VStaging(protocol_vstaging::ValidationProtocol::StatementDistribution(
							protocol_vstaging::StatementDistributionMessage::Statement(r, s)
						)) if *r == relay_parent && s.unchecked_payload() == &CompactStatement::Seconded(candidate_hash)
					);
				}
			);
		}

		overseer
	});
}

// Received advertisement after confirmation but before backing leads to nothing.
#[test]
fn received_advertisement_after_confirmation_before_backing() {
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

	test_harness(config, |state, mut overseer| async move {
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
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
			},
		};

		let statement_c = state
			.sign_statement(
				v_c,
				CompactStatement::Seconded(candidate_hash),
				&SigningContext { parent_hash: relay_parent, session_index: 1 },
			)
			.as_unchecked()
			.clone();
		let statement_d = state
			.sign_statement(
				v_d,
				CompactStatement::Seconded(candidate_hash),
				&SigningContext { parent_hash: relay_parent, session_index: 1 },
			)
			.as_unchecked()
			.clone();

		// Receive an advertisement from C.
		{
			send_peer_message(
				&mut overseer,
				peer_c.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
					manifest.clone(),
				),
			)
			.await;

			// Should send a request to C.
			let statements = vec![
				statement_c.clone(),
				statement_d.clone(),
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

		// Receive advertisement from peer D (after confirmation but before backing).
		{
			send_peer_message(
				&mut overseer,
				peer_d.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
					manifest.clone(),
				),
			)
			.await;
		}

		overseer
	});
}

// TODO [now]: additional statements are shared after manifest exchange

// TODO [now]: grid-sending validator view entering relay-parent leads to advertisement

// TODO [now]: advertisement not re-sent after re-entering relay parent (view oscillation)

// TODO [now]: acknowledgements sent only when candidate backed

// TODO [now]: grid statements imported to backing once candidate enters hypothetical frontier

#[test]
fn advertisements_rejected_from_incorrect_peers() {
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

	test_harness(config, |state, mut overseer| async move {
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

		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let target_group_validators = state.group_validators(other_group, true);
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
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
			},
		};

		// Receive an advertisement from A (our group).
		{
			send_peer_message(
				&mut overseer,
				peer_a.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
					manifest.clone(),
				),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == COST_UNEXPECTED_MANIFEST_DISALLOWED => { }
			);
		}

		// Receive an advertisement from B (our group).
		{
			send_peer_message(
				&mut overseer,
				peer_b.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(manifest),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_b && r == COST_UNEXPECTED_MANIFEST_DISALLOWED => { }
			);
		}

		overseer
	});
}

#[test]
fn manifest_rejected_with_unknown_relay_parent() {
	let validator_count = 6;
	let group_size = 3;
	let config = TestConfig {
		validator_count,
		group_size,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let unknown_parent = Hash::repeat_byte(2);
	let peer_c = PeerId::random();
	let peer_d = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();

		let other_group =
			next_group_index(local_validator.group_index, validator_count, group_size);
		let other_para = ParaId::from(other_group.0);

		let (candidate, pvd) = make_candidate(
			unknown_parent,
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

		// peer C is not in group, has relay parent in view.
		// peer D is not in group, has no relay parent in view.
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

			send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;
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
			relay_parent: unknown_parent,
			candidate_hash,
			group_index: other_group,
			para_id: other_para,
			parent_head_data_hash: pvd.parent_head.hash(),
			statement_knowledge: StatementFilter {
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
			},
		};

		// Receive an advertisement from C.
		{
			send_peer_message(
				&mut overseer,
				peer_c.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
					manifest.clone(),
				),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_c && r == COST_UNEXPECTED_MANIFEST_MISSING_KNOWLEDGE => { }
			);
		}

		overseer
	});
}

#[test]
fn manifest_rejected_when_not_a_validator() {
	let validator_count = 6;
	let group_size = 3;
	let config = TestConfig {
		validator_count,
		group_size,
		local_validator: false,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_c = PeerId::random();
	let peer_d = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let other_group = GroupIndex::from(0);
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

		// peer C is not in group, has relay parent in view.
		// peer D is not in group, has no relay parent in view.
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

			send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;
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
				local_index: None,
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
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
			},
		};

		// Receive an advertisement from C.
		{
			send_peer_message(
				&mut overseer,
				peer_c.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
					manifest.clone(),
				),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_c && r == COST_UNEXPECTED_MANIFEST_MISSING_KNOWLEDGE => { }
			);
		}

		overseer
	});
}

#[test]
fn manifest_rejected_when_group_does_not_match_para() {
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

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();

		let other_group =
			next_group_index(local_validator.group_index, validator_count, group_size);
		// Create a mismatch between group and para.
		let other_para = next_group_index(other_group, validator_count, group_size);
		let other_para = ParaId::from(other_para.0);

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

		// peer C is not in group, has relay parent in view.
		// peer D is not in group, has no relay parent in view.
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

			send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;
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
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
			},
		};

		// Receive an advertisement from C.
		{
			send_peer_message(
				&mut overseer,
				peer_c.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
					manifest.clone(),
				),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_c && r == COST_MALFORMED_MANIFEST => { }
			);
		}

		overseer
	});
}

#[test]
fn peer_reported_for_advertisement_conflicting_with_confirmed_candidate() {
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

	test_harness(config, |state, mut overseer| async move {
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

		let statement_c = state
			.sign_statement(
				v_c,
				CompactStatement::Seconded(candidate_hash),
				&SigningContext { parent_hash: relay_parent, session_index: 1 },
			)
			.as_unchecked()
			.clone();
		let statement_d = state
			.sign_statement(
				v_d,
				CompactStatement::Seconded(candidate_hash),
				&SigningContext { parent_hash: relay_parent, session_index: 1 },
			)
			.as_unchecked()
			.clone();

		// Receive an advertisement from C.
		{
			send_peer_message(
				&mut overseer,
				peer_c.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
					manifest.clone(),
				),
			)
			.await;

			// Should send a request to C.
			let statements = vec![
				statement_c.clone(),
				statement_d.clone(),
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

		// Receive conflicting advertisement from peer D after confirmation.
		//
		// TODO: This cause a conflict because we track received manifests on a per-validator basis,
		// and this is the first time we're getting a manifest from D.
		{
			let mut manifest = manifest.clone();
			manifest.statement_knowledge = StatementFilter {
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 0],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
			};
			send_peer_message(
				&mut overseer,
				peer_d.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(manifest),
			)
			.await;
		}

		dbg!(overseer.recv().await);

		overseer
	});
}
