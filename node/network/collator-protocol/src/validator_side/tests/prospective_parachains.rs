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

//! Tests for the validator side with enabled prospective parachains.

use super::*;

use polkadot_node_subsystem::messages::ChainApiMessage;
use polkadot_primitives::v2::{
	BlockNumber, CandidateCommitments, CommittedCandidateReceipt, Header, SigningContext,
	ValidatorId,
};

const ALLOWED_ANCESTRY: u32 = 3;

fn get_parent_hash(hash: Hash) -> Hash {
	Hash::from_low_u64_be(hash.to_low_u64_be() + 1)
}

async fn assert_assign_incoming(
	virtual_overseer: &mut VirtualOverseer,
	test_state: &TestState,
	hash: Hash,
	number: BlockNumber,
	next_msg: &mut Option<AllMessages>,
) {
	let msg = match next_msg.take() {
		Some(msg) => msg,
		None => overseer_recv(virtual_overseer).await,
	};
	assert_matches!(
		msg,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::Validators(tx))
		) if parent == hash => {
			tx.send(Ok(test_state.validator_public.clone())).unwrap();
		}
	);

	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidatorGroups(tx))
		) if parent == hash => {
			let validator_groups = test_state.validator_groups.clone();
			let mut group_rotation_info = test_state.group_rotation_info.clone();
			group_rotation_info.now = number;
			tx.send(Ok((validator_groups, group_rotation_info))).unwrap();
		}
	);

	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::AvailabilityCores(tx))
		) if parent == hash => {
			tx.send(Ok(test_state.cores.clone())).unwrap();
		}
	);
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
				RuntimeApiRequest::Version(tx),
			)) => {
				tx.send(Ok(RuntimeApiRequest::VALIDITY_CONSTRAINTS)).unwrap();
				(parent, new_view.get(&parent).copied().expect("Unknown parent requested"))
			}
		);

		assert_assign_incoming(
			virtual_overseer,
			test_state,
			leaf_hash,
			leaf_number,
			&mut next_overseer_message,
		)
		.await;

		let min_number = leaf_number.saturating_sub(ALLOWED_ANCESTRY);

		assert_matches!(
			overseer_recv(virtual_overseer).await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::GetMinimumRelayParents(parent, tx),
			) if parent == leaf_hash => {
				tx.send(test_state.chain_ids.iter().map(|para_id| (*para_id, min_number)).collect()).unwrap();
			}
		);

		let ancestry_len = leaf_number + 1 - min_number;
		let ancestry_hashes = std::iter::successors(Some(leaf_hash), |h| Some(get_parent_hash(*h)))
			.take(ancestry_len as usize);
		let ancestry_numbers = (min_number..=leaf_number).rev();
		let ancestry_iter = ancestry_hashes.clone().zip(ancestry_numbers).peekable();

		// How many blocks were actually requested.
		let mut requested_len: usize = 0;
		{
			let mut ancestry_iter = ancestry_iter.clone();
			loop {
				let (hash, number) = match ancestry_iter.next() {
					Some((hash, number)) => (hash, number),
					None => break,
				};

				// May be `None` for the last element.
				let parent_hash =
					ancestry_iter.peek().map(|(h, _)| *h).unwrap_or_else(|| get_parent_hash(hash));

				let msg = match next_overseer_message.take() {
					Some(msg) => msg,
					None => overseer_recv(virtual_overseer).await,
				};

				if !matches!(&msg, AllMessages::ChainApi(ChainApiMessage::BlockHeader(..))) {
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

				requested_len += 1;
			}
		}

		// Skip the leaf.
		for (hash, number) in ancestry_iter.skip(1).take(requested_len.saturating_sub(1)) {
			assert_assign_incoming(
				virtual_overseer,
				test_state,
				hash,
				number,
				&mut next_overseer_message,
			)
			.await;
		}
	}
}

async fn send_seconded_statement(
	virtual_overseer: &mut VirtualOverseer,
	keystore: SyncCryptoStorePtr,
	candidate: &CommittedCandidateReceipt,
) {
	let signing_context = SigningContext { session_index: 0, parent_hash: Hash::zero() };
	let stmt = SignedFullStatement::sign(
		&keystore,
		Statement::Seconded(candidate.clone()),
		&signing_context,
		ValidatorIndex(0),
		&ValidatorId::from(Sr25519Keyring::Alice.public()),
	)
	.await
	.ok()
	.flatten()
	.expect("should be signed");

	overseer_send(
		virtual_overseer,
		CollatorProtocolMessage::Seconded(candidate.descriptor.relay_parent, stmt),
	)
	.await;
}

async fn assert_collation_seconded(
	virtual_overseer: &mut VirtualOverseer,
	relay_parent: Hash,
	peer_id: PeerId,
) {
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(
			peer,
			rep,
		)) => {
			assert_eq!(peer_id, peer);
			assert_eq!(rep, BENEFIT_NOTIFY_GOOD);
		}
	);
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendCollationMessage(
			peers,
			Versioned::VStaging(protocol_vstaging::CollationProtocol::CollatorProtocol(
				protocol_vstaging::CollatorProtocolMessage::CollationSeconded(
					_relay_parent,
					..,
				),
			)),
		)) => {
			assert_eq!(peers, vec![peer_id]);
			assert_eq!(relay_parent, _relay_parent);
		}
	);
}

#[test]
fn v1_advertisement_rejected() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let pair_a = CollatorPair::generate().0;

		let head_b = Hash::from_low_u64_be(128);
		let head_b_num: u32 = 0;

		update_view(&mut virtual_overseer, &test_state, vec![(head_b, head_b_num)], 1).await;

		let peer_a = PeerId::random();

		// Accept both collators from the implicit view.
		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_a,
			pair_a.clone(),
			test_state.chain_ids[0],
			CollationVersion::V1,
		)
		.await;

		advertise_collation(&mut virtual_overseer, peer_a, head_b, None).await;

		// Not reported.
		assert!(overseer_recv_with_timeout(&mut virtual_overseer, Duration::from_millis(50))
			.await
			.is_none());

		virtual_overseer
	});
}

#[test]
fn accept_advertisements_from_implicit_view() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let pair_a = CollatorPair::generate().0;
		let pair_b = CollatorPair::generate().0;

		let head_b = Hash::from_low_u64_be(128);
		let head_b_num: u32 = 2;

		// Grandparent of head `b`.
		// Group rotation frequency is 1 by default, at `c` we're assigned
		// to the first para.
		let head_c = Hash::from_low_u64_be(130);

		// Activated leaf is `b`, but the collation will be based on `c`.
		update_view(&mut virtual_overseer, &test_state, vec![(head_b, head_b_num)], 1).await;

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();

		// Accept both collators from the implicit view.
		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_a,
			pair_a.clone(),
			test_state.chain_ids[0],
			CollationVersion::VStaging,
		)
		.await;
		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_b,
			pair_b.clone(),
			test_state.chain_ids[1],
			CollationVersion::VStaging,
		)
		.await;

		let candidate_hash = CandidateHash::default();
		let parent_head_data_hash = Hash::zero();
		advertise_collation(
			&mut virtual_overseer,
			peer_b,
			head_c,
			Some((candidate_hash, parent_head_data_hash)),
		)
		.await;
		// Advertise with different para.
		advertise_collation(
			&mut virtual_overseer,
			peer_a,
			head_c,
			Some((candidate_hash, parent_head_data_hash)),
		)
		.await;

		let response_channel = assert_fetch_collation_request(
			&mut virtual_overseer,
			head_c,
			test_state.chain_ids[1],
			Some(candidate_hash),
		)
		.await;

		// Respond with an error to abort seconding.
		response_channel
			.send(Err(sc_network::RequestFailure::NotConnected))
			.expect("Sending response should succeed");
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(..),)
		);

		assert_fetch_collation_request(
			&mut virtual_overseer,
			head_c,
			test_state.chain_ids[0],
			Some(candidate_hash),
		)
		.await;

		virtual_overseer
	});
}

#[test]
fn second_multiple_candidates_per_relay_parent() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer, keystore } = test_harness;

		let pair = CollatorPair::generate().0;

		// Grandparent of head `a`.
		let head_b = Hash::from_low_u64_be(128);
		let head_b_num: u32 = 2;

		// Grandparent of head `b`.
		// Group rotation frequency is 1 by default, at `c` we're assigned
		// to the first para.
		let head_c = Hash::from_low_u64_be(130);

		// Activated leaf is `b`, but the collation will be based on `c`.
		update_view(&mut virtual_overseer, &test_state, vec![(head_b, head_b_num)], 1).await;

		let peer_a = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_a,
			pair.clone(),
			test_state.chain_ids[0],
			CollationVersion::VStaging,
		)
		.await;

		for i in 0..(MAX_CANDIDATE_DEPTH + 1) {
			let mut candidate = dummy_candidate_receipt_bad_sig(head_c, Some(Default::default()));
			candidate.descriptor.para_id = test_state.chain_ids[0];
			candidate.descriptor.relay_parent = head_c;
			candidate.descriptor.persisted_validation_data_hash = dummy_pvd().hash();
			let commitments = CandidateCommitments {
				head_data: HeadData(vec![i as u8]),
				horizontal_messages: Vec::new(),
				upward_messages: Vec::new(),
				new_validation_code: None,
				processed_downward_messages: 0,
				hrmp_watermark: 0,
			};
			candidate.commitments_hash = commitments.hash();

			let candidate_hash = candidate.hash();
			let parent_head_data_hash = Hash::zero();

			advertise_collation(
				&mut virtual_overseer,
				peer_a,
				head_c,
				Some((candidate_hash, parent_head_data_hash)),
			)
			.await;

			let response_channel = assert_fetch_collation_request(
				&mut virtual_overseer,
				head_c,
				test_state.chain_ids[0],
				Some(candidate_hash),
			)
			.await;

			let pov = PoV { block_data: BlockData(vec![1]) };

			response_channel
				.send(Ok(request_vstaging::CollationFetchingResponse::Collation(
					candidate.clone(),
					pov.clone(),
				)
				.encode()))
				.expect("Sending response should succeed");

			assert_candidate_backing_second(
				&mut virtual_overseer,
				head_c,
				test_state.chain_ids[0],
				&pov,
				ProspectiveParachainsMode::Enabled,
			)
			.await;

			let candidate =
				CommittedCandidateReceipt { descriptor: candidate.descriptor, commitments };

			send_seconded_statement(&mut virtual_overseer, keystore.clone(), &candidate).await;

			assert_collation_seconded(&mut virtual_overseer, head_c, peer_a).await;
		}

		// No more advertisements can be made for this relay parent.
		let candidate_hash = CandidateHash(Hash::repeat_byte(0xAA));
		advertise_collation(
			&mut virtual_overseer,
			peer_a,
			head_c,
			Some((candidate_hash, Hash::zero())),
		)
		.await;

		// Reported because reached the limit of advertisements per relay parent.
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(peer_id, rep),
			) => {
				assert_eq!(peer_a, peer_id);
				assert_eq!(rep, COST_UNEXPECTED_MESSAGE);
			}
		);

		// By different peer too (not reported).
		let pair_b = CollatorPair::generate().0;
		let peer_b = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_b,
			pair_b.clone(),
			test_state.chain_ids[0],
			CollationVersion::VStaging,
		)
		.await;

		let candidate_hash = CandidateHash(Hash::repeat_byte(0xFF));
		advertise_collation(
			&mut virtual_overseer,
			peer_b,
			head_c,
			Some((candidate_hash, Hash::zero())),
		)
		.await;

		assert!(overseer_recv_with_timeout(&mut virtual_overseer, Duration::from_millis(50))
			.await
			.is_none());

		virtual_overseer
	});
}

#[test]
fn fetched_collation_sanity_check() {
	let test_state = TestState::default();

	test_harness(|test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		let pair = CollatorPair::generate().0;

		// Grandparent of head `a`.
		let head_b = Hash::from_low_u64_be(128);
		let head_b_num: u32 = 2;

		// Grandparent of head `b`.
		// Group rotation frequency is 1 by default, at `c` we're assigned
		// to the first para.
		let head_c = Hash::from_low_u64_be(130);

		// Activated leaf is `b`, but the collation will be based on `c`.
		update_view(&mut virtual_overseer, &test_state, vec![(head_b, head_b_num)], 1).await;

		let peer_a = PeerId::random();

		connect_and_declare_collator(
			&mut virtual_overseer,
			peer_a,
			pair.clone(),
			test_state.chain_ids[0],
			CollationVersion::VStaging,
		)
		.await;

		let mut candidate = dummy_candidate_receipt_bad_sig(head_c, Some(Default::default()));
		candidate.descriptor.para_id = test_state.chain_ids[0];
		candidate.descriptor.relay_parent = head_c;
		let commitments = CandidateCommitments {
			head_data: HeadData(vec![1, 2, 3]),
			horizontal_messages: Vec::new(),
			upward_messages: Vec::new(),
			new_validation_code: None,
			processed_downward_messages: 0,
			hrmp_watermark: 0,
		};
		candidate.commitments_hash = commitments.hash();

		let candidate_hash = CandidateHash(Hash::zero());
		let parent_head_data_hash = Hash::zero();

		advertise_collation(
			&mut virtual_overseer,
			peer_a,
			head_c,
			Some((candidate_hash, parent_head_data_hash)),
		)
		.await;

		let response_channel = assert_fetch_collation_request(
			&mut virtual_overseer,
			head_c,
			test_state.chain_ids[0],
			Some(candidate_hash),
		)
		.await;

		let pov = PoV { block_data: BlockData(vec![1]) };

		response_channel
			.send(Ok(request_vstaging::CollationFetchingResponse::Collation(
				candidate.clone(),
				pov.clone(),
			)
			.encode()))
			.expect("Sending response should succeed");

		// PVD request.
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::GetProspectiveValidationData(request, tx),
			) => {
				assert_eq!(head_c, request.candidate_relay_parent);
				assert_eq!(test_state.chain_ids[0], request.para_id);
				tx.send(Some(dummy_pvd())).unwrap();
			}
		);

		// Reported malicious.
		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(peer_id, rep),
			) => {
				assert_eq!(peer_a, peer_id);
				assert_eq!(rep, COST_REPORT_BAD);
			}
		);

		virtual_overseer
	});
}
