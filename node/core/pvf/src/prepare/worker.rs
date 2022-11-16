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
	sync::Mutex,
	task,
};
use cpu_time::ProcessTime;
use parity_scale_codec::{Decode, Encode};
use sp_core::hexdisplay::HexDisplay;
use std::{panic, sync::Arc, time::Duration};

/// A multiple of the preparation timeout (in CPU time) for which we are willing to wait on the host
/// (in wall clock time). This is lenient because CPU time may go slower than wall clock time.
const PREPARATION_TIMEOUT_WALL_CLOCK_FACTOR: u32 = 4;

/// Some allowed overhead that we account for in the "CPU time monitor" thread's sleeps, on the
/// child process.
const PREPARATION_TIMEOUT_OVERHEAD: Duration = Duration::from_millis(50);

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
		// worker may get killed, or something along these lines.
		//
		// In that case we should propagate the error to the pool.

		#[derive(Debug)]
		enum Selected {
			Done(PrepareResult),
			IoErr,
			Deadline,
		}

		// We use a generous timeout here. This is in addition to the one in the child process, in
		// case the child stalls. We have a wall clock timeout here in the host, but a CPU timeout
		// in the child. We want to use CPU time because it varies less than wall clock time under
		// load, but the CPU resources of the child can only be measured from the parent after the
		// child process terminates.
		let timeout = preparation_timeout * PREPARATION_TIMEOUT_WALL_CLOCK_FACTOR;
		let result = async_std::future::timeout(timeout, framed_recv(&mut stream)).await;

		let selected = match result {
			// TODO: This case is really long, refactor.
			Ok(Ok(response_bytes)) => {
				// Received bytes from worker within the time limit.
				// By convention we expect encoded `PrepareResult`.
				if let Ok(result) = PrepareResult::decode(&mut response_bytes.as_slice()) {
					if let Ok(cpu_time_elapsed) = result {
						if cpu_time_elapsed > preparation_timeout {
							// The job didn't complete within the timeout.
							gum::debug!(
								target: LOG_TARGET,
								worker_pid = %pid,
								"child took {}ms cpu_time, exceeded preparation timeout {}ms",
								cpu_time_elapsed.as_millis(),
								preparation_timeout.as_millis()
							);

							// Return a timeout error. The artifact exists, but is located in a
							// temporary file which will be cleared.
							Selected::Deadline
						} else {
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
					} else {
						Selected::Done(result)
					}
				} else {
					// We received invalid bytes from the worker.
					let bound_bytes = &response_bytes[..response_bytes.len().min(4)];
					gum::warn!(
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
				gum::warn!(
					target: LOG_TARGET,
					worker_pid = %pid,
					"failed to recv a prepare response: {:?}",
					err,
				);
				Selected::IoErr
			},
			Err(_) => {
				// Timed out.
				gum::warn!(
					target: LOG_TARGET,
					worker_pid = %pid,
					"did not recv a prepare response within the time limit",
				);
				Selected::Deadline
			},
		};

		match selected {
			// Timed out. This should already be logged by the child.
			Selected::Done(Err(PrepareError::TimedOut)) => Outcome::TimedOut,
			Selected::Done(result) =>
				Outcome::Concluded { worker: IdleWorker { stream, pid }, result },
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

			// Create a shared Mutex. We lock it when either thread finishes and set the flag.
			let mutex = Arc::new(Mutex::new(false));

			let cpu_time_start = ProcessTime::now();

			// Spawn a new thread that runs the CPU time monitor. Continuously wakes up from
			// sleeping and then either sleeps for the remaining CPU time, or kills the process if
			// we exceed the CPU timeout.
			// TODO: Use a threadpool?
			let (stream_2, cpu_time_start_2, preparation_timeout_2, mutex_2) =
				(stream.clone(), cpu_time_start, preparation_timeout, mutex.clone());
			std::thread::Builder::new().name("CPU time monitor".into()).spawn(move || {
				task::block_on(async {
					cpu_time_monitor_loop(
						stream_2,
						cpu_time_start_2,
						preparation_timeout_2,
						mutex_2,
					)
					.await;
				})
			})?;

			// Prepares the artifact in a separate thread. On error, the serialized error will be
			// written into the socket.
			let result = match prepare_artifact(&code) {
				Err(err) => {
					// Serialized error will be written into the socket.
					Err(err)
				},
				Ok(compiled_artifact) => {
					let cpu_time_elapsed = cpu_time_start.elapsed();

					let mut lock = mutex.lock().await;
					if *lock {
						unreachable!(
							"Hit the timed-out case first; \
								  Set the mutex flag to true; \
								  Called process::exit(); \
								  process::exit: 'This function will never return and \
								  will immediately terminate the current process.'; \
								  This is unreachable; qed"
						);
					}
					*lock = true;

					// Write the serialized artifact into a temp file.
					// PVF host only keeps artifacts statuses in its memory,
					// successfully compiled code gets stored on the disk (and
					// consequently deserialized by execute-workers). The prepare
					// worker is only required to send an empty `Ok` to the pool
					// to indicate the success.

					gum::debug!(
						target: LOG_TARGET,
						worker_pid = %std::process::id(),
						"worker: writing artifact to {}",
						dest.display(),
					);
					async_std::fs::write(&dest, &compiled_artifact).await?;

					// TODO: We are now sending the CPU time back, changing the expected interface. Do
					// we need to account for this breaking change in any way?
					Ok(cpu_time_elapsed)
				},
			};

			framed_send(&mut stream, result.encode().as_slice()).await?;
		}
	});
}

fn prepare_artifact(code: &[u8]) -> Result<CompiledArtifact, PrepareError> {
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

/// Loop that runs in the CPU time monitor thread. Continuously wakes up from sleeping and then
/// either sleeps for the remaining CPU time, or kills the process if we exceed the CPU timeout.
///
/// NOTE: Killed processes are detected and cleaned up in `purge_dead`.
///
/// NOTE: If the artifact preparation completes and this thread is still sleeping, it will continue
/// sleeping in the background. When it wakes, it will see that the mutex flag has been set and
/// return.
async fn cpu_time_monitor_loop(
	mut stream: UnixStream,
	cpu_time_start: ProcessTime,
	preparation_timeout: Duration,
	mutex: Arc<Mutex<bool>>,
) {
	loop {
		let cpu_time_elapsed = cpu_time_start.elapsed();

		// Treat the timeout as CPU time, which is less subject to variance due to load.
		if cpu_time_elapsed > preparation_timeout {
			let mut lock = mutex.lock().await;
			if *lock {
				// Hit the preparation-completed case first, return from this thread.
				return
			}
			*lock = true;

			// TODO: Log if we exceed the timeout.

			// Send back a PrepareError::TimedOut on timeout.
			let result: Result<(), PrepareError> = Err(PrepareError::TimedOut);
			// If we error there is nothing we can do here, and we are killing the process,
			// anyway. The receiving side will just have to time out.
			let _ = framed_send(&mut stream, result.encode().as_slice()).await;

			// Kill the process.
			std::process::exit(1);
		}

		// Sleep for the remaining CPU time, plus a bit to account for overhead. Note that the sleep
		// is wall clock time. The CPU clock may be slower than the wall clock.
		let sleep_interval = preparation_timeout - cpu_time_elapsed + PREPARATION_TIMEOUT_OVERHEAD;
		std::thread::sleep(sleep_interval);
	}
}
