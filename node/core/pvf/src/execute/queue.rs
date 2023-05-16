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
	fmt, mem,
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

enum WorkerState {
	Idle(IdleWorker),
	Assigned(ExecuteJob),
	Busy,
}

struct WorkerData {
	state: WorkerState,
	handle: WorkerHandle,
	executor_params_hash: ExecutorParamsHash,
}

impl WorkerData {
	/// Assigns a job to the worker. Returns the idle worker token. An attempt to assign a job to
	/// a worker already busy with another job means there's a critical bug in code and shall
	/// result in a panic.
	fn assign(&mut self, job: ExecuteJob) -> IdleWorker {
		debug_assert_eq!(self.executor_params_hash, job.executor_params.hash());
		match mem::replace(&mut self.state, WorkerState::Assigned(job)) {
			WorkerState::Idle(idle) => idle,
			_ => unreachable!("Tried to assign a job to already busy worker"),
		}
	}

	/// Runs the assigned job. Returns job data to pass to the worker.
	fn run(&mut self) -> ExecuteJob {
		match mem::replace(&mut self.state, WorkerState::Busy) {
			WorkerState::Assigned(job) => job,
			_ => unreachable!("Tried to run a job that hasn't been assigned"),
		}
	}

	/// Idles the worker. An attempt to idle an already idle worker means there's a critical bug
	/// in code and shall result in a panic.
	fn idle(&mut self, idle: IdleWorker) {
		match mem::replace(&mut self.state, WorkerState::Idle(idle)) {
			WorkerState::Busy => (),
			_ => unreachable!("Tried to idle an already idle worker"),
		}
	}
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
			if matches!(d.1.state, WorkerState::Idle(_)) &&
				d.1.executor_params_hash == executor_params_hash
			{
				Some(d.0)
			} else {
				None
			}
		})
	}

	fn find_idle(&self) -> Option<Worker> {
		self.running.iter().find_map(|d| {
			if matches!(d.1.state, WorkerState::Idle(_)) {
				Some(d.0)
			} else {
				None
			}
		})
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
						self.handle_to_queue(to_queue);
					} else {
						break;
					}
				}
				ev = self.mux.select_next_some() => self.handle_mux(ev).await,
			}

			self.purge_dead().await;
		}
	}

	async fn purge_dead(&mut self) {
		let mut to_remove = vec![];
		for (worker, data) in self.workers.running.iter_mut() {
			if futures::poll!(&mut data.handle).is_ready() {
				// a resolved future means that the worker has terminated. Weed it out.
				to_remove.push(worker);
			}
		}
		for w in to_remove {
			if self.workers.running.remove(w).is_some() {
				self.metrics.execute_worker().on_retired();
			}
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
			self.assign(worker, job);
		} else {
			self.spawn_extra_worker(job);
		}
	}

	fn handle_to_queue(&mut self, to_queue: ToQueue) {
		let ToQueue::Enqueue { artifact, pending_execution_request } = to_queue;
		let PendingExecutionRequest { exec_timeout, params, executor_params, result_tx } =
			pending_execution_request;
		gum::debug!(
			target: LOG_TARGET,
			validation_code_hash = ?artifact.id.code_hash,
			"enqueueing an artifact for execution",
		);
		self.metrics.execute_enqueued();
		let job = ExecuteJob {
			artifact,
			exec_timeout,
			params,
			executor_params,
			result_tx,
			waiting_since: Instant::now(),
		};
		self.queue.push_back(job);
		self.try_assign_next_job(None);
	}

	async fn handle_mux(&mut self, event: QueueEvent) {
		match event {
			QueueEvent::Spawn(idle, handle, job) => {
				self.handle_worker_spawned(idle, handle, job);
			},
			QueueEvent::StartWork(worker, outcome, artifact_id, result_tx) => {
				self.handle_job_finish(worker, outcome, artifact_id, result_tx);
			},
		}
	}

	fn handle_worker_spawned(&mut self, idle: IdleWorker, handle: WorkerHandle, job: ExecuteJob) {
		self.metrics.execute_worker().on_spawned();
		self.workers.spawn_inflight -= 1;
		let worker = self.workers.running.insert(WorkerData {
			state: WorkerState::Idle(idle),
			handle,
			executor_params_hash: job.executor_params.hash(),
		});

		gum::debug!(target: LOG_TARGET, ?worker, "execute worker spawned");

		self.assign(worker, job);
	}

	/// If there are pending jobs in the queue, schedules the next of them onto the just freed up
	/// worker. Otherwise, puts back into the available workers list.
	fn handle_job_finish(
		&mut self,
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

		self.metrics.execute_finished();
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
			if let Some(data) = self.workers.running.get_mut(worker) {
				data.idle(idle_worker);
				return self.try_assign_next_job(Some(worker))
			}
		} else {
			// Note it's possible that the worker was purged already by `purge_dead`
			if self.workers.running.remove(worker).is_some() {
				self.metrics.execute_worker().on_retired();
			}
		}

		self.try_assign_next_job(None);
	}

	fn spawn_extra_worker(&mut self, job: ExecuteJob) {
		self.metrics.execute_worker().on_begin_spawn();
		gum::debug!(target: LOG_TARGET, "spawning an extra worker");

		self.mux
			.push(spawn_worker_task(self.program_path.clone(), job, self.spawn_timeout).boxed());
		self.workers.spawn_inflight += 1;
	}

	/// Ask the given worker to perform the given job.
	///
	/// The worker must be running and idle. The job and the worker must share the same execution
	/// environment parameter set.
	fn assign(&mut self, worker: Worker, job: ExecuteJob) {
		gum::debug!(
			target: LOG_TARGET,
			validation_code_hash = ?job.artifact.id,
			?worker,
			"assigning the execute worker",
		);

		let wrkr = self
			.workers
			.running
			.get_mut(worker)
			.expect("The caller must supply a worker that is idle and running; qed");
		let idle = wrkr.assign(job);
		let job = wrkr.run();

		let execution_timer = self.metrics.time_execution();
		self.mux.push(
			async move {
				let _timer = execution_timer;
				let outcome = super::worker_intf::start_work(
					idle,
					&job.artifact,
					job.exec_timeout,
					&job.params,
				)
				.await;
				QueueEvent::StartWork(worker, outcome, job.artifact.id, job.result_tx)
			}
			.boxed(),
		);
	}
}

/// Spawns a new worker to execute a pre-assigned job.
/// A worker is never spawned as idle; a job to be executed by the worker has to be determined
/// beforehand. In such a way, a race condition is avoided: during the worker being spawned,
/// another job in the queue, with an incompatible execution environment, may become stale, and
/// the queue would have to kill a newly started worker and spawn another one.
/// Nevertheless, if the worker finishes executing the job, it becomes idle and may be used to
/// execute other jobs with a compatible execution environment.
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
