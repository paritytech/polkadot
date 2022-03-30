// Copyright 2020-2021 Parity Technologies (UK) Ltd.
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
	InvalidCandidate as WasmInvalidCandidate, PrepareError, PvfDescriptor, PvfPreimage,
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
	overseer, FromOverseer, OverseerSignal, SpawnedSubsystem, SubsystemContext, SubsystemError,
	SubsystemResult, SubsystemSender,
};
use polkadot_node_subsystem_util::metrics::{self, prometheus};
use polkadot_parachain::primitives::{ValidationParams, ValidationResult as WasmValidationResult};
use polkadot_primitives::v2::{
	BlockNumber, CandidateCommitments, CandidateDescriptor, Hash, PersistedValidationData,
	ValidationCode, ValidationCodeHash,
};

use parity_scale_codec::Encode;

use futures::{channel::oneshot, prelude::*};

use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;

#[cfg(test)]
mod tests;

const LOG_TARGET: &'static str = "parachain::candidate-validation";

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

impl<Context> overseer::Subsystem<Context, SubsystemError> for CandidateValidationSubsystem
where
	Context: SubsystemContext<Message = CandidateValidationMessage>,
	Context: overseer::SubsystemContext<Message = CandidateValidationMessage>,
{
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

async fn run<Context>(
	mut ctx: Context,
	metrics: Metrics,
	pvf_metrics: polkadot_node_core_pvf::Metrics,
	cache_path: PathBuf,
	program_path: PathBuf,
) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = CandidateValidationMessage>,
	Context: overseer::SubsystemContext<Message = CandidateValidationMessage>,
{
	let (validation_host, task) = polkadot_node_core_pvf::start(
		polkadot_node_core_pvf::Config::new(cache_path, program_path),
		pvf_metrics,
	);
	ctx.spawn_blocking("pvf-validation-host", task.boxed())?;

	loop {
		match ctx.recv().await? {
			FromOverseer::Signal(OverseerSignal::ActiveLeaves(_)) => {},
			FromOverseer::Signal(OverseerSignal::BlockFinalized(..)) => {},
			FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOverseer::Communication { msg } => match msg {
				CandidateValidationMessage::ValidateFromChainState(
					descriptor,
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
								descriptor,
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
					descriptor,
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
								Some(validation_code),
								descriptor,
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

#[derive(Debug)]
struct RuntimeRequestFailed;

async fn runtime_api_request<T, Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
	request: RuntimeApiRequest,
	receiver: oneshot::Receiver<Result<T, RuntimeApiError>>,
) -> Result<T, RuntimeRequestFailed>
where
	Sender: SubsystemSender,
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
	Sender: SubsystemSender,
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

async fn request_assumed_validation_data<Sender>(
	sender: &mut Sender,
	descriptor: &CandidateDescriptor,
) -> Result<
	Option<(PersistedValidationData<Hash, BlockNumber>, ValidationCodeHash)>,
	RuntimeRequestFailed,
>
where
	Sender: SubsystemSender,
{
	let (tx, rx) = oneshot::channel();
	runtime_api_request(
		sender,
		descriptor.relay_parent,
		RuntimeApiRequest::AssumedValidationData(
			descriptor.para_id,
			descriptor.persisted_validation_data_hash,
			tx,
		),
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
	Sender: SubsystemSender,
{
	// Even though the PVF host is capable of looking for code by its hash, we still
	// request the validation code from the runtime right away. The reasoning is that
	// the host is only expected to have the PVF cached in case there was a transient error
	// that allows precheck retrying, otherwise the Runtime API request is necessary.
	let validation_code =
		match request_validation_code_by_hash(sender, relay_parent, validation_code_hash).await {
			Ok(Some(code)) => code,
			_ => {
				// The reasoning why this is "failed" and not invalid is because we assume that
				// during pre-checking voting the relay-chain will pin the code. In case the code
				// actually is not there, we issue failed since this looks more like a bug. This
				// leads to us abstaining.
				gum::warn!(
					target: LOG_TARGET,
					?relay_parent,
					?validation_code_hash,
					"precheck: requested validation code is not found on-chain!",
				);
				return PreCheckOutcome::Failed
			},
		};

	let validation_code = match sp_maybe_compressed_blob::decompress(
		&validation_code.0,
		VALIDATION_CODE_BOMB_LIMIT,
	) {
		Ok(code) => PvfPreimage::from_code(code.into_owned()),
		Err(e) => {
			gum::debug!(target: LOG_TARGET, err=?e, "precheck: cannot decompress validation code");
			return PreCheckOutcome::Invalid
		},
	};

	match validation_backend.precheck_pvf(validation_code).await {
		Ok(_) => PreCheckOutcome::Valid,
		Err(prepare_err) => match prepare_err {
			PrepareError::Prevalidation(_) |
			PrepareError::Preparation(_) |
			PrepareError::Panic(_) => PreCheckOutcome::Invalid,
			PrepareError::TimedOut | PrepareError::DidNotMakeIt => PreCheckOutcome::Failed,
		},
	}
}

async fn validate_from_chain_state<Sender>(
	sender: &mut Sender,
	validation_host: impl ValidationBackend,
	descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
	timeout: Duration,
	metrics: &Metrics,
) -> Result<ValidationResult, ValidationFailed>
where
	Sender: SubsystemSender,
{
	let (validation_data, validation_code_hash) =
		match request_assumed_validation_data(sender, &descriptor).await {
			Ok(Some((data, code_hash))) => (data, code_hash),
			Ok(None) => {
				// If neither the assumption of the occupied core having the para included or the assumption
				// of the occupied core timing out are valid, then the persisted_validation_data_hash in the descriptor
				// is not based on the relay parent and is thus invalid.
				return Ok(ValidationResult::Invalid(InvalidCandidate::BadParent))
			},
			Err(_) =>
				return Err(ValidationFailed(
					"Failed to request persisted validation data from the Runtime API".into(),
				)),
		};

	if descriptor.validation_code_hash != validation_code_hash {
		return Ok(ValidationResult::Invalid(InvalidCandidate::CodeHashMismatch))
	}

	let validation_result = validate_candidate_exhaustive(
		sender,
		validation_host,
		validation_data,
		None,
		descriptor.clone(),
		pov,
		timeout,
		metrics,
	)
	.await;

	if let Ok(ValidationResult::Valid(ref outputs, _)) = validation_result {
		let (tx, rx) = oneshot::channel();
		match runtime_api_request(
			sender,
			descriptor.relay_parent,
			RuntimeApiRequest::CheckValidationOutputs(descriptor.para_id, outputs.clone(), tx),
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
	mut validation_backend: impl ValidationBackend,
	persisted_validation_data: PersistedValidationData,
	validation_code: Option<ValidationCode>,
	descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
	timeout: Duration,
	metrics: &Metrics,
) -> Result<ValidationResult, ValidationFailed>
where
	Sender: SubsystemSender,
{
	let _timer = metrics.time_validate_candidate_exhaustive();

	let validation_code_hash = validation_code.as_ref().map(ValidationCode::hash);
	gum::debug!(
		target: LOG_TARGET,
		?validation_code_hash,
		para_id = ?descriptor.para_id,
		"About to validate a candidate.",
	);

	if let Err(e) = perform_basic_checks(
		&descriptor,
		persisted_validation_data.max_pov_size,
		&*pov,
		validation_code_hash.as_ref(),
	) {
		return Ok(ValidationResult::Invalid(e))
	}

	let validation_code = if let Some(validation_code) = validation_code {
		let raw_code = match sp_maybe_compressed_blob::decompress(
			&validation_code.0,
			VALIDATION_CODE_BOMB_LIMIT,
		) {
			Ok(code) => code,
			Err(e) => {
				gum::debug!(target: LOG_TARGET, err=?e, "Code decompression failed");

				// If the validation code is invalid, the candidate certainly is.
				return Ok(ValidationResult::Invalid(InvalidCandidate::CodeDecompressionFailure))
			},
		}
		.to_vec();
		PvfDescriptor::from_code(raw_code)
	} else {
		// In case validation code is not provided, ask the backend to obtain
		// it from the cache using the hash.
		PvfDescriptor::Hash(ValidationCodeHash::from(descriptor.validation_code_hash))
	};

	let raw_block_data =
		match sp_maybe_compressed_blob::decompress(&pov.block_data.0, POV_BOMB_LIMIT) {
			Ok(block_data) => BlockData(block_data.to_vec()),
			Err(e) => {
				gum::debug!(target: LOG_TARGET, err=?e, "Invalid PoV code");

				// If the PoV is invalid, the candidate certainly is.
				return Ok(ValidationResult::Invalid(InvalidCandidate::PoVDecompressionFailure))
			},
		};

	let params = ValidationParams {
		parent_head: persisted_validation_data.parent_head.clone(),
		block_data: raw_block_data,
		relay_parent_number: persisted_validation_data.relay_parent_number,
		relay_parent_storage_root: persisted_validation_data.relay_parent_storage_root,
	};

	let result = match validation_backend
		.validate_candidate(validation_code, timeout, params.clone())
		.await
	{
		Err(ValidationError::ArtifactNotFound) => {
			// In case preimage for the supplied code hash was not found by the
			// validation host, request the code from Runtime API and try again.
			tracing::debug!(
				target: LOG_TARGET,
				validation_code_hash = ?descriptor.validation_code_hash,
				"Validation host failed to find artifact by provided hash",
			);

			let validation_code = match request_validation_code_by_hash(
				sender,
				descriptor.relay_parent,
				descriptor.validation_code_hash,
			)
			.await
			{
				Ok(Some(validation_code)) => validation_code,
				Ok(None) => {
					// This path can only happen when validating candidate from chain state.
					// Earlier we obtained the PVF hash from the runtime and verified
					// that it matches with the one in candidate descriptor.
					// As a result, receiving `None` is unexpected and should hint on a major
					// bug in Runtime API.
					return Err(ValidationFailed(
						"Runtime API returned `None` for the validation \
						code with a known hash"
							.into(),
					))
				},
				Err(_) => return Err(ValidationFailed("Runtime API request failed".into())),
			};

			let raw_code = match sp_maybe_compressed_blob::decompress(
				&validation_code.0,
				VALIDATION_CODE_BOMB_LIMIT,
			) {
				Ok(code) => code,
				Err(e) => {
					tracing::debug!(target: LOG_TARGET, err=?e, "Code decompression failed");

					// If the validation code is invalid, the candidate certainly is.
					return Ok(ValidationResult::Invalid(InvalidCandidate::CodeDecompressionFailure))
				},
			};
			let validation_code = PvfDescriptor::from_code(raw_code.to_vec());
			validation_backend.validate_candidate(validation_code, timeout, params).await
		},
		result => result,
	};

	if let Err(ref e) = result {
		gum::debug!(
			target: LOG_TARGET,
			error = ?e,
			"Failed to validate candidate",
		);
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
		Err(ValidationError::ArtifactNotFound) => {
			// The code was supplied on the second attempt, this
			// error should be unreachable.
			let err = ValidationFailed(
				"Validation host failed to find artifact even though it was supplied".to_string(),
			);
			tracing::error!(target: LOG_TARGET, error = ?err);
			Err(err)
		},
		Err(ValidationError::InvalidCandidate(WasmInvalidCandidate::PrepareError(e))) =>
			Ok(ValidationResult::Invalid(InvalidCandidate::ExecutionError(e))),
		Ok(res) =>
			if res.head_data.hash() != descriptor.para_head {
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
				Ok(ValidationResult::Valid(outputs, persisted_validation_data))
			},
	}
}

#[async_trait]
trait ValidationBackend {
	async fn validate_candidate(
		&mut self,
		validation_code: PvfDescriptor,
		timeout: Duration,
		params: ValidationParams,
	) -> Result<WasmValidationResult, ValidationError>;

	async fn precheck_pvf(&mut self, pvf: PvfPreimage) -> Result<(), PrepareError>;
}

#[async_trait]
impl ValidationBackend for ValidationHost {
	async fn validate_candidate(
		&mut self,
		validation_code: PvfDescriptor,
		timeout: Duration,
		params: ValidationParams,
	) -> Result<WasmValidationResult, ValidationError> {
		let (tx, rx) = oneshot::channel();
		if let Err(err) = self
			.execute_pvf(
				validation_code,
				timeout,
				params.encode(),
				polkadot_node_core_pvf::Priority::Normal,
				tx,
			)
			.await
		{
			return Err(ValidationError::InternalError(format!(
				"cannot send pvf to the validation host: {:?}",
				err
			)))
		}

		let validation_result = rx
			.await
			.map_err(|_| ValidationError::InternalError("validation was cancelled".into()))?;

		validation_result
	}

	async fn precheck_pvf(&mut self, pvf: PvfPreimage) -> Result<(), PrepareError> {
		let (tx, rx) = oneshot::channel();
		if let Err(_) = self.precheck_pvf(pvf, tx).await {
			return Err(PrepareError::DidNotMakeIt)
		}

		let precheck_result = rx.await.or(Err(PrepareError::DidNotMakeIt))?;

		precheck_result
	}
}

/// Does basic checks of a candidate. Provide the encoded PoV-block. Returns `Ok` if basic checks
/// are passed, `Err` otherwise.
fn perform_basic_checks(
	candidate: &CandidateDescriptor,
	max_pov_size: u32,
	pov: &PoV,
	validation_code_hash: Option<&ValidationCodeHash>,
) -> Result<(), InvalidCandidate> {
	let pov_hash = pov.hash();

	let encoded_pov_size = pov.encoded_size();
	if encoded_pov_size > max_pov_size as usize {
		return Err(InvalidCandidate::ParamsTooLarge(encoded_pov_size as u64))
	}

	if pov_hash != candidate.pov_hash {
		return Err(InvalidCandidate::PoVHashMismatch)
	}

	if let Some(false) = validation_code_hash.map(|hash| *hash == candidate.validation_code_hash) {
		return Err(InvalidCandidate::CodeHashMismatch)
	}

	if let Err(()) = candidate.check_collator_signature() {
		return Err(InvalidCandidate::BadSignature)
	}

	Ok(())
}

#[derive(Clone)]
struct MetricsInner {
	validation_requests: prometheus::CounterVec<prometheus::U64>,
	validate_from_chain_state: prometheus::Histogram,
	validate_from_exhaustive: prometheus::Histogram,
	validate_candidate_exhaustive: prometheus::Histogram,
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

	/// Provide a timer for `validate_from_chain_state` which observes on drop.
	fn time_validate_from_chain_state(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.validate_from_chain_state.start_timer())
	}

	/// Provide a timer for `validate_from_exhaustive` which observes on drop.
	fn time_validate_from_exhaustive(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.validate_from_exhaustive.start_timer())
	}

	/// Provide a timer for `validate_candidate_exhaustive` which observes on drop.
	fn time_validate_candidate_exhaustive(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0
			.as_ref()
			.map(|metrics| metrics.validate_candidate_exhaustive.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			validation_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_validation_requests_total",
						"Number of validation requests served.",
					),
					&["validity"],
				)?,
				registry,
			)?,
			validate_from_chain_state: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_candidate_validation_validate_from_chain_state",
					"Time spent within `candidate_validation::validate_from_chain_state`",
				))?,
				registry,
			)?,
			validate_from_exhaustive: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_candidate_validation_validate_from_exhaustive",
					"Time spent within `candidate_validation::validate_from_exhaustive`",
				))?,
				registry,
			)?,
			validate_candidate_exhaustive: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"polkadot_parachain_candidate_validation_validate_candidate_exhaustive",
					"Time spent within `candidate_validation::validate_candidate_exhaustive`",
				))?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
