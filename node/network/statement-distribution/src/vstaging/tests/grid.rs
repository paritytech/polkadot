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
use polkadot_node_network_protocol::vstaging::{
	BackedCandidateAcknowledgement, BackedCandidateManifest,
};
use polkadot_node_subsystem::messages::CandidateBackingMessage;
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			local_para,
			test_leaf.para_data(local_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST.into() => { }
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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_b && r == BENEFIT_VALID_STATEMENT_FIRST.into() => { }
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			other_para,
			test_leaf.para_data(other_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

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
			handle_sent_request(
				&mut overseer,
				peer_c,
				candidate_hash,
				StatementFilter::blank(group_size),
				candidate.clone(),
				pvd.clone(),
				statements,
			)
			.await;

			// C provided two statements we're seeing for the first time.
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into() => { }
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into() => { }
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_RESPONSE.into() => { }
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			other_para,
			test_leaf.para_data(other_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

		let manifest = BackedCandidateManifest {
			relay_parent,
			candidate_hash,
			group_index: other_group,
			para_id: other_para,
			parent_head_data_hash: pvd.parent_head.hash(),
			statement_knowledge: StatementFilter {
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 1],
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
			handle_sent_request(
				&mut overseer,
				peer_c,
				candidate_hash,
				StatementFilter::blank(group_size),
				candidate.clone(),
				pvd.clone(),
				statements,
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_RESPONSE.into()
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
					assert_eq!(messages.len(), 1);
					assert_eq!(messages[0].0, vec![peer_d]);

					assert_matches!(
						&messages[0].1,
						Versioned::VStaging(protocol_vstaging::ValidationProtocol::StatementDistribution(
							protocol_vstaging::StatementDistributionMessage::BackedCandidateKnown(ack)
						)) if *ack == expected_ack
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			other_para,
			test_leaf.para_data(other_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

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
			handle_sent_request(
				&mut overseer,
				peer_c,
				candidate_hash,
				StatementFilter::blank(group_size),
				candidate.clone(),
				pvd.clone(),
				statements,
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_RESPONSE.into()
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

#[test]
fn additional_statements_are_shared_after_manifest_exchange() {
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			other_para,
			test_leaf.para_data(other_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

		// Receive an advertisement from C.
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
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
					manifest.clone(),
				),
			)
			.await;
		}

		// Should send a request to C.
		{
			let statements = vec![
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

			handle_sent_request(
				&mut overseer,
				peer_c,
				candidate_hash,
				StatementFilter::blank(group_size),
				candidate.clone(),
				pvd.clone(),
				statements,
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_RESPONSE.into()
			);
		}

		let hypothetical = HypotheticalCandidate::Complete {
			candidate_hash,
			receipt: Arc::new(candidate.clone()),
			persisted_validation_data: pvd.clone(),
		};
		let membership = vec![(relay_parent, vec![0])];
		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![(hypothetical, membership)],
			None,
			false,
		)
		.await;

		// Statements are sent to the Backing subsystem.
		{
			assert_matches!(
				overseer.recv().await,
				AllMessages::CandidateBacking(
					CandidateBackingMessage::Statement(hash, statement)
				) => {
					assert_eq!(hash, relay_parent);
					assert_matches!(
						statement.payload(),
						FullStatementWithPVD::Seconded(c, p)
							if c == &candidate && p == &pvd
					);
				}
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::CandidateBacking(
					CandidateBackingMessage::Statement(hash, statement)
				) => {
					assert_eq!(hash, relay_parent);
					assert_matches!(
						statement.payload(),
						FullStatementWithPVD::Seconded(c, p)
							if c == &candidate && p == &pvd
					);
				}
			);
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
							seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
							validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
						},
					});
				}
			);

			answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;
		}

		// Receive a manifest about the same candidate from peer D. Contains different statements.
		{
			let manifest = BackedCandidateManifest {
				relay_parent,
				candidate_hash,
				group_index: other_group,
				para_id: other_para,
				parent_head_data_hash: pvd.parent_head.hash(),
				statement_knowledge: StatementFilter {
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 0],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
				},
			};

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
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 0, 1, 1],
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
						)) if *r == relay_parent && s.unchecked_payload() == &CompactStatement::Seconded(candidate_hash) && s.unchecked_validator_index() == v_e
					);
				}
			);
		}

		overseer
	});
}

// Grid-sending validator view entering relay-parent leads to advertisement.
#[test]
fn advertisement_sent_when_peer_enters_relay_parent_view() {
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			local_para,
			test_leaf.para_data(local_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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
		}

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST.into() => { }
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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_b && r == BENEFIT_VALID_STATEMENT_FIRST.into() => { }
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(peers, _)) if peers == vec![peer_a]
			);
		}

		// Send Backed notification.
		overseer
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::Backed(candidate_hash),
			})
			.await;

		answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;

		// Relay parent enters view of peer C.
		{
			send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;

			let expected_manifest = BackedCandidateManifest {
				relay_parent,
				candidate_hash,
				group_index: local_validator.group_index,
				para_id: local_para,
				parent_head_data_hash: pvd.parent_head.hash(),
				statement_knowledge: StatementFilter {
					seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 1],
					validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
				},
			};

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
							protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(manifest)
						)) => {
							assert_eq!(*manifest, expected_manifest);
						}
					);
				}
			);
		}

		overseer
	});
}

// Advertisement not re-sent after re-entering relay parent (view oscillation).
#[test]
fn advertisement_not_re_sent_when_peer_re_enters_view() {
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			local_para,
			test_leaf.para_data(local_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST.into() => { }
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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_b && r == BENEFIT_VALID_STATEMENT_FIRST.into() => { }
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

		// Peer leaves view.
		send_peer_view_change(&mut overseer, peer_c.clone(), view![]).await;

		// Peer re-enters view.
		send_peer_view_change(&mut overseer, peer_c.clone(), view![relay_parent]).await;

		overseer
	});
}

// Grid statements imported to backing once candidate enters hypothetical frontier.
#[test]
fn grid_statements_imported_to_backing() {
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			other_para,
			test_leaf.para_data(other_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

		// Receive an advertisement from C.
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
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(
					manifest.clone(),
				),
			)
			.await;
		}

		// Should send a request to C.
		{
			let statements = vec![
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

			handle_sent_request(
				&mut overseer,
				peer_c,
				candidate_hash,
				StatementFilter::blank(group_size),
				candidate.clone(),
				pvd.clone(),
				statements,
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_RESPONSE.into()
			);
		}

		let hypothetical = HypotheticalCandidate::Complete {
			candidate_hash,
			receipt: Arc::new(candidate.clone()),
			persisted_validation_data: pvd.clone(),
		};
		let membership = vec![(relay_parent, vec![0])];
		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![(hypothetical, membership)],
			None,
			false,
		)
		.await;

		// Receive messages from Backing subsystem.
		{
			assert_matches!(
				overseer.recv().await,
				AllMessages::CandidateBacking(
					CandidateBackingMessage::Statement(hash, statement)
				) => {
					assert_eq!(hash, relay_parent);
					assert_matches!(
						statement.payload(),
						FullStatementWithPVD::Seconded(c, p)
							if c == &candidate && p == &pvd
					);
				}
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::CandidateBacking(
					CandidateBackingMessage::Statement(hash, statement)
				) => {
					assert_eq!(hash, relay_parent);
					assert_matches!(
						statement.payload(),
						FullStatementWithPVD::Seconded(c, p)
							if c == &candidate && p == &pvd
					);
				}
			);
		}

		overseer
	});
}

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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			other_para,
			test_leaf.para_data(other_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_a && r == COST_UNEXPECTED_MANIFEST_DISALLOWED.into() => { }
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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_b && r == COST_UNEXPECTED_MANIFEST_DISALLOWED.into() => { }
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			unknown_parent,
			1,
			other_para,
			test_leaf.para_data(other_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == COST_UNEXPECTED_MANIFEST_MISSING_KNOWLEDGE.into() => { }
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			other_para,
			test_leaf.para_data(other_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == COST_UNEXPECTED_MANIFEST_MISSING_KNOWLEDGE.into() => { }
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			other_para,
			test_leaf.para_data(other_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == COST_MALFORMED_MANIFEST.into() => { }
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

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate, pvd) = make_candidate(
			relay_parent,
			1,
			other_para,
			test_leaf.para_data(other_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let candidate_hash = candidate.hash();

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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// Send gossip topology.
		send_new_topology(&mut overseer, state.make_dummy_topology()).await;

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

			handle_sent_request(
				&mut overseer,
				peer_c,
				candidate_hash,
				StatementFilter::blank(group_size),
				candidate.clone(),
				pvd.clone(),
				statements,
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);
			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_STATEMENT.into()
			);

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == BENEFIT_VALID_RESPONSE.into()
			);

			answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;
		}

		// Receive conflicting advertisement from peer C after confirmation.
		//
		// NOTE: This causes a conflict because we track received manifests on a per-validator
		// basis, and this is the second time we're getting a manifest from C.
		{
			let mut manifest = manifest.clone();
			manifest.statement_knowledge = StatementFilter {
				seconded_in_group: bitvec::bitvec![u8, Lsb0; 1, 1, 0],
				validated_in_group: bitvec::bitvec![u8, Lsb0; 0, 0, 0],
			};
			send_peer_message(
				&mut overseer,
				peer_c.clone(),
				protocol_vstaging::StatementDistributionMessage::BackedCandidateManifest(manifest),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_c && r == COST_CONFLICTING_MANIFEST.into()
			);
		}

		overseer
	});
}
