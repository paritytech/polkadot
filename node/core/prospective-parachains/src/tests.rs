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
		CandidateDescriptor, GroupRotationInfo, HeadData, PersistedValidationData, ScheduledCore,
		SessionIndex, SigningContext, ValidatorId, ValidatorIndex,
	},
	vstaging as vstaging_primitives,
};
use sp_application_crypto::AppKey;
use sp_core::testing::TaskExecutor;
use sp_keyring::Sr25519Keyring;
use sp_keystore::{CryptoStore, SyncCryptoStore, SyncCryptoStorePtr};
use std::sync::Arc;

const ASYNC_BACKING_PARAMETERS: vstaging_primitives::AsyncBackingParameters =
	vstaging_primitives::AsyncBackingParameters { max_candidate_depth: 4, allowed_ancestry_len: 3 };

const ASYNC_BACKING_DISABLED_ERROR: RuntimeApiError =
	RuntimeApiError::NotSupported { runtime_api_name: "test-runtime" };

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<ProspectiveParachainsMessage>;

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

		let signing_context = SigningContext { session_index: 1, parent_hash: relay_parent };

		let validation_data = PersistedValidationData {
			parent_head: HeadData(vec![7, 8, 9]),
			relay_parent_number: 0_u32.into(),
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

async fn test_startup_async_backing_disabled(
	virtual_overseer: &mut VirtualOverseer,
	test_state: &TestState,
) {
	// Start work on some new parent.
	virtual_overseer
		.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			ActivatedLeaf {
				hash: test_state.relay_parent,
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			},
		))))
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

#[test]
fn should_do_no_work_if_async_backing_disabled_for_leaf() {
	let test_state = TestState::default();
	let view = test_harness(|mut virtual_overseer| async move {
		test_startup_async_backing_disabled(&mut virtual_overseer, &test_state).await;

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
