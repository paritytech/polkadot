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

//! Validation host - is the primary interface for this crate. It allows the clients to enqueue
//! jobs for PVF execution or preparation.
//!
//! The validation host is represented by a future/task that runs an event-loop and by a handle,
//! [`ValidationHost`], that allows communication with that event-loop.

use crate::{
	artifacts::{ArtifactId, ArtifactPathId, ArtifactState, Artifacts},
	error::PrepareError,
	execute,
	metrics::Metrics,
	prepare, PrepareResult, Priority, Pvf, ValidationError, LOG_TARGET,
};
use always_assert::never;
use async_std::path::{Path, PathBuf};
use futures::{
	channel::{mpsc, oneshot},
	Future, FutureExt, SinkExt, StreamExt,
};
use polkadot_parachain::primitives::ValidationResult;
use std::{
	collections::HashMap,
	time::{Duration, SystemTime},
};

/// For prechecking requests, the time period after which the preparation worker is considered
/// unresponsive and will be killed.
// NOTE: If you change this make sure to fix the buckets of `pvf_preparation_time` metric.
pub const PRECHECK_PREPARATION_TIMEOUT: Duration = Duration::from_secs(60);

/// For execution and heads-up requests, the time period after which the preparation worker is
/// considered unresponsive and will be killed. More lenient than the timeout for prechecking to
/// prevent honest validators from timing out on valid PVFs.
// NOTE: If you change this make sure to fix the buckets of `pvf_preparation_time` metric.
pub const LENIENT_PREPARATION_TIMEOUT: Duration = Duration::from_secs(360);

/// The time period after which a failed preparation artifact is considered ready to be retried.
/// Note that we will only retry if another request comes in after this cooldown has passed.
#[cfg(not(test))]
pub const PREPARE_FAILURE_COOLDOWN: Duration = Duration::from_secs(15 * 60);
#[cfg(test)]
pub const PREPARE_FAILURE_COOLDOWN: Duration = Duration::from_millis(200);

/// The amount of times we will retry failed prepare jobs.
pub const NUM_PREPARE_RETRIES: u32 = 5;

/// An alias to not spell the type for the oneshot sender for the PVF execution result.
pub(crate) type ResultSender = oneshot::Sender<Result<ValidationResult, ValidationError>>;

/// Transmission end used for sending the PVF preparation result.
pub(crate) type PrepareResultSender = oneshot::Sender<PrepareResult>;

/// A handle to the async process serving the validation host requests.
#[derive(Clone)]
pub struct ValidationHost {
	to_host_tx: mpsc::Sender<ToHost>,
}

impl ValidationHost {
	/// Precheck PVF with the given code, i.e. verify that it compiles within a reasonable time
	/// limit. This will prepare the PVF. The result of preparation will be sent to the provided
	/// result sender.
	///
	/// This is async to accommodate the possibility of back-pressure. In the vast majority of
	/// situations this function should return immediately.
	///
	/// Returns an error if the request cannot be sent to the validation host, i.e. if it shut down.
	pub async fn precheck_pvf(
		&mut self,
		pvf: Pvf,
		result_tx: PrepareResultSender,
	) -> Result<(), String> {
		self.to_host_tx
			.send(ToHost::PrecheckPvf { pvf, result_tx })
			.await
			.map_err(|_| "the inner loop hung up".to_string())
	}

	/// Execute PVF with the given code, execution timeout, parameters and priority.
	/// The result of execution will be sent to the provided result sender.
	///
	/// This is async to accommodate the possibility of back-pressure. In the vast majority of
	/// situations this function should return immediately.
	///
	/// Returns an error if the request cannot be sent to the validation host, i.e. if it shut down.
	pub async fn execute_pvf(
		&mut self,
		pvf: Pvf,
		execution_timeout: Duration,
		params: Vec<u8>,
		priority: Priority,
		result_tx: ResultSender,
	) -> Result<(), String> {
		self.to_host_tx
			.send(ToHost::ExecutePvf(ExecutePvfInputs {
				pvf,
				execution_timeout,
				params,
				priority,
				result_tx,
			}))
			.await
			.map_err(|_| "the inner loop hung up".to_string())
	}

	/// Sends a signal to the validation host requesting to prepare a list of the given PVFs.
	///
	/// This is async to accommodate the possibility of back-pressure. In the vast majority of
	/// situations this function should return immediately.
	///
	/// Returns an error if the request cannot be sent to the validation host, i.e. if it shut down.
	pub async fn heads_up(&mut self, active_pvfs: Vec<Pvf>) -> Result<(), String> {
		self.to_host_tx
			.send(ToHost::HeadsUp { active_pvfs })
			.await
			.map_err(|_| "the inner loop hung up".to_string())
	}
}

enum ToHost {
	PrecheckPvf { pvf: Pvf, result_tx: PrepareResultSender },
	ExecutePvf(ExecutePvfInputs),
	HeadsUp { active_pvfs: Vec<Pvf> },
}

struct ExecutePvfInputs {
	pvf: Pvf,
	execution_timeout: Duration,
	params: Vec<u8>,
	priority: Priority,
	result_tx: ResultSender,
}

/// Configuration for the validation host.
pub struct Config {
	/// The root directory where the prepared artifacts can be stored.
	pub cache_path: PathBuf,
	/// The path to the program that can be used to spawn the prepare workers.
	pub prepare_worker_program_path: PathBuf,
	/// The time allotted for a prepare worker to spawn and report to the host.
	pub prepare_worker_spawn_timeout: Duration,
	/// The maximum number of workers that can be spawned in the prepare pool for tasks with the
	/// priority below critical.
	pub prepare_workers_soft_max_num: usize,
	/// The absolute number of workers that can be spawned in the prepare pool.
	pub prepare_workers_hard_max_num: usize,
	/// The path to the program that can be used to spawn the execute workers.
	pub execute_worker_program_path: PathBuf,
	/// The time allotted for an execute worker to spawn and report to the host.
	pub execute_worker_spawn_timeout: Duration,
	/// The maximum number of execute workers that can run at the same time.
	pub execute_workers_max_num: usize,
}

impl Config {
	/// Create a new instance of the configuration.
	pub fn new(cache_path: std::path::PathBuf, program_path: std::path::PathBuf) -> Self {
		// Do not contaminate the other parts of the codebase with the types from `async_std`.
		let cache_path = PathBuf::from(cache_path);
		let program_path = PathBuf::from(program_path);

		Self {
			cache_path,
			prepare_worker_program_path: program_path.clone(),
			prepare_worker_spawn_timeout: Duration::from_secs(3),
			prepare_workers_soft_max_num: 1,
			prepare_workers_hard_max_num: 1,
			execute_worker_program_path: program_path,
			execute_worker_spawn_timeout: Duration::from_secs(3),
			execute_workers_max_num: 2,
		}
	}
}

/// Start the validation host.
///
/// Returns a [handle][`ValidationHost`] to the started validation host and the future. The future
/// must be polled in order for validation host to function.
///
/// The future should not return normally but if it does then that indicates an unrecoverable error.
/// In that case all pending requests will be canceled, dropping the result senders and new ones
/// will be rejected.
pub fn start(config: Config, metrics: Metrics) -> (ValidationHost, impl Future<Output = ()>) {
	let (to_host_tx, to_host_rx) = mpsc::channel(10);

	let validation_host = ValidationHost { to_host_tx };

	let (to_prepare_pool, from_prepare_pool, run_prepare_pool) = prepare::start_pool(
		metrics.clone(),
		config.prepare_worker_program_path.clone(),
		config.cache_path.clone(),
		config.prepare_worker_spawn_timeout,
	);

	let (to_prepare_queue_tx, from_prepare_queue_rx, run_prepare_queue) = prepare::start_queue(
		metrics.clone(),
		config.prepare_workers_soft_max_num,
		config.prepare_workers_hard_max_num,
		config.cache_path.clone(),
		to_prepare_pool,
		from_prepare_pool,
	);

	let (to_execute_queue_tx, run_execute_queue) = execute::start(
		metrics,
		config.execute_worker_program_path.to_owned(),
		config.execute_workers_max_num,
		config.execute_worker_spawn_timeout,
	);

	let (to_sweeper_tx, to_sweeper_rx) = mpsc::channel(100);
	let run_sweeper = sweeper_task(to_sweeper_rx);

	let run_host = async move {
		let artifacts = Artifacts::new(&config.cache_path).await;

		run(Inner {
			cache_path: config.cache_path,
			cleanup_pulse_interval: Duration::from_secs(3600),
			artifact_ttl: Duration::from_secs(3600 * 24),
			artifacts,
			to_host_rx,
			to_prepare_queue_tx,
			from_prepare_queue_rx,
			to_execute_queue_tx,
			to_sweeper_tx,
			awaiting_prepare: AwaitingPrepare::default(),
		})
		.await
	};

	let task = async move {
		// Bundle the sub-components' tasks together into a single future.
		futures::select! {
			_ = run_host.fuse() => {},
			_ = run_prepare_queue.fuse() => {},
			_ = run_prepare_pool.fuse() => {},
			_ = run_execute_queue.fuse() => {},
			_ = run_sweeper.fuse() => {},
		};
	};

	(validation_host, task)
}

/// An execution request that should execute the PVF (known in the context) and send the results
/// to the given result sender.
#[derive(Debug)]
struct PendingExecutionRequest {
	execution_timeout: Duration,
	params: Vec<u8>,
	result_tx: ResultSender,
}

/// A mapping from an artifact ID which is in preparation state to the list of pending execution
/// requests that should be executed once the artifact's preparation is finished.
#[derive(Default)]
struct AwaitingPrepare(HashMap<ArtifactId, Vec<PendingExecutionRequest>>);

impl AwaitingPrepare {
	fn add(
		&mut self,
		artifact_id: ArtifactId,
		execution_timeout: Duration,
		params: Vec<u8>,
		result_tx: ResultSender,
	) {
		self.0.entry(artifact_id).or_default().push(PendingExecutionRequest {
			execution_timeout,
			params,
			result_tx,
		});
	}

	fn take(&mut self, artifact_id: &ArtifactId) -> Vec<PendingExecutionRequest> {
		self.0.remove(artifact_id).unwrap_or_default()
	}
}

struct Inner {
	cache_path: PathBuf,
	cleanup_pulse_interval: Duration,
	artifact_ttl: Duration,
	artifacts: Artifacts,

	to_host_rx: mpsc::Receiver<ToHost>,

	to_prepare_queue_tx: mpsc::Sender<prepare::ToQueue>,
	from_prepare_queue_rx: mpsc::UnboundedReceiver<prepare::FromQueue>,

	to_execute_queue_tx: mpsc::Sender<execute::ToQueue>,
	to_sweeper_tx: mpsc::Sender<PathBuf>,

	awaiting_prepare: AwaitingPrepare,
}

#[derive(Debug)]
struct Fatal;

async fn run(
	Inner {
		cache_path,
		cleanup_pulse_interval,
		artifact_ttl,
		mut artifacts,
		to_host_rx,
		from_prepare_queue_rx,
		mut to_prepare_queue_tx,
		mut to_execute_queue_tx,
		mut to_sweeper_tx,
		mut awaiting_prepare,
	}: Inner,
) {
	macro_rules! break_if_fatal {
		($expr:expr) => {
			match $expr {
				Err(Fatal) => {
					gum::error!(
						target: LOG_TARGET,
						"Fatal error occurred, terminating the host. Line: {}",
						line!(),
					);
					break
				},
				Ok(v) => v,
			}
		};
	}

	let cleanup_pulse = pulse_every(cleanup_pulse_interval).fuse();
	futures::pin_mut!(cleanup_pulse);

	let mut to_host_rx = to_host_rx.fuse();
	let mut from_prepare_queue_rx = from_prepare_queue_rx.fuse();

	loop {
		// biased to make it behave deterministically for tests.
		futures::select_biased! {
			() = cleanup_pulse.select_next_some() => {
				// `select_next_some` because we don't expect this to fail, but if it does, we
				// still don't fail. The tradeoff is that the compiled cache will start growing
				// in size. That is, however, rather a slow process and hopefully the operator
				// will notice it.

				break_if_fatal!(handle_cleanup_pulse(
					&cache_path,
					&mut to_sweeper_tx,
					&mut artifacts,
					artifact_ttl,
				).await);
			},
			to_host = to_host_rx.next() => {
				let to_host = match to_host {
					None => {
						// The sending half of the channel has been closed, meaning the
						// `ValidationHost` struct was dropped. Shutting down gracefully.
						break;
					},
					Some(to_host) => to_host,
				};

				// If the artifact failed before, it could be re-scheduled for preparation here if
				// the preparation failure cooldown has elapsed.
				break_if_fatal!(handle_to_host(
					&cache_path,
					&mut artifacts,
					&mut to_prepare_queue_tx,
					&mut to_execute_queue_tx,
					&mut awaiting_prepare,
					to_host,
				)
				.await);
			},
			from_prepare_queue = from_prepare_queue_rx.next() => {
				let from_queue = break_if_fatal!(from_prepare_queue.ok_or(Fatal));

				// Note that the preparation outcome is always reported as concluded.
				//
				// That's because the error conditions are written into the artifact and will be
				// reported at the time of the execution. It potentially, but not necessarily, can
				// be scheduled for execution as a result of this function call, in case there are
				// pending executions.
				//
				// We could be eager in terms of reporting and plumb the result from the preparation
				// worker but we don't for the sake of simplicity.
				break_if_fatal!(handle_prepare_done(
					&cache_path,
					&mut artifacts,
					&mut to_execute_queue_tx,
					&mut awaiting_prepare,
					from_queue,
				).await);
			},
		}
	}
}

async fn handle_to_host(
	cache_path: &Path,
	artifacts: &mut Artifacts,
	prepare_queue: &mut mpsc::Sender<prepare::ToQueue>,
	execute_queue: &mut mpsc::Sender<execute::ToQueue>,
	awaiting_prepare: &mut AwaitingPrepare,
	to_host: ToHost,
) -> Result<(), Fatal> {
	match to_host {
		ToHost::PrecheckPvf { pvf, result_tx } => {
			handle_precheck_pvf(artifacts, prepare_queue, pvf, result_tx).await?;
		},
		ToHost::ExecutePvf(inputs) => {
			handle_execute_pvf(
				cache_path,
				artifacts,
				prepare_queue,
				execute_queue,
				awaiting_prepare,
				inputs,
			)
			.await?;
		},
		ToHost::HeadsUp { active_pvfs } =>
			handle_heads_up(artifacts, prepare_queue, active_pvfs).await?,
	}

	Ok(())
}

/// Handles PVF prechecking requests.
///
/// This tries to prepare the PVF by compiling the WASM blob within a given timeout ([`PRECHECK_PREPARATION_TIMEOUT`]).
///
/// If the prepare job failed previously, we may retry it under certain conditions.
async fn handle_precheck_pvf(
	artifacts: &mut Artifacts,
	prepare_queue: &mut mpsc::Sender<prepare::ToQueue>,
	pvf: Pvf,
	result_sender: PrepareResultSender,
) -> Result<(), Fatal> {
	let artifact_id = pvf.as_artifact_id();

	if let Some(state) = artifacts.artifact_state_mut(&artifact_id) {
		match state {
			ArtifactState::Prepared { last_time_needed, cpu_time_elapsed } => {
				*last_time_needed = SystemTime::now();
				let _ = result_sender.send(Ok(*cpu_time_elapsed));
			},
			ArtifactState::Preparing { waiting_for_response, num_failures: _ } =>
				waiting_for_response.push(result_sender),
			ArtifactState::FailedToProcess { error, .. } => {
				// Do not retry failed preparation if another pre-check request comes in. We do not retry pre-checking,
				// anyway.
				let _ = result_sender.send(PrepareResult::Err(error.clone()));
			},
		}
	} else {
		artifacts.insert_preparing(artifact_id, vec![result_sender]);
		send_prepare(
			prepare_queue,
			prepare::ToQueue::Enqueue {
				priority: Priority::Normal,
				pvf,
				preparation_timeout: PRECHECK_PREPARATION_TIMEOUT,
			},
		)
		.await?;
	}
	Ok(())
}

/// Handles PVF execution.
///
/// This will try to prepare the PVF, if a prepared artifact does not already exist. If there is already a
/// preparation job, we coalesce the two preparation jobs.
///
/// If the prepare job failed previously, we may retry it under certain conditions.
///
/// When preparing for execution, we use a more lenient timeout ([`EXECUTE_PREPARATION_TIMEOUT`])
/// than when prechecking.
async fn handle_execute_pvf(
	cache_path: &Path,
	artifacts: &mut Artifacts,
	prepare_queue: &mut mpsc::Sender<prepare::ToQueue>,
	execute_queue: &mut mpsc::Sender<execute::ToQueue>,
	awaiting_prepare: &mut AwaitingPrepare,
	inputs: ExecutePvfInputs,
) -> Result<(), Fatal> {
	let ExecutePvfInputs { pvf, execution_timeout, params, priority, result_tx } = inputs;
	let artifact_id = pvf.as_artifact_id();

	if let Some(state) = artifacts.artifact_state_mut(&artifact_id) {
		match state {
			ArtifactState::Prepared { last_time_needed, .. } => {
				*last_time_needed = SystemTime::now();

				// This artifact has already been prepared, send it to the execute queue.
				send_execute(
					execute_queue,
					execute::ToQueue::Enqueue {
						artifact: ArtifactPathId::new(artifact_id, cache_path),
						execution_timeout,
						params,
						result_tx,
					},
				)
				.await?;
			},
			ArtifactState::Preparing { .. } => {
				awaiting_prepare.add(artifact_id, execution_timeout, params, result_tx);
			},
			ArtifactState::FailedToProcess { last_time_failed, num_failures, error } => {
				if can_retry_prepare_after_failure(*last_time_failed, *num_failures, error) {
					// If we are allowed to retry the failed prepare job, change the state to
					// Preparing and re-queue this job.
					*state = ArtifactState::Preparing {
						waiting_for_response: Vec::new(),
						num_failures: *num_failures,
					};
					send_prepare(
						prepare_queue,
						prepare::ToQueue::Enqueue {
							priority,
							pvf,
							preparation_timeout: LENIENT_PREPARATION_TIMEOUT,
						},
					)
					.await?;
				} else {
					let _ = result_tx.send(Err(ValidationError::from(error.clone())));
				}
			},
		}
	} else {
		// Artifact is unknown: register it and enqueue a job with the corresponding priority and
		// PVF.
		artifacts.insert_preparing(artifact_id.clone(), Vec::new());
		send_prepare(
			prepare_queue,
			prepare::ToQueue::Enqueue {
				priority,
				pvf,
				preparation_timeout: LENIENT_PREPARATION_TIMEOUT,
			},
		)
		.await?;

		// Add an execution request that will wait to run after this prepare job has finished.
		awaiting_prepare.add(artifact_id, execution_timeout, params, result_tx);
	}

	Ok(())
}

async fn handle_heads_up(
	artifacts: &mut Artifacts,
	prepare_queue: &mut mpsc::Sender<prepare::ToQueue>,
	active_pvfs: Vec<Pvf>,
) -> Result<(), Fatal> {
	let now = SystemTime::now();

	for active_pvf in active_pvfs {
		let artifact_id = active_pvf.as_artifact_id();
		if let Some(state) = artifacts.artifact_state_mut(&artifact_id) {
			match state {
				ArtifactState::Prepared { last_time_needed, .. } => {
					*last_time_needed = now;
				},
				ArtifactState::Preparing { .. } => {
					// The artifact is already being prepared, so we don't need to do anything.
				},
				ArtifactState::FailedToProcess { last_time_failed, num_failures, error } => {
					if can_retry_prepare_after_failure(*last_time_failed, *num_failures, error) {
						// If we are allowed to retry the failed prepare job, change the state to
						// Preparing and re-queue this job.
						*state = ArtifactState::Preparing {
							waiting_for_response: vec![],
							num_failures: *num_failures,
						};
						send_prepare(
							prepare_queue,
							prepare::ToQueue::Enqueue {
								priority: Priority::Normal,
								pvf: active_pvf,
								preparation_timeout: LENIENT_PREPARATION_TIMEOUT,
							},
						)
						.await?;
					}
				},
			}
		} else {
			// It's not in the artifacts, so we need to enqueue a job to prepare it.
			artifacts.insert_preparing(artifact_id.clone(), Vec::new());

			send_prepare(
				prepare_queue,
				prepare::ToQueue::Enqueue {
					priority: Priority::Normal,
					pvf: active_pvf,
					preparation_timeout: LENIENT_PREPARATION_TIMEOUT,
				},
			)
			.await?;
		}
	}

	Ok(())
}

async fn handle_prepare_done(
	cache_path: &Path,
	artifacts: &mut Artifacts,
	execute_queue: &mut mpsc::Sender<execute::ToQueue>,
	awaiting_prepare: &mut AwaitingPrepare,
	from_queue: prepare::FromQueue,
) -> Result<(), Fatal> {
	let prepare::FromQueue { artifact_id, result } = from_queue;

	// Make some sanity checks and extract the current state.
	let state = match artifacts.artifact_state_mut(&artifact_id) {
		None => {
			// before sending request to prepare, the artifact is inserted with `preparing` state;
			// the requests are deduplicated for the same artifact id;
			// there is only one possible state change: prepare is done;
			// thus the artifact cannot be unknown, only preparing;
			// qed.
			never!("an unknown artifact was prepared: {:?}", artifact_id);
			return Ok(())
		},
		Some(ArtifactState::Prepared { .. }) => {
			// before sending request to prepare, the artifact is inserted with `preparing` state;
			// the requests are deduplicated for the same artifact id;
			// there is only one possible state change: prepare is done;
			// thus the artifact cannot be prepared, only preparing;
			// qed.
			never!("the artifact is already prepared: {:?}", artifact_id);
			return Ok(())
		},
		Some(ArtifactState::FailedToProcess { .. }) => {
			// The reasoning is similar to the above, the artifact cannot be
			// processed at this point.
			never!("the artifact is already processed unsuccessfully: {:?}", artifact_id);
			return Ok(())
		},
		Some(state @ ArtifactState::Preparing { .. }) => state,
	};

	let num_failures = if let ArtifactState::Preparing { waiting_for_response, num_failures } =
		state
	{
		for result_sender in waiting_for_response.drain(..) {
			let _ = result_sender.send(result.clone());
		}
		num_failures
	} else {
		never!("The reasoning is similar to the above, the artifact can only be preparing at this point; qed");
		return Ok(())
	};

	// It's finally time to dispatch all the execution requests that were waiting for this artifact
	// to be prepared.
	let pending_requests = awaiting_prepare.take(&artifact_id);
	for PendingExecutionRequest { execution_timeout, params, result_tx } in pending_requests {
		if result_tx.is_canceled() {
			// Preparation could've taken quite a bit of time and the requester may be not interested
			// in execution anymore, in which case we just skip the request.
			continue
		}

		// Don't send failed artifacts to the execution's queue.
		if let Err(ref error) = result {
			let _ = result_tx.send(Err(ValidationError::from(error.clone())));
			continue
		}

		send_execute(
			execute_queue,
			execute::ToQueue::Enqueue {
				artifact: ArtifactPathId::new(artifact_id.clone(), cache_path),
				execution_timeout,
				params,
				result_tx,
			},
		)
		.await?;
	}

	*state = match result {
		Ok(cpu_time_elapsed) =>
			ArtifactState::Prepared { last_time_needed: SystemTime::now(), cpu_time_elapsed },
		Err(error) => ArtifactState::FailedToProcess {
			last_time_failed: SystemTime::now(),
			num_failures: *num_failures + 1,
			error,
		},
	};

	Ok(())
}

async fn send_prepare(
	prepare_queue: &mut mpsc::Sender<prepare::ToQueue>,
	to_queue: prepare::ToQueue,
) -> Result<(), Fatal> {
	prepare_queue.send(to_queue).await.map_err(|_| Fatal)
}

async fn send_execute(
	execute_queue: &mut mpsc::Sender<execute::ToQueue>,
	to_queue: execute::ToQueue,
) -> Result<(), Fatal> {
	execute_queue.send(to_queue).await.map_err(|_| Fatal)
}

async fn handle_cleanup_pulse(
	cache_path: &Path,
	sweeper_tx: &mut mpsc::Sender<PathBuf>,
	artifacts: &mut Artifacts,
	artifact_ttl: Duration,
) -> Result<(), Fatal> {
	let to_remove = artifacts.prune(artifact_ttl);
	gum::debug!(
		target: LOG_TARGET,
		"PVF pruning: {} artifacts reached their end of life",
		to_remove.len(),
	);
	for artifact_id in to_remove {
		gum::debug!(
			target: LOG_TARGET,
			validation_code_hash = ?artifact_id.code_hash,
			"pruning artifact",
		);
		let artifact_path = artifact_id.path(cache_path);
		sweeper_tx.send(artifact_path).await.map_err(|_| Fatal)?;
	}

	Ok(())
}

/// A simple task which sole purpose is to delete files thrown at it.
async fn sweeper_task(mut sweeper_rx: mpsc::Receiver<PathBuf>) {
	loop {
		match sweeper_rx.next().await {
			None => break,
			Some(condemned) => {
				let result = async_std::fs::remove_file(&condemned).await;
				gum::trace!(
					target: LOG_TARGET,
					?result,
					"Sweeping the artifact file {}",
					condemned.display(),
				);
			},
		}
	}
}

/// Check if the conditions to retry a prepare job have been met.
fn can_retry_prepare_after_failure(
	last_time_failed: SystemTime,
	num_failures: u32,
	error: &PrepareError,
) -> bool {
	use PrepareError::*;
	match error {
		// Gracefully returned an error, so it will probably be reproducible. Don't retry.
		Prevalidation(_) | Preparation(_) => false,
		// Retry if the retry cooldown has elapsed and if we have already retried less than
		// `NUM_PREPARE_RETRIES` times. IO errors may resolve themselves.
		Panic(_) | TimedOut | DidNotMakeIt =>
			SystemTime::now() >= last_time_failed + PREPARE_FAILURE_COOLDOWN &&
				num_failures <= NUM_PREPARE_RETRIES,
	}
}

/// A stream that yields a pulse continuously at a given interval.
fn pulse_every(interval: std::time::Duration) -> impl futures::Stream<Item = ()> {
	futures::stream::unfold(interval, {
		|interval| async move {
			futures_timer::Delay::new(interval).await;
			Some(((), interval))
		}
	})
	.map(|_| ())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{InvalidCandidate, PrepareError};
	use assert_matches::assert_matches;
	use futures::future::BoxFuture;

	const TEST_EXECUTION_TIMEOUT: Duration = Duration::from_secs(3);

	#[async_std::test]
	async fn pulse_test() {
		let pulse = pulse_every(Duration::from_millis(100));
		futures::pin_mut!(pulse);

		for _ in 0usize..5usize {
			let start = std::time::Instant::now();
			let _ = pulse.next().await.unwrap();

			let el = start.elapsed().as_millis();
			assert!(el > 50 && el < 150, "{}", el);
		}
	}

	/// Creates a new PVF which artifact id can be uniquely identified by the given number.
	fn artifact_id(descriminator: u32) -> ArtifactId {
		Pvf::from_discriminator(descriminator).as_artifact_id()
	}

	fn artifact_path(descriminator: u32) -> PathBuf {
		artifact_id(descriminator).path(&PathBuf::from(std::env::temp_dir())).to_owned()
	}

	struct Builder {
		cleanup_pulse_interval: Duration,
		artifact_ttl: Duration,
		artifacts: Artifacts,
	}

	impl Builder {
		fn default() -> Self {
			Self {
				// these are selected high to not interfere in tests in which pruning is irrelevant.
				cleanup_pulse_interval: Duration::from_secs(3600),
				artifact_ttl: Duration::from_secs(3600),

				artifacts: Artifacts::empty(),
			}
		}

		fn build(self) -> Test {
			Test::new(self)
		}
	}

	struct Test {
		to_host_tx: Option<mpsc::Sender<ToHost>>,

		to_prepare_queue_rx: mpsc::Receiver<prepare::ToQueue>,
		from_prepare_queue_tx: mpsc::UnboundedSender<prepare::FromQueue>,
		to_execute_queue_rx: mpsc::Receiver<execute::ToQueue>,
		to_sweeper_rx: mpsc::Receiver<PathBuf>,

		run: BoxFuture<'static, ()>,
	}

	impl Test {
		fn new(Builder { cleanup_pulse_interval, artifact_ttl, artifacts }: Builder) -> Self {
			let cache_path = PathBuf::from(std::env::temp_dir());

			let (to_host_tx, to_host_rx) = mpsc::channel(10);
			let (to_prepare_queue_tx, to_prepare_queue_rx) = mpsc::channel(10);
			let (from_prepare_queue_tx, from_prepare_queue_rx) = mpsc::unbounded();
			let (to_execute_queue_tx, to_execute_queue_rx) = mpsc::channel(10);
			let (to_sweeper_tx, to_sweeper_rx) = mpsc::channel(10);

			let run = run(Inner {
				cache_path,
				cleanup_pulse_interval,
				artifact_ttl,
				artifacts,
				to_host_rx,
				to_prepare_queue_tx,
				from_prepare_queue_rx,
				to_execute_queue_tx,
				to_sweeper_tx,
				awaiting_prepare: AwaitingPrepare::default(),
			})
			.boxed();

			Self {
				to_host_tx: Some(to_host_tx),
				to_prepare_queue_rx,
				from_prepare_queue_tx,
				to_execute_queue_rx,
				to_sweeper_rx,
				run,
			}
		}

		fn host_handle(&mut self) -> ValidationHost {
			let to_host_tx = self.to_host_tx.take().unwrap();
			ValidationHost { to_host_tx }
		}

		async fn poll_and_recv_to_prepare_queue(&mut self) -> prepare::ToQueue {
			let to_prepare_queue_rx = &mut self.to_prepare_queue_rx;
			run_until(&mut self.run, async { to_prepare_queue_rx.next().await.unwrap() }.boxed())
				.await
		}

		async fn poll_and_recv_to_execute_queue(&mut self) -> execute::ToQueue {
			let to_execute_queue_rx = &mut self.to_execute_queue_rx;
			run_until(&mut self.run, async { to_execute_queue_rx.next().await.unwrap() }.boxed())
				.await
		}

		async fn poll_ensure_to_prepare_queue_is_empty(&mut self) {
			use futures_timer::Delay;

			let to_prepare_queue_rx = &mut self.to_prepare_queue_rx;
			run_until(
				&mut self.run,
				async {
					futures::select! {
						_ = Delay::new(Duration::from_millis(500)).fuse() => (),
						_ = to_prepare_queue_rx.next().fuse() => {
							panic!("the prepare queue is supposed to be empty")
						}
					}
				}
				.boxed(),
			)
			.await
		}

		async fn poll_ensure_to_execute_queue_is_empty(&mut self) {
			use futures_timer::Delay;

			let to_execute_queue_rx = &mut self.to_execute_queue_rx;
			run_until(
				&mut self.run,
				async {
					futures::select! {
						_ = Delay::new(Duration::from_millis(500)).fuse() => (),
						_ = to_execute_queue_rx.next().fuse() => {
							panic!("the execute queue is supposed to be empty")
						}
					}
				}
				.boxed(),
			)
			.await
		}

		async fn poll_ensure_to_sweeper_is_empty(&mut self) {
			use futures_timer::Delay;

			let to_sweeper_rx = &mut self.to_sweeper_rx;
			run_until(
				&mut self.run,
				async {
					futures::select! {
						_ = Delay::new(Duration::from_millis(500)).fuse() => (),
						msg = to_sweeper_rx.next().fuse() => {
							panic!("the sweeper supposed to be empty, but received: {:?}", msg)
						}
					}
				}
				.boxed(),
			)
			.await
		}
	}

	async fn run_until<R>(
		task: &mut (impl Future<Output = ()> + Unpin),
		mut fut: (impl Future<Output = R> + Unpin),
	) -> R {
		use std::task::Poll;

		let start = std::time::Instant::now();
		let fut = &mut fut;
		loop {
			if start.elapsed() > std::time::Duration::from_secs(2) {
				// We expect that this will take only a couple of iterations and thus to take way
				// less than a second.
				panic!("timeout");
			}

			if let Poll::Ready(r) = futures::poll!(&mut *fut) {
				break r
			}

			if futures::poll!(&mut *task).is_ready() {
				panic!()
			}
		}
	}

	#[async_std::test]
	async fn shutdown_on_handle_drop() {
		let test = Builder::default().build();

		let join_handle = async_std::task::spawn(test.run);

		// Dropping the handle will lead to conclusion of the read part and thus will make the event
		// loop to stop, which in turn will resolve the join handle.
		drop(test.to_host_tx);
		join_handle.await;
	}

	#[async_std::test]
	async fn pruning() {
		let mock_now = SystemTime::now() - Duration::from_millis(1000);

		let mut builder = Builder::default();
		builder.cleanup_pulse_interval = Duration::from_millis(100);
		builder.artifact_ttl = Duration::from_millis(500);
		builder.artifacts.insert_prepared(artifact_id(1), mock_now, Duration::default());
		builder.artifacts.insert_prepared(artifact_id(2), mock_now, Duration::default());
		let mut test = builder.build();
		let mut host = test.host_handle();

		host.heads_up(vec![Pvf::from_discriminator(1)]).await.unwrap();

		let to_sweeper_rx = &mut test.to_sweeper_rx;
		run_until(
			&mut test.run,
			async {
				assert_eq!(to_sweeper_rx.next().await.unwrap(), artifact_path(2));
			}
			.boxed(),
		)
		.await;

		// Extend TTL for the first artifact and make sure we don't receive another file removal
		// request.
		host.heads_up(vec![Pvf::from_discriminator(1)]).await.unwrap();
		test.poll_ensure_to_sweeper_is_empty().await;
	}

	#[async_std::test]
	async fn execute_pvf_requests() {
		let mut test = Builder::default().build();
		let mut host = test.host_handle();

		let (result_tx, result_rx_pvf_1_1) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(1),
			TEST_EXECUTION_TIMEOUT,
			b"pvf1".to_vec(),
			Priority::Normal,
			result_tx,
		)
		.await
		.unwrap();

		let (result_tx, result_rx_pvf_1_2) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(1),
			TEST_EXECUTION_TIMEOUT,
			b"pvf1".to_vec(),
			Priority::Critical,
			result_tx,
		)
		.await
		.unwrap();

		let (result_tx, result_rx_pvf_2) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(2),
			TEST_EXECUTION_TIMEOUT,
			b"pvf2".to_vec(),
			Priority::Normal,
			result_tx,
		)
		.await
		.unwrap();

		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);
		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);

		test.from_prepare_queue_tx
			.send(prepare::FromQueue {
				artifact_id: artifact_id(1),
				result: Ok(Duration::default()),
			})
			.await
			.unwrap();
		let result_tx_pvf_1_1 = assert_matches!(
			test.poll_and_recv_to_execute_queue().await,
			execute::ToQueue::Enqueue { result_tx, .. } => result_tx
		);
		let result_tx_pvf_1_2 = assert_matches!(
			test.poll_and_recv_to_execute_queue().await,
			execute::ToQueue::Enqueue { result_tx, .. } => result_tx
		);

		test.from_prepare_queue_tx
			.send(prepare::FromQueue {
				artifact_id: artifact_id(2),
				result: Ok(Duration::default()),
			})
			.await
			.unwrap();
		let result_tx_pvf_2 = assert_matches!(
			test.poll_and_recv_to_execute_queue().await,
			execute::ToQueue::Enqueue { result_tx, .. } => result_tx
		);

		result_tx_pvf_1_1
			.send(Err(ValidationError::InvalidCandidate(InvalidCandidate::AmbiguousWorkerDeath)))
			.unwrap();
		assert_matches!(
			result_rx_pvf_1_1.now_or_never().unwrap().unwrap(),
			Err(ValidationError::InvalidCandidate(InvalidCandidate::AmbiguousWorkerDeath))
		);

		result_tx_pvf_1_2
			.send(Err(ValidationError::InvalidCandidate(InvalidCandidate::AmbiguousWorkerDeath)))
			.unwrap();
		assert_matches!(
			result_rx_pvf_1_2.now_or_never().unwrap().unwrap(),
			Err(ValidationError::InvalidCandidate(InvalidCandidate::AmbiguousWorkerDeath))
		);

		result_tx_pvf_2
			.send(Err(ValidationError::InvalidCandidate(InvalidCandidate::AmbiguousWorkerDeath)))
			.unwrap();
		assert_matches!(
			result_rx_pvf_2.now_or_never().unwrap().unwrap(),
			Err(ValidationError::InvalidCandidate(InvalidCandidate::AmbiguousWorkerDeath))
		);
	}

	#[async_std::test]
	async fn precheck_pvf() {
		let mut test = Builder::default().build();
		let mut host = test.host_handle();

		// First, test a simple precheck request.
		let (result_tx, result_rx) = oneshot::channel();
		host.precheck_pvf(Pvf::from_discriminator(1), result_tx).await.unwrap();

		// The queue received the prepare request.
		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);
		// Send `Ok` right away and poll the host.
		test.from_prepare_queue_tx
			.send(prepare::FromQueue {
				artifact_id: artifact_id(1),
				result: Ok(Duration::default()),
			})
			.await
			.unwrap();
		// No pending execute requests.
		test.poll_ensure_to_execute_queue_is_empty().await;
		// Received the precheck result.
		assert_matches!(result_rx.now_or_never().unwrap().unwrap(), Ok(_));

		// Send multiple requests for the same PVF.
		let mut precheck_receivers = Vec::new();
		for _ in 0..3 {
			let (result_tx, result_rx) = oneshot::channel();
			host.precheck_pvf(Pvf::from_discriminator(2), result_tx).await.unwrap();
			precheck_receivers.push(result_rx);
		}
		// Received prepare request.
		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);
		test.from_prepare_queue_tx
			.send(prepare::FromQueue {
				artifact_id: artifact_id(2),
				result: Err(PrepareError::TimedOut),
			})
			.await
			.unwrap();
		test.poll_ensure_to_execute_queue_is_empty().await;
		for result_rx in precheck_receivers {
			assert_matches!(
				result_rx.now_or_never().unwrap().unwrap(),
				Err(PrepareError::TimedOut)
			);
		}
	}

	#[async_std::test]
	async fn test_prepare_done() {
		let mut test = Builder::default().build();
		let mut host = test.host_handle();

		// Test mixed cases of receiving execute and precheck requests
		// for the same PVF.

		// Send PVF for the execution and request the prechecking for it.
		let (result_tx, result_rx_execute) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(1),
			TEST_EXECUTION_TIMEOUT,
			b"pvf2".to_vec(),
			Priority::Critical,
			result_tx,
		)
		.await
		.unwrap();

		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);

		let (result_tx, result_rx) = oneshot::channel();
		host.precheck_pvf(Pvf::from_discriminator(1), result_tx).await.unwrap();

		// Suppose the preparation failed, the execution queue is empty and both
		// "clients" receive their results.
		test.from_prepare_queue_tx
			.send(prepare::FromQueue {
				artifact_id: artifact_id(1),
				result: Err(PrepareError::TimedOut),
			})
			.await
			.unwrap();
		test.poll_ensure_to_execute_queue_is_empty().await;
		assert_matches!(result_rx.now_or_never().unwrap().unwrap(), Err(PrepareError::TimedOut));
		assert_matches!(
			result_rx_execute.now_or_never().unwrap().unwrap(),
			Err(ValidationError::InternalError(_))
		);

		// Reversed case: first send multiple precheck requests, then ask for an execution.
		let mut precheck_receivers = Vec::new();
		for _ in 0..3 {
			let (result_tx, result_rx) = oneshot::channel();
			host.precheck_pvf(Pvf::from_discriminator(2), result_tx).await.unwrap();
			precheck_receivers.push(result_rx);
		}

		let (result_tx, _result_rx_execute) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(2),
			TEST_EXECUTION_TIMEOUT,
			b"pvf2".to_vec(),
			Priority::Critical,
			result_tx,
		)
		.await
		.unwrap();
		// Received prepare request.
		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);
		test.from_prepare_queue_tx
			.send(prepare::FromQueue {
				artifact_id: artifact_id(2),
				result: Ok(Duration::default()),
			})
			.await
			.unwrap();
		// The execute queue receives new request, preckecking is finished and we can
		// fetch results.
		assert_matches!(
			test.poll_and_recv_to_execute_queue().await,
			execute::ToQueue::Enqueue { .. }
		);
		for result_rx in precheck_receivers {
			assert_matches!(result_rx.now_or_never().unwrap().unwrap(), Ok(_));
		}
	}

	// Test that multiple prechecking requests do not trigger preparation retries if the first one
	// failed.
	#[async_std::test]
	async fn test_precheck_prepare_retry() {
		let mut test = Builder::default().build();
		let mut host = test.host_handle();

		// Submit a precheck request that fails.
		let (result_tx, _result_rx) = oneshot::channel();
		host.precheck_pvf(Pvf::from_discriminator(1), result_tx).await.unwrap();

		// The queue received the prepare request.
		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);
		// Send a PrepareError.
		test.from_prepare_queue_tx
			.send(prepare::FromQueue {
				artifact_id: artifact_id(1),
				result: Err(PrepareError::TimedOut),
			})
			.await
			.unwrap();

		// Submit another precheck request.
		let (result_tx_2, _result_rx_2) = oneshot::channel();
		host.precheck_pvf(Pvf::from_discriminator(1), result_tx_2).await.unwrap();

		// Assert the prepare queue is empty.
		test.poll_ensure_to_prepare_queue_is_empty().await;

		// Pause for enough time to reset the cooldown for this failed prepare request.
		futures_timer::Delay::new(PREPARE_FAILURE_COOLDOWN).await;

		// Submit another precheck request.
		let (result_tx_3, _result_rx_3) = oneshot::channel();
		host.precheck_pvf(Pvf::from_discriminator(1), result_tx_3).await.unwrap();

		// Assert the prepare queue is empty - we do not retry for precheck requests.
		test.poll_ensure_to_prepare_queue_is_empty().await;
	}

	// Test that multiple execution requests trigger preparation retries if the first one failed due
	// to a potentially non-reproducible error.
	#[async_std::test]
	async fn test_execute_prepare_retry() {
		let mut test = Builder::default().build();
		let mut host = test.host_handle();

		// Submit a execute request that fails.
		let (result_tx, _result_rx) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(1),
			TEST_EXECUTION_TIMEOUT,
			b"pvf".to_vec(),
			Priority::Critical,
			result_tx,
		)
		.await
		.unwrap();

		// The queue received the prepare request.
		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);
		// Send a PrepareError.
		test.from_prepare_queue_tx
			.send(prepare::FromQueue {
				artifact_id: artifact_id(1),
				result: Err(PrepareError::TimedOut),
			})
			.await
			.unwrap();

		// Submit another execute request.
		let (result_tx_2, _result_rx_2) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(1),
			TEST_EXECUTION_TIMEOUT,
			b"pvf".to_vec(),
			Priority::Critical,
			result_tx_2,
		)
		.await
		.unwrap();

		// Assert the prepare queue is empty.
		test.poll_ensure_to_prepare_queue_is_empty().await;

		// Pause for enough time to reset the cooldown for this failed prepare request.
		futures_timer::Delay::new(PREPARE_FAILURE_COOLDOWN).await;

		// Submit another execute request.
		let (result_tx_3, _result_rx_3) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(1),
			TEST_EXECUTION_TIMEOUT,
			b"pvf".to_vec(),
			Priority::Critical,
			result_tx_3,
		)
		.await
		.unwrap();

		// Assert the prepare queue contains the request.
		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);
	}

	// Test that multiple execution requests don't trigger preparation retries if the first one
	// failed due to reproducible error (e.g. Prevalidation).
	#[async_std::test]
	async fn test_execute_prepare_no_retry() {
		let mut test = Builder::default().build();
		let mut host = test.host_handle();

		// Submit a execute request that fails.
		let (result_tx, _result_rx) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(1),
			TEST_EXECUTION_TIMEOUT,
			b"pvf".to_vec(),
			Priority::Critical,
			result_tx,
		)
		.await
		.unwrap();

		// The queue received the prepare request.
		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);
		// Send a PrepareError.
		test.from_prepare_queue_tx
			.send(prepare::FromQueue {
				artifact_id: artifact_id(1),
				result: Err(PrepareError::Prevalidation("reproducible error".into())),
			})
			.await
			.unwrap();

		// Submit another execute request.
		let (result_tx_2, _result_rx_2) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(1),
			TEST_EXECUTION_TIMEOUT,
			b"pvf".to_vec(),
			Priority::Critical,
			result_tx_2,
		)
		.await
		.unwrap();

		// Assert the prepare queue is empty.
		test.poll_ensure_to_prepare_queue_is_empty().await;

		// Pause for enough time to reset the cooldown for this failed prepare request.
		futures_timer::Delay::new(PREPARE_FAILURE_COOLDOWN).await;

		// Submit another execute request.
		let (result_tx_3, _result_rx_3) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(1),
			TEST_EXECUTION_TIMEOUT,
			b"pvf".to_vec(),
			Priority::Critical,
			result_tx_3,
		)
		.await
		.unwrap();

		// Assert the prepare queue is empty - we do not retry for prevalidation errors.
		test.poll_ensure_to_prepare_queue_is_empty().await;
	}

	// Test that multiple heads-up requests trigger preparation retries if the first one failed.
	#[async_std::test]
	async fn test_heads_up_prepare_retry() {
		let mut test = Builder::default().build();
		let mut host = test.host_handle();

		// Submit a heads-up request that fails.
		host.heads_up(vec![Pvf::from_discriminator(1)]).await.unwrap();

		// The queue received the prepare request.
		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);
		// Send a PrepareError.
		test.from_prepare_queue_tx
			.send(prepare::FromQueue {
				artifact_id: artifact_id(1),
				result: Err(PrepareError::TimedOut),
			})
			.await
			.unwrap();

		// Submit another heads-up request.
		host.heads_up(vec![Pvf::from_discriminator(1)]).await.unwrap();

		// Assert the prepare queue is empty.
		test.poll_ensure_to_prepare_queue_is_empty().await;

		// Pause for enough time to reset the cooldown for this failed prepare request.
		futures_timer::Delay::new(PREPARE_FAILURE_COOLDOWN).await;

		// Submit another heads-up request.
		host.heads_up(vec![Pvf::from_discriminator(1)]).await.unwrap();

		// Assert the prepare queue contains the request.
		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);
	}

	#[async_std::test]
	async fn cancellation() {
		let mut test = Builder::default().build();
		let mut host = test.host_handle();

		let (result_tx, result_rx) = oneshot::channel();
		host.execute_pvf(
			Pvf::from_discriminator(1),
			TEST_EXECUTION_TIMEOUT,
			b"pvf1".to_vec(),
			Priority::Normal,
			result_tx,
		)
		.await
		.unwrap();

		assert_matches!(
			test.poll_and_recv_to_prepare_queue().await,
			prepare::ToQueue::Enqueue { .. }
		);

		test.from_prepare_queue_tx
			.send(prepare::FromQueue {
				artifact_id: artifact_id(1),
				result: Ok(Duration::default()),
			})
			.await
			.unwrap();

		drop(result_rx);

		test.poll_ensure_to_execute_queue_is_empty().await;
	}
}
