// Copyright (C) Parity Technologies (UK) Ltd.
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

use polkadot_node_core_pvf::{
	InvalidCandidate as WasmInvalidCandidate, PrepareError, PrepareStats, PvfPrepData,
	ValidationError, ValidationHost,
};
use polkadot_node_primitives::{
	BlockData, InvalidCandidate, PoV, ValidationResult, POV_BOMB_LIMIT, VALIDATION_CODE_BOMB_LIMIT,
};
use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	messages::{
		CandidateValidationMessage, PreCheckOutcome, RuntimeApiMessage, RuntimeApiRequest,
		ValidationFailed,
	},
	overseer, FromOrchestra, OverseerSignal, SpawnedSubsystem, SubsystemError, SubsystemResult,
	SubsystemSender,
};
use polkadot_node_subsystem_util::executor_params_at_relay_parent;
use polkadot_parachain::primitives::{ValidationParams, ValidationResult as WasmValidationResult};
use polkadot_primitives::{
	CandidateCommitments, CandidateDescriptor, CandidateReceipt, ExecutorParams, Hash,
	OccupiedCoreAssumption, PersistedValidationData, PvfExecTimeoutKind, PvfPrepTimeoutKind,
	ValidationCode, ValidationCodeHash,
};

use parity_scale_codec::Encode;

use futures::{channel::oneshot, prelude::*};

use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;

mod metrics;
use self::metrics::Metrics;

#[cfg(test)]
mod tests;

const LOG_TARGET: &'static str = "parachain::candidate-validation";

/// The amount of time to wait before retrying after an AmbiguousWorkerDeath validation error.
#[cfg(not(test))]
const PVF_EXECUTION_RETRY_DELAY: Duration = Duration::from_secs(3);
#[cfg(test)]
const PVF_EXECUTION_RETRY_DELAY: Duration = Duration::from_millis(200);

// Default PVF timeouts. Must never be changed! Use executor environment parameters in
// `session_info` pallet to adjust them. See also `PvfTimeoutKind` docs.
const DEFAULT_PRECHECK_PREPARATION_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_LENIENT_PREPARATION_TIMEOUT: Duration = Duration::from_secs(360);
const DEFAULT_BACKING_EXECUTION_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_APPROVAL_EXECUTION_TIMEOUT: Duration = Duration::from_secs(12);

/// Configuration for the candidate validation subsystem
#[derive(Clone)]
pub struct Config {
	/// The path where candidate validation can store compiled artifacts for PVFs.
	pub artifacts_cache_path: PathBuf,
	/// The path to the executable which can be used for spawning PVF compilation & validation
	/// workers.
	pub program_path: PathBuf,
}

/// The candidate validation subsystem.
pub struct CandidateValidationSubsystem {
	#[allow(missing_docs)]
	pub metrics: Metrics,
	#[allow(missing_docs)]
	pub pvf_metrics: polkadot_node_core_pvf::Metrics,
	config: Config,
}

impl CandidateValidationSubsystem {
	/// Create a new `CandidateValidationSubsystem` with the given task spawner and isolation
	/// strategy.
	///
	/// Check out [`IsolationStrategy`] to get more details.
	pub fn with_config(
		config: Config,
		metrics: Metrics,
		pvf_metrics: polkadot_node_core_pvf::Metrics,
	) -> Self {
		CandidateValidationSubsystem { config, metrics, pvf_metrics }
	}
}

#[overseer::subsystem(CandidateValidation, error=SubsystemError, prefix=self::overseer)]
impl<Context> CandidateValidationSubsystem {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = run(
			ctx,
			self.metrics,
			self.pvf_metrics,
			self.config.artifacts_cache_path,
			self.config.program_path,
		)
		.map_err(|e| SubsystemError::with_origin("candidate-validation", e))
		.boxed();
		SpawnedSubsystem { name: "candidate-validation-subsystem", future }
	}
}

#[overseer::contextbounds(CandidateValidation, prefix = self::overseer)]
async fn run<Context>(
	mut ctx: Context,
	metrics: Metrics,
	pvf_metrics: polkadot_node_core_pvf::Metrics,
	cache_path: PathBuf,
	program_path: PathBuf,
) -> SubsystemResult<()> {
	let (validation_host, task) = polkadot_node_core_pvf::start(
		polkadot_node_core_pvf::Config::new(cache_path, program_path),
		pvf_metrics,
	);
	ctx.spawn_blocking("pvf-validation-host", task.boxed())?;

	loop {
		match ctx.recv().await? {
			FromOrchestra::Signal(OverseerSignal::ActiveLeaves(_)) => {},
			FromOrchestra::Signal(OverseerSignal::BlockFinalized(..)) => {},
			FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOrchestra::Communication { msg } => match msg {
				CandidateValidationMessage::ValidateFromChainState(
					candidate_receipt,
					pov,
					timeout,
					response_sender,
				) => {
					let bg = {
						let mut sender = ctx.sender().clone();
						let metrics = metrics.clone();
						let validation_host = validation_host.clone();

						async move {
							let _timer = metrics.time_validate_from_chain_state();
							let res = validate_from_chain_state(
								&mut sender,
								validation_host,
								candidate_receipt,
								pov,
								timeout,
								&metrics,
							)
							.await;

							metrics.on_validation_event(&res);
							let _ = response_sender.send(res);
						}
					};

					ctx.spawn("validate-from-chain-state", bg.boxed())?;
				},
				CandidateValidationMessage::ValidateFromExhaustive(
					persisted_validation_data,
					validation_code,
					candidate_receipt,
					pov,
					timeout,
					response_sender,
				) => {
					let bg = {
						let mut sender = ctx.sender().clone();
						let metrics = metrics.clone();
						let validation_host = validation_host.clone();

						async move {
							let _timer = metrics.time_validate_from_exhaustive();
							let res = validate_candidate_exhaustive(
								&mut sender,
								validation_host,
								persisted_validation_data,
								validation_code,
								candidate_receipt,
								pov,
								timeout,
								&metrics,
							)
							.await;

							metrics.on_validation_event(&res);
							let _ = response_sender.send(res);
						}
					};

					ctx.spawn("validate-from-exhaustive", bg.boxed())?;
				},
				CandidateValidationMessage::PreCheck(
					relay_parent,
					validation_code_hash,
					response_sender,
				) => {
					let bg = {
						let mut sender = ctx.sender().clone();
						let validation_host = validation_host.clone();

						async move {
							let precheck_result = precheck_pvf(
								&mut sender,
								validation_host,
								relay_parent,
								validation_code_hash,
							)
							.await;

							let _ = response_sender.send(precheck_result);
						}
					};

					ctx.spawn("candidate-validation-pre-check", bg.boxed())?;
				},
			},
		}
	}
}

struct RuntimeRequestFailed;

async fn runtime_api_request<T, Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
	request: RuntimeApiRequest,
	receiver: oneshot::Receiver<Result<T, RuntimeApiError>>,
) -> Result<T, RuntimeRequestFailed>
where
	Sender: SubsystemSender<RuntimeApiMessage>,
{
	sender
		.send_message(RuntimeApiMessage::Request(relay_parent, request).into())
		.await;

	receiver
		.await
		.map_err(|_| {
			gum::debug!(target: LOG_TARGET, ?relay_parent, "Runtime API request dropped");

			RuntimeRequestFailed
		})
		.and_then(|res| {
			res.map_err(|e| {
				gum::debug!(
					target: LOG_TARGET,
					?relay_parent,
					err = ?e,
					"Runtime API request internal error"
				);

				RuntimeRequestFailed
			})
		})
}

async fn request_validation_code_by_hash<Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
	validation_code_hash: ValidationCodeHash,
) -> Result<Option<ValidationCode>, RuntimeRequestFailed>
where
	Sender: SubsystemSender<RuntimeApiMessage>,
{
	let (tx, rx) = oneshot::channel();
	runtime_api_request(
		sender,
		relay_parent,
		RuntimeApiRequest::ValidationCodeByHash(validation_code_hash, tx),
		rx,
	)
	.await
}

async fn precheck_pvf<Sender>(
	sender: &mut Sender,
	mut validation_backend: impl ValidationBackend,
	relay_parent: Hash,
	validation_code_hash: ValidationCodeHash,
) -> PreCheckOutcome
where
	Sender: SubsystemSender<RuntimeApiMessage>,
{
	let validation_code =
		match request_validation_code_by_hash(sender, relay_parent, validation_code_hash).await {
			Ok(Some(code)) => code,
			_ => {
				// The reasoning why this is "failed" and not invalid is because we assume that
				// during pre-checking voting the relay-chain will pin the code. In case the code
				// actually is not there, we issue failed since this looks more like a bug.
				gum::warn!(
					target: LOG_TARGET,
					?relay_parent,
					?validation_code_hash,
					"precheck: requested validation code is not found on-chain!",
				);
				return PreCheckOutcome::Failed
			},
		};

	let executor_params =
		if let Ok(executor_params) = executor_params_at_relay_parent(relay_parent, sender).await {
			gum::debug!(
				target: LOG_TARGET,
				?relay_parent,
				?validation_code_hash,
				"precheck: acquired executor params for the session: {:?}",
				executor_params,
			);
			executor_params
		} else {
			gum::warn!(
				target: LOG_TARGET,
				?relay_parent,
				?validation_code_hash,
				"precheck: failed to acquire executor params for the session, thus voting against.",
			);
			return PreCheckOutcome::Invalid
		};

	let timeout = pvf_prep_timeout(&executor_params, PvfPrepTimeoutKind::Precheck);

	let pvf = match sp_maybe_compressed_blob::decompress(
		&validation_code.0,
		VALIDATION_CODE_BOMB_LIMIT,
	) {
		Ok(code) => PvfPrepData::from_code(code.into_owned(), executor_params, timeout),
		Err(e) => {
			gum::debug!(target: LOG_TARGET, err=?e, "precheck: cannot decompress validation code");
			return PreCheckOutcome::Invalid
		},
	};

	match validation_backend.precheck_pvf(pvf).await {
		Ok(_) => PreCheckOutcome::Valid,
		Err(prepare_err) =>
			if prepare_err.is_deterministic() {
				PreCheckOutcome::Invalid
			} else {
				PreCheckOutcome::Failed
			},
	}
}

#[derive(Debug)]
enum AssumptionCheckOutcome {
	Matches(PersistedValidationData, ValidationCode),
	DoesNotMatch,
	BadRequest,
}

async fn check_assumption_validation_data<Sender>(
	sender: &mut Sender,
	descriptor: &CandidateDescriptor,
	assumption: OccupiedCoreAssumption,
) -> AssumptionCheckOutcome
where
	Sender: SubsystemSender<RuntimeApiMessage>,
{
	let validation_data = {
		let (tx, rx) = oneshot::channel();
		let d = runtime_api_request(
			sender,
			descriptor.relay_parent,
			RuntimeApiRequest::PersistedValidationData(descriptor.para_id, assumption, tx),
			rx,
		)
		.await;

		match d {
			Ok(None) | Err(RuntimeRequestFailed) => return AssumptionCheckOutcome::BadRequest,
			Ok(Some(d)) => d,
		}
	};

	let persisted_validation_data_hash = validation_data.hash();

	if descriptor.persisted_validation_data_hash == persisted_validation_data_hash {
		let (code_tx, code_rx) = oneshot::channel();
		let validation_code = runtime_api_request(
			sender,
			descriptor.relay_parent,
			RuntimeApiRequest::ValidationCode(descriptor.para_id, assumption, code_tx),
			code_rx,
		)
		.await;

		match validation_code {
			Ok(None) | Err(RuntimeRequestFailed) => AssumptionCheckOutcome::BadRequest,
			Ok(Some(v)) => AssumptionCheckOutcome::Matches(validation_data, v),
		}
	} else {
		AssumptionCheckOutcome::DoesNotMatch
	}
}

async fn find_assumed_validation_data<Sender>(
	sender: &mut Sender,
	descriptor: &CandidateDescriptor,
) -> AssumptionCheckOutcome
where
	Sender: SubsystemSender<RuntimeApiMessage>,
{
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
		let outcome = check_assumption_validation_data(sender, descriptor, *assumption).await;

		match outcome {
			AssumptionCheckOutcome::Matches(_, _) => return outcome,
			AssumptionCheckOutcome::BadRequest => return outcome,
			AssumptionCheckOutcome::DoesNotMatch => continue,
		}
	}

	AssumptionCheckOutcome::DoesNotMatch
}

/// Returns validation data for a given candidate.
pub async fn find_validation_data<Sender>(
	sender: &mut Sender,
	descriptor: &CandidateDescriptor,
) -> Result<Option<(PersistedValidationData, ValidationCode)>, ValidationFailed>
where
	Sender: SubsystemSender<RuntimeApiMessage>,
{
	match find_assumed_validation_data(sender, &descriptor).await {
		AssumptionCheckOutcome::Matches(validation_data, validation_code) =>
			Ok(Some((validation_data, validation_code))),
		AssumptionCheckOutcome::DoesNotMatch => {
			// If neither the assumption of the occupied core having the para included or the assumption
			// of the occupied core timing out are valid, then the persisted_validation_data_hash in the descriptor
			// is not based on the relay parent and is thus invalid.
			Ok(None)
		},
		AssumptionCheckOutcome::BadRequest =>
			Err(ValidationFailed("Assumption Check: Bad request".into())),
	}
}

async fn validate_from_chain_state<Sender>(
	sender: &mut Sender,
	validation_host: ValidationHost,
	candidate_receipt: CandidateReceipt,
	pov: Arc<PoV>,
	exec_timeout_kind: PvfExecTimeoutKind,
	metrics: &Metrics,
) -> Result<ValidationResult, ValidationFailed>
where
	Sender: SubsystemSender<RuntimeApiMessage>,
{
	let mut new_sender = sender.clone();
	let (validation_data, validation_code) =
		match find_validation_data(&mut new_sender, &candidate_receipt.descriptor).await? {
			Some((validation_data, validation_code)) => (validation_data, validation_code),
			None => return Ok(ValidationResult::Invalid(InvalidCandidate::BadParent)),
		};

	let validation_result = validate_candidate_exhaustive(
		sender,
		validation_host,
		validation_data,
		validation_code,
		candidate_receipt.clone(),
		pov,
		exec_timeout_kind,
		metrics,
	)
	.await;

	if let Ok(ValidationResult::Valid(ref outputs, _)) = validation_result {
		let (tx, rx) = oneshot::channel();
		match runtime_api_request(
			sender,
			candidate_receipt.descriptor.relay_parent,
			RuntimeApiRequest::CheckValidationOutputs(
				candidate_receipt.descriptor.para_id,
				outputs.clone(),
				tx,
			),
			rx,
		)
		.await
		{
			Ok(true) => {},
			Ok(false) => return Ok(ValidationResult::Invalid(InvalidCandidate::InvalidOutputs)),
			Err(RuntimeRequestFailed) =>
				return Err(ValidationFailed("Check Validation Outputs: Bad request".into())),
		}
	}

	validation_result
}

async fn validate_candidate_exhaustive<Sender>(
	sender: &mut Sender,
	mut validation_backend: impl ValidationBackend + Send,
	persisted_validation_data: PersistedValidationData,
	validation_code: ValidationCode,
	candidate_receipt: CandidateReceipt,
	pov: Arc<PoV>,
	exec_timeout_kind: PvfExecTimeoutKind,
	metrics: &Metrics,
) -> Result<ValidationResult, ValidationFailed>
where
	Sender: SubsystemSender<RuntimeApiMessage>,
{
	let _timer = metrics.time_validate_candidate_exhaustive();

	let validation_code_hash = validation_code.hash();
	let para_id = candidate_receipt.descriptor.para_id;
	gum::debug!(
		target: LOG_TARGET,
		?validation_code_hash,
		?para_id,
		"About to validate a candidate.",
	);

	if let Err(e) = perform_basic_checks(
		&candidate_receipt.descriptor,
		persisted_validation_data.max_pov_size,
		&pov,
		&validation_code_hash,
	) {
		gum::info!(target: LOG_TARGET, ?para_id, "Invalid candidate (basic checks)");
		return Ok(ValidationResult::Invalid(e))
	}

	let raw_validation_code = match sp_maybe_compressed_blob::decompress(
		&validation_code.0,
		VALIDATION_CODE_BOMB_LIMIT,
	) {
		Ok(code) => code,
		Err(e) => {
			gum::info!(target: LOG_TARGET, ?para_id, err=?e, "Invalid candidate (validation code)");

			// Code already passed pre-checking, if decompression fails now this most likley means
			// some local corruption happened.
			return Err(ValidationFailed("Code decompression failed".to_string()))
		},
	};
	metrics.observe_code_size(raw_validation_code.len());

	let raw_block_data =
		match sp_maybe_compressed_blob::decompress(&pov.block_data.0, POV_BOMB_LIMIT) {
			Ok(block_data) => BlockData(block_data.to_vec()),
			Err(e) => {
				gum::info!(target: LOG_TARGET, ?para_id, err=?e, "Invalid candidate (PoV code)");

				// If the PoV is invalid, the candidate certainly is.
				return Ok(ValidationResult::Invalid(InvalidCandidate::PoVDecompressionFailure))
			},
		};
	metrics.observe_pov_size(raw_block_data.0.len());

	let params = ValidationParams {
		parent_head: persisted_validation_data.parent_head.clone(),
		block_data: raw_block_data,
		relay_parent_number: persisted_validation_data.relay_parent_number,
		relay_parent_storage_root: persisted_validation_data.relay_parent_storage_root,
	};

	let executor_params = if let Ok(executor_params) =
		executor_params_at_relay_parent(candidate_receipt.descriptor.relay_parent, sender).await
	{
		gum::debug!(
			target: LOG_TARGET,
			?validation_code_hash,
			?para_id,
			"Acquired executor params for the session: {:?}",
			executor_params,
		);
		executor_params
	} else {
		gum::warn!(
			target: LOG_TARGET,
			?validation_code_hash,
			?para_id,
			"Failed to acquire executor params for the session",
		);
		return Ok(ValidationResult::Invalid(InvalidCandidate::BadParent))
	};

	let result = validation_backend
		.validate_candidate_with_retry(
			raw_validation_code.to_vec(),
			pvf_exec_timeout(&executor_params, exec_timeout_kind),
			params,
			executor_params,
		)
		.await;

	if let Err(ref error) = result {
		gum::info!(target: LOG_TARGET, ?para_id, ?error, "Failed to validate candidate");
	}

	match result {
		Err(ValidationError::InternalError(e)) => Err(ValidationFailed(e)),
		Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::HardTimeout)) =>
			Ok(ValidationResult::Invalid(InvalidCandidate::Timeout)),
		Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::WorkerReportedError(e))) =>
			Ok(ValidationResult::Invalid(InvalidCandidate::ExecutionError(e))),
		Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::AmbiguousWorkerDeath)) =>
			Ok(ValidationResult::Invalid(InvalidCandidate::ExecutionError(
				"ambiguous worker death".to_string(),
			))),
		Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::PrepareError(e))) => {
			// In principle if preparation of the `WASM` fails, the current candidate can not be the
			// reason for that. So we can't say whether it is invalid or not. In addition, with
			// pre-checking enabled only valid runtimes should ever get enacted, so we can be
			// reasonably sure that this is some local problem on the current node. However, as this
			// particular error *seems* to indicate a deterministic error, we raise a warning.
			gum::warn!(
				target: LOG_TARGET,
				?para_id,
				?e,
				"Deterministic error occurred during preparation (should have been ruled out by pre-checking phase)",
			);
			Err(ValidationFailed(e))
		},
		Ok(res) =>
			if res.head_data.hash() != candidate_receipt.descriptor.para_head {
				gum::info!(target: LOG_TARGET, ?para_id, "Invalid candidate (para_head)");
				Ok(ValidationResult::Invalid(InvalidCandidate::ParaHeadHashMismatch))
			} else {
				let outputs = CandidateCommitments {
					head_data: res.head_data,
					upward_messages: res.upward_messages,
					horizontal_messages: res.horizontal_messages,
					new_validation_code: res.new_validation_code,
					processed_downward_messages: res.processed_downward_messages,
					hrmp_watermark: res.hrmp_watermark,
				};
				if candidate_receipt.commitments_hash != outputs.hash() {
					gum::info!(
						target: LOG_TARGET,
						?para_id,
						"Invalid candidate (commitments hash)"
					);

					// If validation produced a new set of commitments, we treat the candidate as invalid.
					Ok(ValidationResult::Invalid(InvalidCandidate::CommitmentsHashMismatch))
				} else {
					Ok(ValidationResult::Valid(outputs, persisted_validation_data))
				}
			},
	}
}

#[async_trait]
trait ValidationBackend {
	/// Tries executing a PVF a single time (no retries).
	async fn validate_candidate(
		&mut self,
		pvf: PvfPrepData,
		exec_timeout: Duration,
		encoded_params: Vec<u8>,
	) -> Result<WasmValidationResult, ValidationError>;

	/// Tries executing a PVF. Will retry once if an error is encountered that may have been
	/// transient.
	///
	/// NOTE: Should retry only on errors that are a result of execution itself, and not of
	/// preparation.
	async fn validate_candidate_with_retry(
		&mut self,
		raw_validation_code: Vec<u8>,
		exec_timeout: Duration,
		params: ValidationParams,
		executor_params: ExecutorParams,
	) -> Result<WasmValidationResult, ValidationError> {
		let prep_timeout = pvf_prep_timeout(&executor_params, PvfPrepTimeoutKind::Lenient);
		// Construct the PVF a single time, since it is an expensive operation. Cloning it is cheap.
		let pvf = PvfPrepData::from_code(raw_validation_code, executor_params, prep_timeout);

		let mut validation_result =
			self.validate_candidate(pvf.clone(), exec_timeout, params.encode()).await;

		// Allow limited retries for each kind of error.
		let mut num_internal_retries_left = 1;
		let mut num_awd_retries_left = 1;
		loop {
			match validation_result {
				Err(ValidationError::InvalidCandidate(
					WasmInvalidCandidate::AmbiguousWorkerDeath,
				)) if num_awd_retries_left > 0 => num_awd_retries_left -= 1,
				Err(ValidationError::InternalError(_)) if num_internal_retries_left > 0 =>
					num_internal_retries_left -= 1,
				_ => break,
			}

			// If we got a possibly transient error, retry once after a brief delay, on the assumption
			// that the conditions that caused this error may have resolved on their own.
			{
				// Wait a brief delay before retrying.
				futures_timer::Delay::new(PVF_EXECUTION_RETRY_DELAY).await;

				gum::warn!(
					target: LOG_TARGET,
					?pvf,
					"Re-trying failed candidate validation due to possible transient error: {:?}",
					validation_result
				);

				// Encode the params again when re-trying. We expect the retry case to be relatively
				// rare, and we want to avoid unconditionally cloning data.
				validation_result =
					self.validate_candidate(pvf.clone(), exec_timeout, params.encode()).await;
			}
		}

		validation_result
	}

	async fn precheck_pvf(&mut self, pvf: PvfPrepData) -> Result<PrepareStats, PrepareError>;
}

#[async_trait]
impl ValidationBackend for ValidationHost {
	/// Tries executing a PVF a single time (no retries).
	async fn validate_candidate(
		&mut self,
		pvf: PvfPrepData,
		exec_timeout: Duration,
		encoded_params: Vec<u8>,
	) -> Result<WasmValidationResult, ValidationError> {
		let priority = polkadot_node_core_pvf::Priority::Normal;

		let (tx, rx) = oneshot::channel();
		if let Err(err) = self.execute_pvf(pvf, exec_timeout, encoded_params, priority, tx).await {
			return Err(ValidationError::InternalError(format!(
				"cannot send pvf to the validation host: {:?}",
				err
			)))
		}

		rx.await
			.map_err(|_| ValidationError::InternalError("validation was cancelled".into()))?
	}

	async fn precheck_pvf(&mut self, pvf: PvfPrepData) -> Result<PrepareStats, PrepareError> {
		let (tx, rx) = oneshot::channel();
		if let Err(err) = self.precheck_pvf(pvf, tx).await {
			// Return an IO error if there was an error communicating with the host.
			return Err(PrepareError::IoErr(err))
		}

		let precheck_result = rx.await.map_err(|err| PrepareError::IoErr(err.to_string()))?;

		precheck_result
	}
}

/// Does basic checks of a candidate. Provide the encoded PoV-block. Returns `Ok` if basic checks
/// are passed, `Err` otherwise.
fn perform_basic_checks(
	candidate: &CandidateDescriptor,
	max_pov_size: u32,
	pov: &PoV,
	validation_code_hash: &ValidationCodeHash,
) -> Result<(), InvalidCandidate> {
	let pov_hash = pov.hash();

	let encoded_pov_size = pov.encoded_size();
	if encoded_pov_size > max_pov_size as usize {
		return Err(InvalidCandidate::ParamsTooLarge(encoded_pov_size as u64))
	}

	if pov_hash != candidate.pov_hash {
		return Err(InvalidCandidate::PoVHashMismatch)
	}

	if *validation_code_hash != candidate.validation_code_hash {
		return Err(InvalidCandidate::CodeHashMismatch)
	}

	if let Err(()) = candidate.check_collator_signature() {
		return Err(InvalidCandidate::BadSignature)
	}

	Ok(())
}

fn pvf_prep_timeout(executor_params: &ExecutorParams, kind: PvfPrepTimeoutKind) -> Duration {
	if let Some(timeout) = executor_params.pvf_prep_timeout(kind) {
		return timeout
	}
	match kind {
		PvfPrepTimeoutKind::Precheck => DEFAULT_PRECHECK_PREPARATION_TIMEOUT,
		PvfPrepTimeoutKind::Lenient => DEFAULT_LENIENT_PREPARATION_TIMEOUT,
	}
}

fn pvf_exec_timeout(executor_params: &ExecutorParams, kind: PvfExecTimeoutKind) -> Duration {
	if let Some(timeout) = executor_params.pvf_exec_timeout(kind) {
		return timeout
	}
	match kind {
		PvfExecTimeoutKind::Backing => DEFAULT_BACKING_EXECUTION_TIMEOUT,
		PvfExecTimeoutKind::Approval => DEFAULT_APPROVAL_EXECUTION_TIMEOUT,
	}
}
