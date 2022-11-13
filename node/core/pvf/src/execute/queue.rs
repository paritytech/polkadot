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

//! A queue that handles requests for PVF execution.

use super::worker::Outcome;
use crate::{
	artifacts::{ArtifactId, ArtifactPathId},
	host::ResultSender,
	metrics::Metrics,
	worker_common::{IdleWorker, WorkerHandle},
	InvalidCandidate, ValidationError, LOG_TARGET,
};
use async_std::path::PathBuf;
use futures::{
	channel::mpsc,
	future::BoxFuture,
	stream::{FuturesUnordered, StreamExt as _},
	Future, FutureExt,
};
use polkadot_primitives::vstaging::{ExecutorParams, ExecutorParamsHash};
use slotmap::HopSlotMap;
use std::{
	collections::VecDeque,
	fmt,
	time::{Duration, Instant},
};

const MAX_KEEP_WAITING: u128 = 60000u128; // FIXME: Needs to be evaluated

slotmap::new_key_type! { struct Worker; }

#[derive(Debug)]
pub enum ToQueue {
	Enqueue {
		artifact: ArtifactPathId,
		execution_timeout: Duration,
		params: Vec<u8>,
		executor_params: ExecutorParams,
		result_tx: ResultSender,
	},
}

struct ExecuteJob {
	artifact: ArtifactPathId,
	execution_timeout: Duration,
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

	fn next_job_index(&self, idle_worker: Option<Worker>) -> Option<usize> {
		// New jobs are always pushed to the tail of the queue; the one at its head is always the eldest one.
		// First check if we have a job that cannot wait any more even if we have to kill a worker to run it

		let eldest = if let Some(eldest) = self.queue.get(0) { eldest } else { return None };

		if eldest.waiting_since.elapsed().as_millis() > MAX_KEEP_WAITING {
			return Some(0)
		}

		// No rush, let's try to find the most suitable job for the worker that's just become idle or dead

		if let Some(idle_worker) = idle_worker {
			if let Some(worker) = self.workers.running.get(idle_worker) {
				for (i, job) in self.queue.iter().enumerate() {
					if worker.executor_params_hash == job.executor_params.hash() {
						return Some(i)
					}
				}

				// There are some jobs but an idle worker cannot execute them. Let's instruct the caller to
				// kill the useless worker and spawn a new one to execute the eldest job.
				// FIXME: Worst case scenario should be carefully evaluated and this logic might require
				// adjustment
				return Some(0)
			} else {
				// Worker to which the job was supposed to be assigned is not there any more, just execute
				// the eldest job
				return Some(0)
			}
		} else {
			// Old worker is dead anyway so let's just instruct caller to spawn a new one to
			// execute the eldest job
			return Some(0)
		}
	}

	fn take_job(&mut self, index: usize) -> Option<ExecuteJob> {
		self.queue.remove(index)
	}

	fn take_next_job(&mut self, idle_worker: Option<Worker>) -> Option<ExecuteJob> {
		if let Some(index) = self.next_job_index(idle_worker) {
			self.take_job(index)
		} else {
			None
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

fn try_assign_next_job(queue: &mut Queue) {
	if let Some(ji) = queue.next_job_index(None) {
		if let Some(available) =
			queue.workers.find_available(queue.queue[ji].executor_params.hash())
		{
			let job = queue.take_job(ji).expect("Job is just checked to be in queue; qed");
			assign(queue, available, job);
			return
		} else {
			if let Some(idle) = queue.workers.find_idle() {
				// No available workers of required type but there are some idle ones of other types,
				// have to kill one and re-spawn with the correct type
				if queue.workers.running.remove(idle).is_some() {
					queue.metrics.execute_worker().on_retired();
				}
			}
		}

		if queue.workers.can_afford_one_more() {
			let job = queue.take_job(ji).expect("Job is just checked to be in queue; qed");
			spawn_extra_worker(queue, job);
		}
	}
}

fn handle_to_queue(queue: &mut Queue, to_queue: ToQueue) {
	let ToQueue::Enqueue { artifact, execution_timeout, params, executor_params, result_tx } =
		to_queue;
	gum::debug!(
		target: LOG_TARGET,
		validation_code_hash = ?artifact.id.code_hash,
		"enqueueing an artifact for execution",
	);
	queue.metrics.execute_enqueued();
	let job = ExecuteJob {
		artifact,
		execution_timeout,
		params,
		executor_params,
		result_tx,
		waiting_since: Instant::now(),
	};
	queue.queue.push_back(job);
	try_assign_next_job(queue);
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
	let (idle_worker, result) = match outcome {
		Outcome::Ok { result_descriptor, duration_ms, idle_worker } => {
			// TODO: propagate the soft timeout
			drop(duration_ms);

			(Some(idle_worker), Ok(result_descriptor))
		},
		Outcome::InvalidCandidate { err, idle_worker } => (
			Some(idle_worker),
			Err(ValidationError::InvalidCandidate(InvalidCandidate::WorkerReportedError(err))),
		),
		Outcome::InternalError { err, idle_worker } =>
			(Some(idle_worker), Err(ValidationError::InternalError(err))),
		Outcome::HardTimeout =>
			(None, Err(ValidationError::InvalidCandidate(InvalidCandidate::HardTimeout))),
		Outcome::IoErr =>
			(None, Err(ValidationError::InvalidCandidate(InvalidCandidate::AmbiguousWorkerDeath))),
	};

	queue.metrics.execute_finished();
	gum::debug!(
		target: LOG_TARGET,
		validation_code_hash = ?artifact_id.code_hash,
		?worker,
		worker_rip = idle_worker.is_none(),
		"execute worker concluded",
	);

	// First we send the result. It may fail due the other end of the channel being dropped, that's
	// legitimate and we don't treat that as an error.
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
		}
	} else {
		// Note it's possible that the worker was purged already by `purge_dead`
		if queue.workers.running.remove(worker).is_some() {
			queue.metrics.execute_worker().on_retired();
		}
	}

	try_assign_next_job(queue);
}

fn spawn_extra_worker(queue: &mut Queue, job: ExecuteJob) {
	queue.metrics.execute_worker().on_begin_spawn();
	gum::debug!(target: LOG_TARGET, "spawning an extra worker");

	queue
		.mux
		.push(spawn_worker_task(queue.program_path.clone(), job, queue.spawn_timeout).boxed());
	queue.workers.spawn_inflight += 1;
}

async fn spawn_worker_task(
	program_path: PathBuf,
	job: ExecuteJob,
	spawn_timeout: Duration,
) -> QueueEvent {
	use futures_timer::Delay;

	loop {
		match super::worker::spawn(&program_path, job.executor_params.clone(), spawn_timeout).await
		{
			Ok((idle, handle)) => break QueueEvent::Spawn(idle, handle, job),
			Err(err) => {
				gum::warn!(target: LOG_TARGET, "failed to spawn an execute worker: {:?}", err);

				// Assume that the failure intermittent and retry after a delay.
				Delay::new(Duration::from_secs(3)).await;
			},
		}
	}
}

/// Ask the given worker to perform the given job.
///
/// The worker must be running and idle.
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
			let outcome = super::worker::start_work(
				idle,
				job.artifact.clone(),
				job.execution_timeout,
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
