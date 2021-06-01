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

use crate::{
	worker_common::{IdleWorker, WorkerHandle},
	LOG_TARGET,
};
use super::{
	worker::{self, Outcome},
};
use std::{fmt, sync::Arc, task::Poll, time::Duration};
use async_std::path::{Path, PathBuf};
use futures::{
	Future, FutureExt, StreamExt, channel::mpsc, future::BoxFuture, stream::FuturesUnordered,
};
use slotmap::HopSlotMap;
use assert_matches::assert_matches;
use always_assert::never;

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

	/// If the given worker was started with the background priority, then it will be raised up to
	/// normal priority. Otherwise, it's no-op.
	BumpPriority(Worker),

	/// Request the given worker to start working on the given code.
	///
	/// Once the job either succeeded or failed, a [`FromPool::Concluded`] message will be sent back.
	///
	/// This should not be sent again until the concluded message is received.
	StartWork {
		worker: Worker,
		code: Arc<Vec<u8>>,
		artifact_path: PathBuf,
		background_priority: bool,
	},
}

/// A message sent from pool to its client.
#[derive(Debug)]
pub enum FromPool {
	/// The given worker was just spawned and is ready to be used.
	Spawned(Worker),

	/// The given worker either succeeded or failed the given job. Under any circumstances the
	/// artifact file has been written. The bool says whether the worker ripped.
	Concluded(Worker, bool),

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
					&program_path,
					&cache_path,
					spawn_timeout,
					&mut spawned,
					&mut mux,
					to_pool,
				)
			}
			ev = mux.select_next_some() => break_if_fatal!(handle_mux(&mut from_pool, &mut spawned, ev)),
		}

		break_if_fatal!(purge_dead(&mut from_pool, &mut spawned).await);
	}
}

async fn purge_dead(
	from_pool: &mut mpsc::UnboundedSender<FromPool>,
	spawned: &mut HopSlotMap<Worker, WorkerData>,
) -> Result<(), Fatal> {
	let mut to_remove = vec![];
	for (worker, data) in spawned.iter_mut() {
		if data.idle.is_none() {
			// The idle token is missing, meaning this worker is now occupied: skip it. This is
			// because the worker process is observed by the work task and should it reach the
			// deadline or be terminated it will be handled by the corresponding mux event.
			continue;
		}

		if let Poll::Ready(()) = futures::poll!(&mut data.handle) {
			// a resolved future means that the worker has terminated. Weed it out.
			to_remove.push(worker);
		}
	}
	for w in to_remove {
		let _ = spawned.remove(w);
		reply(from_pool, FromPool::Rip(w))?;
	}
	Ok(())
}

fn handle_to_pool(
	program_path: &Path,
	cache_path: &Path,
	spawn_timeout: Duration,
	spawned: &mut HopSlotMap<Worker, WorkerData>,
	mux: &mut Mux,
	to_pool: ToPool,
) {
	match to_pool {
		ToPool::Spawn => {
			mux.push(spawn_worker_task(program_path.to_owned(), spawn_timeout).boxed());
		}
		ToPool::StartWork {
			worker,
			code,
			artifact_path,
			background_priority,
		} => {
			if let Some(data) = spawned.get_mut(worker) {
				if let Some(idle) = data.idle.take() {
					mux.push(
						start_work_task(
							worker,
							idle,
							code,
							cache_path.to_owned(),
							artifact_path,
							background_priority
						)
						.boxed(),
					);
				} else {
					// idle token is present after spawn and after a job is concluded;
					// the precondition for `StartWork` is it should be sent only if all previous work
					// items concluded;
					// thus idle token is Some;
					// qed.
					never!("unexpected abscence of the idle token in prepare pool");
				}
			} else {
				// That's a relatively normal situation since the queue may send `start_work` and
				// before receiving it the pool would report that the worker died.
			}
		}
		ToPool::Kill(worker) => {
			// It may be absent if it were previously already removed by `purge_dead`.
			let _ = spawned.remove(worker);
		}
		ToPool::BumpPriority(worker) => {
			if let Some(data) = spawned.get(worker) {
				worker::bump_priority(&data.handle);
			}
		}
	}
}

async fn spawn_worker_task(program_path: PathBuf, spawn_timeout: Duration) -> PoolEvent {
	use futures_timer::Delay;

	loop {
		match worker::spawn(&program_path, spawn_timeout).await {
			Ok((idle, handle)) => break PoolEvent::Spawn(idle, handle),
			Err(err) => {
				tracing::warn!(
					target: LOG_TARGET,
					"failed to spawn a prepare worker: {:?}",
					err,
				);

				// Assume that the failure intermittent and retry after a delay.
				Delay::new(Duration::from_secs(3)).await;
			}
		}
	}
}

async fn start_work_task(
	worker: Worker,
	idle: IdleWorker,
	code: Arc<Vec<u8>>,
	cache_path: PathBuf,
	artifact_path: PathBuf,
	background_priority: bool,
) -> PoolEvent {
	let outcome =
		worker::start_work(idle, code, &cache_path, artifact_path, background_priority).await;
	PoolEvent::StartWork(worker, outcome)
}

fn handle_mux(
	from_pool: &mut mpsc::UnboundedSender<FromPool>,
	spawned: &mut HopSlotMap<Worker, WorkerData>,
	event: PoolEvent,
) -> Result<(), Fatal> {
	match event {
		PoolEvent::Spawn(idle, handle) => {
			let worker = spawned.insert(WorkerData {
				idle: Some(idle),
				handle,
			});

			reply(from_pool, FromPool::Spawned(worker))?;

			Ok(())
		}
		PoolEvent::StartWork(worker, outcome) => {
			match outcome {
				Outcome::Concluded(idle) => {
					let data = match spawned.get_mut(worker) {
						None => {
							// Perhaps the worker was killed meanwhile and the result is no longer
							// relevant.
							return Ok(());
						}
						Some(data) => data,
					};

					// We just replace the idle worker that was loaned from this option during
					// the work starting.
					let old = data.idle.replace(idle);
					assert_matches!(old, None, "attempt to overwrite an idle worker");

					reply(from_pool, FromPool::Concluded(worker, false))?;

					Ok(())
				}
				Outcome::DidntMakeIt => {
					if let Some(_data) = spawned.remove(worker) {
						reply(from_pool, FromPool::Concluded(worker, true))?;
					}

					Ok(())
				}
			}
		}
	}
}

fn reply(from_pool: &mut mpsc::UnboundedSender<FromPool>, m: FromPool) -> Result<(), Fatal> {
	from_pool.unbounded_send(m).map_err(|_| Fatal)
}

/// Spins up the pool and returns the future that should be polled to make the pool functional.
pub fn start(
	program_path: PathBuf,
	cache_path: PathBuf,
	spawn_timeout: Duration,
) -> (
	mpsc::Sender<ToPool>,
	mpsc::UnboundedReceiver<FromPool>,
	impl Future<Output = ()>,
) {
	let (to_pool_tx, to_pool_rx) = mpsc::channel(10);
	let (from_pool_tx, from_pool_rx) = mpsc::unbounded();

	let run = run(Pool {
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
