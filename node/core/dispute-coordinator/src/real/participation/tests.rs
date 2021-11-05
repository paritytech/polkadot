// Copyright 2021 Parity Technologies (UK) Ltd.
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

use assert_matches::assert_matches;
use futures::{
	future::{self, BoxFuture},
	StreamExt,
};
use polkadot_node_subsystem_util::TimeoutExt;
use std::{sync::Arc, time::Duration};

use sp_core::testing::TaskExecutor;

use super::*;
use parity_scale_codec::Encode;
use polkadot_node_primitives::{AvailableData, BlockData, InvalidCandidate, PoV};
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		AllMessages, DisputeCoordinatorMessage, DisputeDistributionMessage, RuntimeApiMessage,
		RuntimeApiRequest, ValidationFailed,
	},
	overseer::Subsystem,
	ActivatedLeaf, ActiveLeavesUpdate, LeafStatus,
};
use polkadot_node_subsystem_test_helpers::{
	make_subsystem_context, TestSubsystemContext, TestSubsystemContextHandle,
};
use polkadot_primitives::v1::{BlakeTwo256, CandidateCommitments, HashT, Header, ValidationCode};

type VirtualOverseer = TestSubsystemContextHandle<DisputeCoordinatorMessage>;

pub fn make_our_subsystem_context<S>(
	spawn: S,
) -> (
	TestSubsystemContext<DisputeCoordinatorMessage, S>,
	TestSubsystemContextHandle<DisputeCoordinatorMessage>,
) {
	make_subsystem_context(spawn)
}

async fn participate(
	ctx: &mut impl SubsystemContext,
	participation: &mut Participation,
) -> Result<()> {
	let commitments = CandidateCommitments::default();
	let candidate_receipt = {
		let mut receipt = CandidateReceipt::default();
		receipt.commitments_hash = commitments.hash();
		receipt
	};
	let session = 1;
	let n_validators = 10;

	let req = ParticipationRequest::new(candidate_receipt, session, n_validators);

	participation.queue_participation(ctx, None, req).await
}

async fn activate_leaf(
	ctx: &mut impl SubsystemContext,
	participation: &mut Participation,
	block_number: BlockNumber,
) -> FatalResult<()> {
	let block_header = Header {
		parent_hash: BlakeTwo256::hash(&block_number.encode()),
		number: block_number,
		digest: Default::default(),
		state_root: Default::default(),
		extrinsics_root: Default::default(),
	};

	let block_hash = block_header.hash();

	participation
		.process_active_leaves_update(
			ctx,
			&ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: block_hash,
				span: Arc::new(jaeger::Span::Disabled),
				number: block_number,
				status: LeafStatus::Fresh,
			}),
		)
		.await
}

async fn recover_available_data(virtual_overseer: &mut VirtualOverseer) {
	let pov_block = PoV { block_data: BlockData(Vec::new()) };

	let available_data =
		AvailableData { pov: Arc::new(pov_block), validation_data: Default::default() };

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::AvailabilityRecovery(
			AvailabilityRecoveryMessage::RecoverAvailableData(_, _, _, tx)
		) => {
			tx.send(Ok(available_data)).unwrap();
		},
		"overseer did not receive recover available data message",
	);
}

async fn fetch_validation_code(virtual_overseer: &mut VirtualOverseer) {
	let validation_code = ValidationCode(Vec::new());

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			_,
			RuntimeApiRequest::ValidationCodeByHash(
				_,
				tx,
			)
		)) => {
			tx.send(Ok(Some(validation_code))).unwrap();
		},
		"overseer did not receive runtime API request for validation code",
	);
}

async fn store_available_data(virtual_overseer: &mut VirtualOverseer, success: bool) {
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::AvailabilityStore(AvailabilityStoreMessage::StoreAvailableData { tx, .. }) => {
			if success {
				tx.send(Ok(())).unwrap();
			} else {
				tx.send(Err(())).unwrap();
			}
		},
		"overseer did not receive store available data request",
	);
}

#[test]
fn reqs_get_queued_on_no_recent_block() {
	futures::executor::block_on(async {
		let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());

		let (sender, _worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		participate(&mut ctx, &mut participation).await.unwrap();
		assert!(ctx_handle.recv().timeout(Duration::from_millis(10)).await.is_none());
		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();

		// after activating at least one leaf the recent block
		// state should be available which should lead to trying
		// to participate by first trying to recover the available
		// data
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::AvailabilityRecovery(AvailabilityRecoveryMessage::RecoverAvailableData(
				..
			)),
			"overseer did not receive recover available data message",
		);
	})
}

#[test]
fn cannot_participate_if_cannot_recover_available_data() {
	futures::executor::block_on(async {
		let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());

		let (sender, mut worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();
		participate(&mut ctx, &mut participation).await.unwrap();

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::AvailabilityRecovery(
				AvailabilityRecoveryMessage::RecoverAvailableData(_, _, _, tx)
			) => {
				tx.send(Err(RecoveryError::Unavailable)).unwrap();
			},
			"overseer did not receive recover available data message",
		);
		let result = participation
			.get_participation_result(&mut ctx, worker_receiver.next().await.unwrap())
			.await
			.unwrap();
		assert_matches!(
			result.outcome,
			ParticipationOutcome::Unavailable => {}
		);
	})
}

#[test]
fn cannot_participate_if_cannot_recover_validation_code() {
	futures::executor::block_on(async {
		let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());

		let (sender, mut worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();
		participate(&mut ctx, &mut participation).await.unwrap();

		recover_available_data(&mut ctx_handle).await;

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_,
				RuntimeApiRequest::ValidationCodeByHash(
					_,
					tx,
				)
			)) => {
				tx.send(Ok(None)).unwrap();
			},
			"overseer did not receive runtime API request for validation code",
		);

		let result = participation
			.get_participation_result(&mut ctx, worker_receiver.next().await.unwrap())
			.await
			.unwrap();
		assert_matches!(
			result.outcome,
			ParticipationOutcome::Error => {}
		);
	})
}

#[test]
fn cast_invalid_vote_if_available_data_is_invalid() {
	futures::executor::block_on(async {
		let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());

		let (sender, mut worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();
		participate(&mut ctx, &mut participation).await.unwrap();

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::AvailabilityRecovery(
				AvailabilityRecoveryMessage::RecoverAvailableData(_, _, _, tx)
			) => {
				tx.send(Err(RecoveryError::Invalid)).unwrap();
			},
			"overseer did not receive recover available data message",
		);

		let result = participation
			.get_participation_result(&mut ctx, worker_receiver.next().await.unwrap())
			.await
			.unwrap();
		assert_matches!(
			result.outcome,
			ParticipationOutcome::Invalid => {}
		);
	})
}

#[test]
fn cast_invalid_vote_if_validation_fails_or_is_invalid() {
	futures::executor::block_on(async {
		let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());

		let (sender, mut worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();
		participate(&mut ctx, &mut participation).await.unwrap();

		recover_available_data(&mut ctx_handle).await;
		fetch_validation_code(&mut ctx_handle).await;
		store_available_data(&mut ctx_handle, true).await;

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromExhaustive(_, _, _, _, timeout, tx)
			) if timeout == APPROVAL_EXECUTION_TIMEOUT => {
				tx.send(Ok(ValidationResult::Invalid(InvalidCandidate::Timeout))).unwrap();
			},
			"overseer did not receive candidate validation message",
		);

		let result = participation
			.get_participation_result(&mut ctx, worker_receiver.next().await.unwrap())
			.await
			.unwrap();
		assert_matches!(
			result.outcome,
			ParticipationOutcome::Invalid => {}
		);
	})
}

#[test]
fn cast_invalid_vote_if_validation_passes_but_commitments_dont_match() {
	futures::executor::block_on(async {
		let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());

		let (sender, mut worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();
		participate(&mut ctx, &mut participation).await.unwrap();

		recover_available_data(&mut ctx_handle).await;
		fetch_validation_code(&mut ctx_handle).await;
		store_available_data(&mut ctx_handle, true).await;

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromExhaustive(_, _, _, _, timeout, tx)
			) if timeout == APPROVAL_EXECUTION_TIMEOUT => {
				let mut commitments = CandidateCommitments::default();
				// this should lead to a commitments hash mismatch
				commitments.processed_downward_messages = 42;

				tx.send(Ok(ValidationResult::Valid(commitments, Default::default()))).unwrap();
			},
			"overseer did not receive candidate validation message",
		);

		let result = participation
			.get_participation_result(&mut ctx, worker_receiver.next().await.unwrap())
			.await
			.unwrap();
		assert_matches!(
			result.outcome,
			ParticipationOutcome::Invalid => {}
		);
	})
}

#[test]
fn cast_valid_vote_if_validation_passes() {
	futures::executor::block_on(async {
		let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());

		let (sender, mut worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();
		participate(&mut ctx, &mut participation).await.unwrap();

		recover_available_data(&mut ctx_handle).await;
		fetch_validation_code(&mut ctx_handle).await;
		store_available_data(&mut ctx_handle, true).await;

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromExhaustive(_, _, _, _, timeout, tx)
			) if timeout == APPROVAL_EXECUTION_TIMEOUT => {
				tx.send(Ok(ValidationResult::Valid(Default::default(), Default::default()))).unwrap();
			},
			"overseer did not receive candidate validation message",
		);

		let result = participation
			.get_participation_result(&mut ctx, worker_receiver.next().await.unwrap())
			.await
			.unwrap();
		assert_matches!(
			result.outcome,
			ParticipationOutcome::Valid => {}
		);
	})
}

#[test]
fn failure_to_store_available_data_does_not_preclude_participation() {
	futures::executor::block_on(async {
		let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());

		let (sender, mut worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();
		participate(&mut ctx, &mut participation).await.unwrap();

		recover_available_data(&mut ctx_handle).await;
		fetch_validation_code(&mut ctx_handle).await;
		// the store available data request should fail:
		store_available_data(&mut ctx_handle, false).await;

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromExhaustive(_, _, _, _, timeout, tx)
			) if timeout == APPROVAL_EXECUTION_TIMEOUT => {
				tx.send(Err(ValidationFailed("fail".to_string()))).unwrap();
			},
			"overseer did not receive candidate validation message",
		);

		let result = participation
			.get_participation_result(&mut ctx, worker_receiver.next().await.unwrap())
			.await
			.unwrap();
		assert_matches!(
			result.outcome,
			ParticipationOutcome::Invalid => {}
		);
	})
}
