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
use crate::{
	interceptor::*,
	shared::{MALICIOUS_POV, MALUS},
};

use polkadot_node_core_candidate_validation::find_validation_data;
use polkadot_node_primitives::{InvalidCandidate, ValidationResult};
use polkadot_node_subsystem::{
	messages::{CandidateValidationMessage, ValidationFailed},
	overseer,
};

use polkadot_primitives::v2::{
	CandidateCommitments, CandidateDescriptor, CandidateReceipt, PersistedValidationData,
};

use futures::channel::oneshot;
use rand::distributions::{Bernoulli, Distribution};

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq)]
#[value(rename_all = "kebab-case")]
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
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq)]
#[value(rename_all = "kebab-case")]
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
	distribution: Bernoulli,
	spawner: Spawner,
}

impl<Spawner> ReplaceValidationResult<Spawner>
where
	Spawner: overseer::gen::Spawner,
{
	pub fn new(
		fake_validation: FakeCandidateValidation,
		fake_validation_error: FakeCandidateValidationError,
		percentage: f64,
		spawner: Spawner,
	) -> Self {
		let distribution = Bernoulli::new(percentage / 100.0)
			.expect("Invalid probability! Percentage must be in range [0..=100].");
		Self { fake_validation, fake_validation_error, distribution, spawner }
	}

	/// Creates and sends the validation response for a given candidate. Queries the runtime to obtain the validation data for the
	/// given candidate.
	pub fn send_validation_response<Sender>(
		&self,
		candidate_descriptor: CandidateDescriptor,
		subsystem_sender: Sender,
		response_sender: oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
	) where
		Sender: overseer::CandidateValidationSenderTrait + Clone + Send + 'static,
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
		create_validation_response(validation_data, candidate_descriptor, response_sender);
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
	descriptor: CandidateDescriptor,
	response_sender: oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
) {
	let commitments = create_fake_candidate_commitments(&persisted_validation_data);

	// Craft the new malicious candidate.
	let candidate_receipt = CandidateReceipt { descriptor, commitments_hash: commitments.hash() };

	let result = Ok(ValidationResult::Valid(commitments, persisted_validation_data));

	gum::debug!(
		target: MALUS,
		para_id = ?candidate_receipt.descriptor.para_id,
		candidate_hash = ?candidate_receipt.hash(),
		"ValidationResult: {:?}",
		&result
	);

	response_sender.send(result).unwrap();
}

impl<Sender, Spawner> MessageInterceptor<Sender> for ReplaceValidationResult<Spawner>
where
	Sender: overseer::CandidateValidationSenderTrait + Clone + Send + 'static,
	Spawner: overseer::gen::Spawner + Clone + 'static,
{
	type Message = CandidateValidationMessage;

	// Capture all (approval and backing) candidate validation requests and depending on configuration fail them.
	fn intercept_incoming(
		&self,
		subsystem_sender: &mut Sender,
		msg: FromOrchestra<Self::Message>,
	) -> Option<FromOrchestra<Self::Message>> {
		match msg {
			// Message sent by the approval voting subsystem
			FromOrchestra::Communication {
				msg:
					CandidateValidationMessage::ValidateFromExhaustive(
						validation_data,
						validation_code,
						candidate_receipt,
						pov,
						timeout,
						sender,
					),
			} => {
				match self.fake_validation {
					FakeCandidateValidation::ApprovalValid |
					FakeCandidateValidation::BackingAndApprovalValid => {
						// Behave normally if the `PoV` is not known to be malicious.
						if pov.block_data.0.as_slice() != MALICIOUS_POV {
							return Some(FromOrchestra::Communication {
								msg: CandidateValidationMessage::ValidateFromExhaustive(
									validation_data,
									validation_code,
									candidate_receipt,
									pov,
									timeout,
									sender,
								),
							})
						}
						// Create the fake response with probability `p` if the `PoV` is malicious,
						// where 'p' defaults to 100% for suggest-garbage-candidate variant.
						let behave_maliciously = self.distribution.sample(&mut rand::thread_rng());
						match behave_maliciously {
							true => {
								gum::info!(
									target: MALUS,
									?behave_maliciously,
									"😈 Creating malicious ValidationResult::Valid message with fake candidate commitments.",
								);

								create_validation_response(
									validation_data,
									candidate_receipt.descriptor,
									sender,
								);
								None
							},
							false => {
								// Behave normally with probability `(1-p)` for a malicious `PoV`.
								gum::info!(
									target: MALUS,
									?behave_maliciously,
									"😈 Passing CandidateValidationMessage::ValidateFromExhaustive to the candidate validation subsystem.",
								);

								Some(FromOrchestra::Communication {
									msg: CandidateValidationMessage::ValidateFromExhaustive(
										validation_data,
										validation_code,
										candidate_receipt,
										pov,
										timeout,
										sender,
									),
								})
							},
						}
					},
					FakeCandidateValidation::ApprovalInvalid |
					FakeCandidateValidation::BackingAndApprovalInvalid => {
						// Set the validation result to invalid with probability `p` and trigger a dispute
						let behave_maliciously = self.distribution.sample(&mut rand::thread_rng());
						match behave_maliciously {
							true => {
								let validation_result =
									ValidationResult::Invalid(InvalidCandidate::InvalidOutputs);

								gum::info!(
									target: MALUS,
									?behave_maliciously,
									para_id = ?candidate_receipt.descriptor.para_id,
									"😈 Maliciously sending invalid validation result: {:?}.",
									&validation_result,
								);

								// We're not even checking the candidate, this makes us appear faster than honest validators.
								sender.send(Ok(validation_result)).unwrap();
								None
							},
							false => {
								// Behave normally with probability `(1-p)`
								gum::info!(target: MALUS, "😈 'Decided' to not act maliciously.",);

								Some(FromOrchestra::Communication {
									msg: CandidateValidationMessage::ValidateFromExhaustive(
										validation_data,
										validation_code,
										candidate_receipt,
										pov,
										timeout,
										sender,
									),
								})
							},
						}
					},
					// Handle FakeCandidateValidation::Disabled
					_ => Some(FromOrchestra::Communication {
						msg: CandidateValidationMessage::ValidateFromExhaustive(
							validation_data,
							validation_code,
							candidate_receipt,
							pov,
							timeout,
							sender,
						),
					}),
				}
			},
			// Behaviour related to the backing subsystem
			FromOrchestra::Communication {
				msg:
					CandidateValidationMessage::ValidateFromChainState(
						candidate_receipt,
						pov,
						timeout,
						response_sender,
					),
			} => {
				match self.fake_validation {
					FakeCandidateValidation::BackingValid |
					FakeCandidateValidation::BackingAndApprovalValid => {
						// Behave normally if the `PoV` is not known to be malicious.
						if pov.block_data.0.as_slice() != MALICIOUS_POV {
							return Some(FromOrchestra::Communication {
								msg: CandidateValidationMessage::ValidateFromChainState(
									candidate_receipt,
									pov,
									timeout,
									response_sender,
								),
							})
						}
						// If the `PoV` is malicious, back the candidate with some probability `p`,
						// where 'p' defaults to 100% for suggest-garbage-candidate variant.
						let behave_maliciously = self.distribution.sample(&mut rand::thread_rng());
						match behave_maliciously {
							true => {
								gum::info!(
									target: MALUS,
									?behave_maliciously,
									"😈 Backing candidate with malicious PoV.",
								);

								self.send_validation_response(
									candidate_receipt.descriptor,
									subsystem_sender.clone(),
									response_sender,
								);
								None
							},
							// If the `PoV` is malicious, we behave normally with some probability `(1-p)`
							false => Some(FromOrchestra::Communication {
								msg: CandidateValidationMessage::ValidateFromChainState(
									candidate_receipt,
									pov,
									timeout,
									response_sender,
								),
							}),
						}
					},
					FakeCandidateValidation::BackingInvalid |
					FakeCandidateValidation::BackingAndApprovalInvalid => {
						// Maliciously set the validation result to invalid for a valid candidate with probability `p`
						let behave_maliciously = self.distribution.sample(&mut rand::thread_rng());
						match behave_maliciously {
							true => {
								let validation_result = ValidationResult::Invalid(
									self.fake_validation_error.clone().into(),
								);
								gum::info!(
									target: MALUS,
									para_id = ?candidate_receipt.descriptor.para_id,
									"😈 Maliciously sending invalid validation result: {:?}.",
									&validation_result,
								);
								// We're not even checking the candidate, this makes us appear faster than honest validators.
								response_sender.send(Ok(validation_result)).unwrap();
								None
							},
							// With some probability `(1-p)` we behave normally
							false => {
								gum::info!(target: MALUS, "😈 'Decided' to not act maliciously.",);

								Some(FromOrchestra::Communication {
									msg: CandidateValidationMessage::ValidateFromChainState(
										candidate_receipt,
										pov,
										timeout,
										response_sender,
									),
								})
							},
						}
					},
					_ => Some(FromOrchestra::Communication {
						msg: CandidateValidationMessage::ValidateFromChainState(
							candidate_receipt,
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

	fn intercept_outgoing(
		&self,
		msg: overseer::CandidateValidationOutgoingMessages,
	) -> Option<overseer::CandidateValidationOutgoingMessages> {
		Some(msg)
	}
}
