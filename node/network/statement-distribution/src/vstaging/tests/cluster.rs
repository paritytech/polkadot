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

use polkadot_primitives_test_helpers::make_candidate;

#[test]
fn share_seconded_circulated_to_cluster() {
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

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

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

		// sharing a `Seconded` message confirms a candidate, which leads to new
		// fragment tree updates.
		answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;

		overseer
	});
}

#[test]
fn cluster_valid_statement_before_seconded_ignored() {
	let config = TestConfig {
		validator_count: 20,
		group_size: 3,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

		let test_leaf = state.make_dummy_leaf(relay_parent);

		// peer A is in group, has relay parent in view.
		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];
		connect_peer(
			&mut overseer,
			peer_a.clone(),
			Some(vec![state.discovery_id(v_a)].into_iter().collect()),
		)
		.await;

		send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		let signed_valid = state.sign_statement(
			v_a,
			CompactStatement::Valid(candidate_hash),
			&SigningContext { parent_hash: relay_parent, session_index: 1 },
		);

		send_peer_message(
			&mut overseer,
			peer_a.clone(),
			protocol_vstaging::StatementDistributionMessage::Statement(
				relay_parent,
				signed_valid.as_unchecked().clone(),
			),
		)
		.await;

		assert_matches!(
			overseer.recv().await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r))) => {
				assert_eq!(p, peer_a);
				assert_eq!(r, COST_UNEXPECTED_STATEMENT.into());
			}
		);

		overseer
	});
}

#[test]
fn cluster_statement_bad_signature() {
	let config = TestConfig {
		validator_count: 20,
		group_size: 3,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

		let test_leaf = state.make_dummy_leaf(relay_parent);

		// peer A is in group, has relay parent in view.
		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];
		let v_b = other_group_validators[1];

		connect_peer(
			&mut overseer,
			peer_a.clone(),
			Some(vec![state.discovery_id(v_a)].into_iter().collect()),
		)
		.await;

		send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		// sign statements with wrong signing context, leading to bad signature.
		let statements = vec![
			(v_a, CompactStatement::Seconded(candidate_hash)),
			(v_b, CompactStatement::Seconded(candidate_hash)),
		]
		.into_iter()
		.map(|(v, s)| {
			state.sign_statement(
				v,
				s,
				&SigningContext { parent_hash: Hash::repeat_byte(69), session_index: 1 },
			)
		})
		.map(|s| s.as_unchecked().clone());

		for statement in statements {
			send_peer_message(
				&mut overseer,
				peer_a.clone(),
				protocol_vstaging::StatementDistributionMessage::Statement(
					relay_parent,
					statement.clone(),
				),
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_a && r == COST_INVALID_SIGNATURE.into() => { },
				"{:?}",
				statement
			);
		}

		overseer
	});
}

#[test]
fn useful_cluster_statement_from_non_cluster_peer_rejected() {
	let config = TestConfig {
		validator_count: 20,
		group_size: 3,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

		let test_leaf = state.make_dummy_leaf(relay_parent);

		// peer A is not in group, has relay parent in view.
		let not_our_group =
			if local_validator.group_index.0 == 0 { GroupIndex(1) } else { GroupIndex(0) };

		let that_group_validators = state.group_validators(not_our_group, false);
		let v_non = that_group_validators[0];

		connect_peer(
			&mut overseer,
			peer_a.clone(),
			Some(vec![state.discovery_id(v_non)].into_iter().collect()),
		)
		.await;

		send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		let statement = state
			.sign_statement(
				v_non,
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
				if p == peer_a && r == COST_UNEXPECTED_STATEMENT.into() => { }
		);

		overseer
	});
}

#[test]
fn statement_from_non_cluster_originator_unexpected() {
	let config = TestConfig {
		validator_count: 20,
		group_size: 3,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

		let test_leaf = state.make_dummy_leaf(relay_parent);

		// peer A is not in group, has relay parent in view.
		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];

		connect_peer(&mut overseer, peer_a.clone(), None).await;

		send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

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
				if p == peer_a && r == COST_UNEXPECTED_STATEMENT.into() => { }
		);

		overseer
	});
}

#[test]
fn seconded_statement_leads_to_request() {
	let group_size = 3;
	let config = TestConfig {
		validator_count: 20,
		group_size,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

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

		// peer A is in group, has relay parent in view.
		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];

		connect_peer(
			&mut overseer,
			peer_a.clone(),
			Some(vec![state.discovery_id(v_a)].into_iter().collect()),
		)
		.await;

		send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

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

		handle_sent_request(
			&mut overseer,
			peer_a,
			candidate_hash,
			StatementFilter::blank(group_size),
			candidate.clone(),
			pvd.clone(),
			vec![],
		)
		.await;

		assert_matches!(
			overseer.recv().await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
				if p == peer_a && r == BENEFIT_VALID_RESPONSE.into() => { }
		);

		answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;

		overseer
	});
}

#[test]
fn cluster_statements_shared_seconded_first() {
	let config = TestConfig {
		validator_count: 20,
		group_size: 3,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

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

		// peer A is in group, no relay parent in view.
		{
			let other_group_validators = state.group_validators(local_validator.group_index, true);

			connect_peer(
				&mut overseer,
				peer_a.clone(),
				Some(vec![state.discovery_id(other_group_validators[0])].into_iter().collect()),
			)
			.await;
		}

		activate_leaf(&mut overseer, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

		let full_signed = state
			.sign_statement(
				local_validator.validator_index,
				CompactStatement::Seconded(candidate_hash),
				&SigningContext { session_index: 1, parent_hash: relay_parent },
			)
			.convert_to_superpayload(StatementWithPVD::Seconded(candidate.clone(), pvd.clone()))
			.unwrap();

		let valid_signed = state
			.sign_statement(
				local_validator.validator_index,
				CompactStatement::Valid(candidate_hash),
				&SigningContext { session_index: 1, parent_hash: relay_parent },
			)
			.convert_to_superpayload(StatementWithPVD::Valid(candidate_hash))
			.unwrap();

		overseer
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::Share(relay_parent, full_signed),
			})
			.await;

		// result of new confirmed candidate.
		answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;

		overseer
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::Share(relay_parent, valid_signed),
			})
			.await;

		send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;

		assert_matches!(
			overseer.recv().await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessages(messages)) => {
				assert_eq!(messages.len(), 2);

				assert_eq!(messages[0].0, vec![peer_a]);
				assert_eq!(messages[1].0, vec![peer_a]);

				assert_matches!(
					&messages[0].1,
					Versioned::VStaging(protocol_vstaging::ValidationProtocol::StatementDistribution(
						protocol_vstaging::StatementDistributionMessage::Statement(
							r,
							s,
						)
					)) if r == &relay_parent
						&& s.unchecked_payload() == &CompactStatement::Seconded(candidate_hash) => {}
				);

				assert_matches!(
					&messages[1].1,
					Versioned::VStaging(protocol_vstaging::ValidationProtocol::StatementDistribution(
						protocol_vstaging::StatementDistributionMessage::Statement(
							r,
							s,
						)
					)) if r == &relay_parent
						&& s.unchecked_payload() == &CompactStatement::Valid(candidate_hash) => {}
				);
			}
		);

		overseer
	});
}

#[test]
fn cluster_accounts_for_implicit_view() {
	let config = TestConfig {
		validator_count: 20,
		group_size: 3,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();

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

		// peer A is in group, has relay parent in view.
		// peer B is in group, has no relay parent in view.
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

		// sharing a `Seconded` message confirms a candidate, which leads to new
		// fragment tree updates.
		answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;

		// activate new leaf, which has relay-parent in implicit view.
		let next_relay_parent = Hash::repeat_byte(2);
		let mut next_test_leaf = state.make_dummy_leaf(next_relay_parent);
		next_test_leaf.parent_hash = relay_parent;
		next_test_leaf.number = 2;

		activate_leaf(&mut overseer, &next_test_leaf, &state, false).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(next_relay_parent),
			false,
		)
		.await;

		send_peer_view_change(&mut overseer, peer_a.clone(), view![next_relay_parent]).await;
		send_peer_view_change(&mut overseer, peer_b.clone(), view![next_relay_parent]).await;

		// peer B never had the relay parent in its view, so this tests that
		// the implicit view is working correctly for B.
		//
		// the fact that the statement isn't sent again to A also indicates that it works
		// it's working.
		assert_matches!(
			overseer.recv().await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessages(messages)) => {
				assert_eq!(messages.len(), 1);
				assert_matches!(
					&messages[0],
					(
						peers,
						Versioned::VStaging(protocol_vstaging::ValidationProtocol::StatementDistribution(
							protocol_vstaging::StatementDistributionMessage::Statement(
								r,
								s,
							)
						))
					) => {
						assert_eq!(peers, &vec![peer_b.clone()]);
						assert_eq!(r, &relay_parent);
						assert_eq!(s.unchecked_payload(), &CompactStatement::Seconded(candidate_hash));
						assert_eq!(s.unchecked_validator_index(), local_validator.validator_index);
					}
				)
			}
		);

		overseer
	});
}

#[test]
fn cluster_messages_imported_after_confirmed_candidate_importable_check() {
	let group_size = 3;
	let config = TestConfig {
		validator_count: 20,
		group_size,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

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

		// peer A is in group, has relay parent in view.
		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];
		{
			connect_peer(
				&mut overseer,
				peer_a.clone(),
				Some(vec![state.discovery_id(v_a)].into_iter().collect()),
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

		// Peer sends `Seconded` statement.
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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST.into() => { }
			);
		}

		// Send a request to peer and mock its response.
		{
			handle_sent_request(
				&mut overseer,
				peer_a,
				candidate_hash,
				StatementFilter::blank(group_size),
				candidate.clone(),
				pvd.clone(),
				vec![],
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_a && r == BENEFIT_VALID_RESPONSE.into()
			);
		}

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![(
				HypotheticalCandidate::Complete {
					candidate_hash,
					receipt: Arc::new(candidate.clone()),
					persisted_validation_data: pvd.clone(),
				},
				vec![(relay_parent, vec![0])],
			)],
			None,
			false,
		)
		.await;

		assert_matches!(
			overseer.recv().await,
			AllMessages::CandidateBacking(CandidateBackingMessage::Statement(
				r,
				s,
			)) if r == relay_parent => {
				assert_matches!(
					s.payload(),
					FullStatementWithPVD::Seconded(c, p)
						 if c == &candidate && p == &pvd => {}
				);
				assert_eq!(s.validator_index(), v_a);
			}
		);

		overseer
	});
}

#[test]
fn cluster_messages_imported_after_new_leaf_importable_check() {
	let group_size = 3;
	let config = TestConfig {
		validator_count: 20,
		group_size,
		local_validator: true,
		async_backing_params: None,
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

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

		// peer A is in group, has relay parent in view.
		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];
		{
			connect_peer(
				&mut overseer,
				peer_a.clone(),
				Some(vec![state.discovery_id(v_a)].into_iter().collect()),
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

		// Peer sends `Seconded` statement.
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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST.into() => { }
			);
		}

		// Send a request to peer and mock its response.
		{
			handle_sent_request(
				&mut overseer,
				peer_a,
				candidate_hash,
				StatementFilter::blank(group_size),
				candidate.clone(),
				pvd.clone(),
				vec![],
			)
			.await;

			assert_matches!(
				overseer.recv().await,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(p, r)))
					if p == peer_a && r == BENEFIT_VALID_RESPONSE.into() => { }
			);
		}

		answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;

		let next_relay_parent = Hash::repeat_byte(2);
		let mut next_test_leaf = state.make_dummy_leaf(next_relay_parent);
		next_test_leaf.parent_hash = relay_parent;
		next_test_leaf.number = 2;

		activate_leaf(&mut overseer, &next_test_leaf, &state, false).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![(
				HypotheticalCandidate::Complete {
					candidate_hash,
					receipt: Arc::new(candidate.clone()),
					persisted_validation_data: pvd.clone(),
				},
				vec![(relay_parent, vec![0])],
			)],
			Some(next_relay_parent),
			false,
		)
		.await;

		assert_matches!(
			overseer.recv().await,
			AllMessages::CandidateBacking(CandidateBackingMessage::Statement(
				r,
				s,
			)) if r == relay_parent => {
				assert_matches!(
					s.payload(),
					FullStatementWithPVD::Seconded(c, p)
						 if c == &candidate && p == &pvd
				);
				assert_eq!(s.validator_index(), v_a);
			}
		);

		overseer
	});
}

#[test]
fn ensure_seconding_limit_is_respected() {
	// `max_candidate_depth: 1` for a `seconding_limit` of 2.
	let config = TestConfig {
		validator_count: 20,
		group_size: 4,
		local_validator: true,
		async_backing_params: Some(AsyncBackingParams {
			max_candidate_depth: 1,
			allowed_ancestry_len: 3,
		}),
	};

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);

		let test_leaf = state.make_dummy_leaf(relay_parent);

		let (candidate_1, pvd_1) = make_candidate(
			relay_parent,
			1,
			local_para,
			test_leaf.para_data(local_para).head_data.clone(),
			vec![4, 5, 6].into(),
			Hash::repeat_byte(42).into(),
		);
		let (candidate_2, pvd_2) = make_candidate(
			relay_parent,
			1,
			local_para,
			test_leaf.para_data(local_para).head_data.clone(),
			vec![7, 8, 9].into(),
			Hash::repeat_byte(43).into(),
		);
		let (candidate_3, _pvd_3) = make_candidate(
			relay_parent,
			1,
			local_para,
			test_leaf.para_data(local_para).head_data.clone(),
			vec![10, 11, 12].into(),
			Hash::repeat_byte(44).into(),
		);
		let candidate_hash_1 = candidate_1.hash();
		let candidate_hash_2 = candidate_2.hash();
		let candidate_hash_3 = candidate_3.hash();

		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];

		// peers A,B,C are in group, have relay parent in view.
		{
			connect_peer(
				&mut overseer,
				peer_a.clone(),
				Some(vec![state.discovery_id(v_a)].into_iter().collect()),
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

		// Confirm the candidates locally so that we don't send out requests.

		// Candidate 1.
		{
			let validator_index = state.local.as_ref().unwrap().validator_index;
			let statement = state
				.sign_full_statement(
					validator_index,
					Statement::Seconded(candidate_1),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
					pvd_1,
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

		// Candidate 2.
		{
			let validator_index = state.local.as_ref().unwrap().validator_index;
			let statement = state
				.sign_full_statement(
					validator_index,
					Statement::Seconded(candidate_2),
					&SigningContext { parent_hash: relay_parent, session_index: 1 },
					pvd_2,
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

		// Send first statement from peer A.
		{
			let statement = state
				.sign_statement(
					v_a,
					CompactStatement::Seconded(candidate_hash_1),
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

		// Send second statement from peer A.
		{
			let statement = state
				.sign_statement(
					v_a,
					CompactStatement::Seconded(candidate_hash_2),
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

		// Send third statement from peer A.
		{
			let statement = state
				.sign_statement(
					v_a,
					CompactStatement::Seconded(candidate_hash_3),
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
					if p == peer_a && r == COST_EXCESSIVE_SECONDED.into() => { }
			);
		}

		overseer
	});
}
