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
	grid_topology::TopologyPeerInfo, vstaging::BackedCandidateManifest,
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
			// send_peer_view_change(&mut overseer, peer_b.clone(), view![relay_parent]).await;
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

		// Candidate.
		{
			let validator_index = state.local.as_ref().unwrap().validator_index;
			let statement = state
				.sign_full_statement(
					validator_index,
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

		// Share local statement.
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
		}

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

// TODO [now]: received advertisement before confirmation leads to request

// TODO [now]: received advertisement after backing leads to acknowledgement

// TODO [now]: received advertisement after confirmation but before backing leads to nothing

// TODO [now]: additional statements are shared after manifest exchange

// TODO [now]: grid-sending validator view entering relay-parent leads to advertisement

// TODO [now]: advertisement not re-sent after re-entering relay parent (view oscillation)

// TODO [now]: acknowledgements sent only when candidate backed

// TODO [now]: grid statements imported to backing once candidate enters hypothetical frontier

// TODO [now]: advertisements rejected from incorrect peers

// TODO [now]: manifests rejected with unknown relay parent or when not a validator

// TODO [now]: advertisements rejected when candidate group doers not match para

// TODO [now]: peer reported when advertisement conflicting with confirmed candidate.
