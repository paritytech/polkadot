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
use futures::future::{self, BoxFuture};
use std::sync::Arc;

use sp_core::testing::TaskExecutor;

use super::*;
use parity_scale_codec::Encode;
use polkadot_node_primitives::{AvailableData, BlockData, InvalidCandidate, PoV};
use polkadot_node_subsystem::{
	overseer::Subsystem,
	jaeger, messages::{AllMessages, ValidationFailed}, ActivatedLeaf, ActiveLeavesUpdate, LeafStatus,
};
use polkadot_node_subsystem_test_helpers::{make_subsystem_context, TestSubsystemContextHandle};
use polkadot_primitives::v1::{BlakeTwo256, CandidateCommitments, HashT, Header, ValidationCode};

type VirtualOverseer = TestSubsystemContextHandle<DisputeParticipationMessage>;

fn test_harness<F>(test: F)
where
	F: FnOnce(VirtualOverseer) -> BoxFuture<'static, VirtualOverseer>,
{
	let (ctx, ctx_handle) = make_subsystem_context(TaskExecutor::new());

	let subsystem = DisputeParticipationSubsystem::new();
	let spawned_subsystem = subsystem.start(ctx);
	let test_future = test(ctx_handle);

	let (subsystem_result, _) =
		futures::executor::block_on(future::join(spawned_subsystem.future, async move {
			let mut ctx_handle = test_future.await;
			ctx_handle
				.send(FromOverseer::Signal(OverseerSignal::Conclude))
				.await;

			// no further request is received by the overseer which means that
			// no further attempt to participate was made
			assert!(ctx_handle.try_recv().await.is_none());
		}));

	subsystem_result.unwrap();
}

async fn activate_leaf(virtual_overseer: &mut VirtualOverseer, block_number: BlockNumber) {
	let block_header = Header {
		parent_hash: BlakeTwo256::hash(&block_number.encode()),
		number: block_number,
		digest: Default::default(),
		state_root: Default::default(),
		extrinsics_root: Default::default(),
	};

	let block_hash = block_header.hash();

	virtual_overseer
		.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
			ActiveLeavesUpdate::start_work(ActivatedLeaf {
				hash: block_hash,
				span: Arc::new(jaeger::Span::Disabled),
				number: block_number,
				status: LeafStatus::Fresh,
			}),
		)))
		.await;
}

async fn participate(virtual_overseer: &mut VirtualOverseer) -> oneshot::Receiver<bool> {
	let commitments = CandidateCommitments::default();
	let candidate_receipt = {
		let mut receipt = CandidateReceipt::default();
		receipt.commitments_hash = commitments.hash();
		receipt
	};
	let candidate_hash = candidate_receipt.hash();
	let session = 1;
	let n_validators = 10;

	let (report_availability, receive_availability) = oneshot::channel();

	virtual_overseer
		.send(FromOverseer::Communication {
			msg: DisputeParticipationMessage::Participate {
				candidate_hash,
				candidate_receipt: candidate_receipt.clone(),
				session,
				n_validators,
				report_availability,
			},
	})
	.await;
	receive_availability
}

async fn recover_available_data(virtual_overseer: &mut VirtualOverseer, receive_availability: oneshot::Receiver<bool>) {
	let pov_block = PoV {
		block_data: BlockData(Vec::new()),
	};

	let available_data = AvailableData {
		pov: Arc::new(pov_block),
		validation_data: Default::default(),
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

	assert_eq!(receive_availability.await.expect("Availability should get reported"), true);
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
		AllMessages::AvailabilityStore(AvailabilityStoreMessage::StoreAvailableData(
			_,
			_,
			_,
			_,
			tx,
		)) => {
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
fn cannot_participate_when_recent_block_state_is_missing() {
	test_harness(|mut virtual_overseer| {
		Box::pin(async move {
			let _ = participate(&mut virtual_overseer).await;

			virtual_overseer
		})
	});

	test_harness(|mut virtual_overseer| {
		Box::pin(async move {
			activate_leaf(&mut virtual_overseer, 10).await;
			let _ = participate(&mut virtual_overseer).await;

			// after activating at least one leaf the recent block
			// state should be available which should lead to trying
			// to participate by first trying to recover the available
			// data
			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::AvailabilityRecovery(
					AvailabilityRecoveryMessage::RecoverAvailableData(..)
				),
				"overseer did not receive recover available data message",
			);

			virtual_overseer
		})
	});
}

#[test]
fn cannot_participate_if_cannot_recover_available_data() {
	test_harness(|mut virtual_overseer| {
		Box::pin(async move {
			activate_leaf(&mut virtual_overseer, 10).await;
			let receive_availability = participate(&mut virtual_overseer).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::AvailabilityRecovery(
					AvailabilityRecoveryMessage::RecoverAvailableData(_, _, _, tx)
				) => {
					tx.send(Err(RecoveryError::Unavailable)).unwrap();
				},
				"overseer did not receive recover available data message",
			);

			assert_eq!(receive_availability.await.expect("Availability should get reported"), false);

			virtual_overseer
		})
	});
}

#[test]
fn cannot_participate_if_cannot_recover_validation_code() {
	test_harness(|mut virtual_overseer| {
		Box::pin(async move {
			activate_leaf(&mut virtual_overseer, 10).await;
			let receive_availability = participate(&mut virtual_overseer).await;
			recover_available_data(&mut virtual_overseer, receive_availability).await;

			assert_matches!(
				virtual_overseer.recv().await,
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

			virtual_overseer
		})
	});
}

#[test]
fn cast_invalid_vote_if_available_data_is_invalid() {
	test_harness(|mut virtual_overseer| {
		Box::pin(async move {
			activate_leaf(&mut virtual_overseer, 10).await;
			let receive_availability = participate(&mut virtual_overseer).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::AvailabilityRecovery(
					AvailabilityRecoveryMessage::RecoverAvailableData(_, _, _, tx)
				) => {
					tx.send(Err(RecoveryError::Invalid)).unwrap();
				},
				"overseer did not receive recover available data message",
			);

			assert_eq!(receive_availability.await.expect("Availability should get reported"), true);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::IssueLocalStatement(
					_,
					_,
					_,
					false,
				)),
				"overseer did not receive issue local statement message",
			);

			virtual_overseer
		})
	});
}

#[test]
fn cast_invalid_vote_if_validation_fails_or_is_invalid() {
	test_harness(|mut virtual_overseer| {
		Box::pin(async move {
			activate_leaf(&mut virtual_overseer, 10).await;
			let receive_availability = participate(&mut virtual_overseer).await;
			recover_available_data(&mut virtual_overseer, receive_availability).await;
			fetch_validation_code(&mut virtual_overseer).await;
			store_available_data(&mut virtual_overseer, true).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromExhaustive(_, _, _, _, tx)
				) => {
					tx.send(Ok(ValidationResult::Invalid(InvalidCandidate::Timeout))).unwrap();
				},
				"overseer did not receive candidate validation message",
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::IssueLocalStatement(
					_,
					_,
					_,
					false,
				)),
				"overseer did not receive issue local statement message",
			);

			virtual_overseer
		})
	});
}

#[test]
fn cast_invalid_vote_if_validation_passes_but_commitments_dont_match() {
	test_harness(|mut virtual_overseer| {
		Box::pin(async move {
			activate_leaf(&mut virtual_overseer, 10).await;
			let receive_availability = participate(&mut virtual_overseer).await;
			recover_available_data(&mut virtual_overseer, receive_availability).await;
			fetch_validation_code(&mut virtual_overseer).await;
			store_available_data(&mut virtual_overseer, true).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromExhaustive(_, _, _, _, tx)
				) => {
					let mut commitments = CandidateCommitments::default();
					// this should lead to a commitments hash mismatch
					commitments.processed_downward_messages = 42;

					tx.send(Ok(ValidationResult::Valid(commitments, Default::default()))).unwrap();
				},
				"overseer did not receive candidate validation message",
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::IssueLocalStatement(
					_,
					_,
					_,
					false,
				)),
				"overseer did not receive issue local statement message",
			);

			virtual_overseer
		})
	});
}

#[test]
fn cast_valid_vote_if_validation_passes() {
	test_harness(|mut virtual_overseer| {
		Box::pin(async move {
			activate_leaf(&mut virtual_overseer, 10).await;
			let receive_availability = participate(&mut virtual_overseer).await;
			recover_available_data(&mut virtual_overseer, receive_availability).await;
			fetch_validation_code(&mut virtual_overseer).await;
			store_available_data(&mut virtual_overseer, true).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromExhaustive(_, _, _, _, tx)
				) => {
					tx.send(Ok(ValidationResult::Valid(Default::default(), Default::default()))).unwrap();
				},
				"overseer did not receive candidate validation message",
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::IssueLocalStatement(
					_,
					_,
					_,
					true,
				)),
				"overseer did not receive issue local statement message",
			);

			virtual_overseer
		})
	});
}

#[test]
fn failure_to_store_available_data_does_not_preclude_participation() {
	test_harness(|mut virtual_overseer| {
		Box::pin(async move {
			activate_leaf(&mut virtual_overseer, 10).await;
			let receive_availability = participate(&mut virtual_overseer).await;
			recover_available_data(&mut virtual_overseer, receive_availability).await;
			fetch_validation_code(&mut virtual_overseer).await;
			// the store available data request should fail
			store_available_data(&mut virtual_overseer, false).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromExhaustive(_, _, _, _, tx)
				) => {
					tx.send(Err(ValidationFailed("fail".to_string()))).unwrap();
				},
				"overseer did not receive candidate validation message",
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::IssueLocalStatement(
					_,
					_,
					_,
					false,
				)),
				"overseer did not receive issue local statement message",
			);

			virtual_overseer
		})
	});
}
