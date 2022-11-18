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
use futures::StreamExt;
use polkadot_node_subsystem_util::TimeoutExt;
use std::{sync::Arc, time::Duration};

use sp_core::testing::TaskExecutor;

use super::*;
use ::test_helpers::{
	dummy_candidate_commitments, dummy_candidate_receipt_bad_sig, dummy_digest, dummy_hash,
};
use parity_scale_codec::Encode;
use polkadot_node_primitives::{AvailableData, BlockData, InvalidCandidate, PoV};
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		AllMessages, ChainApiMessage, DisputeCoordinatorMessage, RuntimeApiMessage,
		RuntimeApiRequest,
	},
	ActivatedLeaf, ActiveLeavesUpdate, LeafStatus, SpawnGlue,
};
use polkadot_node_subsystem_test_helpers::{
	make_subsystem_context, TestSubsystemContext, TestSubsystemContextHandle,
};
use polkadot_primitives::v2::{
	BlakeTwo256, CandidateCommitments, HashT, Header, PersistedValidationData, ValidationCode,
};

type VirtualOverseer = TestSubsystemContextHandle<DisputeCoordinatorMessage>;

pub fn make_our_subsystem_context<S>(
	spawner: S,
) -> (
	TestSubsystemContext<DisputeCoordinatorMessage, SpawnGlue<S>>,
	TestSubsystemContextHandle<DisputeCoordinatorMessage>,
) {
	make_subsystem_context(spawner)
}

#[overseer::contextbounds(DisputeCoordinator, prefix = self::overseer)]
async fn participate<Context>(ctx: &mut Context, participation: &mut Participation) -> Result<()> {
	let commitments = CandidateCommitments::default();
	participate_with_commitments_hash(ctx, participation, commitments.hash()).await
}

#[overseer::contextbounds(DisputeCoordinator, prefix = self::overseer)]
async fn participate_with_commitments_hash<Context>(
	ctx: &mut Context,
	participation: &mut Participation,
	commitments_hash: Hash,
) -> Result<()> {
	let candidate_receipt = {
		let mut receipt = dummy_candidate_receipt_bad_sig(dummy_hash(), dummy_hash());
		receipt.commitments_hash = commitments_hash;
		receipt
	};
	let session = 1;

	let req = ParticipationRequest::new(candidate_receipt, session);

	participation
		.queue_participation(ctx, ParticipationPriority::BestEffort, req)
		.await
}

#[overseer::contextbounds(DisputeCoordinator, prefix = self::overseer)]
async fn activate_leaf<Context>(
	ctx: &mut Context,
	participation: &mut Participation,
	block_number: BlockNumber,
) -> FatalResult<()> {
	let block_header = Header {
		parent_hash: BlakeTwo256::hash(&block_number.encode()),
		number: block_number,
		digest: dummy_digest(),
		state_root: dummy_hash(),
		extrinsics_root: dummy_hash(),
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

/// Full participation happy path as seen via the overseer.
pub async fn participation_full_happy_path(
	ctx_handle: &mut VirtualOverseer,
	expected_commitments_hash: Hash,
) {
	recover_available_data(ctx_handle).await;
	fetch_validation_code(ctx_handle).await;

	assert_matches!(
	ctx_handle.recv().await,
	AllMessages::CandidateValidation(
		CandidateValidationMessage::ValidateFromExhaustive(_, _, candidate_receipt, _, timeout, tx)
		) if timeout == APPROVAL_EXECUTION_TIMEOUT => {
			if expected_commitments_hash != candidate_receipt.commitments_hash {
				tx.send(Ok(ValidationResult::Invalid(InvalidCandidate::CommitmentsHashMismatch))).unwrap();
			} else {
				tx.send(Ok(ValidationResult::Valid(dummy_candidate_commitments(None), PersistedValidationData::default()))).unwrap();
			}
	},
	"overseer did not receive candidate validation message",
	);
}

/// Full participation with failing availability recovery.
pub async fn participation_missing_availability(ctx_handle: &mut VirtualOverseer) {
	assert_matches!(
		ctx_handle.recv().await,
		AllMessages::AvailabilityRecovery(
			AvailabilityRecoveryMessage::RecoverAvailableData(_, _, _, tx)
		) => {
			tx.send(Err(RecoveryError::Unavailable)).unwrap();
		},
		"overseer did not receive recover available data message",
	);
}

async fn recover_available_data(virtual_overseer: &mut VirtualOverseer) {
	let pov_block = PoV { block_data: BlockData(Vec::new()) };

	let available_data = AvailableData {
		pov: Arc::new(pov_block),
		validation_data: PersistedValidationData::default(),
	};

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

/// Handles validation code fetch, returns the received relay parent hash.
async fn fetch_validation_code(virtual_overseer: &mut VirtualOverseer) -> Hash {
	let validation_code = ValidationCode(Vec::new());

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			hash,
			RuntimeApiRequest::ValidationCodeByHash(
				_,
				tx,
			)
		)) => {
			tx.send(Ok(Some(validation_code))).unwrap();
			hash
		},
		"overseer did not receive runtime API request for validation code",
	)
}

#[test]
fn same_req_wont_get_queued_if_participation_is_already_running() {
	futures::executor::block_on(async {
		let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());

		let (sender, mut worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();
		participate(&mut ctx, &mut participation).await.unwrap();
		for _ in 0..MAX_PARALLEL_PARTICIPATIONS {
			participate(&mut ctx, &mut participation).await.unwrap();
		}

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

		// we should not have any further results nor recovery requests:
		assert_matches!(ctx_handle.recv().timeout(Duration::from_millis(10)).await, None);
		assert_matches!(worker_receiver.next().timeout(Duration::from_millis(10)).await, None);
	})
}

#[test]
fn reqs_get_queued_when_out_of_capacity() {
	let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());

	let test = async {
		let (sender, mut worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();
		participate(&mut ctx, &mut participation).await.unwrap();
		for i in 0..MAX_PARALLEL_PARTICIPATIONS {
			participate_with_commitments_hash(
				&mut ctx,
				&mut participation,
				Hash::repeat_byte(i as _),
			)
			.await
			.unwrap();
		}

		for _ in 0..MAX_PARALLEL_PARTICIPATIONS + 1 {
			let result = participation
				.get_participation_result(&mut ctx, worker_receiver.next().await.unwrap())
				.await
				.unwrap();
			assert_matches!(
				result.outcome,
				ParticipationOutcome::Unavailable => {}
			);
		}
		// we should not have any further recovery requests:
		assert_matches!(worker_receiver.next().timeout(Duration::from_millis(10)).await, None);
	};

	let request_handler = async {
		let mut recover_available_data_msg_count = 0;
		let mut block_number_msg_count = 0;

		while recover_available_data_msg_count < MAX_PARALLEL_PARTICIPATIONS + 1 ||
			block_number_msg_count < 1
		{
			match ctx_handle.recv().await {
				AllMessages::AvailabilityRecovery(
					AvailabilityRecoveryMessage::RecoverAvailableData(_, _, _, tx),
				) => {
					tx.send(Err(RecoveryError::Unavailable)).unwrap();
					recover_available_data_msg_count += 1;
				},
				AllMessages::ChainApi(ChainApiMessage::BlockNumber(_, tx)) => {
					tx.send(Ok(None)).unwrap();
					block_number_msg_count += 1;
				},
				_ => assert!(false, "Received unexpected message"),
			}
		}

		// we should not have any further results
		assert_matches!(ctx_handle.recv().timeout(Duration::from_millis(10)).await, None);
	};

	futures::executor::block_on(async {
		futures::join!(test, request_handler);
	});
}

#[test]
fn reqs_get_queued_on_no_recent_block() {
	let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());
	let (mut unblock_test, mut wait_for_verification) = mpsc::channel(0);
	let test = async {
		let (sender, _worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		participate(&mut ctx, &mut participation).await.unwrap();

		// We have initiated participation but we'll block `active_leaf` so that we can check that
		// the participation is queued in race-free way
		let _ = wait_for_verification.next().await.unwrap();

		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();
	};

	// Responds to messages from the test and verifies its behaviour
	let request_handler = async {
		// If we receive `BlockNumber` request this implicitly proves that the participation is queued
		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::ChainApi(ChainApiMessage::BlockNumber(_, tx)) => {
				tx.send(Ok(None)).unwrap();
			},
			"overseer did not receive `ChainApiMessage::BlockNumber` message",
		);

		assert!(ctx_handle.recv().timeout(Duration::from_millis(10)).await.is_none());

		// No activity so the participation is queued => unblock the test
		unblock_test.send(()).await.unwrap();

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
	};

	futures::executor::block_on(async {
		futures::join!(test, request_handler);
	});
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
		assert_eq!(
			fetch_validation_code(&mut ctx_handle).await,
			participation.recent_block.unwrap().1
		);

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
fn cast_invalid_vote_if_commitments_dont_match() {
	futures::executor::block_on(async {
		let (mut ctx, mut ctx_handle) = make_our_subsystem_context(TaskExecutor::new());

		let (sender, mut worker_receiver) = mpsc::channel(1);
		let mut participation = Participation::new(sender);
		activate_leaf(&mut ctx, &mut participation, 10).await.unwrap();
		participate(&mut ctx, &mut participation).await.unwrap();

		recover_available_data(&mut ctx_handle).await;
		assert_eq!(
			fetch_validation_code(&mut ctx_handle).await,
			participation.recent_block.unwrap().1
		);

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromExhaustive(_, _, _, _, timeout, tx)
			) if timeout == APPROVAL_EXECUTION_TIMEOUT => {
				tx.send(Ok(ValidationResult::Invalid(InvalidCandidate::CommitmentsHashMismatch))).unwrap();
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
		assert_eq!(
			fetch_validation_code(&mut ctx_handle).await,
			participation.recent_block.unwrap().1
		);

		assert_matches!(
			ctx_handle.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromExhaustive(_, _, _, _, timeout, tx)
			) if timeout == APPROVAL_EXECUTION_TIMEOUT => {
				tx.send(Ok(ValidationResult::Valid(dummy_candidate_commitments(None), PersistedValidationData::default()))).unwrap();
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
