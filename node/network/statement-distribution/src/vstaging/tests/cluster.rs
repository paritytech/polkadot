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

// TODO [now]: cluster 'valid' statement without prior seconded is ignored

// TODO [now]: statement with invalid signature leads to report

// TODO [now]: cluster statement from non-cluster peer is rejected

// TODO [now]: statement from non-cluster originator is rejected

// TODO [now]: cluster statement for unknown candidate leads to request

// TODO [now]: cluster statements are shared with `Seconded` first for all cluster peers
//             with relay-parent in view

// TODO [now]: cluster statements not re-shared on view update

// TODO [now]: cluster statements shared on first time cluster peer gets relay-parent in view.

// TODO [now]: confirmed cluster statement does not import statements until candidate in hypothetical frontier

// TODO [now]: shared valid statement after confirmation sent to all cluster peers with relay-parent
