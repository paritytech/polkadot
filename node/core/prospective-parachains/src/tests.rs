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
use ::polkadot_primitives_test_helpers::dummy_hash;
use assert_matches::assert_matches;
use futures::executor;
use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	messages::{AllMessages, ProspectiveParachainsMessage, ProspectiveValidationDataRequest},
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_types::{jaeger, ActivatedLeaf, LeafStatus};
use polkadot_primitives::{
	v2::{
		CandidateDescriptor, GroupRotationInfo, HeadData, Header, PersistedValidationData,
		ScheduledCore, SessionIndex, SigningContext, ValidatorId, ValidatorIndex,
	},
	vstaging::{AsyncBackingParameters, Constraints, InboundHrmpLimitations},
};
use sp_application_crypto::AppKey;
use sp_core::testing::TaskExecutor;
use sp_keyring::Sr25519Keyring;
use sp_keystore::{CryptoStore, SyncCryptoStore, SyncCryptoStorePtr};
use std::sync::Arc;

const ALLOWED_ANCESTRY_LEN: u32 = 3;
const ASYNC_BACKING_PARAMETERS: AsyncBackingParameters =
	AsyncBackingParameters { max_candidate_depth: 4, allowed_ancestry_len: ALLOWED_ANCESTRY_LEN };

const ASYNC_BACKING_DISABLED_ERROR: RuntimeApiError =
	RuntimeApiError::NotSupported { runtime_api_name: "test-runtime" };

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<ProspectiveParachainsMessage>;

fn make_constraints(
	min_relay_parent_number: BlockNumber,
	valid_watermarks: Vec<BlockNumber>,
	required_parent: HeadData,
) -> Constraints {
	Constraints {
		min_relay_parent_number,
		max_pov_size: 1_000_000,
		max_code_size: 1_000_000,
		ump_remaining: 10,
		ump_remaining_bytes: 1_000,
		max_ump_num_per_candidate: 10,
		dmp_remaining_messages: 10,
		hrmp_inbound: InboundHrmpLimitations { valid_watermarks },
		hrmp_channels_out: vec![],
		max_hrmp_num_per_candidate: 0,
		required_parent,
		validation_code_hash: Hash::repeat_byte(42).into(),
		upgrade_restriction: None,
		future_validation_code: None,
	}
}

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

struct TestState {
	chain_ids: Vec<ParaId>,
	keystore: SyncCryptoStorePtr,
	validators: Vec<Sr25519Keyring>,
	validator_public: Vec<ValidatorId>,
	validation_data: PersistedValidationData,
	validator_groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo),
	availability_cores: Vec<CoreState>,
	head_data: HashMap<ParaId, HeadData>,
	signing_context: SigningContext,
	relay_parent: Hash,
}

impl TestState {
	fn session(&self) -> SessionIndex {
		self.signing_context.session_index
	}
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

		let mut head_data = HashMap::new();
		head_data.insert(chain_a, HeadData(vec![4, 5, 6]));
		head_data.insert(chain_b, HeadData(vec![5, 6, 7]));

		let relay_parent = Hash::repeat_byte(5);
		let relay_parent_number = 0_u32.into();

		let signing_context = SigningContext { session_index: 1, parent_hash: relay_parent };

		let validation_data = PersistedValidationData {
			parent_head: HeadData(vec![7, 8, 9]),
			relay_parent_number,
			max_pov_size: 1024,
			relay_parent_storage_root: dummy_hash(),
		};

		Self {
			chain_ids,
			keystore,
			validators,
			validator_public,
			validator_groups: (validator_groups, group_rotation_info),
			availability_cores,
			head_data,
			validation_data,
			signing_context,
			relay_parent,
		}
	}
}

struct TestLeaf {
	activated: ActivatedLeaf,
	min_relay_parents: Vec<(ParaId, u32)>,
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

async fn activate_leaf(
	virtual_overseer: &mut VirtualOverseer,
	leaf: TestLeaf,
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

	let TestLeaf { activated, min_relay_parents } = leaf;
	let leaf_hash = activated.hash;
	let leaf_number = activated.number;

	virtual_overseer
		.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			activated,
		))))
		.await;

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::StagingAsyncBackingParameters(tx))
		) if parent == leaf_hash => {
			tx.send(Ok(ASYNC_BACKING_PARAMETERS)).unwrap();
		}
	);

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::AvailabilityCores(tx))
		) if parent == leaf_hash => {
			tx.send(Ok(test_state.availability_cores.clone())).unwrap();
		}
	);

	send_header_response(virtual_overseer, leaf_hash, leaf_number).await;

	// Check that subsystem job issues a request for ancestors.
	let min_min = *min_relay_parents
		.iter()
		.map(|(_, block_num)| block_num)
		.min()
		.unwrap_or(&leaf_number);
	let ancestry_len = leaf_number - min_min;
	let ancestry_hashes: Vec<Hash> =
		std::iter::successors(Some(leaf_hash), |h| Some(get_parent_hash(*h)))
			.skip(1)
			.take(ancestry_len as usize)
			.collect();
	let ancestry_numbers = (min_min..leaf_number).rev();
	let ancestry_iter = ancestry_hashes.clone().into_iter().zip(ancestry_numbers).peekable();
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::ChainApi(
			ChainApiMessage::Ancestors{hash, k, response_channel: tx}
		) if hash == leaf_hash && k == ALLOWED_ANCESTRY_LEN as usize => {
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

		let min_relay_parent_number = *min_relay_parents
			.iter()
			.find_map(|(p_id, block_num)| if *p_id == para_id { Some(block_num) } else { None })
			.unwrap_or(&leaf_number);
		let constraints =
			make_constraints(min_relay_parent_number, vec![8, 9], vec![1, 2, 3].into());
		assert_matches!(
			message,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::StagingValidityConstraints(p_id, tx))
			) if parent == leaf_hash && p_id == para_id => {
				tx.send(Ok(Some(constraints))).unwrap();
			}
		);
	}
}

#[test]
fn should_do_no_work_if_async_backing_disabled_for_leaf() {
	async fn test_startup_async_backing_disabled(
		virtual_overseer: &mut VirtualOverseer,
		test_state: &TestState,
	) {
		// Start work on some new parent.
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: test_state.relay_parent,
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
			) if parent == test_state.relay_parent => {
				tx.send(Err(ASYNC_BACKING_DISABLED_ERROR)).unwrap();
			}
		);
	}

	let test_state = TestState::default();
	let view = test_harness(|mut virtual_overseer| async move {
		test_startup_async_backing_disabled(&mut virtual_overseer, &test_state).await;

		virtual_overseer
	});

	assert!(view.active_leaves.is_empty());
	assert!(view.candidate_storage.is_empty());
}

// Send some candidates, check if the candidate won't be found once its relay parent leaves the view.
#[test]
fn check_candidate_parent_leaving_view() {
	let test_state = TestState::default();
	let view = test_harness(|mut virtual_overseer| async move {
		const LEAF_BLOCK_NUMBER: BlockNumber = 100;
		const LEAF_ANCESTRY_LEN: BlockNumber = 3;
		let para_id = test_state.chain_ids[0];

		let leaf_hash = Hash::from_low_u64_be(130);
		let activated = ActivatedLeaf {
			hash: leaf_hash,
			number: LEAF_BLOCK_NUMBER,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		};

		let min_relay_parents = vec![(para_id, LEAF_BLOCK_NUMBER - LEAF_ANCESTRY_LEN)];

		let test_leaf = TestLeaf { activated, min_relay_parents };
		let leaf_parent = get_parent_hash(leaf_hash);

		activate_leaf(&mut virtual_overseer, test_leaf, leaf_parent, &test_state).await;

		virtual_overseer
	});

	assert!(view.active_leaves.is_empty());
	assert!(view.candidate_storage.is_empty());
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
