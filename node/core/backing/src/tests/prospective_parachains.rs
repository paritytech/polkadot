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

use polkadot_node_subsystem::messages::ChainApiMessage;
use polkadot_primitives::v2::{BlockNumber, Header};

use super::*;

const API_VERSION_PROSPECTIVE_ENABLED: u32 = 3;

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
) {
	let TestLeaf { activated, min_relay_parents } = leaf;
	let leaf_hash = activated.hash;
	let leaf_number = activated.number;
	// Start work on some new parent.
	virtual_overseer
		.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			activated,
		))))
		.await;

	// Prospective parachains mode is temporarily defined by the Runtime API version.
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::Version(tx))
		) if parent == leaf_hash => {
			tx.send(Ok(API_VERSION_PROSPECTIVE_ENABLED)).unwrap();
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
	let mut ancestry_iter = ancestry_hashes.clone().zip(ancestry_numbers).peekable();

	loop {
		let (hash, number) = match ancestry_iter.next() {
			Some((hash, number)) => (hash, number),
			None => break,
		};

		// May be `None` for the last element.
		let parent_hash =
			ancestry_iter.peek().map(|(h, _)| *h).unwrap_or_else(|| get_parent_hash(hash));
		assert_matches!(
			virtual_overseer.recv().await,
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
	}

	for hash in ancestry_hashes {
		// Check that subsystem job issues a request for a validator set.
		assert_matches!(
			virtual_overseer.recv().await,
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
				tx.send(Ok(test_state.validator_groups.clone())).unwrap();
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

// Test that `seconding_sanity_check` works when a candidate is allowed
// for all leaves.
#[test]
fn seconding_sanity_check_allowed() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		// Candidate is seconded in a parent of the activated `leaf_a`.
		const LEAF_A_BLOCK_NUMBER: BlockNumber = 100;
		const LEAF_A_DEPTH: BlockNumber = 3;
		let para_id = test_state.chain_ids[0];

		let leaf_a_hash = Hash::from_low_u64_be(128);
		let leaf_a_parent = get_parent_hash(leaf_a_hash);
		let activated = ActivatedLeaf {
			hash: leaf_a_hash,
			number: LEAF_A_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};
		let min_relay_parents = vec![(para_id, LEAF_A_BLOCK_NUMBER - LEAF_A_DEPTH)];
		let test_leaf_a = TestLeaf { activated, min_relay_parents };

		const LEAF_B_BLOCK_NUMBER: BlockNumber = 99;
		const LEAF_B_DEPTH: BlockNumber = 2;

		let leaf_b_hash = Hash::from_low_u64_be(256);
		let activated = ActivatedLeaf {
			hash: leaf_b_hash,
			number: LEAF_B_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};
		let min_relay_parents = vec![(para_id, LEAF_B_BLOCK_NUMBER - LEAF_B_DEPTH)];
		let test_leaf_b = TestLeaf { activated, min_relay_parents };

		activate_leaf(&mut virtual_overseer, test_leaf_a, &test_state).await;
		// activate_leaf(&mut virtual_overseer, test_leaf_b, &test_state).await;

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
			erasure_root: make_erasure_root(&test_state, pov.clone()),
			persisted_validation_data_hash: pvd.hash(),
			validation_code: validation_code.0.clone(),
			..Default::default()
		}
		.build();

		let second = CandidateBackingMessage::Second(
			leaf_a_hash,
			candidate.to_plain(),
			pvd.clone(),
			pov.clone(),
		);

		virtual_overseer.send(FromOverseer::Communication { msg: second }).await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidationCodeByHash(hash, tx))
			) if parent == leaf_a_parent && hash == validation_code.hash() => {
				tx.send(Ok(Some(validation_code.clone()))).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromExhaustive(
					_pvd,
					_validation_code,
					candidate_receipt,
					_pov,
					timeout,
					tx,
				),
			) if _pvd == pvd &&
				_validation_code == validation_code &&
				*_pov == pov && &candidate_receipt.descriptor == candidate.descriptor() &&
				timeout == BACKING_EXECUTION_TIMEOUT &&
				candidate.commitments.hash() == candidate_receipt.commitments_hash =>
			{
				tx.send(Ok(ValidationResult::Valid(
					CandidateCommitments {
						head_data: expected_head_data.clone(),
						horizontal_messages: Vec::new(),
						upward_messages: Vec::new(),
						new_validation_code: None,
						processed_downward_messages: 0,
						hrmp_watermark: 0,
					},
					test_state.validation_data.clone(),
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

		// `seconding_sanity_check`
		let expected_request = HypotheticalDepthRequest {
			candidate_hash: candidate.hash(),
			candidate_para: para_id,
			parent_head_data_hash: pvd.parent_head.hash(),
			candidate_relay_parent: leaf_a_parent,
			fragment_tree_relay_parent: leaf_a_hash,
		};
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(ProspectiveParachainsMessage::GetHypotheticalDepth(
				request,
				tx,
			)) if request == expected_request => {
				tx.send(vec![0, 1, 2, 3]).unwrap();
			}
		);
		// Prospective parachains are notified.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ProspectiveParachains(
				ProspectiveParachainsMessage::CandidateSeconded(
					candidate_para,
					candidate_receipt,
					_pvd,
					tx,
				),
			) if candidate_receipt == candidate && candidate_para == para_id && pvd == _pvd => {
				// Any non-empty response will do.
				tx.send(vec![(leaf_a_hash, vec![0, 1, 2, 3])]).unwrap();
			}
		);

		test_dispute_coordinator_notifications(
			&mut virtual_overseer,
			candidate.hash(),
			test_state.session(),
			vec![ValidatorIndex(0)],
		)
		.await;

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
			.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::stop_work(leaf_a_hash),
			)))
			.await;
		virtual_overseer
	});
}
