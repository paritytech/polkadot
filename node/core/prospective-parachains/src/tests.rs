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
	messages::{
		AllMessages, HypotheticalFrontierRequest, ProspectiveParachainsMessage,
		ProspectiveValidationDataRequest,
	},
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_types::{jaeger, ActivatedLeaf, LeafStatus};
use polkadot_primitives::{
	v2::{
		CandidateCommitments, HeadData, Header, PersistedValidationData, ScheduledCore,
		ValidationCodeHash,
	},
	vstaging::{AsyncBackingParameters, Constraints, InboundHrmpLimitations},
};
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
	parent_head: HeadData,
	head_data: HeadData,
	validation_code_hash: ValidationCodeHash,
) -> (CommittedCandidateReceipt, PersistedValidationData) {
	let pvd = dummy_pvd(parent_head, leaf.number);
	let commitments = CandidateCommitments {
		head_data,
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

struct TestState {
	availability_cores: Vec<CoreState>,
	validation_code_hash: ValidationCodeHash,
}

impl Default for TestState {
	fn default() -> Self {
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);

		let availability_cores = vec![
			CoreState::Scheduled(ScheduledCore { para_id: chain_a, collator: None }),
			CoreState::Scheduled(ScheduledCore { para_id: chain_b, collator: None }),
		];
		let validation_code_hash = Hash::repeat_byte(42).into();

		Self { availability_cores, validation_code_hash }
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
			match run_iteration(&mut context, &mut view, &Metrics(None)).await {
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
	head_data: HeadData,
}

impl PerParaData {
	pub fn new(min_relay_parent: BlockNumber, head_data: HeadData) -> Self {
		Self { min_relay_parent, head_data }
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

async fn send_block_header(virtual_overseer: &mut VirtualOverseer, hash: Hash, number: u32) {
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

async fn activate_leaf(
	virtual_overseer: &mut VirtualOverseer,
	leaf: &TestLeaf,
	test_state: &TestState,
) {
	let TestLeaf { number, hash, para_data: _ } = leaf;

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

	handle_leaf_activation(virtual_overseer, leaf, test_state).await;
}

async fn handle_leaf_activation(
	virtual_overseer: &mut VirtualOverseer,
	leaf: &TestLeaf,
	test_state: &TestState,
) {
	let TestLeaf { number, hash, para_data } = leaf;

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

	send_block_header(virtual_overseer, *hash, *number).await;

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
		send_block_header(virtual_overseer, hash, number).await;
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

		let PerParaData { min_relay_parent, head_data } = leaf.para_data(para_id);
		let constraints = dummy_constraints(
			*min_relay_parent,
			vec![*number],
			head_data.clone(),
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

async fn back_candidate(
	virtual_overseer: &mut VirtualOverseer,
	candidate: &CommittedCandidateReceipt,
	candidate_hash: CandidateHash,
) {
	virtual_overseer
		.send(overseer::FromOrchestra::Communication {
			msg: ProspectiveParachainsMessage::CandidateBacked(
				candidate.descriptor.para_id,
				candidate_hash,
			),
		})
		.await;
}

async fn get_membership(
	virtual_overseer: &mut VirtualOverseer,
	para_id: ParaId,
	candidate_hash: CandidateHash,
	expected_membership_response: Vec<(Hash, Vec<usize>)>,
) {
	let (tx, rx) = oneshot::channel();
	virtual_overseer
		.send(overseer::FromOrchestra::Communication {
			msg: ProspectiveParachainsMessage::GetTreeMembership(para_id, candidate_hash, tx),
		})
		.await;
	let resp = rx.await.unwrap();
	assert_eq!(resp, expected_membership_response);
}

async fn get_backable_candidate(
	virtual_overseer: &mut VirtualOverseer,
	leaf: &TestLeaf,
	para_id: ParaId,
	required_path: Vec<CandidateHash>,
	expected_candidate_hash: Option<CandidateHash>,
) {
	let (tx, rx) = oneshot::channel();
	virtual_overseer
		.send(overseer::FromOrchestra::Communication {
			msg: ProspectiveParachainsMessage::GetBackableCandidate(
				leaf.hash,
				para_id,
				required_path,
				tx,
			),
		})
		.await;
	let resp = rx.await.unwrap();
	assert_eq!(resp, expected_candidate_hash);
}

async fn get_hypothetical_frontier(
	virtual_overseer: &mut VirtualOverseer,
	candidate_hash: CandidateHash,
	receipt: CommittedCandidateReceipt,
	persisted_validation_data: PersistedValidationData,
	fragment_tree_relay_parent: Hash,
	expected_depths: Vec<usize>,
) {
	let hypothetical_candidate = HypotheticalCandidate::Complete {
		candidate_hash,
		receipt: Arc::new(receipt),
		persisted_validation_data,
	};
	let request = HypotheticalFrontierRequest {
		candidates: vec![hypothetical_candidate.clone()],
		fragment_tree_relay_parent: Some(fragment_tree_relay_parent),
	};
	let (tx, rx) = oneshot::channel();
	virtual_overseer
		.send(overseer::FromOrchestra::Communication {
			msg: ProspectiveParachainsMessage::GetHypotheticalFrontier(request, tx),
		})
		.await;
	let resp = rx.await.unwrap();
	let expected_frontier =
		vec![(hypothetical_candidate, vec![(fragment_tree_relay_parent, expected_depths)])];
	assert_eq!(resp, expected_frontier);
}

async fn get_pvd(
	virtual_overseer: &mut VirtualOverseer,
	para_id: ParaId,
	candidate_relay_parent: Hash,
	parent_head_data: HeadData,
	expected_pvd: Option<PersistedValidationData>,
) {
	let request = ProspectiveValidationDataRequest {
		para_id,
		candidate_relay_parent,
		parent_head_data_hash: parent_head_data.hash(),
	};
	let (tx, rx) = oneshot::channel();
	virtual_overseer
		.send(overseer::FromOrchestra::Communication {
			msg: ProspectiveParachainsMessage::GetProspectiveValidationData(request, tx),
		})
		.await;
	let resp = rx.await.unwrap();
	assert_eq!(resp, expected_pvd);
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
		let (candidate_a1, pvd_a1) = make_candidate(
			&leaf_a,
			1.into(),
			HeadData(vec![1, 2, 3]),
			HeadData(vec![1]),
			test_state.validation_code_hash,
		);
		let candidate_hash_a1 = candidate_a1.hash();
		let response_a1 = vec![(leaf_a.hash, vec![0])];

		// Candidate A2
		let (candidate_a2, pvd_a2) = make_candidate(
			&leaf_a,
			2.into(),
			HeadData(vec![2, 3, 4]),
			HeadData(vec![2]),
			test_state.validation_code_hash,
		);
		let candidate_hash_a2 = candidate_a2.hash();
		let response_a2 = vec![(leaf_a.hash, vec![0])];

		// Candidate B
		let (candidate_b, pvd_b) = make_candidate(
			&leaf_b,
			1.into(),
			HeadData(vec![3, 4, 5]),
			HeadData(vec![3]),
			test_state.validation_code_hash,
		);
		let candidate_hash_b = candidate_b.hash();
		let response_b = vec![(leaf_b.hash, vec![0])];

		// Candidate C
		let (candidate_c, pvd_c) = make_candidate(
			&leaf_c,
			2.into(),
			HeadData(vec![6, 7, 8]),
			HeadData(vec![4]),
			test_state.validation_code_hash,
		);
		let candidate_hash_c = candidate_c.hash();
		let response_c = vec![(leaf_c.hash, vec![0])];

		// Second candidates.
		second_candidate(&mut virtual_overseer, candidate_a1, pvd_a1, response_a1.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_a2, pvd_a2, response_a2.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_b, pvd_b, response_b.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_c, pvd_c, response_c.clone()).await;

		// Check candidate tree membership.
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_a1, response_a1).await;
		get_membership(&mut virtual_overseer, 2.into(), candidate_hash_a2, response_a2).await;
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_b, response_b).await;
		get_membership(&mut virtual_overseer, 2.into(), candidate_hash_c, response_c).await;

		// The candidates should not be found on other parachains.
		get_membership(&mut virtual_overseer, 2.into(), candidate_hash_a1, vec![]).await;
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_a2, vec![]).await;
		get_membership(&mut virtual_overseer, 2.into(), candidate_hash_b, vec![]).await;
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_c, vec![]).await;

		virtual_overseer
	});

	assert_eq!(view.active_leaves.len(), 3);
	assert_eq!(view.candidate_storage.len(), 2);
	// Two parents and two candidates per para.
	assert_eq!(view.candidate_storage.get(&1.into()).unwrap().len(), (2, 2));
	assert_eq!(view.candidate_storage.get(&2.into()).unwrap().len(), (2, 2));
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
		let (candidate_a1, pvd_a1) = make_candidate(
			&leaf_a,
			1.into(),
			HeadData(vec![1, 2, 3]),
			HeadData(vec![1]),
			test_state.validation_code_hash,
		);
		let candidate_hash_a1 = candidate_a1.hash();
		let response_a1 = vec![(leaf_a.hash, vec![0])];

		// Candidate A2
		let (candidate_a2, pvd_a2) = make_candidate(
			&leaf_a,
			2.into(),
			HeadData(vec![2, 3, 4]),
			HeadData(vec![2]),
			test_state.validation_code_hash,
		);
		let candidate_hash_a2 = candidate_a2.hash();
		let response_a2 = vec![(leaf_a.hash, vec![0])];

		// Candidate B
		let (candidate_b, pvd_b) = make_candidate(
			&leaf_b,
			1.into(),
			HeadData(vec![3, 4, 5]),
			HeadData(vec![3]),
			test_state.validation_code_hash,
		);
		let candidate_hash_b = candidate_b.hash();
		let response_b = vec![(leaf_b.hash, vec![0])];

		// Candidate C
		let (candidate_c, pvd_c) = make_candidate(
			&leaf_c,
			2.into(),
			HeadData(vec![6, 7, 8]),
			HeadData(vec![4]),
			test_state.validation_code_hash,
		);
		let candidate_hash_c = candidate_c.hash();
		let response_c = vec![(leaf_c.hash, vec![0])];

		// Second candidates.
		second_candidate(&mut virtual_overseer, candidate_a1, pvd_a1, response_a1.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_a2, pvd_a2, response_a2.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_b, pvd_b, response_b.clone()).await;
		second_candidate(&mut virtual_overseer, candidate_c, pvd_c, response_c.clone()).await;

		// Deactivate leaf A.
		deactivate_leaf(&mut virtual_overseer, leaf_a.hash).await;

		// Candidates A1 and A2 should be gone. Candidates B and C should remain.
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_a1, vec![]).await;
		get_membership(&mut virtual_overseer, 2.into(), candidate_hash_a2, vec![]).await;
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_b, response_b).await;
		get_membership(&mut virtual_overseer, 2.into(), candidate_hash_c, response_c.clone()).await;

		// Deactivate leaf B.
		deactivate_leaf(&mut virtual_overseer, leaf_b.hash).await;

		// Candidate B should be gone, C should remain.
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_a1, vec![]).await;
		get_membership(&mut virtual_overseer, 2.into(), candidate_hash_a2, vec![]).await;
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_b, vec![]).await;
		get_membership(&mut virtual_overseer, 2.into(), candidate_hash_c, response_c).await;

		// Deactivate leaf C.
		deactivate_leaf(&mut virtual_overseer, leaf_c.hash).await;

		// Candidate C should be gone.
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_a1, vec![]).await;
		get_membership(&mut virtual_overseer, 2.into(), candidate_hash_a2, vec![]).await;
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_b, vec![]).await;
		get_membership(&mut virtual_overseer, 2.into(), candidate_hash_c, vec![]).await;

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

		// Candidate on leaf A.
		let (candidate_a, pvd_a) = make_candidate(
			&leaf_a,
			1.into(),
			HeadData(vec![1, 2, 3]),
			HeadData(vec![1]),
			test_state.validation_code_hash,
		);
		let candidate_hash_a = candidate_a.hash();
		let response_a = vec![(leaf_a.hash, vec![0])];

		// Candidate on leaf B.
		let (candidate_b, pvd_b) = make_candidate(
			&leaf_b,
			1.into(),
			HeadData(vec![3, 4, 5]),
			HeadData(vec![1]),
			test_state.validation_code_hash,
		);
		let candidate_hash_b = candidate_b.hash();
		let response_b = vec![(leaf_b.hash, vec![0])];

		// Candidate on leaf C.
		let (candidate_c, pvd_c) = make_candidate(
			&leaf_c,
			1.into(),
			HeadData(vec![5, 6, 7]),
			HeadData(vec![1]),
			test_state.validation_code_hash,
		);
		let candidate_hash_c = candidate_c.hash();
		let response_c = vec![(leaf_c.hash, vec![0])];

		// Second candidate on all three leaves.
		second_candidate(
			&mut virtual_overseer,
			candidate_a.clone(),
			pvd_a.clone(),
			response_a.clone(),
		)
		.await;
		second_candidate(
			&mut virtual_overseer,
			candidate_b.clone(),
			pvd_b.clone(),
			response_b.clone(),
		)
		.await;
		second_candidate(
			&mut virtual_overseer,
			candidate_c.clone(),
			pvd_c.clone(),
			response_c.clone(),
		)
		.await;

		// Check candidate tree membership.
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_a, response_a).await;
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_b, response_b).await;
		get_membership(&mut virtual_overseer, 1.into(), candidate_hash_c, response_c).await;

		virtual_overseer
	});

	assert_eq!(view.active_leaves.len(), 3);
	assert_eq!(view.candidate_storage.len(), 2);
	// Three parents and three candidates on para 1.
	assert_eq!(view.candidate_storage.get(&1.into()).unwrap().len(), (3, 3));
	assert_eq!(view.candidate_storage.get(&2.into()).unwrap().len(), (0, 0));
}

// Backs some candidates and tests `GetBackableCandidate`.
#[test]
fn check_backable_query() {
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

		// Activate leaves.
		activate_leaf(&mut virtual_overseer, &leaf_a, &test_state).await;

		// Candidate A
		let (candidate_a, pvd_a) = make_candidate(
			&leaf_a,
			1.into(),
			HeadData(vec![1, 2, 3]),
			HeadData(vec![1]),
			test_state.validation_code_hash,
		);
		let candidate_hash_a = candidate_a.hash();
		let response_a = vec![(leaf_a.hash, vec![0])];

		// Candidate B
		let (mut candidate_b, pvd_b) = make_candidate(
			&leaf_a,
			1.into(),
			HeadData(vec![1]),
			HeadData(vec![2]),
			test_state.validation_code_hash,
		);
		// Set a field to make this candidate unique.
		candidate_b.descriptor.para_head = Hash::from_low_u64_le(1000);
		let candidate_hash_b = candidate_b.hash();
		let response_b = vec![(leaf_a.hash, vec![1])];

		// Second candidates.
		second_candidate(
			&mut virtual_overseer,
			candidate_a.clone(),
			pvd_a.clone(),
			response_a.clone(),
		)
		.await;
		second_candidate(
			&mut virtual_overseer,
			candidate_b.clone(),
			pvd_b.clone(),
			response_b.clone(),
		)
		.await;

		// Should not get any backable candidates.
		get_backable_candidate(
			&mut virtual_overseer,
			&leaf_a,
			1.into(),
			vec![candidate_hash_a],
			None,
		)
		.await;

		// Back candidates.
		back_candidate(&mut virtual_overseer, &candidate_a, candidate_hash_a).await;
		back_candidate(&mut virtual_overseer, &candidate_b, candidate_hash_b).await;

		// Get backable candidate.
		get_backable_candidate(
			&mut virtual_overseer,
			&leaf_a,
			1.into(),
			vec![],
			Some(candidate_hash_a),
		)
		.await;
		get_backable_candidate(
			&mut virtual_overseer,
			&leaf_a,
			1.into(),
			vec![candidate_hash_a],
			Some(candidate_hash_b),
		)
		.await;

		// Should not get anything at the wrong path.
		get_backable_candidate(
			&mut virtual_overseer,
			&leaf_a,
			1.into(),
			vec![candidate_hash_b],
			None,
		)
		.await;

		virtual_overseer
	});

	assert_eq!(view.active_leaves.len(), 1);
	assert_eq!(view.candidate_storage.len(), 2);
	// Two parents and two candidates on para 1.
	assert_eq!(view.candidate_storage.get(&1.into()).unwrap().len(), (2, 2));
	assert_eq!(view.candidate_storage.get(&2.into()).unwrap().len(), (0, 0));
}

// Test depth query.
#[test]
fn check_depth_query() {
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

		// Activate leaves.
		activate_leaf(&mut virtual_overseer, &leaf_a, &test_state).await;

		// Candidate A.
		let (candidate_a, pvd_a) = make_candidate(
			&leaf_a,
			1.into(),
			HeadData(vec![1, 2, 3]),
			HeadData(vec![1]),
			test_state.validation_code_hash,
		);
		let candidate_hash_a = candidate_a.hash();
		let response_a = vec![(leaf_a.hash, vec![0])];

		// Candidate B.
		let (candidate_b, pvd_b) = make_candidate(
			&leaf_a,
			1.into(),
			HeadData(vec![1]),
			HeadData(vec![2]),
			test_state.validation_code_hash,
		);
		let candidate_hash_b = candidate_b.hash();
		let response_b = vec![(leaf_a.hash, vec![1])];

		// Candidate C.
		let (candidate_c, pvd_c) = make_candidate(
			&leaf_a,
			1.into(),
			HeadData(vec![2]),
			HeadData(vec![3]),
			test_state.validation_code_hash,
		);
		let candidate_hash_c = candidate_c.hash();
		let response_c = vec![(leaf_a.hash, vec![2])];

		// Get hypothetical frontier of candidate A before adding it.
		get_hypothetical_frontier(
			&mut virtual_overseer,
			candidate_hash_a,
			candidate_a.clone(),
			pvd_a.clone(),
			leaf_a.hash,
			vec![0],
		)
		.await;

		// Add candidate A.
		second_candidate(
			&mut virtual_overseer,
			candidate_a.clone(),
			pvd_a.clone(),
			response_a.clone(),
		)
		.await;

		// Get frontier of candidate A after adding it.
		get_hypothetical_frontier(
			&mut virtual_overseer,
			candidate_hash_a,
			candidate_a.clone(),
			pvd_a.clone(),
			leaf_a.hash,
			vec![0],
		)
		.await;

		// Get hypothetical frontier of candidate B before adding it.
		get_hypothetical_frontier(
			&mut virtual_overseer,
			candidate_hash_b,
			candidate_b.clone(),
			pvd_b.clone(),
			leaf_a.hash,
			vec![1],
		)
		.await;

		// Add candidate B.
		second_candidate(
			&mut virtual_overseer,
			candidate_b.clone(),
			pvd_b.clone(),
			response_b.clone(),
		)
		.await;

		// Get frontier of candidate B after adding it.
		get_hypothetical_frontier(
			&mut virtual_overseer,
			candidate_hash_b,
			candidate_b,
			pvd_b.clone(),
			leaf_a.hash,
			vec![1],
		)
		.await;

		// Get hypothetical frontier of candidate C before adding it.
		get_hypothetical_frontier(
			&mut virtual_overseer,
			candidate_hash_c,
			candidate_c.clone(),
			pvd_c.clone(),
			leaf_a.hash,
			vec![2],
		)
		.await;

		// Add candidate C.
		second_candidate(
			&mut virtual_overseer,
			candidate_c.clone(),
			pvd_c.clone(),
			response_c.clone(),
		)
		.await;

		// Get frontier of candidate C after adding it.
		get_hypothetical_frontier(
			&mut virtual_overseer,
			candidate_hash_c,
			candidate_c,
			pvd_c.clone(),
			leaf_a.hash,
			vec![2],
		)
		.await;

		virtual_overseer
	});

	assert_eq!(view.active_leaves.len(), 1);
	assert_eq!(view.candidate_storage.len(), 2);
}

#[test]
fn check_pvd_query() {
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

		// Activate leaves.
		activate_leaf(&mut virtual_overseer, &leaf_a, &test_state).await;

		// Candidate A.
		let (candidate_a, pvd_a) = make_candidate(
			&leaf_a,
			1.into(),
			HeadData(vec![1, 2, 3]),
			HeadData(vec![1]),
			test_state.validation_code_hash,
		);
		let response_a = vec![(leaf_a.hash, vec![0])];

		// Candidate B.
		let (candidate_b, pvd_b) = make_candidate(
			&leaf_a,
			1.into(),
			HeadData(vec![1]),
			HeadData(vec![2]),
			test_state.validation_code_hash,
		);
		let response_b = vec![(leaf_a.hash, vec![1])];

		// Candidate C.
		let (candidate_c, pvd_c) = make_candidate(
			&leaf_a,
			1.into(),
			HeadData(vec![2]),
			HeadData(vec![3]),
			test_state.validation_code_hash,
		);
		let response_c = vec![(leaf_a.hash, vec![2])];

		// Get pvd of candidate A before adding it.
		get_pvd(
			&mut virtual_overseer,
			1.into(),
			leaf_a.hash,
			HeadData(vec![1, 2, 3]),
			Some(pvd_a.clone()),
		)
		.await;

		// Add candidate A.
		second_candidate(
			&mut virtual_overseer,
			candidate_a.clone(),
			pvd_a.clone(),
			response_a.clone(),
		)
		.await;
		back_candidate(&mut virtual_overseer, &candidate_a, candidate_a.hash()).await;

		// Get pvd of candidate A after adding it.
		get_pvd(
			&mut virtual_overseer,
			1.into(),
			leaf_a.hash,
			HeadData(vec![1, 2, 3]),
			Some(pvd_a.clone()),
		)
		.await;

		// Get pvd of candidate B before adding it.
		get_pvd(
			&mut virtual_overseer,
			1.into(),
			leaf_a.hash,
			HeadData(vec![1]),
			Some(pvd_b.clone()),
		)
		.await;

		// Add candidate B.
		second_candidate(&mut virtual_overseer, candidate_b, pvd_b.clone(), response_b.clone())
			.await;

		// Get pvd of candidate B after adding it.
		get_pvd(
			&mut virtual_overseer,
			1.into(),
			leaf_a.hash,
			HeadData(vec![1]),
			Some(pvd_b.clone()),
		)
		.await;

		// Get pvd of candidate C before adding it.
		get_pvd(
			&mut virtual_overseer,
			1.into(),
			leaf_a.hash,
			HeadData(vec![2]),
			Some(pvd_c.clone()),
		)
		.await;

		// Add candidate C.
		second_candidate(&mut virtual_overseer, candidate_c, pvd_c.clone(), response_c.clone())
			.await;

		// Get pvd of candidate C after adding it.
		get_pvd(
			&mut virtual_overseer,
			1.into(),
			leaf_a.hash,
			HeadData(vec![2]),
			Some(pvd_c.clone()),
		)
		.await;

		virtual_overseer
	});

	assert_eq!(view.active_leaves.len(), 1);
	assert_eq!(view.candidate_storage.len(), 2);
}

// Test simultaneously activating and deactivating leaves, and simultaneously deactivating multiple
// leaves.
#[test]
fn correctly_updates_leaves() {
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

		// Try activating a duplicate leaf.
		activate_leaf(&mut virtual_overseer, &leaf_b, &test_state).await;

		// Pass in an empty update.
		let update = ActiveLeavesUpdate::default();
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)))
			.await;

		// Activate a leaf and remove one at the same time.
		let activated = ActivatedLeaf {
			hash: leaf_c.hash,
			number: leaf_c.number,
			span: Arc::new(jaeger::Span::Disabled),
			status: LeafStatus::Fresh,
		};
		let update = ActiveLeavesUpdate {
			activated: Some(activated),
			deactivated: [leaf_b.hash][..].into(),
		};
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)))
			.await;
		handle_leaf_activation(&mut virtual_overseer, &leaf_c, &test_state).await;

		// Remove all remaining leaves.
		let update = ActiveLeavesUpdate {
			deactivated: [leaf_a.hash, leaf_c.hash][..].into(),
			..Default::default()
		};
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)))
			.await;

		// Activate and deactivate the same leaf.
		let activated = ActivatedLeaf {
			hash: leaf_a.hash,
			number: leaf_a.number,
			span: Arc::new(jaeger::Span::Disabled),
			status: LeafStatus::Fresh,
		};
		let update = ActiveLeavesUpdate {
			activated: Some(activated),
			deactivated: [leaf_a.hash][..].into(),
		};
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)))
			.await;
		handle_leaf_activation(&mut virtual_overseer, &leaf_a, &test_state).await;

		// Remove the leaf again. Send some unnecessary hashes.
		let update = ActiveLeavesUpdate {
			deactivated: [leaf_a.hash, leaf_b.hash, leaf_c.hash][..].into(),
			..Default::default()
		};
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)))
			.await;

		virtual_overseer
	});

	assert_eq!(view.active_leaves.len(), 0);
	assert_eq!(view.candidate_storage.len(), 0);
}
