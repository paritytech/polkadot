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

// Filter wrapping related types.
use crate::{interceptor::*, shared::MALUS, FakeCandidateValidation, FakeCandidateValidationError};
use polkadot_node_primitives::{InvalidCandidate, ValidationResult};
use polkadot_node_subsystem::messages::CandidateValidationMessage;

#[derive(Clone, Debug)]
/// An interceptor which fakes validation result with a preconfigured result.
/// Replaces `CandidateValidationSubsystem`.
pub struct ReplaceValidationResult {
	fake_validation: FakeCandidateValidation,
	fake_validation_error: FakeCandidateValidationError,
}

impl ReplaceValidationResult {
	pub fn new(
		fake_validation: FakeCandidateValidation,
		fake_validation_error: FakeCandidateValidationError,
	) -> Self {
		Self { fake_validation, fake_validation_error }
	}
}

impl<Sender> MessageInterceptor<Sender> for ReplaceValidationResult
where
	Sender: overseer::SubsystemSender<CandidateValidationMessage> + Clone + Send + 'static,
{
	type Message = CandidateValidationMessage;

	// Capture all candidate validation requests and depending on configuration fail them.
	// MaybeTODO: add option to configure the failure reason.
	fn intercept_incoming(
		&self,
		_sender: &mut Sender,
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
					FakeCandidateValidation::ApprovalInvalid |
					FakeCandidateValidation::BackingAndApprovalInvalid => {
						let validation_result =
							ValidationResult::Invalid(InvalidCandidate::InvalidOutputs);

						tracing::info!(
							target = MALUS,
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
					CandidateValidationMessage::ValidateFromChainState(descriptor, pov, timeout, sender),
			} => {
				match self.fake_validation {
					FakeCandidateValidation::BackingInvalid |
					FakeCandidateValidation::BackingAndApprovalInvalid => {
						let validation_result =
							ValidationResult::Invalid(self.fake_validation_error.clone().into());
						tracing::info!(
							target = MALUS,
							para_id = ?descriptor.para_id,
							candidate_hash = ?descriptor.para_head,
							"ValidateFromChainState result: {:?}",
							&validation_result
						);

						// We're not even checking the candidate, this makes us appear faster than honest validators.
						sender.send(Ok(validation_result)).unwrap();
						None
					},
					_ => Some(FromOverseer::Communication {
						msg: CandidateValidationMessage::ValidateFromChainState(
							descriptor, pov, timeout, sender,
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
