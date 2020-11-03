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

#![deny(unused_crate_dependencies, unused_results)]
#![warn(missing_docs)]

use polkadot_subsystem::{
	Subsystem, SubsystemContext, SpawnedSubsystem, SubsystemResult, SubsystemError,
	FromOverseer, OverseerSignal,
	messages::{
		AllMessages, CandidateValidationMessage, RuntimeApiMessage,
		ValidationFailed, RuntimeApiRequest,
	},
};
use polkadot_node_subsystem_util::{
	metrics::{self, prometheus},
};
use polkadot_subsystem::errors::RuntimeApiError;
use polkadot_node_primitives::{ValidationResult, InvalidCandidate};
use polkadot_primitives::v1::{
	ValidationCode, PoV, CandidateDescriptor, PersistedValidationData,
	OccupiedCoreAssumption, Hash, ValidationOutputs,
};
use polkadot_parachain::wasm_executor::{
	self, ValidationPool, ExecutionMode, ValidationError,
	InvalidCandidate as WasmInvalidCandidate,
};
use polkadot_parachain::primitives::{ValidationResult as WasmValidationResult, ValidationParams};

use parity_scale_codec::Encode;
use sp_core::traits::SpawnNamed;

use futures::channel::oneshot;
use futures::prelude::*;

use std::sync::Arc;

const LOG_TARGET: &'static str = "candidate_validation";

/// The candidate validation subsystem.
pub struct CandidateValidationSubsystem<S> {
	spawn: S,
	metrics: Metrics,
}

#[derive(Clone)]
struct MetricsInner {
	validation_requests: prometheus::CounterVec<prometheus::U64>,
}

/// Candidate validation metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_validation_event(&self, event: &Result<ValidationResult, ValidationFailed>) {
		if let Some(metrics) = &self.0 {
			match event {
				Ok(ValidationResult::Valid(_, _)) => {
					metrics.validation_requests.with_label_values(&["valid"]).inc();
				},
				Ok(ValidationResult::Invalid(_)) => {
					metrics.validation_requests.with_label_values(&["invalid"]).inc();
				},
				Err(_) => {
					metrics.validation_requests.with_label_values(&["validation failure"]).inc();
				},
			}
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			validation_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_validation_requests_total",
						"Number of validation requests served.",
					),
					&["validity"],
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

impl<S> CandidateValidationSubsystem<S> {
	/// Create a new `CandidateValidationSubsystem` with the given task spawner.
	pub fn new(spawn: S, metrics: Metrics) -> Self {
		CandidateValidationSubsystem { spawn, metrics }
	}
}

impl<S, C> Subsystem<C> for CandidateValidationSubsystem<S> where
	C: SubsystemContext<Message = CandidateValidationMessage>,
	S: SpawnNamed + Clone + 'static,
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		let future = run(ctx, self.spawn, self.metrics)
			.map_err(|e| SubsystemError::with_origin("candidate-validation", e))
			.boxed();
		SpawnedSubsystem {
			name: "candidate-validation-subsystem",
			future,
		}
	}
}

async fn run(
	mut ctx: impl SubsystemContext<Message = CandidateValidationMessage>,
	spawn: impl SpawnNamed + Clone + 'static,
	metrics: Metrics,
)
	-> SubsystemResult<()>
{
	let execution_mode = ExecutionMode::ExternalProcessSelfHost(ValidationPool::new());

	loop {
		match ctx.recv().await? {
			FromOverseer::Signal(OverseerSignal::ActiveLeaves(_)) => {}
			FromOverseer::Signal(OverseerSignal::BlockFinalized(_)) => {}
			FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOverseer::Communication { msg } => match msg {
				CandidateValidationMessage::ValidateFromChainState(
					descriptor,
					pov,
					response_sender,
				) => {
					let res = spawn_validate_from_chain_state(
						&mut ctx,
						execution_mode.clone(),
						descriptor,
						pov,
						spawn.clone(),
					).await;

					match res {
						Ok(x) => {
							metrics.on_validation_event(&x);
							let _ = response_sender.send(x);
						}
						Err(e) => return Err(e),
					}
				}
				CandidateValidationMessage::ValidateFromExhaustive(
					persisted_validation_data,
					validation_code,
					descriptor,
					pov,
					response_sender,
				) => {
					let res = spawn_validate_exhaustive(
						&mut ctx,
						execution_mode.clone(),
						persisted_validation_data,
						validation_code,
						descriptor,
						pov,
						spawn.clone(),
					).await;

					match res {
						Ok(x) => {
							metrics.on_validation_event(&x);
							if let Err(_e) = response_sender.send(x) {
								log::warn!(
									target: LOG_TARGET,
									"Requester of candidate validation dropped",
								)
							}
						},
						Err(e) => return Err(e),
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
	receiver: oneshot::Receiver<Result<T, RuntimeApiError>>,
) -> SubsystemResult<Result<T, RuntimeApiError>> {
	ctx.send_message(
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			relay_parent,
			request,
		))
	).await?;

	receiver.await.map_err(Into::into)
}

#[derive(Debug)]
enum AssumptionCheckOutcome {
	Matches(PersistedValidationData, ValidationCode),
	DoesNotMatch,
	BadRequest,
}

async fn check_assumption_validation_data(
	ctx: &mut impl SubsystemContext<Message = CandidateValidationMessage>,
	descriptor: &CandidateDescriptor,
	assumption: OccupiedCoreAssumption,
) -> SubsystemResult<AssumptionCheckOutcome> {
	let validation_data = {
		let (tx, rx) = oneshot::channel();
		let d = runtime_api_request(
			ctx,
			descriptor.relay_parent,
			RuntimeApiRequest::PersistedValidationData(
				descriptor.para_id,
				assumption,
				tx,
			),
			rx,
		).await?;

		match d {
			Ok(None) | Err(_) => {
				return Ok(AssumptionCheckOutcome::BadRequest);
			}
			Ok(Some(d)) => d,
		}
	};

	let persisted_validation_data_hash = validation_data.hash();

	SubsystemResult::Ok(if descriptor.persisted_validation_data_hash == persisted_validation_data_hash {
		let (code_tx, code_rx) = oneshot::channel();
		let validation_code = runtime_api_request(
			ctx,
			descriptor.relay_parent,
			RuntimeApiRequest::ValidationCode(
				descriptor.para_id,
				assumption,
				code_tx,
			),
			code_rx,
		).await?;

		match validation_code {
			Ok(None) | Err(_) => AssumptionCheckOutcome::BadRequest,
			Ok(Some(v)) => AssumptionCheckOutcome::Matches(validation_data, v),
		}
	} else {
		AssumptionCheckOutcome::DoesNotMatch
	})
}

async fn find_assumed_validation_data(
	ctx: &mut impl SubsystemContext<Message = CandidateValidationMessage>,
	descriptor: &CandidateDescriptor,
) -> SubsystemResult<AssumptionCheckOutcome> {
	// The candidate descriptor has a `persisted_validation_data_hash` which corresponds to
	// one of up to two possible values that we can derive from the state of the
	// relay-parent. We can fetch these values by getting the persisted validation data
	// based on the different `OccupiedCoreAssumption`s.

	const ASSUMPTIONS: &[OccupiedCoreAssumption] = &[
		OccupiedCoreAssumption::Included,
		OccupiedCoreAssumption::TimedOut,
		// `TimedOut` and `Free` both don't perform any speculation and therefore should be the same
		// for our purposes here. In other words, if `TimedOut` matched then the `Free` must be
		// matched as well.
	];

	// Consider running these checks in parallel to reduce validation latency.
	for assumption in ASSUMPTIONS {
		let outcome = check_assumption_validation_data(ctx, descriptor, *assumption).await?;

		match outcome {
			AssumptionCheckOutcome::Matches(_, _) => return Ok(outcome),
			AssumptionCheckOutcome::BadRequest => return Ok(outcome),
			AssumptionCheckOutcome::DoesNotMatch => continue,
		}
	}

	Ok(AssumptionCheckOutcome::DoesNotMatch)
}

async fn spawn_validate_from_chain_state(
	ctx: &mut impl SubsystemContext<Message = CandidateValidationMessage>,
	execution_mode: ExecutionMode,
	descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
	spawn: impl SpawnNamed + 'static,
) -> SubsystemResult<Result<ValidationResult, ValidationFailed>> {
	let (validation_data, validation_code) =
		match find_assumed_validation_data(ctx, &descriptor).await? {
			AssumptionCheckOutcome::Matches(validation_data, validation_code) => {
				(validation_data, validation_code)
			}
			AssumptionCheckOutcome::DoesNotMatch => {
				// If neither the assumption of the occupied core having the para included or the assumption
				// of the occupied core timing out are valid, then the persisted_validation_data_hash in the descriptor
				// is not based on the relay parent and is thus invalid.
				return Ok(Ok(ValidationResult::Invalid(InvalidCandidate::BadParent)));
			}
			AssumptionCheckOutcome::BadRequest => {
				return Ok(Err(ValidationFailed("Assumption Check: Bad request".into())));
			}
		};

	let validation_result = spawn_validate_exhaustive(
		ctx,
		execution_mode,
		validation_data,
		validation_code,
		descriptor.clone(),
		pov,
		spawn,
	)
	.await;

	if let Ok(Ok(ValidationResult::Valid(ref outputs, _))) = validation_result {
		let (tx, rx) = oneshot::channel();
		match runtime_api_request(
			ctx,
			descriptor.relay_parent,
			RuntimeApiRequest::CheckValidationOutputs(descriptor.para_id, outputs.clone(), tx),
			rx,
		)
		.await?
		{
			Ok(true) => {}
			Ok(false) => {
				return Ok(Ok(ValidationResult::Invalid(
					InvalidCandidate::InvalidOutputs,
				)));
			}
			Err(_) => {
				return Ok(Err(ValidationFailed("Check Validation Outputs: Bad request".into())));
			}
		}
	}

	validation_result
}

async fn spawn_validate_exhaustive(
	ctx: &mut impl SubsystemContext<Message = CandidateValidationMessage>,
	execution_mode: ExecutionMode,
	persisted_validation_data: PersistedValidationData,
	validation_code: ValidationCode,
	descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
	spawn: impl SpawnNamed + 'static,
) -> SubsystemResult<Result<ValidationResult, ValidationFailed>> {
	let (tx, rx) = oneshot::channel();
	let fut = async move {
		let res = validate_candidate_exhaustive::<RealValidationBackend, _>(
			execution_mode,
			persisted_validation_data,
			validation_code,
			descriptor,
			pov,
			spawn,
		);

		let _ = tx.send(res);
	};

	ctx.spawn_blocking("blocking-candidate-validation-task", fut.boxed()).await?;
	rx.await.map_err(Into::into)
}

/// Does basic checks of a candidate. Provide the encoded PoV-block. Returns `Ok` if basic checks
/// are passed, `Err` otherwise.
fn perform_basic_checks(
	candidate: &CandidateDescriptor,
	max_block_data_size: Option<u64>,
	pov: &PoV,
) -> Result<(), InvalidCandidate> {
	let encoded_pov = pov.encode();
	let hash = pov.hash();

	if let Some(max_size) = max_block_data_size {
		if encoded_pov.len() as u64 > max_size {
			return Err(InvalidCandidate::ParamsTooLarge(encoded_pov.len() as u64));
		}
	}

	if hash != candidate.pov_hash {
		return Err(InvalidCandidate::HashMismatch);
	}

	if let Err(()) = candidate.check_collator_signature() {
		return Err(InvalidCandidate::BadSignature);
	}

	Ok(())
}

trait ValidationBackend {
	type Arg;

	fn validate<S: SpawnNamed + 'static>(
		arg: Self::Arg,
		validation_code: &ValidationCode,
		params: ValidationParams,
		spawn: S,
	) -> Result<WasmValidationResult, ValidationError>;
}

struct RealValidationBackend;

impl ValidationBackend for RealValidationBackend {
	type Arg = ExecutionMode;

	fn validate<S: SpawnNamed + 'static>(
		execution_mode: ExecutionMode,
		validation_code: &ValidationCode,
		params: ValidationParams,
		spawn: S,
	) -> Result<WasmValidationResult, ValidationError> {
		wasm_executor::validate_candidate(
			&validation_code.0,
			params,
			&execution_mode,
			spawn,
		)
	}
}

/// Validates the candidate from exhaustive parameters.
///
/// Sends the result of validation on the channel once complete.
fn validate_candidate_exhaustive<B: ValidationBackend, S: SpawnNamed + 'static>(
	backend_arg: B::Arg,
	persisted_validation_data: PersistedValidationData,
	validation_code: ValidationCode,
	descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
	spawn: S,
) -> Result<ValidationResult, ValidationFailed> {
	if let Err(e) = perform_basic_checks(&descriptor, None, &*pov) {
		return Ok(ValidationResult::Invalid(e))
	}

	let params = ValidationParams {
		parent_head: persisted_validation_data.parent_head.clone(),
		block_data: pov.block_data.clone(),
		relay_chain_height: persisted_validation_data.block_number,
		dmq_mqc_head: persisted_validation_data.dmq_mqc_head,
		hrmp_mqc_heads: persisted_validation_data.hrmp_mqc_heads.clone(),
	};

	match B::validate(backend_arg, &validation_code, params, spawn) {
		Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::Timeout)) =>
			Ok(ValidationResult::Invalid(InvalidCandidate::Timeout)),
		Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::ParamsTooLarge(l))) =>
			Ok(ValidationResult::Invalid(InvalidCandidate::ParamsTooLarge(l as u64))),
		Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::CodeTooLarge(l))) =>
			Ok(ValidationResult::Invalid(InvalidCandidate::CodeTooLarge(l as u64))),
		Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::BadReturn)) =>
			Ok(ValidationResult::Invalid(InvalidCandidate::BadReturn)),
		Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::WasmExecutor(e))) =>
			Ok(ValidationResult::Invalid(InvalidCandidate::ExecutionError(e.to_string()))),
		Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::ExternalWasmExecutor(e))) =>
			Ok(ValidationResult::Invalid(InvalidCandidate::ExecutionError(e.to_string()))),
		Err(ValidationError::Internal(e)) => Err(ValidationFailed(e.to_string())),
		Ok(res) => {
			let outputs = ValidationOutputs {
				head_data: res.head_data,
				upward_messages: res.upward_messages,
				new_validation_code: res.new_validation_code,
				processed_downward_messages: res.processed_downward_messages,
			};
			Ok(ValidationResult::Valid(outputs, persisted_validation_data))
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_node_subsystem_test_helpers as test_helpers;
	use polkadot_primitives::v1::{HeadData, BlockData, UpwardMessage};
	use sp_core::testing::TaskExecutor;
	use futures::executor;
	use assert_matches::assert_matches;
	use sp_keyring::Sr25519Keyring;

	struct MockValidationBackend;

	struct MockValidationArg {
		result: Result<WasmValidationResult, ValidationError>,
	}

	impl ValidationBackend for MockValidationBackend {
		type Arg = MockValidationArg;

		fn validate<S: SpawnNamed + 'static>(
			arg: Self::Arg,
			_validation_code: &ValidationCode,
			_params: ValidationParams,
			_spawn: S,
		) -> Result<WasmValidationResult, ValidationError> {
			arg.result
		}
	}

	fn collator_sign(descriptor: &mut CandidateDescriptor, collator: Sr25519Keyring) {
		descriptor.collator = collator.public().into();
		let payload = polkadot_primitives::v1::collator_signature_payload(
			&descriptor.relay_parent,
			&descriptor.para_id,
			&descriptor.persisted_validation_data_hash,
			&descriptor.pov_hash,
		);

		descriptor.signature = collator.sign(&payload[..]).into();
		assert!(descriptor.check_collator_signature().is_ok());
	}

	#[test]
	fn correctly_checks_included_assumption() {
		let validation_data: PersistedValidationData = Default::default();
		let validation_code: ValidationCode = vec![1, 2, 3].into();

		let persisted_validation_data_hash = validation_data.hash();
		let relay_parent = [2; 32].into();
		let para_id = 5.into();

		let mut candidate = CandidateDescriptor::default();
		candidate.relay_parent = relay_parent;
		candidate.persisted_validation_data_hash = persisted_validation_data_hash;
		candidate.para_id = para_id;

		let pool = TaskExecutor::new();
		let (mut ctx, mut ctx_handle) = test_helpers::make_subsystem_context(pool.clone());

		let (check_fut, check_result) = check_assumption_validation_data(
			&mut ctx,
			&candidate,
			OccupiedCoreAssumption::Included,
		).remote_handle();

		let test_fut = async move {
			assert_matches!(
				ctx_handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					rp,
					RuntimeApiRequest::PersistedValidationData(
						p,
						OccupiedCoreAssumption::Included,
						tx
					),
				)) => {
					assert_eq!(rp, relay_parent);
					assert_eq!(p, para_id);

					let _ = tx.send(Ok(Some(validation_data.clone())));
				}
			);

			assert_matches!(
				ctx_handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					rp,
					RuntimeApiRequest::ValidationCode(p, OccupiedCoreAssumption::Included, tx)
				)) => {
					assert_eq!(rp, relay_parent);
					assert_eq!(p, para_id);

					let _ = tx.send(Ok(Some(validation_code.clone())));
				}
			);

			assert_matches!(check_result.await.unwrap(), AssumptionCheckOutcome::Matches(o, v) => {
				assert_eq!(o, validation_data);
				assert_eq!(v, validation_code);
			});
		};

		let test_fut = future::join(test_fut, check_fut);
		executor::block_on(test_fut);
	}

	#[test]
	fn correctly_checks_timed_out_assumption() {
		let validation_data: PersistedValidationData = Default::default();
		let validation_code: ValidationCode = vec![1, 2, 3].into();

		let persisted_validation_data_hash = validation_data.hash();
		let relay_parent = [2; 32].into();
		let para_id = 5.into();

		let mut candidate = CandidateDescriptor::default();
		candidate.relay_parent = relay_parent;
		candidate.persisted_validation_data_hash = persisted_validation_data_hash;
		candidate.para_id = para_id;

		let pool = TaskExecutor::new();
		let (mut ctx, mut ctx_handle) = test_helpers::make_subsystem_context(pool.clone());

		let (check_fut, check_result) = check_assumption_validation_data(
			&mut ctx,
			&candidate,
			OccupiedCoreAssumption::TimedOut,
		).remote_handle();

		let test_fut = async move {
			assert_matches!(
				ctx_handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					rp,
					RuntimeApiRequest::PersistedValidationData(
						p,
						OccupiedCoreAssumption::TimedOut,
						tx
					),
				)) => {
					assert_eq!(rp, relay_parent);
					assert_eq!(p, para_id);

					let _ = tx.send(Ok(Some(validation_data.clone())));
				}
			);

			assert_matches!(
				ctx_handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					rp,
					RuntimeApiRequest::ValidationCode(p, OccupiedCoreAssumption::TimedOut, tx)
				)) => {
					assert_eq!(rp, relay_parent);
					assert_eq!(p, para_id);

					let _ = tx.send(Ok(Some(validation_code.clone())));
				}
			);

			assert_matches!(check_result.await.unwrap(), AssumptionCheckOutcome::Matches(o, v) => {
				assert_eq!(o, validation_data);
				assert_eq!(v, validation_code);
			});
		};

		let test_fut = future::join(test_fut, check_fut);
		executor::block_on(test_fut);
	}

	#[test]
	fn check_is_bad_request_if_no_validation_data() {
		let validation_data: PersistedValidationData = Default::default();
		let persisted_validation_data_hash = validation_data.hash();
		let relay_parent = [2; 32].into();
		let para_id = 5.into();

		let mut candidate = CandidateDescriptor::default();
		candidate.relay_parent = relay_parent;
		candidate.persisted_validation_data_hash = persisted_validation_data_hash;
		candidate.para_id = para_id;

		let pool = TaskExecutor::new();
		let (mut ctx, mut ctx_handle) = test_helpers::make_subsystem_context(pool.clone());

		let (check_fut, check_result) = check_assumption_validation_data(
			&mut ctx,
			&candidate,
			OccupiedCoreAssumption::Included,
		).remote_handle();

		let test_fut = async move {
			assert_matches!(
				ctx_handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					rp,
					RuntimeApiRequest::PersistedValidationData(
						p,
						OccupiedCoreAssumption::Included,
						tx
					),
				)) => {
					assert_eq!(rp, relay_parent);
					assert_eq!(p, para_id);

					let _ = tx.send(Ok(None));
				}
			);

			assert_matches!(check_result.await.unwrap(), AssumptionCheckOutcome::BadRequest);
		};

		let test_fut = future::join(test_fut, check_fut);
		executor::block_on(test_fut);
	}

	#[test]
	fn check_is_bad_request_if_no_validation_code() {
		let validation_data: PersistedValidationData = Default::default();
		let persisted_validation_data_hash = validation_data.hash();
		let relay_parent = [2; 32].into();
		let para_id = 5.into();

		let mut candidate = CandidateDescriptor::default();
		candidate.relay_parent = relay_parent;
		candidate.persisted_validation_data_hash = persisted_validation_data_hash;
		candidate.para_id = para_id;

		let pool = TaskExecutor::new();
		let (mut ctx, mut ctx_handle) = test_helpers::make_subsystem_context(pool.clone());

		let (check_fut, check_result) = check_assumption_validation_data(
			&mut ctx,
			&candidate,
			OccupiedCoreAssumption::TimedOut,
		).remote_handle();

		let test_fut = async move {
			assert_matches!(
				ctx_handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					rp,
					RuntimeApiRequest::PersistedValidationData(
						p,
						OccupiedCoreAssumption::TimedOut,
						tx
					),
				)) => {
					assert_eq!(rp, relay_parent);
					assert_eq!(p, para_id);

					let _ = tx.send(Ok(Some(validation_data.clone())));
				}
			);

			assert_matches!(
				ctx_handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					rp,
					RuntimeApiRequest::ValidationCode(p, OccupiedCoreAssumption::TimedOut, tx)
				)) => {
					assert_eq!(rp, relay_parent);
					assert_eq!(p, para_id);

					let _ = tx.send(Ok(None));
				}
			);

			assert_matches!(check_result.await.unwrap(), AssumptionCheckOutcome::BadRequest);
		};

		let test_fut = future::join(test_fut, check_fut);
		executor::block_on(test_fut);
	}

	#[test]
	fn check_does_not_match() {
		let validation_data: PersistedValidationData = Default::default();
		let relay_parent = [2; 32].into();
		let para_id = 5.into();

		let mut candidate = CandidateDescriptor::default();
		candidate.relay_parent = relay_parent;
		candidate.persisted_validation_data_hash = [3; 32].into();
		candidate.para_id = para_id;

		let pool = TaskExecutor::new();
		let (mut ctx, mut ctx_handle) = test_helpers::make_subsystem_context(pool.clone());

		let (check_fut, check_result) = check_assumption_validation_data(
			&mut ctx,
			&candidate,
			OccupiedCoreAssumption::Included,
		).remote_handle();

		let test_fut = async move {
			assert_matches!(
				ctx_handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					rp,
					RuntimeApiRequest::PersistedValidationData(
						p,
						OccupiedCoreAssumption::Included,
						tx
					),
				)) => {
					assert_eq!(rp, relay_parent);
					assert_eq!(p, para_id);

					let _ = tx.send(Ok(Some(validation_data.clone())));
				}
			);

			assert_matches!(check_result.await.unwrap(), AssumptionCheckOutcome::DoesNotMatch);
		};

		let test_fut = future::join(test_fut, check_fut);
		executor::block_on(test_fut);
	}

	#[test]
	fn candidate_validation_ok_is_ok() {
		let validation_data: PersistedValidationData = Default::default();

		let pov = PoV { block_data: BlockData(vec![1; 32]) };

		let mut descriptor = CandidateDescriptor::default();
		descriptor.pov_hash = pov.hash();
		collator_sign(&mut descriptor, Sr25519Keyring::Alice);

		assert!(perform_basic_checks(&descriptor, Some(1024), &pov).is_ok());

		let validation_result = WasmValidationResult {
			head_data: HeadData(vec![1, 1, 1]),
			new_validation_code: Some(vec![2, 2, 2].into()),
			upward_messages: Vec::new(),
			processed_downward_messages: 0,
		};

		let v = validate_candidate_exhaustive::<MockValidationBackend, _>(
			MockValidationArg { result: Ok(validation_result) },
			validation_data.clone(),
			vec![1, 2, 3].into(),
			descriptor,
			Arc::new(pov),
			TaskExecutor::new(),
		).unwrap();

		assert_matches!(v, ValidationResult::Valid(outputs, used_validation_data) => {
			assert_eq!(outputs.head_data, HeadData(vec![1, 1, 1]));
			assert_eq!(outputs.upward_messages, Vec::<UpwardMessage>::new());
			assert_eq!(outputs.new_validation_code, Some(vec![2, 2, 2].into()));
			assert_eq!(used_validation_data, validation_data);
		});
	}

	#[test]
	fn candidate_validation_bad_return_is_invalid() {
		let validation_data: PersistedValidationData = Default::default();

		let pov = PoV { block_data: BlockData(vec![1; 32]) };

		let mut descriptor = CandidateDescriptor::default();
		descriptor.pov_hash = pov.hash();
		collator_sign(&mut descriptor, Sr25519Keyring::Alice);

		assert!(perform_basic_checks(&descriptor, Some(1024), &pov).is_ok());

		let v = validate_candidate_exhaustive::<MockValidationBackend, _>(
			MockValidationArg {
				result: Err(ValidationError::InvalidCandidate(
					WasmInvalidCandidate::BadReturn
				))
			},
			validation_data,
			vec![1, 2, 3].into(),
			descriptor,
			Arc::new(pov),
			TaskExecutor::new(),
		).unwrap();

		assert_matches!(v, ValidationResult::Invalid(InvalidCandidate::BadReturn));
	}

	#[test]
	fn candidate_validation_timeout_is_internal_error() {
		let validation_data: PersistedValidationData = Default::default();

		let pov = PoV { block_data: BlockData(vec![1; 32]) };

		let mut descriptor = CandidateDescriptor::default();
		descriptor.pov_hash = pov.hash();
		collator_sign(&mut descriptor, Sr25519Keyring::Alice);

		assert!(perform_basic_checks(&descriptor, Some(1024), &pov).is_ok());

		let v = validate_candidate_exhaustive::<MockValidationBackend, _>(
			MockValidationArg {
				result: Err(ValidationError::InvalidCandidate(
					WasmInvalidCandidate::Timeout
				))
			},
			validation_data,
			vec![1, 2, 3].into(),
			descriptor,
			Arc::new(pov),
			TaskExecutor::new(),
		);

		assert_matches!(v, Ok(ValidationResult::Invalid(InvalidCandidate::Timeout)));
	}
}
