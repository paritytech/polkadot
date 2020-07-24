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
	Subsystem, SubsystemContext, SpawnedSubsystem, SubsystemResult,
	FromOverseer, OverseerSignal,
};
use polkadot_subsystem::messages::{
	AllMessages, CandidateValidationMessage, RuntimeApiMessage, ValidationFailed, RuntimeApiRequest,
};
use polkadot_node_primitives::{ValidationResult, ValidationOutputs};
use polkadot_primitives::v1::{
	ValidationCode, OmittedValidationData, PoV, CandidateDescriptor, LocalValidationData,
	GlobalValidationData, OccupiedCoreAssumption, Hash, validation_data_hash,
};
use polkadot_parachain::wasm_executor::{self, ValidationPool, ExecutionMode};
use polkadot_parachain::primitives::{ValidationResult as WasmValidationResult, ValidationParams};

use parity_scale_codec::Encode;

use futures::channel::oneshot;
use futures::prelude::*;

use std::sync::Arc;

/// The candidate validation subsystem.
pub struct CandidateValidationSubsystem;

impl<C> Subsystem<C> for CandidateValidationSubsystem
	where C: SubsystemContext<Message = CandidateValidationMessage>
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		SpawnedSubsystem {
			name: "candidate-validation-subsystem",
			future: run(ctx).map(|_| ()).boxed(),
		}
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
					let res = spawn_validate_from_chain_state(
						&mut ctx,
						Some(pool.clone()),
						descriptor,
						pov,
					).await;

					match res {
						Ok(x) => { let _ = response_sender.send(x); }
						Err(e)=> return Err(e),
					}
				}
				CandidateValidationMessage::ValidateFromExhaustive(
					omitted_validation,
					validation_code,
					descriptor,
					pov,
					response_sender,
				) => {
					let res = spawn_validate_exhaustive(
						&mut ctx,
						Some(pool.clone()),
						omitted_validation,
						validation_code,
						descriptor,
						pov,
					).await;

					match res {
						Ok(x) => { let _ = response_sender.send(x); }
						Err(e)=> return Err(e),
					}
				}
			}
		}
	}
}

async fn runtime_api_request<T>(
	ctx: &mut impl SubsystemContext<Message = CandidateValidationMessage>,
	relay_parent: Hash,
	request: RuntimeApiRequest,
	receiver: oneshot::Receiver<T>,
) -> SubsystemResult<T> {
	ctx.send_message(
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			relay_parent,
			request,
		))
	).await?;

	receiver.await.map_err(Into::into)
}

enum AssumptionCheckOutcome {
	Matches(OmittedValidationData, ValidationCode),
	DoesNotMatch,
	BadRequest,
}

async fn check_assumption_validation_data(
	ctx: &mut impl SubsystemContext<Message = CandidateValidationMessage>,
	descriptor: &CandidateDescriptor,
	global_validation_data: &GlobalValidationData,
	assumption: OccupiedCoreAssumption,
) -> SubsystemResult<AssumptionCheckOutcome> {
	let local_validation_data = {
		let (tx, rx) = oneshot::channel();
		let d = runtime_api_request(
			ctx,
			descriptor.relay_parent,
			RuntimeApiRequest::LocalValidationData(
				descriptor.para_id,
				assumption,
				tx,
			),
			rx,
		).await?;

		match d {
			None => {
				return Ok(AssumptionCheckOutcome::BadRequest);
			}
			Some(d) => d,
		}
	};

	let validation_data_hash = validation_data_hash(
		&global_validation_data,
		&local_validation_data,
	);

	SubsystemResult::Ok(if descriptor.validation_data_hash == validation_data_hash {
		let omitted_validation = OmittedValidationData {
			global_validation: global_validation_data.clone(),
			local_validation: local_validation_data,
		};

		let (code_tx, code_rx) = oneshot::channel();
		let validation_code = runtime_api_request(
			ctx,
			descriptor.relay_parent,
			RuntimeApiRequest::ValidationCode_New(
				descriptor.para_id,
				OccupiedCoreAssumption::Included,
				code_tx,
			),
			code_rx,
		).await?;

		match validation_code {
			None => AssumptionCheckOutcome::BadRequest,
			Some(v) => AssumptionCheckOutcome::Matches(omitted_validation, v),
		}
	} else {
		AssumptionCheckOutcome::DoesNotMatch
	})
}

async fn spawn_validate_from_chain_state(
	ctx: &mut impl SubsystemContext<Message = CandidateValidationMessage>,
	validation_pool: Option<ValidationPool>,
	descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
) -> SubsystemResult<Result<ValidationResult, ValidationFailed>> {
	// The candidate descriptor has a `validation_data_hash` which corresponds to
	// one of up to two possible values that we can derive from the state of the
	// relay-parent. We can fetch these values by getting the `global_validation_data`,
	// and both `local_validation_data` based on the different `OccupiedCoreAssumption`s.
	let global_validation_data = {
		let (tx, rx) = oneshot::channel();
		runtime_api_request(
			ctx,
			descriptor.relay_parent,
			RuntimeApiRequest::GlobalValidationData(tx),
			rx,
		).await?
	};

	match check_assumption_validation_data(
		ctx,
		&descriptor,
		&global_validation_data,
		OccupiedCoreAssumption::Included,
	).await? {
		AssumptionCheckOutcome::Matches(omitted_validation, validation_code) => {
			return spawn_validate_exhaustive(
				ctx,
				validation_pool,
				omitted_validation,
				validation_code,
				descriptor,
				pov,
			).await;
		}
		AssumptionCheckOutcome::DoesNotMatch => {},
		AssumptionCheckOutcome::BadRequest => return Ok(Err(ValidationFailed)),
	}

	match check_assumption_validation_data(
		ctx,
		&descriptor,
		&global_validation_data,
		OccupiedCoreAssumption::TimedOut,
	).await? {
		AssumptionCheckOutcome::Matches(omitted_validation, validation_code) => {
			return spawn_validate_exhaustive(
				ctx,
				validation_pool,
				omitted_validation,
				validation_code,
				descriptor,
				pov,
			).await;
		}
		AssumptionCheckOutcome::DoesNotMatch => {},
		AssumptionCheckOutcome::BadRequest => return Ok(Err(ValidationFailed)),
	}

	// If neither the assumption of the occupied core having the para included or the assumption
	// of the occupied core timing out are valid, then the validation_data_hash in the descriptor
	// is not based on the relay parent and is thus invalid.
	Ok(Ok(ValidationResult::Invalid))
}

async fn spawn_validate_exhaustive(
	ctx: &mut impl SubsystemContext<Message = CandidateValidationMessage>,
	validation_pool: Option<ValidationPool>,
	omitted_validation: OmittedValidationData,
	validation_code: ValidationCode,
	descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
) -> SubsystemResult<Result<ValidationResult, ValidationFailed>> {
	let (tx, rx) = oneshot::channel();
	let fut = async move {
		let res = validate_candidate_exhaustive(
			validation_pool,
			omitted_validation,
			validation_code,
			descriptor,
			pov,
		);

		let _ = tx.send(res);
	};

	ctx.spawn("blocking-candidate-validation-task", fut.boxed()).await?;
	rx.await.map_err(Into::into)
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
	global_validation_data: &GlobalValidationData,
	_local_validation_data: &LocalValidationData,
	result: &WasmValidationResult,
) -> bool {
	if result.head_data.0.len() > global_validation_data.max_head_data_size as _ {
		return false
	}

	if let Some(ref code) = result.new_validation_code {
		if code.0.len() > global_validation_data.max_code_size as _ {
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
) -> Result<ValidationResult, ValidationFailed> {
	if !passes_basic_checks(&descriptor, None, &*pov) {
		return Ok(ValidationResult::Invalid);
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

	match wasm_executor::validate_candidate(
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
					global_validation_data: global_validation,
					local_validation_data: local_validation,
					upward_messages: res.upward_messages,
					fees: 0,
					new_validation_code: res.new_validation_code,
				})
			} else {
				ValidationResult::Invalid
			})
		}
	}
}
