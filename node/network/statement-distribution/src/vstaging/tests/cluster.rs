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
	let config = TestConfig { validator_count: 20, group_size: 3, local_validator: true };

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();

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
	let config = TestConfig { validator_count: 20, group_size: 3, local_validator: true };

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);
		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

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
		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];
		connect_peer(
			&mut overseer,
			peer_a.clone(),
			Some(vec![state.discovery_id(v_a)].into_iter().collect()),
		)
		.await;

		send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

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
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r)) => {
				assert_eq!(p, peer_a);
				assert_eq!(r, COST_UNEXPECTED_STATEMENT);
			}
		);

		overseer
	});
}

#[test]
fn cluster_statement_bad_signature() {
	let config = TestConfig { validator_count: 20, group_size: 3, local_validator: true };

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);
		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

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
		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

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
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == COST_INVALID_SIGNATURE => { },
				"{:?}",
				statement
			);
		}

		overseer
	});
}

#[test]
fn useful_cluster_statement_from_non_cluster_peer_rejected() {
	let config = TestConfig { validator_count: 20, group_size: 3, local_validator: true };

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);
		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

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
		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

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
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
				if p == peer_a && r == COST_UNEXPECTED_STATEMENT => { }
		);

		overseer
	});
}

#[test]
fn statement_from_non_cluster_originator_unexpected() {
	let config = TestConfig { validator_count: 20, group_size: 3, local_validator: true };

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);
		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

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

		// peer A is not in group, has relay parent in view.
		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];

		connect_peer(&mut overseer, peer_a.clone(), None).await;

		send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

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
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
				if p == peer_a && r == COST_UNEXPECTED_STATEMENT => { }
		);

		overseer
	});
}

#[test]
fn seconded_statement_leads_to_request() {
	let config = TestConfig { validator_count: 20, group_size: 3, local_validator: true };

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

	test_harness(config, |state, mut overseer| async move {
		let local_validator = state.local.clone().unwrap();
		let local_para = ParaId::from(local_validator.group_index.0);
		let candidate_hash = CandidateHash(Hash::repeat_byte(42));

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
		let other_group_validators = state.group_validators(local_validator.group_index, true);
		let v_a = other_group_validators[0];

		connect_peer(
			&mut overseer,
			peer_a.clone(),
			Some(vec![state.discovery_id(v_a)].into_iter().collect()),
		)
		.await;

		send_peer_view_change(&mut overseer, peer_a.clone(), view![relay_parent]).await;
		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

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
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
				if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST => { }
		);

		assert_matches!(
			overseer.recv().await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(requests, IfDisconnected::ImmediateError)) => {
				assert_eq!(requests.len(), 1);
				assert_matches!(
					&requests[0],
					Requests::AttestedCandidateV2(outgoing) => {
						assert_eq!(outgoing.peer, Recipient::Peer(peer_a.clone()));
						assert_eq!(outgoing.payload.candidate_hash, candidate_hash);
					}
				);
			}
		);

		overseer
	});
}

#[test]
fn cluster_statements_shared_seconded_first() {
	let config = TestConfig { validator_count: 20, group_size: 3, local_validator: true };

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();

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

		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

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
	let config = TestConfig { validator_count: 20, group_size: 3, local_validator: true };

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();

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

		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

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
		let next_test_leaf = TestLeaf {
			number: 2,
			hash: next_relay_parent,
			parent_hash: relay_parent,
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

		activate_leaf(&mut overseer, local_para, &next_test_leaf, &state, false).await;

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
	let config = TestConfig { validator_count: 20, group_size: 3, local_validator: true };

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();

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

		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

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
			protocol_vstaging::StatementDistributionMessage::Statement(relay_parent, a_seconded),
		)
		.await;

		assert_matches!(
			overseer.recv().await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
				if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST => { }
		);

		let req = assert_matches!(
			overseer.recv().await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(mut requests, IfDisconnected::ImmediateError)) => {
				assert_eq!(requests.len(), 1);
				assert_matches!(
					requests.pop().unwrap(),
					Requests::AttestedCandidateV2(mut outgoing) => {
						assert_eq!(outgoing.peer, Recipient::Peer(peer_a.clone()));
						assert_eq!(outgoing.payload.candidate_hash, candidate_hash);

						let res = AttestedCandidateResponse {
							candidate_receipt: candidate.clone(),
							persisted_validation_data: pvd.clone(),
							statements: vec![],
						};
						outgoing.pending_response.send(Ok(res.encode()));
					}
				);
			}
		);

		assert_matches!(
			overseer.recv().await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
				if p == peer_a && r == BENEFIT_VALID_RESPONSE => { }
		);

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
	let config = TestConfig { validator_count: 20, group_size: 3, local_validator: true };

	let relay_parent = Hash::repeat_byte(1);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();

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

		activate_leaf(&mut overseer, local_para, &test_leaf, &state, true).await;

		answer_expected_hypothetical_depth_request(
			&mut overseer,
			vec![],
			Some(relay_parent),
			false,
		)
		.await;

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
			protocol_vstaging::StatementDistributionMessage::Statement(relay_parent, a_seconded),
		)
		.await;

		assert_matches!(
			overseer.recv().await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
				if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST => { }
		);

		let req = assert_matches!(
			overseer.recv().await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(mut requests, IfDisconnected::ImmediateError)) => {
				assert_eq!(requests.len(), 1);
				assert_matches!(
					requests.pop().unwrap(),
					Requests::AttestedCandidateV2(mut outgoing) => {
						assert_eq!(outgoing.peer, Recipient::Peer(peer_a.clone()));
						assert_eq!(outgoing.payload.candidate_hash, candidate_hash);

						let res = AttestedCandidateResponse {
							candidate_receipt: candidate.clone(),
							persisted_validation_data: pvd.clone(),
							statements: vec![],
						};
						outgoing.pending_response.send(Ok(res.encode()));
					}
				);
			}
		);

		assert_matches!(
			overseer.recv().await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
				if p == peer_a && r == BENEFIT_VALID_RESPONSE => { }
		);

		answer_expected_hypothetical_depth_request(&mut overseer, vec![], None, false).await;

		let next_relay_parent = Hash::repeat_byte(2);
		let next_test_leaf = TestLeaf {
			number: 2,
			hash: next_relay_parent,
			parent_hash: relay_parent,
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

		activate_leaf(&mut overseer, local_para, &next_test_leaf, &state, false).await;

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
						 if c == &candidate && p == &pvd => {}
				);
				assert_eq!(s.validator_index(), v_a);
			}
		);

		overseer
	});
}

// TODO [now]: ensure seconding limit is respected
