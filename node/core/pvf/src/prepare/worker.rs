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
		bytes_to_path, cpu_time_monitor_loop, framed_recv, framed_send, path_to_bytes,
		spawn_with_program_path, tmpfile_in, worker_event_loop, IdleWorker, JobKind, SpawnErr,
		WorkerHandle, JOB_TIMEOUT_WALL_CLOCK_FACTOR,
	},
	LOG_TARGET,
};
use async_std::{
	io,
	os::unix::net::UnixStream,
	path::{Path, PathBuf},
	task,
};
use cpu_time::ProcessTime;
use parity_scale_codec::{Decode, Encode};
use sp_core::hexdisplay::HexDisplay;
use std::{
	panic,
	sync::{
		atomic::{AtomicBool, Ordering},
		Arc,
	},
	thread,
	time::Duration,
};

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

#[derive(Debug)]
enum Selected {
	Done(PrepareResult),
	IoErr,
	Deadline,
}

/// Given the idle token of a worker and parameters of work, communicates with the worker and
/// returns the outcome.
///
/// NOTE: Returning the `TimedOut` or `DidNotMakeIt` errors will trigger the child process being
/// killed.
pub async fn start_work(
	worker: IdleWorker,
	code: Arc<Vec<u8>>,
	cache_path: &Path,
	artifact_path: PathBuf,
	preparation_timeout: Duration,
) -> Outcome {
	let IdleWorker { mut stream, pid } = worker;

	gum::debug!(
		target: LOG_TARGET,
		worker_pid = %pid,
		"starting prepare for {}",
		artifact_path.display(),
	);

	with_tmp_file(pid, cache_path, |tmp_file| async move {
		if let Err(err) = send_request(&mut stream, code, &tmp_file, preparation_timeout).await {
			gum::warn!(
				target: LOG_TARGET,
				worker_pid = %pid,
				"failed to send a prepare request: {:?}",
				err,
			);
			return Outcome::Unreachable
		}

		// Wait for the result from the worker, keeping in mind that there may be a timeout, the
		// worker may get killed, or something along these lines. In that case we should propagate
		// the error to the pool.
		//
		// We use a generous timeout here. This is in addition to the one in the child process, in
		// case the child stalls. We have a wall clock timeout here in the host, but a CPU timeout
		// in the child. We want to use CPU time because it varies less than wall clock time under
		// load, but the CPU resources of the child can only be measured from the parent after the
		// child process terminates.
		let timeout = preparation_timeout * JOB_TIMEOUT_WALL_CLOCK_FACTOR;
		let result = async_std::future::timeout(timeout, framed_recv(&mut stream)).await;

		let selected = match result {
			// Received bytes from worker within the time limit.
			Ok(Ok(response_bytes)) =>
				handle_response_bytes(
					response_bytes,
					pid,
					tmp_file,
					artifact_path,
					preparation_timeout,
				)
				.await,
			Ok(Err(err)) => {
				// Communication error within the time limit.
				gum::warn!(
					target: LOG_TARGET,
					worker_pid = %pid,
					"failed to recv a prepare response: {:?}",
					err,
				);
				Selected::IoErr
			},
			Err(_) => {
				// Timed out here on the host.
				gum::warn!(
					target: LOG_TARGET,
					worker_pid = %pid,
					"did not recv a prepare response within the time limit",
				);
				Selected::Deadline
			},
		};

		// NOTE: A `TimedOut` or `DidNotMakeIt` error triggers the child process being killed.
		match selected {
			// Timed out on the child. This should already be logged by the child.
			Selected::Done(Err(PrepareError::TimedOut)) => Outcome::TimedOut,
			Selected::Done(result) =>
				Outcome::Concluded { worker: IdleWorker { stream, pid }, result },
			Selected::Deadline => Outcome::TimedOut,
			Selected::IoErr => Outcome::DidNotMakeIt,
		}
	})
	.await
}

/// Handles the case where we successfully received response bytes on the host from the child.
///
/// NOTE: Here we know the artifact exists, but is still located in a temporary file which will be
/// cleared by `with_tmp_file`.
async fn handle_response_bytes(
	response_bytes: Vec<u8>,
	pid: u32,
	tmp_file: PathBuf,
	artifact_path: PathBuf,
	preparation_timeout: Duration,
) -> Selected {
	// By convention we expect encoded `PrepareResult`.
	let result = match PrepareResult::decode(&mut response_bytes.as_slice()) {
		Ok(result) => result,
		Err(_) => {
			// We received invalid bytes from the worker.
			let bound_bytes = &response_bytes[..response_bytes.len().min(4)];
			gum::warn!(
				target: LOG_TARGET,
				worker_pid = %pid,
				"received unexpected response from the prepare worker: {}",
				HexDisplay::from(&bound_bytes),
			);
			return Selected::IoErr
		},
	};
	let cpu_time_elapsed = match result {
		Ok(result) => result,
		Err(_) => return Selected::Done(result),
	};

	if cpu_time_elapsed > preparation_timeout {
		// The job didn't complete within the timeout.
		gum::warn!(
			target: LOG_TARGET,
			worker_pid = %pid,
			"prepare job took {}ms cpu time, exceeded preparation timeout {}ms. Clearing WIP artifact {}",
			cpu_time_elapsed.as_millis(),
			preparation_timeout.as_millis(),
			tmp_file.display(),
		);

		// Return a timeout error.
		return Selected::Deadline
	}

	gum::debug!(
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
			gum::warn!(
				target: LOG_TARGET,
				worker_pid = %pid,
				"failed to rename the artifact from {} to {}: {:?}",
				tmp_file.display(),
				artifact_path.display(),
				err,
			);
			Selected::IoErr
		})
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
			gum::warn!(
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
			gum::warn!(
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
	preparation_timeout: Duration,
) -> io::Result<()> {
	framed_send(stream, &code).await?;
	framed_send(stream, path_to_bytes(tmp_file)).await?;
	framed_send(stream, &preparation_timeout.encode()).await?;
	Ok(())
}

async fn recv_request(stream: &mut UnixStream) -> io::Result<(Vec<u8>, PathBuf, Duration)> {
	let code = framed_recv(stream).await?;
	let tmp_file = framed_recv(stream).await?;
	let tmp_file = bytes_to_path(&tmp_file).ok_or_else(|| {
		io::Error::new(
			io::ErrorKind::Other,
			"prepare pvf recv_request: non utf-8 artifact path".to_string(),
		)
	})?;
	let preparation_timeout = framed_recv(stream).await?;
	let preparation_timeout = Duration::decode(&mut &preparation_timeout[..]).map_err(|_| {
		io::Error::new(
			io::ErrorKind::Other,
			"prepare pvf recv_request: failed to decode duration".to_string(),
		)
	})?;
	Ok((code, tmp_file, preparation_timeout))
}

/// The entrypoint that the spawned prepare worker should start with. The `socket_path` specifies
/// the path to the socket used to communicate with the host.
pub fn worker_entrypoint(socket_path: &str) {
	worker_event_loop("prepare", socket_path, |mut stream| async move {
		loop {
			let (code, dest, preparation_timeout) = recv_request(&mut stream).await?;

			gum::debug!(
				target: LOG_TARGET,
				worker_pid = %std::process::id(),
				"worker: preparing artifact",
			);

			// Create a lock flag. We set it when either thread finishes.
			let lock = Arc::new(AtomicBool::new(false));
			let cpu_time_start = ProcessTime::now();

			// Spawn a new thread that runs the CPU time monitor. Continuously wakes up from
			// sleeping and then either sleeps for the remaining CPU time, or kills the process if
			// we exceed the CPU timeout.
			let (stream_2, cpu_time_start_2, preparation_timeout_2, lock_2) =
				(stream.clone(), cpu_time_start, preparation_timeout, lock.clone());
			let handle =
				thread::Builder::new().name("CPU time monitor".into()).spawn(move || {
					task::block_on(async {
						cpu_time_monitor_loop(
							JobKind::Prepare,
							stream_2,
							cpu_time_start_2,
							preparation_timeout_2,
							lock_2,
						)
						.await;
					})
				})?;

			// Prepares the artifact in a separate thread.
			let result = match prepare_artifact(&code).await {
				Err(err) => {
					// Serialized error will be written into the socket.
					Err(err)
				},
				Ok(compiled_artifact) => {
					let cpu_time_elapsed = cpu_time_start.elapsed();

					let lock_result =
						lock.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed);
					if lock_result.is_err() {
						// The other thread is still sending an error response over the socket. Wait on it and
						// return.
						let _ = handle.join();
						// Monitor thread detected timeout and likely already terminated the
						// process, nothing to do.
						continue
					}

					// Write the serialized artifact into a temp file.
					//
					// PVF host only keeps artifacts statuses in its memory, successfully compiled code gets stored
					// on the disk (and consequently deserialized by execute-workers). The prepare worker is only
					// required to send `Ok` to the pool to indicate the success.

					gum::debug!(
						target: LOG_TARGET,
						worker_pid = %std::process::id(),
						"worker: writing artifact to {}",
						dest.display(),
					);
					async_std::fs::write(&dest, &compiled_artifact).await?;

					Ok(cpu_time_elapsed)
				},
			};

			framed_send(&mut stream, result.encode().as_slice()).await?;
		}
	});
}

async fn prepare_artifact(code: &[u8]) -> Result<CompiledArtifact, PrepareError> {
	panic::catch_unwind(|| {
		let blob = match crate::executor_intf::prevalidate(code) {
			Err(err) => return Err(PrepareError::Prevalidation(format!("{:?}", err))),
			Ok(b) => b,
		};

		match crate::executor_intf::prepare(blob) {
			Ok(compiled_artifact) => Ok(CompiledArtifact::new(compiled_artifact)),
			Err(err) => Err(PrepareError::Preparation(format!("{:?}", err))),
		}
	})
	.map_err(|panic_payload| {
		PrepareError::Panic(crate::error::stringify_panic_payload(panic_payload))
	})
	.and_then(|inner_result| inner_result)
}
