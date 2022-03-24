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

//! Implements common code for nemesis. Currently, only `FakeValidationResult`
//! interceptor is implemented.

#![allow(missing_docs)]

use crate::{interceptor::*, shared::MALUS};
use polkadot_node_primitives::{InvalidCandidate, PoV, ValidationResult};
use polkadot_node_subsystem::messages::{
	AvailabilityRecoveryMessage, CandidateValidationMessage, ValidationFailed,
};
use polkadot_node_subsystem_util as util;

use polkadot_primitives::v2::{
	CandidateCommitments, CandidateDescriptor, CandidateReceipt, Hash, PersistedValidationData,
	ValidationCode,
};

use polkadot_cli::service::SpawnNamed;

use futures::channel::oneshot;
use std::sync::Arc;
use clap::ArgEnum;


#[derive(ArgEnum, Clone, Copy, Debug, PartialEq)]
#[clap(rename_all = "kebab-case")]
pub enum FakeCandidateValidation {
	Disabled,
	BackingInvalid,
	ApprovalInvalid,
	BackingAndApprovalInvalid,
	BackingValid,
	ApprovalValid,
	BackingAndApprovalValid,
	// TODO: later.
	// BackingValidAndApprovalInvalid,
	// BackingInvalidAndApprovalValid,
}

/// Candidate invalidity details
#[derive(ArgEnum, Clone, Copy, Debug, PartialEq)]
#[clap(rename_all = "kebab-case")]
pub enum FakeCandidateValidationError {
	/// Validation outputs check doesn't pass.
	InvalidOutputs,
	/// Failed to execute.`validate_block`. This includes function panicking.
	ExecutionError,
	/// Execution timeout.
	Timeout,
	/// Validation input is over the limit.
	ParamsTooLarge,
	/// Code size is over the limit.
	CodeTooLarge,
	/// Code does not decompress correctly.
	CodeDecompressionFailure,
	/// PoV does not decompress correctly.
	POVDecompressionFailure,
	/// Validation function returned invalid data.
	BadReturn,
	/// Invalid relay chain parent.
	BadParent,
	/// POV hash does not match.
	POVHashMismatch,
	/// Bad collator signature.
	BadSignature,
	/// Para head hash does not match.
	ParaHeadHashMismatch,
	/// Validation code hash does not match.
	CodeHashMismatch,
}

impl Into<InvalidCandidate> for FakeCandidateValidationError {
	fn into(self) -> InvalidCandidate {
		match self {
			FakeCandidateValidationError::ExecutionError =>
				InvalidCandidate::ExecutionError("Malus".into()),
			FakeCandidateValidationError::InvalidOutputs => InvalidCandidate::InvalidOutputs,
			FakeCandidateValidationError::Timeout => InvalidCandidate::Timeout,
			FakeCandidateValidationError::ParamsTooLarge => InvalidCandidate::ParamsTooLarge(666),
			FakeCandidateValidationError::CodeTooLarge => InvalidCandidate::CodeTooLarge(666),
			FakeCandidateValidationError::CodeDecompressionFailure =>
				InvalidCandidate::CodeDecompressionFailure,
			FakeCandidateValidationError::POVDecompressionFailure =>
				InvalidCandidate::PoVDecompressionFailure,
			FakeCandidateValidationError::BadReturn => InvalidCandidate::BadReturn,
			FakeCandidateValidationError::BadParent => InvalidCandidate::BadParent,
			FakeCandidateValidationError::POVHashMismatch => InvalidCandidate::PoVHashMismatch,
			FakeCandidateValidationError::BadSignature => InvalidCandidate::BadSignature,
			FakeCandidateValidationError::ParaHeadHashMismatch =>
				InvalidCandidate::ParaHeadHashMismatch,
			FakeCandidateValidationError::CodeHashMismatch => InvalidCandidate::CodeHashMismatch,
		}
	}
}

#[derive(Clone, Debug)]
/// An interceptor which fakes validation result with a preconfigured result.
/// Replaces `CandidateValidationSubsystem`.
pub struct ReplaceValidationResult<Spawner> {
	fake_validation: FakeCandidateValidation,
	fake_validation_error: FakeCandidateValidationError,
	spawner: Spawner,
}

impl<Spawner> ReplaceValidationResult<Spawner>
where
	Spawner: SpawnNamed,
{
	pub fn new(
		fake_validation: FakeCandidateValidation,
		fake_validation_error: FakeCandidateValidationError,
		spawner: Spawner,
	) -> Self {
		Self { fake_validation, fake_validation_error, spawner }
	}

	pub fn send_validation_response<Sender>(
		&self,
		candidate_descriptor: CandidateDescriptor,
		pov: Arc<PoV>,
		sender: Sender,
		response_sender: oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
	) where
		Sender: overseer::SubsystemSender<AllMessages>
			+ overseer::SubsystemSender<CandidateValidationMessage>
			+ Clone
			+ Send
			+ 'static,
	{
		let candidate_receipt = create_candidate_receipt(candidate_descriptor.clone());
		let mut subsystem_sender = sender.clone();
		self.spawner.spawn(
			"malus-send-fake-validation",
			Some("malus"),
			Box::pin(async move {
				let relay_parent = candidate_descriptor.relay_parent;
				let session_index =
					util::request_session_index_for_child(relay_parent, &mut subsystem_sender)
						.await;
				let session_index = session_index.await.unwrap().unwrap();

				let (a_tx, a_rx) = oneshot::channel();

				subsystem_sender
					.send_message(AllMessages::from(
						AvailabilityRecoveryMessage::RecoverAvailableData(
							candidate_receipt,
							session_index,
							None,
							a_tx,
						),
					))
					.await;

				if let Ok(Ok(availability_data)) = a_rx.await {
					create_validation_response(
						availability_data.validation_data,
						None,
						candidate_descriptor,
						pov,
						response_sender,
					);
				} else {
					gum::info!(target: MALUS, "Could not get availability data, can't back");
				}
			}),
		);
	}
}

fn create_validation_response(
	persisted_validation_data: PersistedValidationData,
	validation_code: Option<ValidationCode>,
	_candidate_descriptor: CandidateDescriptor,
	_pov: Arc<PoV>,
	response_sender: oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
) {
	let candidate_commitmentments = CandidateCommitments {
		head_data: persisted_validation_data.parent_head.clone(),
		new_validation_code: validation_code,
		..Default::default()
	};

	response_sender
		.send(Ok(ValidationResult::Valid(candidate_commitmentments, persisted_validation_data)))
		.unwrap();
}

fn create_candidate_receipt(descriptor: CandidateDescriptor) -> CandidateReceipt {
	CandidateReceipt { descriptor, commitments_hash: Hash::zero() }
}

impl<Sender, Spawner> MessageInterceptor<Sender> for ReplaceValidationResult<Spawner>
where
	Sender: overseer::SubsystemSender<CandidateValidationMessage>
		+ overseer::SubsystemSender<AllMessages>
		+ Clone
		+ Send
		+ 'static,
	Spawner: SpawnNamed + Clone + 'static,
{
	type Message = CandidateValidationMessage;

	// Capture all candidate validation requests and depending on configuration fail them.
	fn intercept_incoming(
		&self,
		subsystem_sender: &mut Sender,
		msg: FromOverseer<Self::Message>,
	) -> Option<FromOverseer<Self::Message>> {
		if self.fake_validation == FakeCandidateValidation::Disabled {
			return Some(msg)
		}

		match msg {
			FromOverseer::Communication {
				msg:
					CandidateValidationMessage::ValidateFromExhaustive(
						validation_data,
						validation_code,
						descriptor,
						pov,
						timeout,
						sender,
					),
			} => {
				match self.fake_validation {
					FakeCandidateValidation::ApprovalValid |
					FakeCandidateValidation::BackingAndApprovalValid => {
						create_validation_response(
							validation_data,
							Some(validation_code),
							descriptor,
							pov,
							sender,
						);
						None
					},
					FakeCandidateValidation::ApprovalInvalid |
					FakeCandidateValidation::BackingAndApprovalInvalid => {
						let validation_result =
							ValidationResult::Invalid(InvalidCandidate::InvalidOutputs);

						gum::info!(
							target: MALUS,
							para_id = ?descriptor.para_id,
							candidate_hash = ?descriptor.para_head,
							"ValidateFromExhaustive result: {:?}",
							&validation_result
						);
						// We're not even checking the candidate, this makes us appear faster than honest validators.
						sender.send(Ok(validation_result)).unwrap();
						None
					},
					_ => Some(FromOverseer::Communication {
						msg: CandidateValidationMessage::ValidateFromExhaustive(
							validation_data,
							validation_code,
							descriptor,
							pov,
							timeout,
							sender,
						),
					}),
				}
			},
			FromOverseer::Communication {
				msg:
					CandidateValidationMessage::ValidateFromChainState(
						descriptor,
						pov,
						timeout,
						response_sender,
					),
			} => {
				match self.fake_validation {
					FakeCandidateValidation::BackingValid |
					FakeCandidateValidation::BackingAndApprovalValid => {
						self.send_validation_response(
							descriptor,
							pov,
							subsystem_sender.clone(),
							response_sender,
						);
						None
					},
					FakeCandidateValidation::BackingInvalid |
					FakeCandidateValidation::BackingAndApprovalInvalid => {
						let validation_result =
							ValidationResult::Invalid(self.fake_validation_error.clone().into());
						gum::info!(
							target: MALUS,
							para_id = ?descriptor.para_id,
							candidate_hash = ?descriptor.para_head,
							"ValidateFromChainState result: {:?}",
							&validation_result
						);

						// We're not even checking the candidate, this makes us appear faster than honest validators.
						response_sender.send(Ok(validation_result)).unwrap();
						None
					},
					_ => Some(FromOverseer::Communication {
						msg: CandidateValidationMessage::ValidateFromChainState(
							descriptor,
							pov,
							timeout,
							response_sender,
						),
					}),
				}
			},
			msg => Some(msg),
		}
	}

	fn intercept_outgoing(&self, msg: AllMessages) -> Option<AllMessages> {
		Some(msg)
	}
}
