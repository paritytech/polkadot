// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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

use super::worker::Outcome;
use crate::{
	artifacts::ArtifactId,
	error::PrecheckError,
	worker_common::{IdleWorker, Worker, WorkerData, WorkerHandle, Workers},
	PrecheckResult, Pvf, LOG_TARGET,
};
use async_std::path::PathBuf;
use futures::{
	channel::{mpsc, oneshot},
	future::BoxFuture,
	stream::{FuturesUnordered, StreamExt},
	FutureExt,
};
use std::{
	collections::{HashMap, VecDeque},
	future::Future,
	time::Duration,
};

/// Transmission end used for sending the PVF prechecking result.
pub type PrecheckResultSender = oneshot::Sender<PrecheckResult>;

/// Precheck request sent from the host to the queue.
pub struct ToQueue {
	pub pvf: Pvf,
	pub result_sender: PrecheckResultSender,
}

/// An event produced by futures spawned by the queue.
enum QueueEvent {
	/// A new worker spawned.
	Spawn(IdleWorker, WorkerHandle),
	/// A worker finished the PVF processing. If it was successful
	/// and worker survived, its token is returned along with the outcome.
	StartWork { worker: Worker, outcome: Outcome, artifact_id: ArtifactId },
}

/// A set of asynchronous tasks spawned by the queue.
type Mux = FuturesUnordered<BoxFuture<'static, QueueEvent>>;

/// Status of the PVF prechecking. Only makes sense for requests whose
/// processing was initiated, i.e. a worker was previously assigned to it.
/// Queued requests are stored separately in the queue.
enum PrecheckStatus {
	/// A worker is currently busy with the prechecking.
	InProgress { waiting_for_response: Vec<PrecheckResultSender> },
	/// Cached result for processed requests.
	Done(PrecheckResult),
}

/// Handles the request which is already present in the queue.
fn deduplicate_request(status: &mut PrecheckStatus, request: ToQueue) {
	match status {
		PrecheckStatus::InProgress { waiting_for_response } =>
			waiting_for_response.push(request.result_sender),
		PrecheckStatus::Done(result) => {
			let _ = request.result_sender.send(result.clone());
		},
	}
}

struct Queue {
	artifacts: HashMap<ArtifactId, PrecheckStatus>,
	to_queue_rx: mpsc::Receiver<ToQueue>,

	program_path: PathBuf,
	queue: VecDeque<ToQueue>,
	workers: Workers,
	mux: Mux,

	precheck_timeout: Duration,
	spawn_timeout: Duration,
}

impl Queue {
	fn new(
		to_queue_rx: mpsc::Receiver<ToQueue>,
		program_path: PathBuf,
		precheck_timeout: Duration,
		spawn_timeout: Duration,
		worker_capacity: usize,
	) -> Self {
		Self {
			artifacts: HashMap::new(),
			to_queue_rx,
			program_path,
			queue: VecDeque::new(),
			workers: Workers::with_capacity(worker_capacity),
			mux: Mux::new(),
			precheck_timeout,
			spawn_timeout,
		}
	}

	async fn run(mut self) {
		loop {
			futures::select! {
				request = self.to_queue_rx.next() => {
					if let Some(request) = request {
						self.handle_to_queue(request)
					} else {
						break
					}
				}
				ev = self.mux.select_next_some() => self.handle_mux(ev),
			}

			self.purge_dead().await
		}
	}

	fn handle_to_queue(&mut self, request: ToQueue) {
		// Deduplicate the request if necessary.
		if let Some(status) = self.artifacts.get_mut(&request.pvf.as_artifact_id()) {
			deduplicate_request(status, request);
			return
		}

		tracing::debug!(
			target: LOG_TARGET,
			validation_code_hash = ?request.pvf.code_hash,
			"enqueueing pvf for prechecking",
		);

		if let Some(available) = self.workers.find_available() {
			self.assign(available, request);
		} else {
			if self.workers.can_afford_one_more() {
				self.spawn_extra_worker();
			}
			// Queue the request, the worker will pull it later.
			self.queue.push_back(request);
		}
	}

	fn handle_mux(&mut self, event: QueueEvent) {
		match event {
			QueueEvent::Spawn(idle, handle) => {
				self.handle_worker_spawned(idle, handle);
			},
			QueueEvent::StartWork { worker, outcome, artifact_id } =>
				self.handle_job_finished(worker, outcome, artifact_id),
		}
	}

	fn handle_worker_spawned(&mut self, idle: IdleWorker, handle: WorkerHandle) {
		self.workers.spawn_inflight -= 1;
		let worker = self.workers.running.insert(WorkerData { idle: Some(idle), handle });

		tracing::debug!(target: LOG_TARGET, ?worker, "precheck worker spawned");

		if let Some(request) = self.queue.pop_front() {
			self.assign(worker, request);
		}
	}

	fn handle_job_finished(&mut self, worker: Worker, outcome: Outcome, artifact_id: ArtifactId) {
		let (result, idle_worker) = match outcome {
			Outcome::Ok { precheck_result, idle_worker } => (precheck_result, Some(idle_worker)),
			Outcome::TimedOut => (Err(PrecheckError::TimedOut), None),
			Outcome::IoErr(err) => (Err(PrecheckError::Internal(err.to_string())), None),
		};

		let waiting_for_response = match self.artifacts.remove(&artifact_id) {
			Some(PrecheckStatus::InProgress { waiting_for_response }) => waiting_for_response,
			Some(PrecheckStatus::Done(_)) => {
				always_assert::never!(
					"every request is processed exactly once;
                    the `Done` status is set when the worker finishes the job;
                    thus, the status is `InProgress`; qed."
				);
				return
			},
			None => {
				always_assert::never!(
					"before assigning request to the worker, the corresponding
                    artifact id is stored in the map;
                    thus, it cannot be `None`; qed."
				);
				return
			},
		};

		for result_sender in waiting_for_response {
			let _ = result_sender.send(result.clone());
		}

		// Cache the prechecking result.
		self.artifacts.insert(artifact_id, PrecheckStatus::Done(result));

		if let Some(idle_worker) = idle_worker {
			if let Some(data) = self.workers.running.get_mut(worker) {
				data.idle = Some(idle_worker);

				if let Some(request) = self.queue.pop_front() {
					self.assign(worker, request);
				}
			}
		} else {
			// Note it's possible that the worker was purged already by `purge_dead`.
			self.workers.running.remove(worker);

			if !self.queue.is_empty() && self.workers.find_available().is_none() {
				self.spawn_extra_worker();
			}
		}
	}

	async fn purge_dead(&mut self) {
		let mut to_remove = Vec::new();
		for (worker, data) in self.workers.running.iter_mut() {
			if futures::poll!(&mut data.handle).is_ready() {
				// a resolved future means that the worker has terminated. Weed it out.
				to_remove.push(worker);
			}
		}
		for w in to_remove {
			self.workers.running.remove(w);
		}
	}

	fn assign(&mut self, worker: Worker, request: ToQueue) {
		// Queued items still can be duplicated. Before actually assigning it to the worker
		// verify its not present in the artifacts map yet.
		if let Some(status) = self.artifacts.get_mut(&request.pvf.as_artifact_id()) {
			deduplicate_request(status, request);
			return
		}

		let ToQueue { pvf, result_sender } = request;
		tracing::debug!(
			target: LOG_TARGET,
			validation_code_hash = ?pvf.code_hash,
			?worker,
			"assigning the precheck worker",
		);

		let idle = self.workers.claim_idle(worker).expect(
			"this caller must supply a worker which is idle and running;
            thus claim_idle cannot return None; qed.",
		);

		self.artifacts.insert(
			pvf.as_artifact_id(),
			PrecheckStatus::InProgress { waiting_for_response: vec![result_sender] },
		);

		self.mux.push(start_work(worker, idle, self.precheck_timeout, pvf).boxed());
	}

	fn spawn_extra_worker(&mut self) {
		self.mux
			.push(spawn_with_retries(self.program_path.clone(), self.spawn_timeout).boxed());

		self.workers.spawn_inflight += 1;
	}
}

async fn spawn_with_retries(program_path: PathBuf, spawn_timeout: Duration) -> QueueEvent {
	loop {
		match super::worker::spawn(&program_path, spawn_timeout).await {
			Ok((idle, handle)) => break QueueEvent::Spawn(idle, handle),
			Err(err) => {
				tracing::warn!(target: LOG_TARGET, "failed to spawn an precheck worker: {:?}", err);

				// Assume that the failure intermittent and retry after a delay.
				async_std::task::sleep(Duration::from_secs(3)).await;
			},
		}
	}
}

async fn start_work(
	worker: Worker,
	idle: IdleWorker,
	precheck_timeout: Duration,
	pvf: Pvf,
) -> QueueEvent {
	let artifact_id = pvf.as_artifact_id();
	let outcome = super::worker::start_work(idle, pvf, precheck_timeout).await;
	QueueEvent::StartWork { worker, outcome, artifact_id }
}

pub fn start(
	program_path: PathBuf,
	precheck_timeout: Duration,
	spawn_timeout: Duration,
	worker_capacity: usize,
) -> (mpsc::Sender<ToQueue>, impl Future<Output = ()>) {
	let (to_queue_tx, to_queue_rx) = mpsc::channel(32);
	let run =
		Queue::new(to_queue_rx, program_path, precheck_timeout, spawn_timeout, worker_capacity)
			.run();
	(to_queue_tx, run)
}
