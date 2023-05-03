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

//! A queue that handles requests for PVF execution.

use super::worker_intf::Outcome;
use crate::{
	artifacts::{ArtifactId, ArtifactPathId},
	host::ResultSender,
	metrics::Metrics,
	worker_common::{IdleWorker, WorkerHandle},
	InvalidCandidate, ValidationError, LOG_TARGET,
};
use futures::{
	channel::mpsc,
	future::BoxFuture,
	stream::{FuturesUnordered, StreamExt as _},
	Future, FutureExt,
};
use polkadot_primitives::{ExecutorParams, ExecutorParamsHash};
use slotmap::HopSlotMap;
use std::{
	collections::VecDeque,
	fmt,
	path::PathBuf,
	time::{Duration, Instant},
};

/// The amount of time a job for which the queue does not have a compatible worker may wait in the
/// queue. After that time passes, the queue will kill the first worker which becomes idle to
/// re-spawn a new worker to execute the job immediately.
/// To make any sense and not to break things, the value should be greater than minimal execution
/// timeout in use, and less than the block time.
const MAX_KEEP_WAITING: Duration = Duration::from_secs(4);

slotmap::new_key_type! { struct Worker; }

#[derive(Debug)]
pub enum ToQueue {
	Enqueue { artifact: ArtifactPathId, pending_execution_request: PendingExecutionRequest },
}

/// An execution request that should execute the PVF (known in the context) and send the results
/// to the given result sender.
#[derive(Debug)]
pub struct PendingExecutionRequest {
	pub exec_timeout: Duration,
	pub params: Vec<u8>,
	pub executor_params: ExecutorParams,
	pub result_tx: ResultSender,
}

struct ExecuteJob {
	artifact: ArtifactPathId,
	exec_timeout: Duration,
	params: Vec<u8>,
	executor_params: ExecutorParams,
	result_tx: ResultSender,
	waiting_since: Instant,
}

struct WorkerData {
	idle: Option<IdleWorker>,
	handle: WorkerHandle,
	executor_params_hash: ExecutorParamsHash,
}

impl fmt::Debug for WorkerData {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "WorkerData(pid={})", self.handle.id())
	}
}

struct Workers {
	/// The registry of running workers.
	running: HopSlotMap<Worker, WorkerData>,

	/// The number of spawning but not yet spawned workers.
	spawn_inflight: usize,

	/// The maximum number of workers queue can have at once.
	capacity: usize,
}

impl Workers {
	fn can_afford_one_more(&self) -> bool {
		self.spawn_inflight + self.running.len() < self.capacity
	}

	fn find_available(&self, executor_params_hash: ExecutorParamsHash) -> Option<Worker> {
		self.running.iter().find_map(|d| {
			if d.1.idle.is_some() && d.1.executor_params_hash == executor_params_hash {
				Some(d.0)
			} else {
				None
			}
		})
	}

	fn find_idle(&self) -> Option<Worker> {
		self.running
			.iter()
			.find_map(|d| if d.1.idle.is_some() { Some(d.0) } else { None })
	}

	/// Find the associated data by the worker token and extract it's [`IdleWorker`] token.
	///
	/// Returns `None` if either worker is not recognized or idle token is absent.
	fn claim_idle(&mut self, worker: Worker) -> Option<IdleWorker> {
		self.running.get_mut(worker)?.idle.take()
	}
}

enum QueueEvent {
	Spawn(IdleWorker, WorkerHandle, ExecuteJob),
	StartWork(Worker, Outcome, ArtifactId, ResultSender),
}

type Mux = FuturesUnordered<BoxFuture<'static, QueueEvent>>;

struct Queue {
	metrics: Metrics,

	/// The receiver that receives messages to the pool.
	to_queue_rx: mpsc::Receiver<ToQueue>,

	program_path: PathBuf,
	spawn_timeout: Duration,

	/// The queue of jobs that are waiting for a worker to pick up.
	queue: VecDeque<ExecuteJob>,
	workers: Workers,
	mux: Mux,
}

impl Queue {
	fn new(
		metrics: Metrics,
		program_path: PathBuf,
		worker_capacity: usize,
		spawn_timeout: Duration,
		to_queue_rx: mpsc::Receiver<ToQueue>,
	) -> Self {
		Self {
			metrics,
			program_path,
			spawn_timeout,
			to_queue_rx,
			queue: VecDeque::new(),
			mux: Mux::new(),
			workers: Workers {
				running: HopSlotMap::with_capacity_and_key(10),
				spawn_inflight: 0,
				capacity: worker_capacity,
			},
		}
	}

	async fn run(mut self) {
		loop {
			futures::select! {
				to_queue = self.to_queue_rx.next() => {
					if let Some(to_queue) = to_queue {
						handle_to_queue(&mut self, to_queue);
					} else {
						break;
					}
				}
				ev = self.mux.select_next_some() => handle_mux(&mut self, ev).await,
			}

			purge_dead(&self.metrics, &mut self.workers).await;
		}
	}

	/// Tries to assign a job in the queue to a worker. If an idle worker is provided, it does its
	/// best to find a job with a compatible execution environment unless there are jobs in the
	/// queue waiting too long. In that case, it kills an existing idle worker and spawns a new
	/// one. It may spawn an additional worker if that is affordable.
	/// If all the workers are busy or the queue is empty, it does nothing.
	/// Should be called every time a new job arrives to the queue or a job finishes.
	fn try_assign_next_job(&mut self, finished_worker: Option<Worker>) {
		// New jobs are always pushed to the tail of the queue; the one at its head is always
		// the eldest one.
		let eldest = if let Some(eldest) = self.queue.get(0) { eldest } else { return };

		// By default, we're going to execute the eldest job on any worker slot available, even if
		// we have to kill and re-spawn a worker
		let mut worker = None;
		let mut job_index = 0;

		// But if we're not pressed for time, we can try to find a better job-worker pair not
		// requiring the expensive kill-spawn operation
		if eldest.waiting_since.elapsed() < MAX_KEEP_WAITING {
			if let Some(finished_worker) = finished_worker {
				if let Some(worker_data) = self.workers.running.get(finished_worker) {
					for (i, job) in self.queue.iter().enumerate() {
						if worker_data.executor_params_hash == job.executor_params.hash() {
							(worker, job_index) = (Some(finished_worker), i);
							break
						}
					}
				}
			}
		}

		if worker.is_none() {
			// Try to obtain a worker for the job
			worker = self.workers.find_available(self.queue[job_index].executor_params.hash());
		}

		if worker.is_none() {
			if let Some(idle) = self.workers.find_idle() {
				// No available workers of required type but there are some idle ones of other
				// types, have to kill one and re-spawn with the correct type
				if self.workers.running.remove(idle).is_some() {
					self.metrics.execute_worker().on_retired();
				}
			}
		}

		if worker.is_none() && !self.workers.can_afford_one_more() {
			// Bad luck, no worker slot can be used to execute the job
			return
		}

		let job = self.queue.remove(job_index).expect("Job is just checked to be in queue; qed");

		if let Some(worker) = worker {
			assign(self, worker, job);
		} else {
			spawn_extra_worker(self, job);
		}
	}
}

async fn purge_dead(metrics: &Metrics, workers: &mut Workers) {
	let mut to_remove = vec![];
	for (worker, data) in workers.running.iter_mut() {
		if futures::poll!(&mut data.handle).is_ready() {
			// a resolved future means that the worker has terminated. Weed it out.
			to_remove.push(worker);
		}
	}
	for w in to_remove {
		if workers.running.remove(w).is_some() {
			metrics.execute_worker().on_retired();
		}
	}
}

fn handle_to_queue(queue: &mut Queue, to_queue: ToQueue) {
	let ToQueue::Enqueue { artifact, pending_execution_request } = to_queue;
	let PendingExecutionRequest { exec_timeout, params, executor_params, result_tx } =
		pending_execution_request;
	gum::debug!(
		target: LOG_TARGET,
		validation_code_hash = ?artifact.id.code_hash,
		"enqueueing an artifact for execution",
	);
	queue.metrics.execute_enqueued();
	let job = ExecuteJob {
		artifact,
		exec_timeout,
		params,
		executor_params,
		result_tx,
		waiting_since: Instant::now(),
	};
	queue.queue.push_back(job);
	queue.try_assign_next_job(None);
}

async fn handle_mux(queue: &mut Queue, event: QueueEvent) {
	match event {
		QueueEvent::Spawn(idle, handle, job) => {
			handle_worker_spawned(queue, idle, handle, job);
		},
		QueueEvent::StartWork(worker, outcome, artifact_id, result_tx) => {
			handle_job_finish(queue, worker, outcome, artifact_id, result_tx);
		},
	}
}

fn handle_worker_spawned(
	queue: &mut Queue,
	idle: IdleWorker,
	handle: WorkerHandle,
	job: ExecuteJob,
) {
	queue.metrics.execute_worker().on_spawned();
	queue.workers.spawn_inflight -= 1;
	let worker = queue.workers.running.insert(WorkerData {
		idle: Some(idle),
		handle,
		executor_params_hash: job.executor_params.hash(),
	});

	gum::debug!(target: LOG_TARGET, ?worker, "execute worker spawned");

	assign(queue, worker, job);
}

/// If there are pending jobs in the queue, schedules the next of them onto the just freed up
/// worker. Otherwise, puts back into the available workers list.
fn handle_job_finish(
	queue: &mut Queue,
	worker: Worker,
	outcome: Outcome,
	artifact_id: ArtifactId,
	result_tx: ResultSender,
) {
	let (idle_worker, result, duration) = match outcome {
		Outcome::Ok { result_descriptor, duration, idle_worker } => {
			// TODO: propagate the soft timeout

			(Some(idle_worker), Ok(result_descriptor), Some(duration))
		},
		Outcome::InvalidCandidate { err, idle_worker } => (
			Some(idle_worker),
			Err(ValidationError::InvalidCandidate(InvalidCandidate::WorkerReportedError(err))),
			None,
		),
		Outcome::InternalError { err, idle_worker } =>
			(Some(idle_worker), Err(ValidationError::InternalError(err)), None),
		Outcome::HardTimeout =>
			(None, Err(ValidationError::InvalidCandidate(InvalidCandidate::HardTimeout)), None),
		Outcome::IoErr => (
			None,
			Err(ValidationError::InvalidCandidate(InvalidCandidate::AmbiguousWorkerDeath)),
			None,
		),
	};

	queue.metrics.execute_finished();
	if let Err(ref err) = result {
		gum::warn!(
			target: LOG_TARGET,
			?artifact_id,
			?worker,
			worker_rip = idle_worker.is_none(),
			"execution worker concluded, error occurred: {:?}",
			err
		);
	} else {
		gum::debug!(
			target: LOG_TARGET,
			?artifact_id,
			?worker,
			worker_rip = idle_worker.is_none(),
			?duration,
			"execute worker concluded successfully",
		);
	}

	// First we send the result. It may fail due to the other end of the channel being dropped,
	// that's legitimate and we don't treat that as an error.
	let _ = result_tx.send(result);

	// Then, we should deal with the worker:
	//
	// - if the `idle_worker` token was returned we should either schedule the next task or just put
	//   it back so that the next incoming job will be able to claim it
	//
	// - if the `idle_worker` token was consumed, all the metadata pertaining to that worker should
	//   be removed.
	if let Some(idle_worker) = idle_worker {
		if let Some(data) = queue.workers.running.get_mut(worker) {
			data.idle = Some(idle_worker);
			return queue.try_assign_next_job(Some(worker))
		}
	} else {
		// Note it's possible that the worker was purged already by `purge_dead`
		if queue.workers.running.remove(worker).is_some() {
			queue.metrics.execute_worker().on_retired();
		}
	}

	queue.try_assign_next_job(None);
}

fn spawn_extra_worker(queue: &mut Queue, job: ExecuteJob) {
	queue.metrics.execute_worker().on_begin_spawn();
	gum::debug!(target: LOG_TARGET, "spawning an extra worker");

	queue
		.mux
		.push(spawn_worker_task(queue.program_path.clone(), job, queue.spawn_timeout).boxed());
	queue.workers.spawn_inflight += 1;
}

/// Spawns a new worker to execute a pre-assigned job.
/// A worker is never spawned as idle; a job to be executed by the worker has to be determined
/// beforehand. In such a way, a race condition is avoided: during the worker being spawned,
/// another job in the queue, with an incompatible execution environment, may become stale, and
/// the queue would have to kill a newly started worker and spawn another one.
/// Nevertheless, if the worker finishes executing the job, it becomes idle and may be used to execute other jobs with a compatible execution environment.
async fn spawn_worker_task(
	program_path: PathBuf,
	job: ExecuteJob,
	spawn_timeout: Duration,
) -> QueueEvent {
	use futures_timer::Delay;

	loop {
		match super::worker_intf::spawn(&program_path, job.executor_params.clone(), spawn_timeout)
			.await
		{
			Ok((idle, handle)) => break QueueEvent::Spawn(idle, handle, job),
			Err(err) => {
				gum::warn!(target: LOG_TARGET, "failed to spawn an execute worker: {:?}", err);

				// Assume that the failure is intermittent and retry after a delay.
				Delay::new(Duration::from_secs(3)).await;
			},
		}
	}
}

/// Ask the given worker to perform the given job.
///
/// The worker must be running and idle. The job and the worker must share the same execution
/// environment parameter set.
fn assign(queue: &mut Queue, worker: Worker, job: ExecuteJob) {
	gum::debug!(
		target: LOG_TARGET,
		validation_code_hash = ?job.artifact.id,
		?worker,
		"assigning the execute worker",
	);

	debug_assert_eq!(
		queue
			.workers
			.running
			.get(worker)
			.expect("caller must provide existing worker; qed")
			.executor_params_hash,
		job.executor_params.hash()
	);

	let idle = queue.workers.claim_idle(worker).expect(
		"this caller must supply a worker which is idle and running;
			thus claim_idle cannot return None;
			qed.",
	);
	let execution_timer = queue.metrics.time_execution();
	queue.mux.push(
		async move {
			let _timer = execution_timer;
			let outcome = super::worker_intf::start_work(
				idle,
				job.artifact.clone(),
				job.exec_timeout,
				job.params,
			)
			.await;
			QueueEvent::StartWork(worker, outcome, job.artifact.id, job.result_tx)
		}
		.boxed(),
	);
}

pub fn start(
	metrics: Metrics,
	program_path: PathBuf,
	worker_capacity: usize,
	spawn_timeout: Duration,
) -> (mpsc::Sender<ToQueue>, impl Future<Output = ()>) {
	let (to_queue_tx, to_queue_rx) = mpsc::channel(20);
	let run = Queue::new(metrics, program_path, worker_capacity, spawn_timeout, to_queue_rx).run();
	(to_queue_tx, run)
}
