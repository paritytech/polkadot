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

use parity_scale_codec::{Decode, Encode};
use polkadot_node_network_protocol::request_response::vstaging as request_vstaging;
use polkadot_primitives_test_helpers::make_candidate;
use sc_network::config::IncomingRequest as RawIncomingRequest;

// TODO [now]: peer reported for providing statements meant to be masked out

// TODO [now]: peer reported for not providing enough statements, request retried

#[test]
fn peer_reported_for_duplicate_statements() {
	let config = TestConfig { validator_count: 20, group_size: 3, local_validator: true };

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

		// TODO: Get the candidate confirmed.

		// TODO: Send a manifest to the requesting peer.

		// Send request for candidates.
		{
			let mask = StatementFilter::blank(state.config.group_size);
			let (pending_response, rx) = oneshot::channel();
			state
				.candidate_req_cfg
				.inbound_queue
				.as_mut()
				.unwrap()
				.send(RawIncomingRequest {
					// TODO: Request from peer that received manifest.
					peer: peer_id,
					payload: request_vstaging::AttestedCandidateRequest {
						candidate_hash: candidate.hash(),
						mask,
					}
					.encode(),
					pending_response,
				})
				.await
				.unwrap();

			let expected_statements = vec![];
			assert_matches!(
				rx.await,
				Ok(full_response) => {
					// Response is the same for vstaging.
					let request_vstaging::AttestedCandidateResponse{ candidate_receipt, persisted_validation_data,  statements }
						= request_vstaging::AttestedCandidateResponse::decode(
							&mut full_response.result
							.expect("We should have a proper answer").as_ref()
					)
					.expect("Decoding should work");
					assert_eq!(candidate_receipt, candidate);
					assert_eq!(persisted_validation_data, pvd);
					assert_eq!(statements, expected_statements);
				}
			);
		}

		overseer
	});

	todo!()
}

// TODO [now]: peer reported for providing statements with invalid signatures or wrong validator IDs

// TODO [now]: local node sanity checks incoming requests

// TODO [now]: local node respects statement mask
