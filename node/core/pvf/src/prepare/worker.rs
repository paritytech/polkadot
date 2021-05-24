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
	LOG_TARGET,
	artifacts::Artifact,
	worker_common::{
		IdleWorker, SpawnErr, WorkerHandle, bytes_to_path, framed_recv, framed_send, path_to_bytes,
		spawn_with_program_path, tmpfile, worker_event_loop,
	},
};
use async_std::{
	io,
	os::unix::net::UnixStream,
	path::{PathBuf, Path},
};
use futures::FutureExt as _;
use futures_timer::Delay;
use std::{sync::Arc, time::Duration};

const NICENESS_BACKGROUND: i32 = 10;
const NICENESS_FOREGROUND: i32 = 0;

const COMPILATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Spawns a new worker with the given program path that acts as the worker and the spawn timeout.
///
/// The program should be able to handle `<program-path> prepare-worker <socket-path>` invocation.
pub async fn spawn(
	program_path: &Path,
	spawn_timeout: Duration,
) -> Result<(IdleWorker, WorkerHandle), SpawnErr> {
	spawn_with_program_path(
		"prepare",
		program_path,
		&["prepare-worker"],
		spawn_timeout,
	)
	.await
}

pub enum Outcome {
	/// The worker has finished the work assigned to it.
	Concluded(IdleWorker),
	/// The execution was interrupted abruptly and the worker is not available anymore. For example,
	/// this could've happen because the worker hadn't finished the work until the given deadline.
	///
	/// Note that in this case the artifact file is written (unless there was an error writing the
	/// the artifact).
	///
	/// This doesn't return an idle worker instance, thus this worker is no longer usable.
	DidntMakeIt,
}

/// Given the idle token of a worker and parameters of work, communicates with the worker and
/// returns the outcome.
pub async fn start_work(
	worker: IdleWorker,
	code: Arc<Vec<u8>>,
	artifact_path: PathBuf,
	background_priority: bool,
) -> Outcome {
	let IdleWorker { mut stream, pid } = worker;

	tracing::debug!(
		target: LOG_TARGET,
		worker_pid = %pid,
		%background_priority,
		"starting prepare for {}",
		artifact_path.display(),
	);

	if background_priority {
		renice(pid, NICENESS_BACKGROUND);
	}

	if let Err(err) = send_request(&mut stream, code).await {
		tracing::warn!("failed to send a prepare request to pid={}: {:?}", pid, err);
		return Outcome::DidntMakeIt;
	}

	// Wait for the result from the worker, keeping in mind that there may be a timeout, the
	// worker may get killed, or something along these lines.
	//
	// In that case we should handle these gracefully by writing the artifact file by ourselves.
	// We may potentially overwrite the artifact in rare cases where the worker didn't make
	// it to report back the result.

	enum Selected {
		Done,
		IoErr,
		Deadline,
	}

	let selected = futures::select! {
		artifact_path_bytes = framed_recv(&mut stream).fuse() => {
			match artifact_path_bytes {
				Ok(bytes) => {
					if let Some(tmp_path) = bytes_to_path(&bytes) {
						async_std::fs::rename(tmp_path, &artifact_path)
							.await
							.map(|_| Selected::Done)
							.unwrap_or(Selected::IoErr)
					} else {
						Selected::IoErr
					}
				},
				Err(_) => Selected::IoErr,
			}
		},
		_ = Delay::new(COMPILATION_TIMEOUT).fuse() => Selected::Deadline,
	};

	match selected {
		Selected::Done => {
			renice(pid, NICENESS_FOREGROUND);
			Outcome::Concluded(IdleWorker { stream, pid })
		}
		Selected::IoErr | Selected::Deadline => {
			let bytes = Artifact::DidntMakeIt.serialize();
			// best effort: there is nothing we can do here if the write fails.
			let _ = async_std::fs::write(&artifact_path, &bytes).await;
			Outcome::DidntMakeIt
		}
	}
}

async fn send_request(stream: &mut UnixStream, code: Arc<Vec<u8>>) -> io::Result<()> {
	framed_send(stream, &*code).await
}

async fn recv_request(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
	framed_recv(stream).await
}

pub fn bump_priority(handle: &WorkerHandle) {
	let pid = handle.id();
	renice(pid, NICENESS_FOREGROUND);
}

fn renice(pid: u32, niceness: i32) {
	tracing::debug!(
		target: LOG_TARGET,
		worker_pid = %pid,
		"changing niceness to {}",
		niceness,
	);

	// Consider upstreaming this to the `nix` crate.
	unsafe {
		if -1 == libc::setpriority(libc::PRIO_PROCESS, pid, niceness) {
			let err = std::io::Error::last_os_error();
			tracing::warn!(target: LOG_TARGET, "failed to set the priority: {:?}", err,);
		}
	}
}

/// The entrypoint that the spawned prepare worker should start with. The socket_path specifies
/// the path to the socket used to communicate with the host.
pub fn worker_entrypoint(socket_path: &str) {
	worker_event_loop("prepare", socket_path, |mut stream| async move {
		loop {
			let code = recv_request(&mut stream).await?;

			tracing::debug!(
				target: LOG_TARGET,
				worker_pid = %std::process::id(),
				"worker: preparing artifact",
			);
			let artifact_bytes = prepare_artifact(&code).serialize();

			// Write the serialized artifact into into a temp file.
			let dest = tmpfile("prepare-artifact-").await?;
			tracing::debug!(
				target: LOG_TARGET,
				worker_pid = %std::process::id(),
				"worker: writing artifact to {}",
				dest.display(),
			);
			async_std::fs::write(&dest, &artifact_bytes).await?;

			// Communicate the results back to the host.
			framed_send(&mut stream, &path_to_bytes(&dest)).await?;
		}
	});
}

fn prepare_artifact(code: &[u8]) -> Artifact {
	let blob = match crate::executor_intf::prevalidate(code) {
		Err(err) => {
			return Artifact::PrevalidationErr(format!("{:?}", err));
		}
		Ok(b) => b,
	};

	match crate::executor_intf::prepare(blob) {
		Ok(compiled_artifact) => Artifact::Compiled { compiled_artifact },
		Err(err) => Artifact::PreparationErr(format!("{:?}", err)),
	}
}
