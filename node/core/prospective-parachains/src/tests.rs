use super::*;
use assert_matches::assert_matches;
use futures::executor;
use polkadot_node_subsystem::messages::{
	AllMessages, ProspectiveParachainsMessage, ProspectiveValidationDataRequest,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_types::{jaeger, ActivatedLeaf, LeafStatus};
use polkadot_primitives::vstaging as vstaging_primitives;
use sp_core::testing::TaskExecutor;
use std::sync::Arc;

const ASYNC_BACKING_PARAMETERS: vstaging_primitives::AsyncBackingParameters =
	vstaging_primitives::AsyncBackingParameters { max_candidate_depth: 4, allowed_ancestry_len: 3 };

#[test]
fn correctly_updates_leaves() {
	let first_block_hash = [1; 32].into();
	let second_block_hash = [2; 32].into();
	let third_block_hash = [3; 32].into();

	let pool = TaskExecutor::new();
	let (mut ctx, mut ctx_handle) =
		test_helpers::make_subsystem_context::<ProspectiveParachainsMessage, _>(pool.clone());

	let mut view = View::new();

	let check_fut = async move {
		// Activate a leaf.
		let activated = ActivatedLeaf {
			hash: first_block_hash,
			number: 1,
			span: Arc::new(jaeger::Span::Disabled),
			status: LeafStatus::Fresh,
		};
		let update = ActiveLeavesUpdate::start_work(activated);
		handle_active_leaves_update(&mut ctx, &mut view, update).await.unwrap();

		// Activate another leaf.
		let activated = ActivatedLeaf {
			hash: second_block_hash,
			number: 2,
			span: Arc::new(jaeger::Span::Disabled),
			status: LeafStatus::Fresh,
		};
		let update = ActiveLeavesUpdate::start_work(activated);
		handle_active_leaves_update(&mut ctx, &mut view, update.clone()).await.unwrap();

		// Try activating a duplicate leaf.
		handle_active_leaves_update(&mut ctx, &mut view, update).await.unwrap();

		// Pass in an empty update.
		let update = ActiveLeavesUpdate::default();
		handle_active_leaves_update(&mut ctx, &mut view, update).await.unwrap();

		// Activate a leaf and remove one at the same time.
		let activated = ActivatedLeaf {
			hash: third_block_hash,
			number: 3,
			span: Arc::new(jaeger::Span::Disabled),
			status: LeafStatus::Fresh,
		};
		let update = ActiveLeavesUpdate {
			activated: Some(activated),
			deactivated: [second_block_hash][..].into(),
		};
		handle_active_leaves_update(&mut ctx, &mut view, update).await.unwrap();

		// TODO: Check tree contents.
		assert_eq!(view.active_leaves.len(), 2);

		// Remove all remaining leaves.
		let update = ActiveLeavesUpdate {
			deactivated: [first_block_hash, third_block_hash][..].into(),
			..Default::default()
		};
		handle_active_leaves_update(&mut ctx, &mut view, update).await.unwrap();

		// Check final tree contents.
		assert!(view.active_leaves.is_empty() && view.candidate_storage.is_empty());
	};

	let test_fut = async move {
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::StagingAsyncBackingParameters(tx))
			) if parent == first_block_hash => {
				tx.send(Ok(ASYNC_BACKING_PARAMETERS)).unwrap();
			}
		);

		// TODO: Fill this out.

		let message = ctx_handle.recv().await;
		println!("{:?}", message);
	};

	let test_fut = future::join(test_fut, check_fut);
	executor::block_on(test_fut);
}
