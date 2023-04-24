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

use super::worker_intf::{self, Outcome};
use crate::{
	error::{PrepareError, PrepareResult},
	metrics::Metrics,
	pvf::PvfPrepData,
	worker_common::{IdleWorker, WorkerHandle},
	LOG_TARGET,
};
use always_assert::never;
use futures::{
	channel::mpsc, future::BoxFuture, stream::FuturesUnordered, Future, FutureExt, StreamExt,
};
use slotmap::HopSlotMap;
use std::{
	fmt,
	path::{Path, PathBuf},
	task::Poll,
	time::Duration,
};

slotmap::new_key_type! { pub struct Worker; }

/// Messages that the pool handles.
#[derive(Debug, PartialEq, Eq)]
pub enum ToPool {
	/// Request a new worker to spawn.
	///
	/// This request won't fail in case if the worker cannot be created. Instead, we consider
	/// the failures transient and we try to spawn a worker after a delay.
	///
	/// [`FromPool::Spawned`] will be returned as soon as the worker is spawned.
	///
	/// The client should anticipate a [`FromPool::Rip`] message, in case the spawned worker was
	/// stopped for some reason.
	Spawn,

	/// Kill the given worker. No-op if the given worker is not running.
	///
	/// [`FromPool::Rip`] won't be sent in this case. However, the client should be prepared to
	/// receive [`FromPool::Rip`] nonetheless, since the worker may be have been ripped before
	/// this message is processed.
	Kill(Worker),

	/// Request the given worker to start working on the given code.
	///
	/// Once the job either succeeded or failed, a [`FromPool::Concluded`] message will be sent back.
	/// It's also possible that the worker dies before handling the message in which case [`FromPool::Rip`]
	/// will be sent back.
	///
	/// In either case, the worker is considered busy and no further `StartWork` messages should be
	/// sent until either `Concluded` or `Rip` message is received.
	StartWork { worker: Worker, pvf: PvfPrepData, artifact_path: PathBuf },
}

/// A message sent from pool to its client.
#[derive(Debug)]
pub enum FromPool {
	/// The given worker was just spawned and is ready to be used.
	Spawned(Worker),

	/// The given worker either succeeded or failed the given job.
	Concluded {
		/// A key for retrieving the worker data from the pool.
		worker: Worker,
		/// Indicates whether the worker process was killed.
		rip: bool,
		/// [`Ok`] indicates that compiled artifact is successfully stored on disk.
		/// Otherwise, an [error](PrepareError) is supplied.
		result: PrepareResult,
	},

	/// The given worker ceased to exist.
	Rip(Worker),
}

struct WorkerData {
	idle: Option<IdleWorker>,
	handle: WorkerHandle,
}

impl fmt::Debug for WorkerData {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "WorkerData(pid={})", self.handle.id())
	}
}

enum PoolEvent {
	Spawn(IdleWorker, WorkerHandle),
	StartWork(Worker, Outcome),
}

type Mux = FuturesUnordered<BoxFuture<'static, PoolEvent>>;

struct Pool {
	program_path: PathBuf,
	cache_path: PathBuf,
	spawn_timeout: Duration,
	to_pool: mpsc::Receiver<ToPool>,
	from_pool: mpsc::UnboundedSender<FromPool>,
	spawned: HopSlotMap<Worker, WorkerData>,
	mux: Mux,
	metrics: Metrics,
}

/// A fatal error that warrants stopping the event loop of the pool.
struct Fatal;

async fn run(
	Pool {
		program_path,
		cache_path,
		spawn_timeout,
		to_pool,
		mut from_pool,
		mut spawned,
		mut mux,
		metrics,
	}: Pool,
) {
	macro_rules! break_if_fatal {
		($expr:expr) => {
			match $expr {
				Err(Fatal) => break,
				Ok(v) => v,
			}
		};
	}

	let mut to_pool = to_pool.fuse();

	loop {
		futures::select! {
			to_pool = to_pool.next() => {
				let to_pool = break_if_fatal!(to_pool.ok_or(Fatal));
				handle_to_pool(
					&metrics,
					&program_path,
					&cache_path,
					spawn_timeout,
					&mut spawned,
					&mut mux,
					to_pool,
				)
			}
			ev = mux.select_next_some() => {
				break_if_fatal!(handle_mux(&metrics, &mut from_pool, &mut spawned, ev))
			}
		}

		break_if_fatal!(purge_dead(&metrics, &mut from_pool, &mut spawned).await);
	}
}

async fn purge_dead(
	metrics: &Metrics,
	from_pool: &mut mpsc::UnboundedSender<FromPool>,
	spawned: &mut HopSlotMap<Worker, WorkerData>,
) -> Result<(), Fatal> {
	let mut to_remove = vec![];
	for (worker, data) in spawned.iter_mut() {
		if data.idle.is_none() {
			// The idle token is missing, meaning this worker is now occupied: skip it. This is
			// because the worker process is observed by the work task and should it reach the
			// deadline or be terminated it will be handled by the corresponding mux event.
			continue
		}

		if let Poll::Ready(()) = futures::poll!(&mut data.handle) {
			// a resolved future means that the worker has terminated. Weed it out.
			to_remove.push(worker);
		}
	}
	for w in to_remove {
		if attempt_retire(metrics, spawned, w) {
			reply(from_pool, FromPool::Rip(w))?;
		}
	}
	Ok(())
}

fn handle_to_pool(
	metrics: &Metrics,
	program_path: &Path,
	cache_path: &Path,
	spawn_timeout: Duration,
	spawned: &mut HopSlotMap<Worker, WorkerData>,
	mux: &mut Mux,
	to_pool: ToPool,
) {
	match to_pool {
		ToPool::Spawn => {
			gum::debug!(target: LOG_TARGET, "spawning a new prepare worker");
			metrics.prepare_worker().on_begin_spawn();
			mux.push(spawn_worker_task(program_path.to_owned(), spawn_timeout).boxed());
		},
		ToPool::StartWork { worker, pvf, artifact_path } => {
			if let Some(data) = spawned.get_mut(worker) {
				if let Some(idle) = data.idle.take() {
					let preparation_timer = metrics.time_preparation();
					mux.push(
						start_work_task(
							metrics.clone(),
							worker,
							idle,
							pvf,
							cache_path.to_owned(),
							artifact_path,
							preparation_timer,
						)
						.boxed(),
					);
				} else {
					// idle token is present after spawn and after a job is concluded;
					// the precondition for `StartWork` is it should be sent only if all previous work
					// items concluded;
					// thus idle token is Some;
					// qed.
					never!("unexpected absence of the idle token in prepare pool");
				}
			} else {
				// That's a relatively normal situation since the queue may send `start_work` and
				// before receiving it the pool would report that the worker died.
			}
		},
		ToPool::Kill(worker) => {
			gum::debug!(target: LOG_TARGET, ?worker, "killing prepare worker");
			// It may be absent if it were previously already removed by `purge_dead`.
			let _ = attempt_retire(metrics, spawned, worker);
		},
	}
}

async fn spawn_worker_task(program_path: PathBuf, spawn_timeout: Duration) -> PoolEvent {
	use futures_timer::Delay;

	loop {
		match worker_intf::spawn(&program_path, spawn_timeout).await {
			Ok((idle, handle)) => break PoolEvent::Spawn(idle, handle),
			Err(err) => {
				gum::warn!(target: LOG_TARGET, "failed to spawn a prepare worker: {:?}", err);

				// Assume that the failure intermittent and retry after a delay.
				Delay::new(Duration::from_secs(3)).await;
			},
		}
	}
}

async fn start_work_task<Timer>(
	metrics: Metrics,
	worker: Worker,
	idle: IdleWorker,
	pvf: PvfPrepData,
	cache_path: PathBuf,
	artifact_path: PathBuf,
	_preparation_timer: Option<Timer>,
) -> PoolEvent {
	let outcome = worker_intf::start_work(&metrics, idle, pvf, &cache_path, artifact_path).await;
	PoolEvent::StartWork(worker, outcome)
}

fn handle_mux(
	metrics: &Metrics,
	from_pool: &mut mpsc::UnboundedSender<FromPool>,
	spawned: &mut HopSlotMap<Worker, WorkerData>,
	event: PoolEvent,
) -> Result<(), Fatal> {
	match event {
		PoolEvent::Spawn(idle, handle) => {
			metrics.prepare_worker().on_spawned();

			let worker = spawned.insert(WorkerData { idle: Some(idle), handle });

			reply(from_pool, FromPool::Spawned(worker))?;

			Ok(())
		},
		PoolEvent::StartWork(worker, outcome) => {
			// If we receive an outcome that the worker is unreachable or that an error occurred on
			// the worker, we attempt to kill the worker process.
			match outcome {
				Outcome::Concluded { worker: idle, result } =>
					handle_concluded_no_rip(from_pool, spawned, worker, idle, result),
				// Return `Concluded`, but do not kill the worker since the error was on the host side.
				Outcome::CreateTmpFileErr { worker: idle, err } => handle_concluded_no_rip(
					from_pool,
					spawned,
					worker,
					idle,
					Err(PrepareError::CreateTmpFileErr(err)),
				),
				// Return `Concluded`, but do not kill the worker since the error was on the host side.
				Outcome::RenameTmpFileErr { worker: idle, result: _, err } =>
					handle_concluded_no_rip(
						from_pool,
						spawned,
						worker,
						idle,
						Err(PrepareError::RenameTmpFileErr(err)),
					),
				Outcome::Unreachable => {
					if attempt_retire(metrics, spawned, worker) {
						reply(from_pool, FromPool::Rip(worker))?;
					}

					Ok(())
				},
				Outcome::IoErr(err) => {
					if attempt_retire(metrics, spawned, worker) {
						reply(
							from_pool,
							FromPool::Concluded {
								worker,
								rip: true,
								result: Err(PrepareError::IoErr(err)),
							},
						)?;
					}

					Ok(())
				},
				Outcome::TimedOut => {
					if attempt_retire(metrics, spawned, worker) {
						reply(
							from_pool,
							FromPool::Concluded {
								worker,
								rip: true,
								result: Err(PrepareError::TimedOut),
							},
						)?;
					}

					Ok(())
				},
			}
		},
	}
}

fn reply(from_pool: &mut mpsc::UnboundedSender<FromPool>, m: FromPool) -> Result<(), Fatal> {
	from_pool.unbounded_send(m).map_err(|_| Fatal)
}

/// Removes the given worker from the registry if it there. This will lead to dropping and hence
/// to killing the worker process.
///
/// Returns `true` if the worker exists and was removed and the process was killed.
///
/// This function takes care about counting the retired workers metric.
fn attempt_retire(
	metrics: &Metrics,
	spawned: &mut HopSlotMap<Worker, WorkerData>,
	worker: Worker,
) -> bool {
	if spawned.remove(worker).is_some() {
		metrics.prepare_worker().on_retired();
		true
	} else {
		false
	}
}

/// Handles the case where we received a response. There potentially was an error, but not the fault
/// of the worker as far as we know, so the worker should not be killed.
///
/// This function tries to put the idle worker back into the pool and then replies with
/// `FromPool::Concluded` with `rip: false`.
fn handle_concluded_no_rip(
	from_pool: &mut mpsc::UnboundedSender<FromPool>,
	spawned: &mut HopSlotMap<Worker, WorkerData>,
	worker: Worker,
	idle: IdleWorker,
	result: PrepareResult,
) -> Result<(), Fatal> {
	let data = match spawned.get_mut(worker) {
		None => {
			// Perhaps the worker was killed meanwhile and the result is no longer relevant. We
			// already send `Rip` when purging if we detect that the worker is dead.
			return Ok(())
		},
		Some(data) => data,
	};

	// We just replace the idle worker that was loaned from this option during
	// the work starting.
	let old = data.idle.replace(idle);
	never!(
		old.is_some(),
		"old idle worker was taken out when starting work; we only replace it here; qed"
	);

	reply(from_pool, FromPool::Concluded { worker, rip: false, result })?;

	Ok(())
}

/// Spins up the pool and returns the future that should be polled to make the pool functional.
pub fn start(
	metrics: Metrics,
	program_path: PathBuf,
	cache_path: PathBuf,
	spawn_timeout: Duration,
) -> (mpsc::Sender<ToPool>, mpsc::UnboundedReceiver<FromPool>, impl Future<Output = ()>) {
	let (to_pool_tx, to_pool_rx) = mpsc::channel(10);
	let (from_pool_tx, from_pool_rx) = mpsc::unbounded();

	let run = run(Pool {
		metrics,
		program_path,
		cache_path,
		spawn_timeout,
		to_pool: to_pool_rx,
		from_pool: from_pool_tx,
		spawned: HopSlotMap::with_capacity_and_key(20),
		mux: Mux::new(),
	});

	(to_pool_tx, from_pool_rx, run)
}
