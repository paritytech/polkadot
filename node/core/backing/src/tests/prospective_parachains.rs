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

//! Tests for the backing subsystem with enabled prospective parachains.

use polkadot_node_subsystem::{
	messages::{ChainApiMessage, FragmentTreeMembership},
	TimeoutExt,
};
use polkadot_primitives::{vstaging as vstaging_primitives, BlockNumber, Header, OccupiedCore};

use super::*;

const ASYNC_BACKING_PARAMETERS: vstaging_primitives::AsyncBackingParams =
	vstaging_primitives::AsyncBackingParams { max_candidate_depth: 4, allowed_ancestry_len: 3 };

struct TestLeaf {
	activated: ActivatedLeaf,
	min_relay_parents: Vec<(ParaId, u32)>,
}

fn get_parent_hash(hash: Hash) -> Hash {
	Hash::from_low_u64_be(hash.to_low_u64_be() + 1)
}

async fn activate_leaf(
	virtual_overseer: &mut VirtualOverseer,
	leaf: TestLeaf,
	test_state: &TestState,
	seconded_in_view: usize,
) {
	let TestLeaf { activated, min_relay_parents } = leaf;
	let leaf_hash = activated.hash;
	let leaf_number = activated.number;
	// Start work on some new parent.
	virtual_overseer
		.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			activated,
		))))
		.await;

	// Prospective parachains mode is temporarily defined by the Runtime API version.
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::StagingAsyncBackingParams(tx))
		) if parent == leaf_hash => {
			tx.send(Ok(ASYNC_BACKING_PARAMETERS)).unwrap();
		}
	);

	let min_min = *min_relay_parents
		.iter()
		.map(|(_, block_num)| block_num)
		.min()
		.unwrap_or(&leaf_number);

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::ProspectiveParachains(
			ProspectiveParachainsMessage::GetMinimumRelayParents(parent, tx)
		) if parent == leaf_hash => {
			tx.send(min_relay_parents).unwrap();
		}
	);

	let ancestry_len = leaf_number + 1 - min_min;

	let ancestry_hashes = std::iter::successors(Some(leaf_hash), |h| Some(get_parent_hash(*h)))
		.take(ancestry_len as usize);
	let ancestry_numbers = (min_min..=leaf_number).rev();
	let ancestry_iter = ancestry_hashes.zip(ancestry_numbers).peekable();

	let mut next_overseer_message = None;
	// How many blocks were actually requested.
	let mut requested_len = 0;
	{
		let mut ancestry_iter = ancestry_iter.clone();
		while let Some((hash, number)) = ancestry_iter.next() {
			// May be `None` for the last element.
			let parent_hash =
				ancestry_iter.peek().map(|(h, _)| *h).unwrap_or_else(|| get_parent_hash(hash));

			let msg = virtual_overseer.recv().await;
			// It may happen that some blocks were cached by implicit view,
			// reuse the message.
			if !matches!(&msg, AllMessages::ChainApi(ChainApiMessage::BlockHeader(..))) {
				next_overseer_message.replace(msg);
				break
			}

			assert_matches!(
				msg,
				AllMessages::ChainApi(
					ChainApiMessage::BlockHeader(_hash, tx)
				) if _hash == hash => {
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

	for _ in 0..seconded_in_view {
		let msg = match next_overseer_message.take() {
			Some(msg) => msg,
			None => virtual_overseer.recv().await,
		};
		assert_matches!(
			msg,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::GetTreeMembership(.., tx),
			) => {
				tx.send(Vec::new()).unwrap();
			}
		);
	}

	for (hash, number) in ancestry_iter.take(requested_len) {
		// Check that subsystem job issues a request for a validator set.
		let msg = match next_overseer_message.take() {
			Some(msg) => msg,
			None => virtual_overseer.recv().await,
		};
		assert_matches!(
			msg,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::Validators(tx))
			) if parent == hash => {
				tx.send(Ok(test_state.validator_public.clone())).unwrap();
			}
		);

		// Check that subsystem job issues a request for the validator groups.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidatorGroups(tx))
			) if parent == hash => {
				let (validator_groups, mut group_rotation_info) = test_state.validator_groups.clone();
				group_rotation_info.now = number;
				tx.send(Ok((validator_groups, group_rotation_info))).unwrap();
			}
		);

		// Check that subsystem job issues a request for the session index for child.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::SessionIndexForChild(tx))
			) if parent == hash => {
				tx.send(Ok(test_state.signing_context.session_index)).unwrap();
			}
		);

		// Check that subsystem job issues a request for the availability cores.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::AvailabilityCores(tx))
			) if parent == hash => {
				tx.send(Ok(test_state.availability_cores.clone())).unwrap();
			}
		);
	}
}

async fn assert_validate_seconded_candidate(
	virtual_overseer: &mut VirtualOverseer,
	relay_parent: Hash,
	candidate: &CommittedCandidateReceipt,
	pov: &PoV,
	pvd: &PersistedValidationData,
	validation_code: &ValidationCode,
	expected_head_data: &HeadData,
	fetch_pov: bool,
) {
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidationCodeByHash(hash, tx))
		) if parent == relay_parent && hash == validation_code.hash() => {
			tx.send(Ok(Some(validation_code.clone()))).unwrap();
		}
	);

	if fetch_pov {
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityDistribution(
				AvailabilityDistributionMessage::FetchPoV {
					relay_parent: hash,
					tx,
					..
				}
			) if hash == relay_parent => {
				tx.send(pov.clone()).unwrap();
			}
		);
	}

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::CandidateValidation(CandidateValidationMessage::ValidateFromExhaustive(
			_pvd,
			_validation_code,
			candidate_receipt,
			_pov,
			timeout,
			tx,
		)) if &_pvd == pvd &&
			&_validation_code == validation_code &&
			&*_pov == pov &&
			&candidate_receipt.descriptor == candidate.descriptor() &&
			timeout == PvfExecTimeoutKind::Backing &&
			candidate.commitments.hash() == candidate_receipt.commitments_hash =>
		{
			tx.send(Ok(ValidationResult::Valid(
				CandidateCommitments {
					head_data: expected_head_data.clone(),
					horizontal_messages: Default::default(),
					upward_messages: Default::default(),
					new_validation_code: None,
					processed_downward_messages: 0,
					hrmp_watermark: 0,
				},
				pvd.clone(),
			)))
			.unwrap();
		}
	);

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::AvailabilityStore(
			AvailabilityStoreMessage::StoreAvailableData { candidate_hash, tx, .. }
		) if candidate_hash == candidate.hash() => {
			tx.send(Ok(())).unwrap();
		}
	);
}

async fn assert_hypothetical_frontier_requests(
	virtual_overseer: &mut VirtualOverseer,
	mut expected_requests: Vec<(
		HypotheticalFrontierRequest,
		Vec<(HypotheticalCandidate, FragmentTreeMembership)>,
	)>,
) {
	// Requests come with no particular order.
	let requests_num = expected_requests.len();

	for _ in 0..requests_num {
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::GetHypotheticalFrontier(request, tx),
			) => {
				let idx = match expected_requests.iter().position(|r| r.0 == request) {
					Some(idx) => idx,
					None => panic!(
						"unexpected hypothetical frontier request, no match found for {:?}",
						request
					),
				};
				let resp = std::mem::take(&mut expected_requests[idx].1);
				tx.send(resp).unwrap();

				expected_requests.remove(idx);
			}
		);
	}
}

fn make_hypothetical_frontier_response(
	depths: Vec<usize>,
	hypothetical_candidate: HypotheticalCandidate,
	relay_parent_hash: Hash,
) -> Vec<(HypotheticalCandidate, FragmentTreeMembership)> {
	vec![(hypothetical_candidate, vec![(relay_parent_hash, depths)])]
}

// Test that `seconding_sanity_check` works when a candidate is allowed
// for all leaves.
#[test]
fn seconding_sanity_check_allowed() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		// Candidate is seconded in a parent of the activated `leaf_a`.
		const LEAF_A_BLOCK_NUMBER: BlockNumber = 100;
		const LEAF_A_ANCESTRY_LEN: BlockNumber = 3;
		let para_id = test_state.chain_ids[0];

		// `a` is grandparent of `b`.
		let leaf_a_hash = Hash::from_low_u64_be(130);
		let leaf_a_parent = get_parent_hash(leaf_a_hash);
		let activated = ActivatedLeaf {
			hash: leaf_a_hash,
			number: LEAF_A_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};
		let min_relay_parents = vec![(para_id, LEAF_A_BLOCK_NUMBER - LEAF_A_ANCESTRY_LEN)];
		let test_leaf_a = TestLeaf { activated, min_relay_parents };

		const LEAF_B_BLOCK_NUMBER: BlockNumber = LEAF_A_BLOCK_NUMBER + 2;
		const LEAF_B_ANCESTRY_LEN: BlockNumber = 4;

		let leaf_b_hash = Hash::from_low_u64_be(128);
		let activated = ActivatedLeaf {
			hash: leaf_b_hash,
			number: LEAF_B_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};
		let min_relay_parents = vec![(para_id, LEAF_B_BLOCK_NUMBER - LEAF_B_ANCESTRY_LEN)];
		let test_leaf_b = TestLeaf { activated, min_relay_parents };

		activate_leaf(&mut virtual_overseer, test_leaf_a, &test_state, 0).await;
		activate_leaf(&mut virtual_overseer, test_leaf_b, &test_state, 0).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };
		let pvd = dummy_pvd();
		let validation_code = ValidationCode(vec![1, 2, 3]);

		let expected_head_data = test_state.head_data.get(&para_id).unwrap();

		let pov_hash = pov.hash();
		let candidate = TestCandidateBuilder {
			para_id,
			relay_parent: leaf_a_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone(), pvd.clone()),
			persisted_validation_data_hash: pvd.hash(),
			validation_code: validation_code.0.clone(),
		}
		.build();

		let second = CandidateBackingMessage::Second(
			leaf_a_hash,
			candidate.to_plain(),
			pvd.clone(),
			pov.clone(),
		);

		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		assert_validate_seconded_candidate(
			&mut virtual_overseer,
			leaf_a_parent,
			&candidate,
			&pov,
			&pvd,
			&validation_code,
			expected_head_data,
			false,
		)
		.await;

		// `seconding_sanity_check`
		let hypothetical_candidate = HypotheticalCandidate::Complete {
			candidate_hash: candidate.hash(),
			receipt: Arc::new(candidate.clone()),
			persisted_validation_data: pvd.clone(),
		};
		let expected_request_a = HypotheticalFrontierRequest {
			candidates: vec![hypothetical_candidate.clone()],
			fragment_tree_relay_parent: Some(leaf_a_hash),
			backed_in_path_only: false,
		};
		let expected_response_a = make_hypothetical_frontier_response(
			vec![0, 1, 2, 3],
			hypothetical_candidate.clone(),
			leaf_a_hash,
		);
		let expected_request_b = HypotheticalFrontierRequest {
			candidates: vec![hypothetical_candidate.clone()],
			fragment_tree_relay_parent: Some(leaf_b_hash),
			backed_in_path_only: false,
		};
		let expected_response_b =
			make_hypothetical_frontier_response(vec![3], hypothetical_candidate, leaf_b_hash);
		assert_hypothetical_frontier_requests(
			&mut virtual_overseer,
			vec![
				(expected_request_a, expected_response_a),
				(expected_request_b, expected_response_b),
			],
		)
		.await;
		// Prospective parachains are notified.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::IntroduceCandidate(
					req,
					tx,
				),
			) if
				req.candidate_receipt == candidate
				&& req.candidate_para == para_id
				&& pvd == req.persisted_validation_data => {
				// Any non-empty response will do.
				tx.send(vec![(leaf_a_hash, vec![0, 1, 2, 3])]).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(ProspectiveParachainsMessage::CandidateSeconded(
				_,
				_
			))
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(
				StatementDistributionMessage::Share(
					parent_hash,
					_signed_statement,
				)
			) if parent_hash == leaf_a_parent => {}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CollatorProtocol(CollatorProtocolMessage::Seconded(hash, statement)) => {
				assert_eq!(leaf_a_parent, hash);
				assert_matches!(statement.payload(), Statement::Seconded(_));
			}
		);

		virtual_overseer
	});
}

// Test that `seconding_sanity_check` works when a candidate is disallowed
// for at least one leaf.
#[test]
fn seconding_sanity_check_disallowed() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		// Candidate is seconded in a parent of the activated `leaf_a`.
		const LEAF_A_BLOCK_NUMBER: BlockNumber = 100;
		const LEAF_A_ANCESTRY_LEN: BlockNumber = 3;
		let para_id = test_state.chain_ids[0];

		let leaf_b_hash = Hash::from_low_u64_be(128);
		// `a` is grandparent of `b`.
		let leaf_a_hash = Hash::from_low_u64_be(130);
		let leaf_a_parent = get_parent_hash(leaf_a_hash);
		let activated = ActivatedLeaf {
			hash: leaf_a_hash,
			number: LEAF_A_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};
		let min_relay_parents = vec![(para_id, LEAF_A_BLOCK_NUMBER - LEAF_A_ANCESTRY_LEN)];
		let test_leaf_a = TestLeaf { activated, min_relay_parents };

		const LEAF_B_BLOCK_NUMBER: BlockNumber = LEAF_A_BLOCK_NUMBER + 2;
		const LEAF_B_ANCESTRY_LEN: BlockNumber = 4;

		let activated = ActivatedLeaf {
			hash: leaf_b_hash,
			number: LEAF_B_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};
		let min_relay_parents = vec![(para_id, LEAF_B_BLOCK_NUMBER - LEAF_B_ANCESTRY_LEN)];
		let test_leaf_b = TestLeaf { activated, min_relay_parents };

		activate_leaf(&mut virtual_overseer, test_leaf_a, &test_state, 0).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };
		let pvd = dummy_pvd();
		let validation_code = ValidationCode(vec![1, 2, 3]);

		let expected_head_data = test_state.head_data.get(&para_id).unwrap();

		let pov_hash = pov.hash();
		let candidate = TestCandidateBuilder {
			para_id,
			relay_parent: leaf_a_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone(), pvd.clone()),
			persisted_validation_data_hash: pvd.hash(),
			validation_code: validation_code.0.clone(),
		}
		.build();

		let second = CandidateBackingMessage::Second(
			leaf_a_hash,
			candidate.to_plain(),
			pvd.clone(),
			pov.clone(),
		);

		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		assert_validate_seconded_candidate(
			&mut virtual_overseer,
			leaf_a_parent,
			&candidate,
			&pov,
			&pvd,
			&validation_code,
			expected_head_data,
			false,
		)
		.await;

		// `seconding_sanity_check`
		let hypothetical_candidate = HypotheticalCandidate::Complete {
			candidate_hash: candidate.hash(),
			receipt: Arc::new(candidate.clone()),
			persisted_validation_data: pvd.clone(),
		};
		let expected_request_a = HypotheticalFrontierRequest {
			candidates: vec![hypothetical_candidate.clone()],
			fragment_tree_relay_parent: Some(leaf_a_hash),
			backed_in_path_only: false,
		};
		let expected_response_a = make_hypothetical_frontier_response(
			vec![0, 1, 2, 3],
			hypothetical_candidate,
			leaf_a_hash,
		);
		assert_hypothetical_frontier_requests(
			&mut virtual_overseer,
			vec![(expected_request_a, expected_response_a)],
		)
		.await;
		// Prospective parachains are notified.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::IntroduceCandidate(
					req,
					tx,
				),
			) if
				req.candidate_receipt == candidate
				&& req.candidate_para == para_id
				&& pvd == req.persisted_validation_data => {
				// Any non-empty response will do.
				tx.send(vec![(leaf_a_hash, vec![0, 2, 3])]).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(ProspectiveParachainsMessage::CandidateSeconded(
				_,
				_
			))
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(
				StatementDistributionMessage::Share(
					parent_hash,
					_signed_statement,
				)
			) if parent_hash == leaf_a_parent => {}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CollatorProtocol(CollatorProtocolMessage::Seconded(hash, statement)) => {
				assert_eq!(leaf_a_parent, hash);
				assert_matches!(statement.payload(), Statement::Seconded(_));
			}
		);

		// A seconded candidate occupies a depth, try to second another one.
		// It is allowed in a new leaf but not allowed in the old one.
		// Expect it to be rejected.
		activate_leaf(&mut virtual_overseer, test_leaf_b, &test_state, 1).await;
		let leaf_a_grandparent = get_parent_hash(leaf_a_parent);
		let candidate = TestCandidateBuilder {
			para_id,
			relay_parent: leaf_a_grandparent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone(), pvd.clone()),
			persisted_validation_data_hash: pvd.hash(),
			validation_code: validation_code.0.clone(),
		}
		.build();

		let second = CandidateBackingMessage::Second(
			leaf_a_hash,
			candidate.to_plain(),
			pvd.clone(),
			pov.clone(),
		);

		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		assert_validate_seconded_candidate(
			&mut virtual_overseer,
			leaf_a_grandparent,
			&candidate,
			&pov,
			&pvd,
			&validation_code,
			expected_head_data,
			false,
		)
		.await;

		// `seconding_sanity_check`

		let hypothetical_candidate = HypotheticalCandidate::Complete {
			candidate_hash: candidate.hash(),
			receipt: Arc::new(candidate),
			persisted_validation_data: pvd,
		};
		let expected_request_a = HypotheticalFrontierRequest {
			candidates: vec![hypothetical_candidate.clone()],
			fragment_tree_relay_parent: Some(leaf_a_hash),
			backed_in_path_only: false,
		};
		let expected_response_a = make_hypothetical_frontier_response(
			vec![3],
			hypothetical_candidate.clone(),
			leaf_a_hash,
		);
		let expected_request_b = HypotheticalFrontierRequest {
			candidates: vec![hypothetical_candidate.clone()],
			fragment_tree_relay_parent: Some(leaf_b_hash),
			backed_in_path_only: false,
		};
		let expected_response_b =
			make_hypothetical_frontier_response(vec![1], hypothetical_candidate, leaf_b_hash);
		assert_hypothetical_frontier_requests(
			&mut virtual_overseer,
			vec![
				(expected_request_a, expected_response_a), // All depths are occupied.
				(expected_request_b, expected_response_b),
			],
		)
		.await;

		assert!(virtual_overseer
			.recv()
			.timeout(std::time::Duration::from_millis(50))
			.await
			.is_none());

		virtual_overseer
	});
}

// Test that a seconded candidate which is not approved by prospective parachains
// subsystem doesn't change the view.
#[test]
fn prospective_parachains_reject_candidate() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		// Candidate is seconded in a parent of the activated `leaf_a`.
		const LEAF_A_BLOCK_NUMBER: BlockNumber = 100;
		const LEAF_A_ANCESTRY_LEN: BlockNumber = 3;
		let para_id = test_state.chain_ids[0];

		let leaf_a_hash = Hash::from_low_u64_be(130);
		let leaf_a_parent = get_parent_hash(leaf_a_hash);
		let activated = ActivatedLeaf {
			hash: leaf_a_hash,
			number: LEAF_A_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};
		let min_relay_parents = vec![(para_id, LEAF_A_BLOCK_NUMBER - LEAF_A_ANCESTRY_LEN)];
		let test_leaf_a = TestLeaf { activated, min_relay_parents };

		activate_leaf(&mut virtual_overseer, test_leaf_a, &test_state, 0).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };
		let pvd = dummy_pvd();
		let validation_code = ValidationCode(vec![1, 2, 3]);

		let expected_head_data = test_state.head_data.get(&para_id).unwrap();

		let pov_hash = pov.hash();
		let candidate = TestCandidateBuilder {
			para_id,
			relay_parent: leaf_a_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone(), pvd.clone()),
			persisted_validation_data_hash: pvd.hash(),
			validation_code: validation_code.0.clone(),
		}
		.build();

		let second = CandidateBackingMessage::Second(
			leaf_a_hash,
			candidate.to_plain(),
			pvd.clone(),
			pov.clone(),
		);

		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		assert_validate_seconded_candidate(
			&mut virtual_overseer,
			leaf_a_parent,
			&candidate,
			&pov,
			&pvd,
			&validation_code,
			expected_head_data,
			false,
		)
		.await;

		// `seconding_sanity_check`
		let hypothetical_candidate = HypotheticalCandidate::Complete {
			candidate_hash: candidate.hash(),
			receipt: Arc::new(candidate.clone()),
			persisted_validation_data: pvd.clone(),
		};
		let expected_request_a = vec![(
			HypotheticalFrontierRequest {
				candidates: vec![hypothetical_candidate.clone()],
				fragment_tree_relay_parent: Some(leaf_a_hash),
				backed_in_path_only: false,
			},
			make_hypothetical_frontier_response(
				vec![0, 1, 2, 3],
				hypothetical_candidate,
				leaf_a_hash,
			),
		)];
		assert_hypothetical_frontier_requests(&mut virtual_overseer, expected_request_a.clone())
			.await;

		// Prospective parachains are notified.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::IntroduceCandidate(
					req,
					tx,
				),
			) if
				req.candidate_receipt == candidate
				&& req.candidate_para == para_id
				&& pvd == req.persisted_validation_data => {
				// Reject it.
				tx.send(Vec::new()).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CollatorProtocol(CollatorProtocolMessage::Invalid(
				relay_parent,
				candidate_receipt,
			)) if candidate_receipt.descriptor() == candidate.descriptor() &&
				candidate_receipt.commitments_hash == candidate.commitments.hash() &&
				relay_parent == leaf_a_parent
		);

		// Try seconding the same candidate.

		let second = CandidateBackingMessage::Second(
			leaf_a_hash,
			candidate.to_plain(),
			pvd.clone(),
			pov.clone(),
		);

		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		assert_validate_seconded_candidate(
			&mut virtual_overseer,
			leaf_a_parent,
			&candidate,
			&pov,
			&pvd,
			&validation_code,
			expected_head_data,
			false,
		)
		.await;

		// `seconding_sanity_check`
		assert_hypothetical_frontier_requests(&mut virtual_overseer, expected_request_a).await;
		// Prospective parachains are notified.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::IntroduceCandidate(
					req,
					tx,
				),
			) if
				req.candidate_receipt == candidate
				&& req.candidate_para == para_id
				&& pvd == req.persisted_validation_data => {
				// Any non-empty response will do.
				tx.send(vec![(leaf_a_hash, vec![0, 2, 3])]).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(ProspectiveParachainsMessage::CandidateSeconded(
				_,
				_
			))
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(
				StatementDistributionMessage::Share(
					parent_hash,
					_signed_statement,
				)
			) if parent_hash == leaf_a_parent => {}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CollatorProtocol(CollatorProtocolMessage::Seconded(hash, statement)) => {
				assert_eq!(leaf_a_parent, hash);
				assert_matches!(statement.payload(), Statement::Seconded(_));
			}
		);

		virtual_overseer
	});
}

// Test that a validator can second multiple candidates per single relay parent.
#[test]
fn second_multiple_candidates_per_relay_parent() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		// Candidate `a` is seconded in a parent of the activated `leaf`.
		const LEAF_BLOCK_NUMBER: BlockNumber = 100;
		const LEAF_ANCESTRY_LEN: BlockNumber = 3;
		let para_id = test_state.chain_ids[0];

		let leaf_hash = Hash::from_low_u64_be(130);
		let leaf_parent = get_parent_hash(leaf_hash);
		let leaf_grandparent = get_parent_hash(leaf_parent);
		let activated = ActivatedLeaf {
			hash: leaf_hash,
			number: LEAF_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};
		let min_relay_parents = vec![(para_id, LEAF_BLOCK_NUMBER - LEAF_ANCESTRY_LEN)];
		let test_leaf_a = TestLeaf { activated, min_relay_parents };

		activate_leaf(&mut virtual_overseer, test_leaf_a, &test_state, 0).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };
		let pvd = dummy_pvd();
		let validation_code = ValidationCode(vec![1, 2, 3]);

		let expected_head_data = test_state.head_data.get(&para_id).unwrap();

		let pov_hash = pov.hash();
		let candidate_a = TestCandidateBuilder {
			para_id,
			relay_parent: leaf_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone(), pvd.clone()),
			persisted_validation_data_hash: pvd.hash(),
			validation_code: validation_code.0.clone(),
		};
		let mut candidate_b = candidate_a.clone();
		candidate_b.relay_parent = leaf_grandparent;

		// With depths.
		let candidate_a = (candidate_a.build(), 1);
		let candidate_b = (candidate_b.build(), 2);

		for candidate in &[candidate_a, candidate_b] {
			let (candidate, depth) = candidate;
			let second = CandidateBackingMessage::Second(
				leaf_hash,
				candidate.to_plain(),
				pvd.clone(),
				pov.clone(),
			);

			virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

			assert_validate_seconded_candidate(
				&mut virtual_overseer,
				candidate.descriptor().relay_parent,
				&candidate,
				&pov,
				&pvd,
				&validation_code,
				expected_head_data,
				false,
			)
			.await;

			// `seconding_sanity_check`
			let hypothetical_candidate = HypotheticalCandidate::Complete {
				candidate_hash: candidate.hash(),
				receipt: Arc::new(candidate.clone()),
				persisted_validation_data: pvd.clone(),
			};
			let expected_request_a = vec![(
				HypotheticalFrontierRequest {
					candidates: vec![hypothetical_candidate.clone()],
					fragment_tree_relay_parent: Some(leaf_hash),
					backed_in_path_only: false,
				},
				make_hypothetical_frontier_response(
					vec![*depth],
					hypothetical_candidate,
					leaf_hash,
				),
			)];
			assert_hypothetical_frontier_requests(
				&mut virtual_overseer,
				expected_request_a.clone(),
			)
			.await;

			// Prospective parachains are notified.
			assert_matches!(
						   virtual_overseer.recv().await,
						   AllMessages::ProspectiveParachains(
							   ProspectiveParachainsMessage::IntroduceCandidate(
								   req,
								   tx,
							   ),
						   ) if
							   &req.candidate_receipt == candidate
							   && req.candidate_para == para_id
							   && pvd == req.persisted_validation_data
			=> {
							   // Any non-empty response will do.
							   tx.send(vec![(leaf_hash, vec![0, 2, 3])]).unwrap();
						   }
					   );

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::ProspectiveParachains(
					ProspectiveParachainsMessage::CandidateSeconded(_, _)
				)
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::Share(
						parent_hash,
						_signed_statement,
					)
				) if parent_hash == candidate.descriptor().relay_parent => {}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CollatorProtocol(CollatorProtocolMessage::Seconded(hash, statement)) => {
					assert_eq!(candidate.descriptor().relay_parent, hash);
					assert_matches!(statement.payload(), Statement::Seconded(_));
				}
			);
		}

		virtual_overseer
	});
}

// Test that the candidate reaches quorum successfully.
#[test]
fn backing_works() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		// Candidate `a` is seconded in a parent of the activated `leaf`.
		const LEAF_BLOCK_NUMBER: BlockNumber = 100;
		const LEAF_ANCESTRY_LEN: BlockNumber = 3;
		let para_id = test_state.chain_ids[0];

		let leaf_hash = Hash::from_low_u64_be(130);
		let leaf_parent = get_parent_hash(leaf_hash);
		let activated = ActivatedLeaf {
			hash: leaf_hash,
			number: LEAF_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};
		let min_relay_parents = vec![(para_id, LEAF_BLOCK_NUMBER - LEAF_ANCESTRY_LEN)];
		let test_leaf_a = TestLeaf { activated, min_relay_parents };

		activate_leaf(&mut virtual_overseer, test_leaf_a, &test_state, 0).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };
		let pvd = dummy_pvd();
		let validation_code = ValidationCode(vec![1, 2, 3]);

		let expected_head_data = test_state.head_data.get(&para_id).unwrap();

		let pov_hash = pov.hash();

		let candidate_a = TestCandidateBuilder {
			para_id,
			relay_parent: leaf_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone(), pvd.clone()),
			validation_code: validation_code.0.clone(),
			persisted_validation_data_hash: pvd.hash(),
		}
		.build();

		let candidate_a_hash = candidate_a.hash();
		let candidate_a_para_head = candidate_a.descriptor().para_head;

		let public1 = Keystore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[5].to_seed()),
		)
		.expect("Insert key into keystore");
		let public2 = Keystore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[2].to_seed()),
		)
		.expect("Insert key into keystore");

		// Signing context should have a parent hash candidate is based on.
		let signing_context =
			SigningContext { parent_hash: leaf_parent, session_index: test_state.session() };
		let signed_a = SignedFullStatementWithPVD::sign(
			&test_state.keystore,
			StatementWithPVD::Seconded(candidate_a.clone(), pvd.clone()),
			&signing_context,
			ValidatorIndex(2),
			&public2.into(),
		)
		.ok()
		.flatten()
		.expect("should be signed");

		let signed_b = SignedFullStatementWithPVD::sign(
			&test_state.keystore,
			StatementWithPVD::Valid(candidate_a_hash),
			&signing_context,
			ValidatorIndex(5),
			&public1.into(),
		)
		.ok()
		.flatten()
		.expect("should be signed");

		let statement = CandidateBackingMessage::Statement(leaf_parent, signed_a.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		// Prospective parachains are notified about candidate seconded first.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::IntroduceCandidate(
					req,
					tx,
				),
			) if
				req.candidate_receipt == candidate_a
				&& req.candidate_para == para_id
				&& pvd == req.persisted_validation_data => {
				// Any non-empty response will do.
				tx.send(vec![(leaf_hash, vec![0, 2, 3])]).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(ProspectiveParachainsMessage::CandidateSeconded(
				_,
				_
			))
		);

		assert_validate_seconded_candidate(
			&mut virtual_overseer,
			candidate_a.descriptor().relay_parent,
			&candidate_a,
			&pov,
			&pvd,
			&validation_code,
			expected_head_data,
			true,
		)
		.await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(
				StatementDistributionMessage::Share(hash, _stmt)
			) => {
				assert_eq!(leaf_parent, hash);
			}
		);

		// Prospective parachains and collator protocol are notified about candidate backed.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::CandidateBacked(
					candidate_para_id, candidate_hash
				),
			) if candidate_a_hash == candidate_hash && candidate_para_id == para_id
		);
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CollatorProtocol(CollatorProtocolMessage::Backed {
				para_id: _para_id,
				para_head,
			}) if para_id == _para_id && candidate_a_para_head == para_head
		);
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(StatementDistributionMessage::Backed (
				candidate_hash
			)) if candidate_a_hash == candidate_hash
		);

		let statement = CandidateBackingMessage::Statement(leaf_parent, signed_b.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		virtual_overseer
	});
}

// Tests that validators start work on consecutive prospective parachain blocks.
#[test]
fn concurrent_dependent_candidates() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		// Candidate `a` is seconded in a grandparent of the activated `leaf`,
		// candidate `b` -- in parent.
		const LEAF_BLOCK_NUMBER: BlockNumber = 100;
		const LEAF_ANCESTRY_LEN: BlockNumber = 3;
		let para_id = test_state.chain_ids[0];

		let leaf_hash = Hash::from_low_u64_be(130);
		let leaf_parent = get_parent_hash(leaf_hash);
		let leaf_grandparent = get_parent_hash(leaf_parent);
		let activated = ActivatedLeaf {
			hash: leaf_hash,
			number: LEAF_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};
		let min_relay_parents = vec![(para_id, LEAF_BLOCK_NUMBER - LEAF_ANCESTRY_LEN)];
		let test_leaf_a = TestLeaf { activated, min_relay_parents };

		activate_leaf(&mut virtual_overseer, test_leaf_a, &test_state, 0).await;

		let head_data = &[
			HeadData(vec![10, 20, 30]), // Before `a`.
			HeadData(vec![11, 21, 31]), // After `a`.
			HeadData(vec![12, 22]),     // After `b`.
		];

		let pov_a = PoV { block_data: BlockData(vec![42, 43, 44]) };
		let pvd_a = PersistedValidationData {
			parent_head: head_data[0].clone(),
			relay_parent_number: LEAF_BLOCK_NUMBER - 2,
			relay_parent_storage_root: Hash::zero(),
			max_pov_size: 1024,
		};

		let pov_b = PoV { block_data: BlockData(vec![22, 14, 100]) };
		let pvd_b = PersistedValidationData {
			parent_head: head_data[1].clone(),
			relay_parent_number: LEAF_BLOCK_NUMBER - 1,
			relay_parent_storage_root: Hash::zero(),
			max_pov_size: 1024,
		};
		let validation_code = ValidationCode(vec![1, 2, 3]);

		let candidate_a = TestCandidateBuilder {
			para_id,
			relay_parent: leaf_grandparent,
			pov_hash: pov_a.hash(),
			head_data: head_data[1].clone(),
			erasure_root: make_erasure_root(&test_state, pov_a.clone(), pvd_a.clone()),
			persisted_validation_data_hash: pvd_a.hash(),
			validation_code: validation_code.0.clone(),
		}
		.build();
		let candidate_b = TestCandidateBuilder {
			para_id,
			relay_parent: leaf_parent,
			pov_hash: pov_b.hash(),
			head_data: head_data[2].clone(),
			erasure_root: make_erasure_root(&test_state, pov_b.clone(), pvd_b.clone()),
			persisted_validation_data_hash: pvd_b.hash(),
			validation_code: validation_code.0.clone(),
		}
		.build();
		let candidate_a_hash = candidate_a.hash();
		let candidate_b_hash = candidate_b.hash();

		let public1 = Keystore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[5].to_seed()),
		)
		.expect("Insert key into keystore");
		let public2 = Keystore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[2].to_seed()),
		)
		.expect("Insert key into keystore");

		// Signing context should have a parent hash candidate is based on.
		let signing_context =
			SigningContext { parent_hash: leaf_grandparent, session_index: test_state.session() };
		let signed_a = SignedFullStatementWithPVD::sign(
			&test_state.keystore,
			StatementWithPVD::Seconded(candidate_a.clone(), pvd_a.clone()),
			&signing_context,
			ValidatorIndex(2),
			&public2.into(),
		)
		.ok()
		.flatten()
		.expect("should be signed");

		let signing_context =
			SigningContext { parent_hash: leaf_parent, session_index: test_state.session() };
		let signed_b = SignedFullStatementWithPVD::sign(
			&test_state.keystore,
			StatementWithPVD::Seconded(candidate_b.clone(), pvd_b.clone()),
			&signing_context,
			ValidatorIndex(5),
			&public1.into(),
		)
		.ok()
		.flatten()
		.expect("should be signed");

		let statement_a = CandidateBackingMessage::Statement(leaf_grandparent, signed_a.clone());
		let statement_b = CandidateBackingMessage::Statement(leaf_parent, signed_b.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement_a }).await;
		// At this point the subsystem waits for response, the previous message is received,
		// send a second one without blocking.
		let _ = virtual_overseer
			.tx
			.start_send_unpin(FromOrchestra::Communication { msg: statement_b });

		let mut valid_statements = HashSet::new();
		let mut backed_statements = HashSet::new();

		loop {
			let msg = virtual_overseer
				.recv()
				.timeout(std::time::Duration::from_secs(1))
				.await
				.expect("overseer recv timed out");

			// Order is not guaranteed since we have 2 statements being handled concurrently.
			match msg {
				AllMessages::ProspectiveParachains(
					ProspectiveParachainsMessage::IntroduceCandidate(_, tx),
				) => {
					tx.send(vec![(leaf_hash, vec![0, 2, 3])]).unwrap();
				},
				AllMessages::ProspectiveParachains(
					ProspectiveParachainsMessage::CandidateSeconded(_, _),
				) => {},
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_,
					RuntimeApiRequest::ValidationCodeByHash(_, tx),
				)) => {
					tx.send(Ok(Some(validation_code.clone()))).unwrap();
				},
				AllMessages::AvailabilityDistribution(
					AvailabilityDistributionMessage::FetchPoV { candidate_hash, tx, .. },
				) => {
					let pov = if candidate_hash == candidate_a_hash {
						&pov_a
					} else if candidate_hash == candidate_b_hash {
						&pov_b
					} else {
						panic!("unknown candidate hash")
					};
					tx.send(pov.clone()).unwrap();
				},
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromExhaustive(.., candidate, _, _, tx),
				) => {
					let candidate_hash = candidate.hash();
					let (head_data, pvd) = if candidate_hash == candidate_a_hash {
						(&head_data[1], &pvd_a)
					} else if candidate_hash == candidate_b_hash {
						(&head_data[2], &pvd_b)
					} else {
						panic!("unknown candidate hash")
					};
					tx.send(Ok(ValidationResult::Valid(
						CandidateCommitments {
							head_data: head_data.clone(),
							horizontal_messages: Default::default(),
							upward_messages: Default::default(),
							new_validation_code: None,
							processed_downward_messages: 0,
							hrmp_watermark: 0,
						},
						pvd.clone(),
					)))
					.unwrap();
				},
				AllMessages::AvailabilityStore(AvailabilityStoreMessage::StoreAvailableData {
					tx,
					..
				}) => {
					tx.send(Ok(())).unwrap();
				},
				AllMessages::ProspectiveParachains(
					ProspectiveParachainsMessage::CandidateBacked(..),
				) => {},
				AllMessages::CollatorProtocol(CollatorProtocolMessage::Backed { .. }) => {},
				AllMessages::StatementDistribution(StatementDistributionMessage::Share(
					_,
					statement,
				)) => {
					assert_eq!(statement.validator_index(), ValidatorIndex(0));
					let payload = statement.payload();
					assert_matches!(
						payload.clone(),
						StatementWithPVD::Valid(hash)
							if hash == candidate_a_hash || hash == candidate_b_hash =>
						{
							assert!(valid_statements.insert(hash));
						}
					);
				},
				AllMessages::StatementDistribution(StatementDistributionMessage::Backed(hash)) => {
					// Ensure that `Share` was received first for the candidate.
					assert!(valid_statements.contains(&hash));
					backed_statements.insert(hash);

					if backed_statements.len() == 2 {
						break
					}
				},
				_ => panic!("unexpected message received from overseer: {:?}", msg),
			}
		}

		assert!(valid_statements.contains(&candidate_a_hash));
		assert!(valid_statements.contains(&candidate_b_hash));
		assert!(backed_statements.contains(&candidate_a_hash));
		assert!(backed_statements.contains(&candidate_b_hash));

		virtual_overseer
	});
}

// Test that multiple candidates from different paras can occupy the same depth
// in a given relay parent.
#[test]
fn seconding_sanity_check_occupy_same_depth() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		// Candidate `a` is seconded in a parent of the activated `leaf`.
		const LEAF_BLOCK_NUMBER: BlockNumber = 100;
		const LEAF_ANCESTRY_LEN: BlockNumber = 3;

		let para_id_a = test_state.chain_ids[0];
		let para_id_b = test_state.chain_ids[1];

		let leaf_hash = Hash::from_low_u64_be(130);
		let leaf_parent = get_parent_hash(leaf_hash);

		let activated = ActivatedLeaf {
			hash: leaf_hash,
			number: LEAF_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};

		let min_block_number = LEAF_BLOCK_NUMBER - LEAF_ANCESTRY_LEN;
		let min_relay_parents = vec![(para_id_a, min_block_number), (para_id_b, min_block_number)];
		let test_leaf_a = TestLeaf { activated, min_relay_parents };

		activate_leaf(&mut virtual_overseer, test_leaf_a, &test_state, 0).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };
		let pvd = dummy_pvd();
		let validation_code = ValidationCode(vec![1, 2, 3]);

		let expected_head_data_a = test_state.head_data.get(&para_id_a).unwrap();
		let expected_head_data_b = test_state.head_data.get(&para_id_b).unwrap();

		let pov_hash = pov.hash();
		let candidate_a = TestCandidateBuilder {
			para_id: para_id_a,
			relay_parent: leaf_parent,
			pov_hash,
			head_data: expected_head_data_a.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone(), pvd.clone()),
			persisted_validation_data_hash: pvd.hash(),
			validation_code: validation_code.0.clone(),
		};

		let mut candidate_b = candidate_a.clone();
		candidate_b.para_id = para_id_b;
		candidate_b.head_data = expected_head_data_b.clone();
		// A rotation happens, test validator is assigned to second para here.
		candidate_b.relay_parent = leaf_hash;

		let candidate_a = (candidate_a.build(), expected_head_data_a, para_id_a);
		let candidate_b = (candidate_b.build(), expected_head_data_b, para_id_b);

		for candidate in &[candidate_a, candidate_b] {
			let (candidate, expected_head_data, para_id) = candidate;
			let second = CandidateBackingMessage::Second(
				leaf_hash,
				candidate.to_plain(),
				pvd.clone(),
				pov.clone(),
			);

			virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

			assert_validate_seconded_candidate(
				&mut virtual_overseer,
				candidate.descriptor().relay_parent,
				&candidate,
				&pov,
				&pvd,
				&validation_code,
				expected_head_data,
				false,
			)
			.await;

			// `seconding_sanity_check`
			let hypothetical_candidate = HypotheticalCandidate::Complete {
				candidate_hash: candidate.hash(),
				receipt: Arc::new(candidate.clone()),
				persisted_validation_data: pvd.clone(),
			};
			let expected_request_a = vec![(
				HypotheticalFrontierRequest {
					candidates: vec![hypothetical_candidate.clone()],
					fragment_tree_relay_parent: Some(leaf_hash),
					backed_in_path_only: false,
				},
				// Send the same membership for both candidates.
				make_hypothetical_frontier_response(vec![0, 1], hypothetical_candidate, leaf_hash),
			)];

			assert_hypothetical_frontier_requests(
				&mut virtual_overseer,
				expected_request_a.clone(),
			)
			.await;

			// Prospective parachains are notified.
			assert_matches!(
						   virtual_overseer.recv().await,
						   AllMessages::ProspectiveParachains(
							   ProspectiveParachainsMessage::IntroduceCandidate(
								   req,
								   tx,
							   ),
						   ) if
							   &req.candidate_receipt == candidate
							   && &req.candidate_para == para_id
							   && pvd == req.persisted_validation_data
			=> {
							   // Any non-empty response will do.
							   tx.send(vec![(leaf_hash, vec![0, 2, 3])]).unwrap();
						   }
					   );

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::ProspectiveParachains(
					ProspectiveParachainsMessage::CandidateSeconded(_, _)
				)
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::Share(
						parent_hash,
						_signed_statement,
					)
				) if parent_hash == candidate.descriptor().relay_parent => {}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CollatorProtocol(CollatorProtocolMessage::Seconded(hash, statement)) => {
					assert_eq!(candidate.descriptor().relay_parent, hash);
					assert_matches!(statement.payload(), Statement::Seconded(_));
				}
			);
		}

		virtual_overseer
	});
}

// Test that the subsystem doesn't skip occupied cores assignments.
#[test]
fn occupied_core_assignment() {
	let mut test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		// Candidate is seconded in a parent of the activated `leaf_a`.
		const LEAF_A_BLOCK_NUMBER: BlockNumber = 100;
		const LEAF_A_ANCESTRY_LEN: BlockNumber = 3;
		let para_id = test_state.chain_ids[0];

		// Set the core state to occupied.
		let mut candidate_descriptor = ::test_helpers::dummy_candidate_descriptor(Hash::zero());
		candidate_descriptor.para_id = para_id;
		test_state.availability_cores[0] = CoreState::Occupied(OccupiedCore {
			group_responsible: Default::default(),
			next_up_on_available: None,
			occupied_since: 100_u32,
			time_out_at: 200_u32,
			next_up_on_time_out: None,
			availability: Default::default(),
			candidate_descriptor,
			candidate_hash: Default::default(),
		});

		let leaf_a_hash = Hash::from_low_u64_be(130);
		let leaf_a_parent = get_parent_hash(leaf_a_hash);
		let activated = ActivatedLeaf {
			hash: leaf_a_hash,
			number: LEAF_A_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};
		let min_relay_parents = vec![(para_id, LEAF_A_BLOCK_NUMBER - LEAF_A_ANCESTRY_LEN)];
		let test_leaf_a = TestLeaf { activated, min_relay_parents };

		activate_leaf(&mut virtual_overseer, test_leaf_a, &test_state, 0).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };
		let pvd = dummy_pvd();
		let validation_code = ValidationCode(vec![1, 2, 3]);

		let expected_head_data = test_state.head_data.get(&para_id).unwrap();

		let pov_hash = pov.hash();
		let candidate = TestCandidateBuilder {
			para_id,
			relay_parent: leaf_a_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone(), pvd.clone()),
			persisted_validation_data_hash: pvd.hash(),
			validation_code: validation_code.0.clone(),
		}
		.build();

		let second = CandidateBackingMessage::Second(
			leaf_a_hash,
			candidate.to_plain(),
			pvd.clone(),
			pov.clone(),
		);

		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		assert_validate_seconded_candidate(
			&mut virtual_overseer,
			leaf_a_parent,
			&candidate,
			&pov,
			&pvd,
			&validation_code,
			expected_head_data,
			false,
		)
		.await;

		// `seconding_sanity_check`
		let hypothetical_candidate = HypotheticalCandidate::Complete {
			candidate_hash: candidate.hash(),
			receipt: Arc::new(candidate.clone()),
			persisted_validation_data: pvd.clone(),
		};
		let expected_request = vec![(
			HypotheticalFrontierRequest {
				candidates: vec![hypothetical_candidate.clone()],
				fragment_tree_relay_parent: Some(leaf_a_hash),
				backed_in_path_only: false,
			},
			make_hypothetical_frontier_response(
				vec![0, 1, 2, 3],
				hypothetical_candidate,
				leaf_a_hash,
			),
		)];
		assert_hypothetical_frontier_requests(&mut virtual_overseer, expected_request).await;
		// Prospective parachains are notified.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::IntroduceCandidate(
					req,
					tx,
				),
			) if
				req.candidate_receipt == candidate
				&& req.candidate_para == para_id
				&& pvd == req.persisted_validation_data
			=> {
				// Any non-empty response will do.
				tx.send(vec![(leaf_a_hash, vec![0, 1, 2, 3])]).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(ProspectiveParachainsMessage::CandidateSeconded(
				_,
				_
			))
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(
				StatementDistributionMessage::Share(
					parent_hash,
					_signed_statement,
				)
			) if parent_hash == leaf_a_parent => {}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CollatorProtocol(CollatorProtocolMessage::Seconded(hash, statement)) => {
				assert_eq!(leaf_a_parent, hash);
				assert_matches!(statement.payload(), Statement::Seconded(_));
			}
		);

		virtual_overseer
	});
}
