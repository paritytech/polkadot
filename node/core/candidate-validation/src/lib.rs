// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! The Candidate Validation subsystem.
//!
//! This handles incoming requests from other subsystems to validate candidates
//! according to a validation function. This delegates validation to an underlying
//! pool of processes used for execution of the Wasm.

use polkadot_subsystem::{
	Subsystem, SubsystemContext, SpawnedSubsystem, SubsystemError, SubsystemResult,
	FromOverseer, OverseerSignal,
};
use polkadot_subsystem::messages::{
	AllMessages, CandidateValidationMessage, RuntimeApiMessage, ValidationFailed, RuntimeApiRequest,
};
use polkadot_node_primitives::{ValidationResult, ValidationOutputs};
use polkadot_primitives::v1::{
	ValidationCode, OmittedValidationData, PoV, CandidateDescriptor, LocalValidationData,
	GlobalValidationSchedule,
};
use polkadot_parachain::wasm_executor::{self, ValidationPool, ExecutionMode};
use polkadot_parachain::primitives::{ValidationResult as WasmValidationResult, ValidationParams};

use parity_scale_codec::{Encode, Decode};

use futures::channel::oneshot;
use futures::prelude::*;

use std::sync::Arc;

/// The candidate validation subsystem.
pub struct CandidateValidationSubsystem;

impl<C> Subsystem<C> for CandidateValidationSubsystem
	where C: SubsystemContext<Message = CandidateValidationMessage>
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		SpawnedSubsystem(run(ctx).map(|_| ()).boxed())
	}
}

async fn run(mut ctx: impl SubsystemContext<Message = CandidateValidationMessage>)
	-> SubsystemResult<()>
{
	let pool = ValidationPool::new();

	loop {
		match ctx.recv().await? {
			FromOverseer::Signal(OverseerSignal::StartWork(_)) => {}
			FromOverseer::Signal(OverseerSignal::StopWork(_)) => {}
			FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOverseer::Communication { msg } => match msg {
				CandidateValidationMessage::ValidateFromChainState(
					descriptor,
					pov,
					response_sender,
				) => {
					spawn_validate_from_chain_state(
						&mut ctx,
						Some(pool.clone()),
						descriptor,
						pov,
						response_sender
					).await?;
				}
				CandidateValidationMessage::ValidateFromExhaustive(
					omitted_validation,
					validation_code,
					descriptor,
					pov,
					response_sender,
				) => {
					spawn_validate_exhaustive(
						&mut ctx,
						Some(pool.clone()),
						omitted_validation,
						validation_code,
						descriptor,
						pov,
						response_sender
					).await?;
				}
			}
		}
	}
}

async fn spawn_validate_from_chain_state(
	ctx: &mut impl SubsystemContext<Message = CandidateValidationMessage>,
	validation_pool: Option<ValidationPool>,
	descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
	response_sender: oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
) -> SubsystemResult<()> {
	// TODO [now]: check that there is no candidate for the para at the relay-parent
	// and then fetch global/local validation info & validation code.
	let omitted_validation = unimplemented!();
	let validation_code = unimplemented!();

	spawn_validate_exhaustive(
		ctx,
		validation_pool,
		omitted_validation,
		validation_code,
		descriptor,
		pov,
		response_sender,
	).await
}

async fn spawn_validate_exhaustive(
	ctx: &mut impl SubsystemContext<Message = CandidateValidationMessage>,
	validation_pool: Option<ValidationPool>,
	omitted_validation: OmittedValidationData,
	validation_code: ValidationCode,
	descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
	response_sender: oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
) -> SubsystemResult<()> {
	let fut = async move {
		validate_candidate_exhaustive(
			validation_pool,
			omitted_validation,
			validation_code,
			descriptor,
			pov,
			response_sender,
		);
	};

	ctx.spawn(fut.boxed()).await
}

/// Does basic checks of a candidate. Provide the encoded PoV-block. Returns `true` if basic checks
/// are passed, false otherwise.
fn passes_basic_checks(
	candidate: &CandidateDescriptor,
	max_block_data_size: Option<u64>,
	pov: &PoV,
) -> bool {
	let encoded_pov = pov.encode();
	let hash = pov.hash();

	if let Some(max_size) = max_block_data_size {
		if encoded_pov.len() as u64 > max_size {
			return false;
		}
	}

	if hash != candidate.pov_hash {
		return false;
	}

	if let Err(()) = candidate.check_collator_signature() {
		return false;
	}

	true
}

/// Check the result of Wasm execution against the constraints given by the relay-chain.
///
/// Returns `true` if checks pass, false otherwise.
fn check_wasm_result_against_constraints(
	global_validation_schedule: &GlobalValidationSchedule,
	_local_validation_data: &LocalValidationData,
	result: &WasmValidationResult,
) -> bool {
	if result.head_data.0.len() > global_validation_schedule.max_head_data_size as _ {
		return false
	}

	if let Some(ref code) = result.new_validation_code {
		if code.0.len() > global_validation_schedule.max_code_size as _ {
			return false
		}
	}

	true
}

/// Validates the candidate from exhaustive parameters.
///
/// Sends the result of validation on the channel once complete.
fn validate_candidate_exhaustive(
	validation_pool: Option<ValidationPool>,
	omitted_validation: OmittedValidationData,
	validation_code: ValidationCode,
	descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
	response_sender: oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
) {
	if !passes_basic_checks(&descriptor, None, &*pov) {
		let _ = response_sender.send(Ok(ValidationResult::Invalid));
		return;
	}

	let execution_mode = validation_pool.as_ref()
		.map(ExecutionMode::Remote)
		.unwrap_or(ExecutionMode::Local);

	let OmittedValidationData { global_validation, local_validation } = omitted_validation;

	let params = ValidationParams {
		parent_head: local_validation.parent_head.clone(),
		block_data: pov.block_data.clone(),
		max_code_size: global_validation.max_code_size,
		max_head_data_size: global_validation.max_head_data_size,
		relay_chain_height: global_validation.block_number,
		code_upgrade_allowed: local_validation.code_upgrade_allowed,
	};

	let res = match wasm_executor::validate_candidate(
		&validation_code.0,
		params,
		execution_mode,
	) {
		Err(wasm_executor::Error::BadReturn) => Ok(ValidationResult::Invalid),
		Err(_) => Err(ValidationFailed),
		Ok(res) => {

			let passes_post_checks = check_wasm_result_against_constraints(
				&global_validation,
				&local_validation,
				&res,
			);

			Ok(if passes_post_checks {
				ValidationResult::Valid(ValidationOutputs {
					head_data: res.head_data,
					global_validation_schedule: global_validation,
					local_validation_data: local_validation,
					upward_messages: res.upward_messages,
					fees: 0,
					new_validation_code: res.new_validation_code,
				})
			} else {
				ValidationResult::Invalid
			})
		}
	};

	let _ = response_sender.send(res);
}
