// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Tests for the collator side with enabled prospective parachains.

use super::*;

use polkadot_node_subsystem::messages::{ChainApiMessage, ProspectiveParachainsMessage};
use polkadot_primitives::{vstaging as vstaging_primitives, Header, OccupiedCore};

const ASYNC_BACKING_PARAMETERS: vstaging_primitives::AsyncBackingParams =
	vstaging_primitives::AsyncBackingParams { max_candidate_depth: 4, allowed_ancestry_len: 3 };

fn get_parent_hash(hash: Hash) -> Hash {
	Hash::from_low_u64_be(hash.to_low_u64_be() + 1)
}

/// Handle a view update.
async fn update_view(
	virtual_overseer: &mut VirtualOverseer,
	test_state: &TestState,
	new_view: Vec<(Hash, u32)>, // Hash and block number.
	activated: u8,              // How many new heads does this update contain?
) {
	let new_view: HashMap<Hash, u32> = HashMap::from_iter(new_view);

	let our_view =
		OurView::new(new_view.keys().map(|hash| (*hash, Arc::new(jaeger::Span::Disabled))), 0);

	overseer_send(
		virtual_overseer,
		CollatorProtocolMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(our_view)),
	)
	.await;

	let mut next_overseer_message = None;
	for _ in 0..activated {
		let (leaf_hash, leaf_number) = assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				parent,
				RuntimeApiRequest::StagingAsyncBackingParams(tx),
			)) => {
				tx.send(Ok(ASYNC_BACKING_PARAMETERS)).unwrap();
				(parent, new_view.get(&parent).copied().expect("Unknown parent requested"))
			}
		);

		let min_number = leaf_number.saturating_sub(ASYNC_BACKING_PARAMETERS.allowed_ancestry_len);

		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::GetMinimumRelayParents(parent, tx),
			) if parent == leaf_hash => {
				tx.send(vec![(test_state.para_id, min_number)]).unwrap();
			}
		);

		let ancestry_len = leaf_number + 1 - min_number;
		let ancestry_hashes = std::iter::successors(Some(leaf_hash), |h| Some(get_parent_hash(*h)))
			.take(ancestry_len as usize);
		let ancestry_numbers = (min_number..=leaf_number).rev();
		let mut ancestry_iter = ancestry_hashes.clone().zip(ancestry_numbers).peekable();

		while let Some((hash, number)) = ancestry_iter.next() {
			// May be `None` for the last element.
			let parent_hash =
				ancestry_iter.peek().map(|(h, _)| *h).unwrap_or_else(|| get_parent_hash(hash));

			let msg = match next_overseer_message.take() {
				Some(msg) => Some(msg),
				None =>
					overseer_recv_with_timeout(virtual_overseer, Duration::from_millis(50)).await,
			};

			let msg = match msg {
				Some(msg) => msg,
				None => {
					// We're done.
					return
				},
			};

			if !matches!(
				&msg,
				AllMessages::ChainApi(ChainApiMessage::BlockHeader(_hash, ..))
					if *_hash == hash
			) {
				// Ancestry has already been cached for this leaf.
				next_overseer_message.replace(msg);
				break
			}

			assert_matches!(
				msg,
				AllMessages::ChainApi(ChainApiMessage::BlockHeader(.., tx)) => {
					let header = Header {
						parent_hash,
						number,
						state_root: Hash::zero(),
						extrinsics_root: Hash::zero(),
						digest: Default::default(),
					};

					tx.send(Ok(Some(header))).unwrap();
				}
			);
		}
	}
}

/// Check that the next received message is a `Declare` message.
pub(super) async fn expect_declare_msg_vstaging(
	virtual_overseer: &mut VirtualOverseer,
	test_state: &TestState,
	peer: &PeerId,
) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendCollationMessage(
			to,
			Versioned::VStaging(protocol_vstaging::CollationProtocol::CollatorProtocol(
				wire_message,
			)),
		)) => {
			assert_eq!(to[0], *peer);
			assert_matches!(
				wire_message,
				protocol_vstaging::CollatorProtocolMessage::Declare(
					collator_id,
					para_id,
					signature,
				) => {
					assert!(signature.verify(
						&*protocol_vstaging::declare_signature_payload(&test_state.local_peer_id),
						&collator_id),
					);
					assert_eq!(collator_id, test_state.collator_pair.public());
					assert_eq!(para_id, test_state.para_id);
				}
			);
		}
	);
}

/// Test that a collator distributes a collation from the allowed ancestry
/// to correct validators group.
#[test]
fn distribute_collation_from_implicit_view() {
	let head_a = Hash::from_low_u64_be(126);
	let head_a_num: u32 = 66;

	// Grandparent of head `a`.
	let head_b = Hash::from_low_u64_be(128);
	let head_b_num: u32 = 64;

	// Grandparent of head `b`.
	let head_c = Hash::from_low_u64_be(130);
	let head_c_num = 62;

	let group_rotation_info = GroupRotationInfo {
		session_start_block: head_c_num - 2,
		group_rotation_frequency: 3,
		now: head_c_num,
	};

	let mut test_state = TestState::default();
	test_state.group_rotation_info = group_rotation_info;

	let local_peer_id = test_state.local_peer_id;
	let collator_pair = test_state.collator_pair.clone();

	test_harness(
		local_peer_id,
		collator_pair,
		ReputationAggregator::new(|_| true),
		|mut test_harness| async move {
			let virtual_overseer = &mut test_harness.virtual_overseer;

			// Set collating para id.
			overseer_send(virtual_overseer, CollatorProtocolMessage::CollateOn(test_state.para_id))
				.await;
			// Activated leaf is `b`, but the collation will be based on `c`.
			update_view(virtual_overseer, &test_state, vec![(head_b, head_b_num)], 1).await;

			let validator_peer_ids = test_state.current_group_validator_peer_ids();
			for (val, peer) in test_state
				.current_group_validator_authority_ids()
				.into_iter()
				.zip(validator_peer_ids.clone())
			{
				connect_peer(virtual_overseer, peer, CollationVersion::VStaging, Some(val.clone()))
					.await;
			}

			// Collator declared itself to each peer.
			for peer_id in &validator_peer_ids {
				expect_declare_msg_vstaging(virtual_overseer, &test_state, peer_id).await;
			}

			let pov = PoV { block_data: BlockData(vec![1, 2, 3]) };
			let parent_head_data_hash = Hash::repeat_byte(0xAA);
			let candidate = TestCandidateBuilder {
				para_id: test_state.para_id,
				relay_parent: head_c,
				pov_hash: pov.hash(),
				..Default::default()
			}
			.build();
			let DistributeCollation { candidate, pov_block: _ } =
				distribute_collation_with_receipt(
					virtual_overseer,
					&test_state,
					head_c,
					false, // Check the group manually.
					candidate,
					pov,
					parent_head_data_hash,
				)
				.await;
			assert_matches!(
				overseer_recv(virtual_overseer).await,
				AllMessages::NetworkBridgeTx(
					NetworkBridgeTxMessage::ConnectToValidators { validator_ids, .. }
				) => {
					let expected_validators = test_state.current_group_validator_authority_ids();

					assert_eq!(expected_validators, validator_ids);
				}
			);

			let candidate_hash = candidate.hash();

			// Update peer views.
			for peed_id in &validator_peer_ids {
				send_peer_view_change(virtual_overseer, peed_id, vec![head_b]).await;
				expect_advertise_collation_msg(
					virtual_overseer,
					peed_id,
					head_c,
					Some(vec![candidate_hash]),
				)
				.await;
			}

			// Head `c` goes out of view.
			// Build a different candidate for this relay parent and attempt to distribute it.
			update_view(virtual_overseer, &test_state, vec![(head_a, head_a_num)], 1).await;

			let pov = PoV { block_data: BlockData(vec![4, 5, 6]) };
			let parent_head_data_hash = Hash::repeat_byte(0xBB);
			let candidate = TestCandidateBuilder {
				para_id: test_state.para_id,
				relay_parent: head_c,
				pov_hash: pov.hash(),
				..Default::default()
			}
			.build();
			overseer_send(
				virtual_overseer,
				CollatorProtocolMessage::DistributeCollation(
					candidate.clone(),
					parent_head_data_hash,
					pov.clone(),
					None,
				),
			)
			.await;

			// Parent out of view, nothing happens.
			assert!(overseer_recv_with_timeout(virtual_overseer, Duration::from_millis(100))
				.await
				.is_none());

			test_harness
		},
	)
}

/// Tests that collator can distribute up to `MAX_CANDIDATE_DEPTH + 1` candidates
/// per relay parent.
#[test]
fn distribute_collation_up_to_limit() {
	let test_state = TestState::default();

	let local_peer_id = test_state.local_peer_id;
	let collator_pair = test_state.collator_pair.clone();

	test_harness(
		local_peer_id,
		collator_pair,
		ReputationAggregator::new(|_| true),
		|mut test_harness| async move {
			let virtual_overseer = &mut test_harness.virtual_overseer;

			let head_a = Hash::from_low_u64_be(128);
			let head_a_num: u32 = 64;

			// Grandparent of head `a`.
			let head_b = Hash::from_low_u64_be(130);

			// Set collating para id.
			overseer_send(virtual_overseer, CollatorProtocolMessage::CollateOn(test_state.para_id))
				.await;
			// Activated leaf is `a`, but the collation will be based on `b`.
			update_view(virtual_overseer, &test_state, vec![(head_a, head_a_num)], 1).await;

			for i in 0..(ASYNC_BACKING_PARAMETERS.max_candidate_depth + 1) {
				let pov = PoV { block_data: BlockData(vec![i as u8]) };
				let parent_head_data_hash = Hash::repeat_byte(0xAA);
				let candidate = TestCandidateBuilder {
					para_id: test_state.para_id,
					relay_parent: head_b,
					pov_hash: pov.hash(),
					..Default::default()
				}
				.build();
				distribute_collation_with_receipt(
					virtual_overseer,
					&test_state,
					head_b,
					true,
					candidate,
					pov,
					parent_head_data_hash,
				)
				.await;
			}

			let pov = PoV { block_data: BlockData(vec![10, 12, 6]) };
			let parent_head_data_hash = Hash::repeat_byte(0xBB);
			let candidate = TestCandidateBuilder {
				para_id: test_state.para_id,
				relay_parent: head_b,
				pov_hash: pov.hash(),
				..Default::default()
			}
			.build();
			overseer_send(
				virtual_overseer,
				CollatorProtocolMessage::DistributeCollation(
					candidate.clone(),
					parent_head_data_hash,
					pov.clone(),
					None,
				),
			)
			.await;

			// Limit has been reached.
			assert!(overseer_recv_with_timeout(virtual_overseer, Duration::from_millis(100))
				.await
				.is_none());

			test_harness
		},
	)
}

/// Tests that collator correctly handles peer V2 requests.
#[test]
fn advertise_and_send_collation_by_hash() {
	let test_state = TestState::default();

	let local_peer_id = test_state.local_peer_id;
	let collator_pair = test_state.collator_pair.clone();

	test_harness(
		local_peer_id,
		collator_pair,
		ReputationAggregator::new(|_| true),
		|test_harness| async move {
			let mut virtual_overseer = test_harness.virtual_overseer;
			let req_v1_cfg = test_harness.req_v1_cfg;
			let mut req_vstaging_cfg = test_harness.req_vstaging_cfg;

			let head_a = Hash::from_low_u64_be(128);
			let head_a_num: u32 = 64;

			// Parent of head `a`.
			let head_b = Hash::from_low_u64_be(129);
			let head_b_num: u32 = 63;

			// Set collating para id.
			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::CollateOn(test_state.para_id),
			)
			.await;
			update_view(&mut virtual_overseer, &test_state, vec![(head_b, head_b_num)], 1).await;
			update_view(&mut virtual_overseer, &test_state, vec![(head_a, head_a_num)], 1).await;

			let candidates: Vec<_> = (0..2)
				.map(|i| {
					let pov = PoV { block_data: BlockData(vec![i as u8]) };
					let candidate = TestCandidateBuilder {
						para_id: test_state.para_id,
						relay_parent: head_b,
						pov_hash: pov.hash(),
						..Default::default()
					}
					.build();
					(candidate, pov)
				})
				.collect();
			for (candidate, pov) in &candidates {
				distribute_collation_with_receipt(
					&mut virtual_overseer,
					&test_state,
					head_b,
					true,
					candidate.clone(),
					pov.clone(),
					Hash::zero(),
				)
				.await;
			}

			let peer = test_state.validator_peer_id[0];
			let validator_id = test_state.current_group_validator_authority_ids()[0].clone();
			connect_peer(
				&mut virtual_overseer,
				peer,
				CollationVersion::VStaging,
				Some(validator_id.clone()),
			)
			.await;
			expect_declare_msg_vstaging(&mut virtual_overseer, &test_state, &peer).await;

			// Head `b` is not a leaf, but both advertisements are still relevant.
			send_peer_view_change(&mut virtual_overseer, &peer, vec![head_b]).await;
			let hashes: Vec<_> = candidates.iter().map(|(candidate, _)| candidate.hash()).collect();
			expect_advertise_collation_msg(&mut virtual_overseer, &peer, head_b, Some(hashes))
				.await;

			for (candidate, pov_block) in candidates {
				let (pending_response, rx) = oneshot::channel();
				req_vstaging_cfg
					.inbound_queue
					.as_mut()
					.unwrap()
					.send(RawIncomingRequest {
						peer,
						payload: request_vstaging::CollationFetchingRequest {
							relay_parent: head_b,
							para_id: test_state.para_id,
							candidate_hash: candidate.hash(),
						}
						.encode(),
						pending_response,
					})
					.await
					.unwrap();

				assert_matches!(
					rx.await,
					Ok(full_response) => {
						// Response is the same for vstaging.
						let request_v1::CollationFetchingResponse::Collation(receipt, pov): request_v1::CollationFetchingResponse
							= request_v1::CollationFetchingResponse::decode(
								&mut full_response.result
								.expect("We should have a proper answer").as_ref()
						)
						.expect("Decoding should work");
						assert_eq!(receipt, candidate);
						assert_eq!(pov, pov_block);
					}
				);
			}

			TestHarness { virtual_overseer, req_v1_cfg, req_vstaging_cfg }
		},
	)
}

/// Tests that collator distributes collation built on top of occupied core.
#[test]
fn advertise_core_occupied() {
	let mut test_state = TestState::default();
	let candidate =
		TestCandidateBuilder { para_id: test_state.para_id, ..Default::default() }.build();
	test_state.availability_cores[0] = CoreState::Occupied(OccupiedCore {
		next_up_on_available: None,
		occupied_since: 0,
		time_out_at: 0,
		next_up_on_time_out: None,
		availability: BitVec::default(),
		group_responsible: GroupIndex(0),
		candidate_hash: candidate.hash(),
		candidate_descriptor: candidate.descriptor,
	});

	let local_peer_id = test_state.local_peer_id;
	let collator_pair = test_state.collator_pair.clone();

	test_harness(
		local_peer_id,
		collator_pair,
		ReputationAggregator::new(|_| true),
		|mut test_harness| async move {
			let virtual_overseer = &mut test_harness.virtual_overseer;

			let head_a = Hash::from_low_u64_be(128);
			let head_a_num: u32 = 64;

			// Grandparent of head `a`.
			let head_b = Hash::from_low_u64_be(130);

			// Set collating para id.
			overseer_send(virtual_overseer, CollatorProtocolMessage::CollateOn(test_state.para_id))
				.await;
			// Activated leaf is `a`, but the collation will be based on `b`.
			update_view(virtual_overseer, &test_state, vec![(head_a, head_a_num)], 1).await;

			let pov = PoV { block_data: BlockData(vec![1, 2, 3]) };
			let candidate = TestCandidateBuilder {
				para_id: test_state.para_id,
				relay_parent: head_b,
				pov_hash: pov.hash(),
				..Default::default()
			}
			.build();
			let candidate_hash = candidate.hash();
			distribute_collation_with_receipt(
				virtual_overseer,
				&test_state,
				head_b,
				true,
				candidate,
				pov,
				Hash::zero(),
			)
			.await;

			let validators = test_state.current_group_validator_authority_ids();
			let peer_ids = test_state.current_group_validator_peer_ids();

			connect_peer(
				virtual_overseer,
				peer_ids[0],
				CollationVersion::VStaging,
				Some(validators[0].clone()),
			)
			.await;
			expect_declare_msg_vstaging(virtual_overseer, &test_state, &peer_ids[0]).await;
			// Peer is aware of the leaf.
			send_peer_view_change(virtual_overseer, &peer_ids[0], vec![head_a]).await;

			// Collation is advertised.
			expect_advertise_collation_msg(
				virtual_overseer,
				&peer_ids[0],
				head_b,
				Some(vec![candidate_hash]),
			)
			.await;

			test_harness
		},
	)
}
