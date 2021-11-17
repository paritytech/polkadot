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
	artifacts::CompiledArtifact,
	error::{PrepareError, PrepareResult},
	worker_common::{
		bytes_to_path, framed_recv, framed_send, path_to_bytes, spawn_with_program_path,
		tmpfile_in, worker_event_loop, IdleWorker, SpawnErr, WorkerHandle,
	},
	LOG_TARGET,
};
use async_std::{
	io,
	os::unix::net::UnixStream,
	path::{Path, PathBuf},
};
use parity_scale_codec::{Decode, Encode};
use sp_core::hexdisplay::HexDisplay;
use std::{sync::Arc, time::Duration};

const NICENESS_BACKGROUND: i32 = 10;
const NICENESS_FOREGROUND: i32 = 0;

/// The time period after which the preparation worker is considered unresponsive and will be killed.
// NOTE: If you change this make sure to fix the buckets of `pvf_preparation_time` metric.
const COMPILATION_TIMEOUT: Duration = Duration::from_secs(60);

/// Spawns a new worker with the given program path that acts as the worker and the spawn timeout.
///
/// The program should be able to handle `<program-path> prepare-worker <socket-path>` invocation.
pub async fn spawn(
	program_path: &Path,
	spawn_timeout: Duration,
) -> Result<(IdleWorker, WorkerHandle), SpawnErr> {
	spawn_with_program_path("prepare", program_path, &["prepare-worker"], spawn_timeout).await
}

pub enum Outcome {
	/// The worker has finished the work assigned to it.
	Concluded { worker: IdleWorker, result: PrepareResult },
	/// The host tried to reach the worker but failed. This is most likely because the worked was
	/// killed by the system.
	Unreachable,
	/// The worker failed to finish the job until the given deadline.
	///
	/// The worker is no longer usable and should be killed.
	TimedOut,
	/// The execution was interrupted abruptly and the worker is not available anymore.
	///
	/// This doesn't return an idle worker instance, thus this worker is no longer usable.
	DidNotMakeIt,
}

/// Given the idle token of a worker and parameters of work, communicates with the worker and
/// returns the outcome.
pub async fn start_work(
	worker: IdleWorker,
	code: Arc<Vec<u8>>,
	cache_path: &Path,
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

	with_tmp_file(pid, cache_path, |tmp_file| async move {
		if let Err(err) = send_request(&mut stream, code, &tmp_file).await {
			tracing::warn!(
				target: LOG_TARGET,
				worker_pid = %pid,
				"failed to send a prepare request: {:?}",
				err,
			);
			return Outcome::Unreachable
		}

		// Wait for the result from the worker, keeping in mind that there may be a timeout, the
		// worker may get killed, or something along these lines.
		//
		// In that case we should propagate the error to the pool.

		#[derive(Debug)]
		enum Selected {
			Done(PrepareResult),
			IoErr,
			Deadline,
		}

		let selected =
			match async_std::future::timeout(COMPILATION_TIMEOUT, framed_recv(&mut stream)).await {
				Ok(Ok(response_bytes)) => {
					// Received bytes from worker within the time limit.
					// By convention we expect encoded `PrepareResult`.
					if let Ok(result) = PrepareResult::decode(&mut response_bytes.as_slice()) {
						if result.is_ok() {
							tracing::debug!(
								target: LOG_TARGET,
								worker_pid = %pid,
								"promoting WIP artifact {} to {}",
								tmp_file.display(),
								artifact_path.display(),
							);

							async_std::fs::rename(&tmp_file, &artifact_path)
								.await
								.map(|_| Selected::Done(result))
								.unwrap_or_else(|err| {
									tracing::warn!(
										target: LOG_TARGET,
										worker_pid = %pid,
										"failed to rename the artifact from {} to {}: {:?}",
										tmp_file.display(),
										artifact_path.display(),
										err,
									);
									Selected::IoErr
								})
						} else {
							Selected::Done(result)
						}
					} else {
						// We received invalid bytes from the worker.
						let bound_bytes = &response_bytes[..response_bytes.len().min(4)];
						tracing::warn!(
							target: LOG_TARGET,
							worker_pid = %pid,
							"received unexpected response from the prepare worker: {}",
							HexDisplay::from(&bound_bytes),
						);
						Selected::IoErr
					}
				},
				Ok(Err(err)) => {
					// Communication error within the time limit.
					tracing::warn!(
						target: LOG_TARGET,
						worker_pid = %pid,
						"failed to recv a prepare response: {:?}",
						err,
					);
					Selected::IoErr
				},
				Err(_) => {
					// Timed out.
					Selected::Deadline
				},
			};

		match selected {
			Selected::Done(result) => {
				renice(pid, NICENESS_FOREGROUND);
				Outcome::Concluded { worker: IdleWorker { stream, pid }, result }
			},
			Selected::Deadline => Outcome::TimedOut,
			Selected::IoErr => Outcome::DidNotMakeIt,
		}
	})
	.await
}

/// Create a temporary file for an artifact at the given cache path and execute the given
/// future/closure passing the file path in.
///
/// The function will try best effort to not leave behind the temporary file.
async fn with_tmp_file<F, Fut>(pid: u32, cache_path: &Path, f: F) -> Outcome
where
	Fut: futures::Future<Output = Outcome>,
	F: FnOnce(PathBuf) -> Fut,
{
	let tmp_file = match tmpfile_in("prepare-artifact-", cache_path).await {
		Ok(f) => f,
		Err(err) => {
			tracing::warn!(
				target: LOG_TARGET,
				worker_pid = %pid,
				"failed to create a temp file for the artifact: {:?}",
				err,
			);
			return Outcome::DidNotMakeIt
		},
	};

	let outcome = f(tmp_file.clone()).await;

	// The function called above is expected to move `tmp_file` to a new location upon success. However,
	// the function may as well fail and in that case we should remove the tmp file here.
	//
	// In any case, we try to remove the file here so that there are no leftovers. We only report
	// errors that are different from the `NotFound`.
	match async_std::fs::remove_file(tmp_file).await {
		Ok(()) => (),
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => (),
		Err(err) => {
			tracing::warn!(
				target: LOG_TARGET,
				worker_pid = %pid,
				"failed to remove the tmp file: {:?}",
				err,
			);
		},
	}

	outcome
}

async fn send_request(
	stream: &mut UnixStream,
	code: Arc<Vec<u8>>,
	tmp_file: &Path,
) -> io::Result<()> {
	framed_send(stream, &*code).await?;
	framed_send(stream, path_to_bytes(tmp_file)).await?;
	Ok(())
}

async fn recv_request(stream: &mut UnixStream) -> io::Result<(Vec<u8>, PathBuf)> {
	let code = framed_recv(stream).await?;
	let tmp_file = framed_recv(stream).await?;
	let tmp_file = bytes_to_path(&tmp_file).ok_or_else(|| {
		io::Error::new(
			io::ErrorKind::Other,
			"prepare pvf recv_request: non utf-8 artifact path".to_string(),
		)
	})?;
	Ok((code, tmp_file))
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
			tracing::warn!(target: LOG_TARGET, "failed to set the priority: {:?}", err);
		}
	}
}

/// The entrypoint that the spawned prepare worker should start with. The `socket_path` specifies
/// the path to the socket used to communicate with the host.
pub fn worker_entrypoint(socket_path: &str) {
	worker_event_loop("prepare", socket_path, |mut stream| async move {
		loop {
			let (code, dest) = recv_request(&mut stream).await?;

			tracing::debug!(
				target: LOG_TARGET,
				worker_pid = %std::process::id(),
				"worker: preparing artifact",
			);

			let result = match prepare_artifact(&code) {
				Err(err) => {
					// Serialized error will be written into the socket.
					Err(err)
				},
				Ok(compiled_artifact) => {
					// Write the serialized artifact into a temp file.
					// PVF host only keeps artifacts statuses in its memory,
					// successfully compiled code gets stored on the disk (and
					// consequently deserialized by execute-workers). The prepare
					// worker is only required to send an empty `Ok` to the pool
					// to indicate the success.

					let artifact_bytes = compiled_artifact.encode();

					tracing::debug!(
						target: LOG_TARGET,
						worker_pid = %std::process::id(),
						"worker: writing artifact to {}",
						dest.display(),
					);
					async_std::fs::write(&dest, &artifact_bytes).await?;

					Ok(())
				},
			};

			framed_send(&mut stream, result.encode().as_slice()).await?;
		}
	});
}

fn prepare_artifact(code: &[u8]) -> Result<CompiledArtifact, PrepareError> {
	let blob = match crate::executor_intf::prevalidate(code) {
		Err(err) => return Err(PrepareError::Prevalidation(format!("{:?}", err))),
		Ok(b) => b,
	};

	match crate::executor_intf::prepare(blob) {
		Ok(compiled_artifact) => Ok(CompiledArtifact::new(compiled_artifact)),
		Err(err) => Err(PrepareError::Preparation(format!("{:?}", err))),
	}
}
