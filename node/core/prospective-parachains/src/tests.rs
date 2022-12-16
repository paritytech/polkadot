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

use super::*;
use ::polkadot_primitives_test_helpers::{dummy_candidate_receipt_bad_sig, dummy_hash};
use assert_matches::assert_matches;
use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	messages::{AllMessages, ProspectiveParachainsMessage},
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_types::{jaeger, ActivatedLeaf, LeafStatus};
use polkadot_primitives::{
	v2::{
		CandidateCommitments, GroupRotationInfo, HeadData, Header, PersistedValidationData,
		ScheduledCore, ValidationCodeHash, ValidatorId, ValidatorIndex,
	},
	vstaging::{AsyncBackingParameters, Constraints, InboundHrmpLimitations},
};
use sp_application_crypto::AppKey;
use sp_keyring::Sr25519Keyring;
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
use std::sync::Arc;

const ALLOWED_ANCESTRY_LEN: u32 = 3;
const ASYNC_BACKING_PARAMETERS: AsyncBackingParameters =
	AsyncBackingParameters { max_candidate_depth: 4, allowed_ancestry_len: ALLOWED_ANCESTRY_LEN };

const ASYNC_BACKING_DISABLED_ERROR: RuntimeApiError =
	RuntimeApiError::NotSupported { runtime_api_name: "test-runtime" };

const MAX_POV_SIZE: u32 = 1_000_000;

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<ProspectiveParachainsMessage>;

fn dummy_constraints(
	min_relay_parent_number: BlockNumber,
	valid_watermarks: Vec<BlockNumber>,
	required_parent: HeadData,
	validation_code_hash: ValidationCodeHash,
) -> Constraints {
	Constraints {
		min_relay_parent_number,
		max_pov_size: MAX_POV_SIZE,
		max_code_size: 1_000_000,
		ump_remaining: 10,
		ump_remaining_bytes: 1_000,
		max_ump_num_per_candidate: 10,
		dmp_remaining_messages: 10,
		hrmp_inbound: InboundHrmpLimitations { valid_watermarks },
		hrmp_channels_out: vec![],
		max_hrmp_num_per_candidate: 0,
		required_parent,
		validation_code_hash,
		upgrade_restriction: None,
		future_validation_code: None,
	}
}

fn dummy_pvd(parent_head: HeadData, relay_parent_number: u32) -> PersistedValidationData {
	PersistedValidationData {
		parent_head,
		relay_parent_number,
		max_pov_size: MAX_POV_SIZE,
		relay_parent_storage_root: dummy_hash(),
	}
}

fn make_candidate(
	leaf: &TestLeaf,
	para_id: ParaId,
	validation_code_hash: ValidationCodeHash,
) -> (CommittedCandidateReceipt, PersistedValidationData) {
	let PerParaData { min_relay_parent: _, required_parent } = leaf.para_data(para_id);
	let pvd = dummy_pvd(required_parent.clone(), leaf.number);
	let commitments = CandidateCommitments {
		head_data: required_parent.clone(),
		horizontal_messages: Vec::new(),
		upward_messages: Vec::new(),
		new_validation_code: None,
		processed_downward_messages: 0,
		hrmp_watermark: leaf.number,
	};
	let mut candidate = dummy_candidate_receipt_bad_sig(leaf.hash, Some(Default::default()));
	candidate.commitments_hash = commitments.hash();
	candidate.descriptor.para_id = para_id;
	candidate.descriptor.persisted_validation_data_hash = pvd.hash();
	candidate.descriptor.validation_code_hash = validation_code_hash;
	let candidate = CommittedCandidateReceipt { descriptor: candidate.descriptor, commitments };

	(candidate, pvd)
}

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

struct TestState {
	chain_ids: Vec<ParaId>,
	keystore: SyncCryptoStorePtr,
	validators: Vec<Sr25519Keyring>,
	validator_public: Vec<ValidatorId>,
	validator_groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo),
	availability_cores: Vec<CoreState>,
	validation_code_hash: ValidationCodeHash,
}

impl Default for TestState {
	fn default() -> Self {
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);

		let chain_ids = vec![chain_a, chain_b];

		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
			Sr25519Keyring::One,
		];

		let keystore = Arc::new(sc_keystore::LocalKeystore::in_memory());
		// Make sure `Alice` key is in the keystore, so this mocked node will be a parachain validator.
		SyncCryptoStore::sr25519_generate_new(
			&*keystore,
			ValidatorId::ID,
			Some(&validators[0].to_seed()),
		)
		.expect("Insert key into keystore");

		let validator_public = validator_pubkeys(&validators);

		let validator_groups = vec![vec![2, 0, 3, 5], vec![1]]
			.into_iter()
			.map(|g| g.into_iter().map(ValidatorIndex).collect())
			.collect();
		let group_rotation_info =
			GroupRotationInfo { session_start_block: 0, group_rotation_frequency: 100, now: 1 };

		let availability_cores = vec![
			CoreState::Scheduled(ScheduledCore { para_id: chain_a, collator: None }),
			CoreState::Scheduled(ScheduledCore { para_id: chain_b, collator: None }),
		];
		let validation_code_hash = Hash::repeat_byte(42).into();

		Self {
			chain_ids,
			keystore,
			validators,
			validator_public,
			validator_groups: (validator_groups, group_rotation_info),
			availability_cores,
			validation_code_hash,
		}
	}
}

fn get_parent_hash(hash: Hash) -> Hash {
	Hash::from_low_u64_be(hash.to_low_u64_be() + 1)
}

fn test_harness<T: Future<Output = VirtualOverseer>>(
	test: impl FnOnce(VirtualOverseer) -> T,
) -> View {
	let pool = sp_core::testing::TaskExecutor::new();

	let (mut context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let mut view = View::new();
	let subsystem = async move {
		loop {
			match run_iteration(&mut context, &mut view).await {
				Ok(()) => break,
				Err(e) => panic!("{:?}", e),
			}
		}

		view
	};

	let test_fut = test(virtual_overseer);

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);
	let (_, view) = futures::executor::block_on(future::join(
		async move {
			let mut virtual_overseer = test_fut.await;
			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
		},
		subsystem,
	));

	view
}

struct PerParaData {
	min_relay_parent: BlockNumber,
	required_parent: HeadData,
}

impl PerParaData {
	pub fn new(min_relay_parent: BlockNumber, required_parent: HeadData) -> Self {
		Self { min_relay_parent, required_parent }
	}
}

struct TestLeaf {
	number: BlockNumber,
	hash: Hash,
	para_data: Vec<(ParaId, PerParaData)>,
}

impl TestLeaf {
	pub fn para_data(&self, para_id: ParaId) -> &PerParaData {
		self.para_data
			.iter()
			.find_map(|(p_id, data)| if *p_id == para_id { Some(data) } else { None })
			.unwrap()
	}
}

async fn activate_leaf(
	virtual_overseer: &mut VirtualOverseer,
	leaf: &TestLeaf,
	test_state: &TestState,
) {
	async fn send_header_response(virtual_overseer: &mut VirtualOverseer, hash: Hash, number: u32) {
		let header = Header {
			parent_hash: get_parent_hash(hash),
			number,
			state_root: Hash::zero(),
			extrinsics_root: Hash::zero(),
			digest: Default::default(),
		};

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ChainApi(
				ChainApiMessage::BlockHeader(parent, tx)
			) if parent == hash => {
				tx.send(Ok(Some(header))).unwrap();
			}
		);
	}

	let TestLeaf { number, hash, para_data } = leaf;

	let activated = ActivatedLeaf {
		hash: *hash,
		number: *number,
		status: LeafStatus::Fresh,
		span: Arc::new(jaeger::Span::Disabled),
	};

	virtual_overseer
		.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			activated,
		))))
		.await;

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::StagingAsyncBackingParameters(tx))
		) if parent == *hash => {
			tx.send(Ok(ASYNC_BACKING_PARAMETERS)).unwrap();
		}
	);

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::AvailabilityCores(tx))
		) if parent == *hash => {
			tx.send(Ok(test_state.availability_cores.clone())).unwrap();
		}
	);

	send_header_response(virtual_overseer, *hash, *number).await;

	// Check that subsystem job issues a request for ancestors.
	let min_min = para_data.iter().map(|(_, data)| data.min_relay_parent).min().unwrap_or(*number);
	let ancestry_len = number - min_min;
	let ancestry_hashes: Vec<Hash> =
		std::iter::successors(Some(*hash), |h| Some(get_parent_hash(*h)))
			.skip(1)
			.take(ancestry_len as usize)
			.collect();
	let ancestry_numbers = (min_min..*number).rev();
	let ancestry_iter = ancestry_hashes.clone().into_iter().zip(ancestry_numbers).peekable();
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::ChainApi(
			ChainApiMessage::Ancestors{hash: block_hash, k, response_channel: tx}
		) if block_hash == *hash && k == ALLOWED_ANCESTRY_LEN as usize => {
			tx.send(Ok(ancestry_hashes.clone())).unwrap();
		}
	);

	for (hash, number) in ancestry_iter {
		send_header_response(virtual_overseer, hash, number).await;
	}

	for _ in 0..test_state.availability_cores.len() {
		let message = virtual_overseer.recv().await;
		// Get the para we are working with since the order is not deterministic.
		let para_id = match message {
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_,
				RuntimeApiRequest::StagingValidityConstraints(p_id, _),
			)) => p_id,
			_ => panic!("received unexpected message {:?}", message),
		};

		let PerParaData { min_relay_parent, required_parent } = leaf.para_data(para_id);
		let constraints = dummy_constraints(
			*min_relay_parent,
			vec![*number],
			required_parent.clone(),
			test_state.validation_code_hash,
		);
		assert_matches!(
			message,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::StagingValidityConstraints(p_id, tx))
			) if parent == *hash && p_id == para_id => {
				tx.send(Ok(Some(constraints))).unwrap();
			}
		);
	}

	// Get minimum relay parents.
	let (tx, rx) = oneshot::channel();
	virtual_overseer
		.send(overseer::FromOrchestra::Communication {
			msg: ProspectiveParachainsMessage::GetMinimumRelayParents(*hash, tx),
		})
		.await;
	let mut resp = rx.await.unwrap();
	resp.sort();
	let mrp_response: Vec<(ParaId, BlockNumber)> = para_data
		.iter()
		.map(|(para_id, data)| (*para_id, data.min_relay_parent))
		.collect();
	assert_eq!(resp, mrp_response);
}

async fn deactivate_leaf(virtual_overseer: &mut VirtualOverseer, hash: Hash) {
	virtual_overseer
		.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::stop_work(
			hash,
		))))
		.await;
}

async fn second_candidate(
	virtual_overseer: &mut VirtualOverseer,
	candidate: CommittedCandidateReceipt,
	pvd: PersistedValidationData,
	expected_candidate_response: Vec<(Hash, Vec<usize>)>,
) {
	let (tx, rx) = oneshot::channel();
	virtual_overseer
		.send(overseer::FromOrchestra::Communication {
			msg: ProspectiveParachainsMessage::CandidateSeconded(
				candidate.descriptor.para_id,
				candidate,
				pvd,
				tx,
			),
		})
		.await;
	let resp = rx.await.unwrap();
	assert_eq!(resp, expected_candidate_response);
}

async fn check_membership(
	virtual_overseer: &mut VirtualOverseer,
	para_id: ParaId,
	candidate_hash: CandidateHash,
	expected_candidate_response: Vec<(Hash, Vec<usize>)>,
) {
	let (tx, rx) = oneshot::channel();
	virtual_overseer
		.send(overseer::FromOrchestra::Communication {
			msg: ProspectiveParachainsMessage::GetTreeMembership(para_id, candidate_hash, tx),
		})
		.await;
	let resp = rx.await.unwrap();
	assert_eq!(resp, expected_candidate_response);
}

#[test]
fn should_do_no_work_if_async_backing_disabled_for_leaf() {
	async fn activate_leaf_async_backing_disabled(virtual_overseer: &mut VirtualOverseer) {
		let hash = Hash::from_low_u64_be(130);

		// Start work on some new parent.
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::StagingAsyncBackingParameters(tx))
			) if parent == hash => {
				tx.send(Err(ASYNC_BACKING_DISABLED_ERROR)).unwrap();
			}
		);
	}

	let view = test_harness(|mut virtual_overseer| async move {
		activate_leaf_async_backing_disabled(&mut virtual_overseer).await;

		virtual_overseer
	});

	assert!(view.active_leaves.is_empty());
	assert!(view.candidate_storage.is_empty());
}

// Send some candidates and make sure all are found:
// - Two for the same leaf A
// - One for leaf B on parachain 1
// - One for leaf C on parachain 2
#[test]
fn send_candidates_and_check_if_found() {
	let test_state = TestState::default();
	let view = test_harness(|mut virtual_overseer| async move {
		// Leaf A
		let leaf_a = TestLeaf {
			number: 100,
			hash: Hash::from_low_u64_be(130),
			para_data: vec![
				(1.into(), PerParaData::new(97, HeadData(vec![1, 2, 3]))),
				(2.into(), PerParaData::new(100, HeadData(vec![2, 3, 4]))),
			],
		};
		// Leaf B
		let leaf_b = TestLeaf {
			number: 101,
			hash: Hash::from_low_u64_be(131),
			para_data: vec![
				(1.into(), PerParaData::new(99, HeadData(vec![3, 4, 5]))),
				(2.into(), PerParaData::new(101, HeadData(vec![4, 5, 6]))),
			],
		};
		// Leaf C
		let leaf_c = TestLeaf {
			number: 102,
			hash: Hash::from_low_u64_be(132),
			para_data: vec![
				(1.into(), PerParaData::new(102, HeadData(vec![5, 6, 7]))),
				(2.into(), PerParaData::new(98, HeadData(vec![6, 7, 8]))),
			],
		};

		// Activate leaves.
		activate_leaf(&mut virtual_overseer, &leaf_a, &test_state).await;
		activate_leaf(&mut virtual_overseer, &leaf_b, &test_state).await;
		activate_leaf(&mut virtual_overseer, &leaf_c, &test_state).await;

		// Candidate A1
		let (candidate_a1, pvd_a1) =
			make_candidate(&leaf_a, 1.into(), test_state.validation_code_hash);
		let candidate_hash_a1 = candidate_a1.hash();
		// TODO: Is this correct?
		let response_a1 = vec![(leaf_a.hash, vec![0, 1, 2, 3, 4])];

		// Candidate A2
		let (candidate_a2, pvd_a2) =
			make_candidate(&leaf_a, 2.into(), test_state.validation_code_hash);
		let candidate_hash_a2 = candidate_a2.hash();
		// TODO: Is this correct?
		let response_a2 = vec![(leaf_a.hash, vec![0, 1, 2, 3, 4])];

		// Candidate B
		let (candidate_b, pvd_b) =
			make_candidate(&leaf_b, 1.into(), test_state.validation_code_hash);
		let candidate_hash_b = candidate_b.hash();
		// TODO: Is this correct?
		let response_b = vec![(leaf_b.hash, vec![0, 1, 2, 3, 4])];

		// Candidate C
		let (candidate_c, pvd_c) =
			make_candidate(&leaf_c, 2.into(), test_state.validation_code_hash);
		let candidate_hash_c = candidate_c.hash();
		// TODO: Is this correct?
		let response_c = vec![(leaf_c.hash, vec![0, 1, 2, 3, 4])];

		// Second candidates.
		second_candidate(&mut virtual_overseer, candidate_a1, pvd_a1, response_a1.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_a2, pvd_a2, response_a2.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_b, pvd_b, response_b.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_c, pvd_c, response_c.clone()).await;

		// Check candidate tree membership.
		check_membership(&mut virtual_overseer, 1.into(), candidate_hash_a1, response_a1).await;
		check_membership(&mut virtual_overseer, 2.into(), candidate_hash_a2, response_a2).await;
		check_membership(&mut virtual_overseer, 1.into(), candidate_hash_b, response_b).await;
		check_membership(&mut virtual_overseer, 2.into(), candidate_hash_c, response_c).await;

		// The candidates should not be found on other parachains.
		check_membership(&mut virtual_overseer, 2.into(), candidate_hash_a1, vec![]).await;
		check_membership(&mut virtual_overseer, 1.into(), candidate_hash_a2, vec![]).await;
		check_membership(&mut virtual_overseer, 2.into(), candidate_hash_b, vec![]).await;
		check_membership(&mut virtual_overseer, 1.into(), candidate_hash_c, vec![]).await;

		virtual_overseer
	});

	assert_eq!(view.active_leaves.len(), 3);
	assert_eq!(view.candidate_storage.len(), 2);
}

// Send some candidates, check if the candidate won't be found once its relay parent leaves the view.
#[test]
fn check_candidate_parent_leaving_view() {
	let test_state = TestState::default();
	let view = test_harness(|mut virtual_overseer| async move {
		// Leaf A
		let leaf_a = TestLeaf {
			number: 100,
			hash: Hash::from_low_u64_be(130),
			para_data: vec![
				(1.into(), PerParaData::new(97, HeadData(vec![1, 2, 3]))),
				(2.into(), PerParaData::new(100, HeadData(vec![2, 3, 4]))),
			],
		};
		// Leaf B
		let leaf_b = TestLeaf {
			number: 101,
			hash: Hash::from_low_u64_be(131),
			para_data: vec![
				(1.into(), PerParaData::new(99, HeadData(vec![3, 4, 5]))),
				(2.into(), PerParaData::new(101, HeadData(vec![4, 5, 6]))),
			],
		};
		// Leaf C
		let leaf_c = TestLeaf {
			number: 102,
			hash: Hash::from_low_u64_be(132),
			para_data: vec![
				(1.into(), PerParaData::new(102, HeadData(vec![5, 6, 7]))),
				(2.into(), PerParaData::new(98, HeadData(vec![6, 7, 8]))),
			],
		};

		// Activate leaves.
		activate_leaf(&mut virtual_overseer, &leaf_a, &test_state).await;
		activate_leaf(&mut virtual_overseer, &leaf_b, &test_state).await;
		activate_leaf(&mut virtual_overseer, &leaf_c, &test_state).await;

		// Candidate A1
		let (candidate_a1, pvd_a1) =
			make_candidate(&leaf_a, 1.into(), test_state.validation_code_hash);
		let candidate_hash_a1 = candidate_a1.hash();
		// TODO: Is this correct?
		let response_a1 = vec![(leaf_a.hash, vec![0, 1, 2, 3, 4])];

		// Candidate A2
		let (candidate_a2, pvd_a2) =
			make_candidate(&leaf_a, 2.into(), test_state.validation_code_hash);
		let candidate_hash_a2 = candidate_a2.hash();
		// TODO: Is this correct?
		let response_a2 = vec![(leaf_a.hash, vec![0, 1, 2, 3, 4])];

		// Candidate B
		let (candidate_b, pvd_b) =
			make_candidate(&leaf_b, 1.into(), test_state.validation_code_hash);
		let candidate_hash_b = candidate_b.hash();
		// TODO: Is this correct?
		let response_b = vec![(leaf_b.hash, vec![0, 1, 2, 3, 4])];

		// Candidate C
		let (candidate_c, pvd_c) =
			make_candidate(&leaf_c, 2.into(), test_state.validation_code_hash);
		let candidate_hash_c = candidate_c.hash();
		// TODO: Is this correct?
		let response_c = vec![(leaf_c.hash, vec![0, 1, 2, 3, 4])];

		// Second candidates.
		second_candidate(&mut virtual_overseer, candidate_a1, pvd_a1, response_a1.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_a2, pvd_a2, response_a2.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_b, pvd_b, response_b.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_c, pvd_c, response_c.clone()).await;

		// Deactivate leaf A.
		deactivate_leaf(&mut virtual_overseer, leaf_a.hash).await;

		// Candidates A1 and A2 should be gone. Candidates B and C should remain.
		check_membership(&mut virtual_overseer, 1.into(), candidate_hash_a1, vec![]).await;
		check_membership(&mut virtual_overseer, 2.into(), candidate_hash_a2, vec![]).await;
		check_membership(&mut virtual_overseer, 1.into(), candidate_hash_b, response_b).await;
		check_membership(&mut virtual_overseer, 2.into(), candidate_hash_c, response_c.clone())
			.await;

		// Deactivate leaf B.
		deactivate_leaf(&mut virtual_overseer, leaf_b.hash).await;

		// Candidate B should be gone, C should remain.
		check_membership(&mut virtual_overseer, 1.into(), candidate_hash_a1, vec![]).await;
		check_membership(&mut virtual_overseer, 2.into(), candidate_hash_a2, vec![]).await;
		check_membership(&mut virtual_overseer, 1.into(), candidate_hash_b, vec![]).await;
		check_membership(&mut virtual_overseer, 2.into(), candidate_hash_c, response_c).await;

		// Deactivate leaf C.
		deactivate_leaf(&mut virtual_overseer, leaf_c.hash).await;

		// Candidate C should be gone.
		check_membership(&mut virtual_overseer, 1.into(), candidate_hash_a1, vec![]).await;
		check_membership(&mut virtual_overseer, 2.into(), candidate_hash_a2, vec![]).await;
		check_membership(&mut virtual_overseer, 1.into(), candidate_hash_b, vec![]).await;
		check_membership(&mut virtual_overseer, 2.into(), candidate_hash_c, vec![]).await;

		virtual_overseer
	});

	assert_eq!(view.active_leaves.len(), 0);
	assert_eq!(view.candidate_storage.len(), 0);
}

// Introduce a candidate to multiple forks, see how the membership is returned.
#[test]
fn check_candidate_on_multiple_forks() {
	let test_state = TestState::default();
	let view = test_harness(|mut virtual_overseer| async move {
		// Leaf A
		let leaf_a = TestLeaf {
			number: 100,
			hash: Hash::from_low_u64_be(130),
			para_data: vec![
				(1.into(), PerParaData::new(97, HeadData(vec![1, 2, 3]))),
				(2.into(), PerParaData::new(100, HeadData(vec![2, 3, 4]))),
			],
		};
		// Leaf B
		let leaf_b = TestLeaf {
			number: 101,
			hash: Hash::from_low_u64_be(131),
			para_data: vec![
				(1.into(), PerParaData::new(99, HeadData(vec![3, 4, 5]))),
				(2.into(), PerParaData::new(101, HeadData(vec![4, 5, 6]))),
			],
		};
		// Leaf C
		let leaf_c = TestLeaf {
			number: 102,
			hash: Hash::from_low_u64_be(132),
			para_data: vec![
				(1.into(), PerParaData::new(102, HeadData(vec![5, 6, 7]))),
				(2.into(), PerParaData::new(98, HeadData(vec![6, 7, 8]))),
			],
		};

		// Activate leaves.
		activate_leaf(&mut virtual_overseer, &leaf_a, &test_state).await;
		activate_leaf(&mut virtual_overseer, &leaf_b, &test_state).await;
		activate_leaf(&mut virtual_overseer, &leaf_c, &test_state).await;

		// Candidate
		let (candidate, pvd) = make_candidate(&leaf_a, 1.into(), test_state.validation_code_hash);
		let candidate_hash = candidate.hash();
		// TODO: Is this correct?
		let response = vec![(leaf_a.hash, vec![0, 1, 2, 3, 4])];

		// Second candidate on all three leaves.
		second_candidate(&mut virtual_overseer, candidate.clone(), pvd.clone(), response.clone())
			.await;
		second_candidate(
			&mut virtual_overseer,
			candidate.clone(),
			pvd.clone(),
			// TODO: is this right?
			vec![],
		)
		.await;
		second_candidate(
			&mut virtual_overseer,
			candidate.clone(),
			pvd.clone(),
			// TODO: is this right?
			response.clone(),
		)
		.await;

		// Check membership.

		virtual_overseer
	});

	assert_eq!(view.active_leaves.len(), 3);
	assert_eq!(view.candidate_storage.len(), 2);
}

#[test]
fn check_backable_query() {
	todo!();

	// TODO: Get backable candidate.
	// println!("\nGetting backable candidate...\n");
	// let required_path = vec![candidate_hash];
	// let (tx, rx) = oneshot::channel();
	// virtual_overseer
	// 	.send(overseer::FromOrchestra::Communication {
	// 		msg: ProspectiveParachainsMessage::GetBackableCandidate(
	// 			leaf_hash,
	// 			para_id,
	// 			required_path,
	// 			tx,
	// 		),
	// 	})
	// 	.await;
	// let resp = rx.await.unwrap();
	// println!("Get Backable Resp: {:#?}", resp);
}

// #[test]
// fn correctly_updates_leaves() {
// 	let first_block_hash = [1; 32].into();
// 	let second_block_hash = [2; 32].into();
// 	let third_block_hash = [3; 32].into();

// 	let pool = TaskExecutor::new();
// 	let (mut ctx, mut ctx_handle) =
// 		test_helpers::make_subsystem_context::<ProspectiveParachainsMessage, _>(pool.clone());

// 	let mut view = View::new();

// 	let check_fut = async move {
// 		// Activate a leaf.
// 		let activated = ActivatedLeaf {
// 			hash: first_block_hash,
// 			number: 1,
// 			span: Arc::new(jaeger::Span::Disabled),
// 			status: LeafStatus::Fresh,
// 		};
// 		let update = ActiveLeavesUpdate::start_work(activated);
// 		handle_active_leaves_update(&mut ctx, &mut view, update).await.unwrap();

// 		// Activate another leaf.
// 		let activated = ActivatedLeaf {
// 			hash: second_block_hash,
// 			number: 2,
// 			span: Arc::new(jaeger::Span::Disabled),
// 			status: LeafStatus::Fresh,
// 		};
// 		let update = ActiveLeavesUpdate::start_work(activated);
// 		handle_active_leaves_update(&mut ctx, &mut view, update.clone()).await.unwrap();

// 		// Try activating a duplicate leaf.
// 		handle_active_leaves_update(&mut ctx, &mut view, update).await.unwrap();

// 		// Pass in an empty update.
// 		let update = ActiveLeavesUpdate::default();
// 		handle_active_leaves_update(&mut ctx, &mut view, update).await.unwrap();

// 		// Activate a leaf and remove one at the same time.
// 		let activated = ActivatedLeaf {
// 			hash: third_block_hash,
// 			number: 3,
// 			span: Arc::new(jaeger::Span::Disabled),
// 			status: LeafStatus::Fresh,
// 		};
// 		let update = ActiveLeavesUpdate {
// 			activated: Some(activated),
// 			deactivated: [second_block_hash][..].into(),
// 		};
// 		handle_active_leaves_update(&mut ctx, &mut view, update).await.unwrap();

// 		// TODO: Check tree contents.
// 		assert_eq!(view.active_leaves.len(), 2);

// 		// Remove all remaining leaves.
// 		let update = ActiveLeavesUpdate {
// 			deactivated: [first_block_hash, third_block_hash][..].into(),
// 			..Default::default()
// 		};
// 		handle_active_leaves_update(&mut ctx, &mut view, update).await.unwrap();

// 		// Check final tree contents.
// 		assert!(view.active_leaves.is_empty() && view.candidate_storage.is_empty());
// 	};

// 	let test_fut = async move {
// 		assert_matches!(
// 			ctx_handle.recv().await,
// 			AllMessages::RuntimeApi(
// 				RuntimeApiMessage::Request(parent, RuntimeApiRequest::StagingAsyncBackingParameters(tx))
// 			) if parent == first_block_hash => {
// 				tx.send(Ok(ASYNC_BACKING_PARAMETERS)).unwrap();
// 			}
// 		);

// 		// TODO: Fill this out.

// 		let message = ctx_handle.recv().await;
// 		println!("{:?}", message);
// 	};

// 	let test_fut = future::join(test_fut, check_fut);
// 	executor::block_on(test_fut);
// }
