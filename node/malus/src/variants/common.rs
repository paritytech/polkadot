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

use crate::{
	interceptor::*,
	shared::{MALICIOUS_POV, MALUS},
};

use polkadot_node_core_candidate_validation::find_validation_data;
use polkadot_node_primitives::{InvalidCandidate, PoV, ValidationResult};
use polkadot_node_subsystem::messages::{CandidateValidationMessage, ValidationFailed};

use polkadot_primitives::v2::{CandidateCommitments, CandidateDescriptor, PersistedValidationData};

use polkadot_cli::service::SpawnNamed;

use futures::channel::oneshot;
use std::sync::Arc;

#[derive(clap::ArgEnum, Clone, Copy, Debug, PartialEq)]
#[clap(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum FakeCandidateValidation {
	Disabled,
	BackingInvalid,
	ApprovalInvalid,
	BackingAndApprovalInvalid,
	BackingValid,
	ApprovalValid,
	BackingAndApprovalValid,
}

/// Candidate invalidity details
#[derive(clap::ArgEnum, Clone, Copy, Debug, PartialEq)]
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

	/// Creates and sends the validation response for a given candidate. Queries the runtime to obtain the validation data for the
	/// given candidate.
	pub fn send_validation_response<Sender>(
		&self,
		candidate_descriptor: CandidateDescriptor,
		pov: Arc<PoV>,
		subsystem_sender: Sender,
		response_sender: oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
	) where
		Sender: overseer::SubsystemSender<AllMessages>
			+ overseer::SubsystemSender<CandidateValidationMessage>
			+ Clone
			+ Send
			+ 'static,
	{
		let _candidate_descriptor = candidate_descriptor.clone();
		let mut subsystem_sender = subsystem_sender.clone();
		let (sender, receiver) = std::sync::mpsc::channel();
		self.spawner.spawn_blocking(
			"malus-get-validation-data",
			Some("malus"),
			Box::pin(async move {
				match find_validation_data(&mut subsystem_sender, &_candidate_descriptor).await {
					Ok(Some((validation_data, validation_code))) => {
						sender
							.send((validation_data, validation_code))
							.expect("channel is still open");
					},
					_ => {
						panic!("Unable to fetch validation data");
					},
				}
			}),
		);
		let (validation_data, _) = receiver.recv().unwrap();
		create_validation_response(validation_data, candidate_descriptor, pov, response_sender);
	}
}

pub fn create_fake_candidate_commitments(
	persisted_validation_data: &PersistedValidationData,
) -> CandidateCommitments {
	CandidateCommitments {
		upward_messages: Vec::new(),
		horizontal_messages: Vec::new(),
		new_validation_code: None,
		head_data: persisted_validation_data.parent_head.clone(),
		processed_downward_messages: 0,
		hrmp_watermark: persisted_validation_data.relay_parent_number,
	}
}

// Create and send validation response. This function needs the persistent validation data.
fn create_validation_response(
	persisted_validation_data: PersistedValidationData,
	candidate_descriptor: CandidateDescriptor,
	_pov: Arc<PoV>,
	response_sender: oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
) {
	let result = Ok(ValidationResult::Valid(
		create_fake_candidate_commitments(&persisted_validation_data),
		persisted_validation_data,
	));

	gum::debug!(
		target: MALUS,
		para_id = ?candidate_descriptor.para_id,
		"ValidationResult: {:?}",
		&result
	);

	response_sender.send(result).unwrap();
}

// TODO: Only fake validation for `MALICIOUS_POV`, otherwise behave normally.
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
						commitments_hash,
					),
			} => {
				match self.fake_validation {
					FakeCandidateValidation::ApprovalValid |
					FakeCandidateValidation::BackingAndApprovalValid => {
						// Behave normally if the `PoV` is not known to be malicious.
						if pov.block_data.0.as_slice() != MALICIOUS_POV {
							return Some(FromOverseer::Communication {
								msg: CandidateValidationMessage::ValidateFromExhaustive(
									validation_data,
									validation_code,
									descriptor,
									pov,
									timeout,
									sender,
									commitments_hash,
								),
							})
						}
						create_validation_response(validation_data, descriptor, pov, sender);
						None
					},
					FakeCandidateValidation::ApprovalInvalid |
					FakeCandidateValidation::BackingAndApprovalInvalid => {
						let validation_result =
							ValidationResult::Invalid(InvalidCandidate::InvalidOutputs);

						gum::info!(
							target: MALUS,
							para_id = ?descriptor.para_id,
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
							commitments_hash,
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
						commitments_hash,
					),
			} => {
				match self.fake_validation {
					FakeCandidateValidation::BackingValid |
					FakeCandidateValidation::BackingAndApprovalValid => {
						// Behave normally if the `PoV` is not known to be malicious.
						if pov.block_data.0.as_slice() != MALICIOUS_POV {
							return Some(FromOverseer::Communication {
								msg: CandidateValidationMessage::ValidateFromChainState(
									descriptor,
									pov,
									timeout,
									response_sender,
									commitments_hash,
								),
							})
						}
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
							commitments_hash,
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
