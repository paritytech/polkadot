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

#[cfg(any(target_os = "linux", feature = "jemalloc-allocator"))]
use super::memory_stats::memory_tracker::{get_memory_tracker_loop_stats, memory_tracker_loop};
use super::memory_stats::{get_max_rss_thread, observe_memory_metrics, MemoryStats};
use crate::{
	artifacts::CompiledArtifact,
	error::{PrepareError, PrepareResult},
	metrics::Metrics,
	worker_common::{
		bytes_to_path, cpu_time_monitor_loop, framed_recv, framed_send, path_to_bytes,
		spawn_with_program_path, tmpfile_in, worker_event_loop, IdleWorker, SpawnErr, WorkerHandle,
		JOB_TIMEOUT_WALL_CLOCK_FACTOR,
	},
	LOG_TARGET,
};
use cpu_time::ProcessTime;
use futures::{pin_mut, select_biased, FutureExt};
use parity_scale_codec::{Decode, Encode};
use polkadot_primitives::vstaging::ExecutorParams;
use sp_core::hexdisplay::HexDisplay;
use std::{
	panic,
	path::{Path, PathBuf},
	sync::{mpsc::channel, Arc},
	time::Duration,
};
use tokio::{io, net::UnixStream};

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
	/// The temporary file for the artifact could not be created at the given cache path.
	CreateTmpFileErr { worker: IdleWorker, err: String },
	/// The response from the worker is received, but the file cannot be renamed (moved) to the
	/// final destination location.
	RenameTmpFileErr { worker: IdleWorker, result: PrepareResult, err: String },
	/// The worker failed to finish the job until the given deadline.
	///
	/// The worker is no longer usable and should be killed.
	TimedOut,
	/// An IO error occurred while receiving the result from the worker process.
	///
	/// This doesn't return an idle worker instance, thus this worker is no longer usable.
	IoErr(String),
}

/// Given the idle token of a worker and parameters of work, communicates with the worker and
/// returns the outcome.
///
/// NOTE: Returning the `TimedOut`, `IoErr` or `Unreachable` outcomes will trigger the child process
/// being killed.
pub async fn start_work(
	metrics: &Metrics,
	worker: IdleWorker,
	code: Arc<Vec<u8>>,
	cache_path: &Path,
	artifact_path: PathBuf,
	executor_params: ExecutorParams,
	preparation_timeout: Duration,
) -> Outcome {
	let IdleWorker { stream, pid } = worker;

	gum::debug!(
		target: LOG_TARGET,
		worker_pid = %pid,
		"starting prepare for {}",
		artifact_path.display(),
	);

	with_tmp_file(stream, pid, cache_path, |tmp_file, mut stream| async move {
		if let Err(err) =
			send_request(&mut stream, code, &tmp_file, &executor_params, preparation_timeout).await
		{
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
		let result = tokio::time::timeout(timeout, recv_response(&mut stream, pid)).await;

		match result {
			// Received bytes from worker within the time limit.
			Ok(Ok((prepare_result, memory_stats))) =>
				handle_response(
					metrics,
					IdleWorker { stream, pid },
					prepare_result,
					memory_stats,
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
				Outcome::IoErr(err.to_string())
			},
			Err(_) => {
				// Timed out here on the host.
				gum::warn!(
					target: LOG_TARGET,
					worker_pid = %pid,
					"did not recv a prepare response within the time limit",
				);
				Outcome::TimedOut
			},
		}
	})
	.await
}

/// Handles the case where we successfully received response bytes on the host from the child.
///
/// NOTE: Here we know the artifact exists, but is still located in a temporary file which will be
/// cleared by `with_tmp_file`.
async fn handle_response(
	metrics: &Metrics,
	worker: IdleWorker,
	result: PrepareResult,
	memory_stats: Option<MemoryStats>,
	pid: u32,
	tmp_file: PathBuf,
	artifact_path: PathBuf,
	preparation_timeout: Duration,
) -> Outcome {
	let cpu_time_elapsed = match result {
		Ok(result) => result,
		// Timed out on the child. This should already be logged by the child.
		Err(PrepareError::TimedOut) => return Outcome::TimedOut,
		Err(_) => return Outcome::Concluded { worker, result },
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
		return Outcome::TimedOut
	}

	gum::debug!(
		target: LOG_TARGET,
		worker_pid = %pid,
		"promoting WIP artifact {} to {}",
		tmp_file.display(),
		artifact_path.display(),
	);

	let outcome = match tokio::fs::rename(&tmp_file, &artifact_path).await {
		Ok(()) => Outcome::Concluded { worker, result },
		Err(err) => {
			gum::warn!(
				target: LOG_TARGET,
				worker_pid = %pid,
				"failed to rename the artifact from {} to {}: {:?}",
				tmp_file.display(),
				artifact_path.display(),
				err,
			);
			Outcome::RenameTmpFileErr { worker, result, err: format!("{:?}", err) }
		},
	};

	// If there were no errors up until now, log the memory stats for a successful preparation, if
	// available.
	if let Some(memory_stats) = memory_stats {
		observe_memory_metrics(metrics, memory_stats, pid);
	}

	outcome
}

/// Create a temporary file for an artifact at the given cache path and execute the given
/// future/closure passing the file path in.
///
/// The function will try best effort to not leave behind the temporary file.
async fn with_tmp_file<F, Fut>(stream: UnixStream, pid: u32, cache_path: &Path, f: F) -> Outcome
where
	Fut: futures::Future<Output = Outcome>,
	F: FnOnce(PathBuf, UnixStream) -> Fut,
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
			return Outcome::CreateTmpFileErr {
				worker: IdleWorker { stream, pid },
				err: format!("{:?}", err),
			}
		},
	};

	let outcome = f(tmp_file.clone(), stream).await;

	// The function called above is expected to move `tmp_file` to a new location upon success. However,
	// the function may as well fail and in that case we should remove the tmp file here.
	//
	// In any case, we try to remove the file here so that there are no leftovers. We only report
	// errors that are different from the `NotFound`.
	match tokio::fs::remove_file(tmp_file).await {
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
	executor_params: &ExecutorParams,
	preparation_timeout: Duration,
) -> io::Result<()> {
	framed_send(stream, &code).await?;
	framed_send(stream, path_to_bytes(tmp_file)).await?;
	framed_send(stream, &executor_params.encode()).await?;
	framed_send(stream, &preparation_timeout.encode()).await?;
	Ok(())
}

async fn recv_request(
	stream: &mut UnixStream,
) -> io::Result<(Vec<u8>, PathBuf, ExecutorParams, Duration)> {
	let code = framed_recv(stream).await?;
	let tmp_file = framed_recv(stream).await?;
	let tmp_file = bytes_to_path(&tmp_file).ok_or_else(|| {
		io::Error::new(
			io::ErrorKind::Other,
			"prepare pvf recv_request: non utf-8 artifact path".to_string(),
		)
	})?;
	let executor_params_enc = framed_recv(stream).await?;
	let executor_params = ExecutorParams::decode(&mut &executor_params_enc[..]).map_err(|_| {
		io::Error::new(
			io::ErrorKind::Other,
			"prepare pvf recv_request: failed to decode ExecutorParams".to_string(),
		)
	})?;
	let preparation_timeout = framed_recv(stream).await?;
	let preparation_timeout = Duration::decode(&mut &preparation_timeout[..]).map_err(|e| {
		io::Error::new(
			io::ErrorKind::Other,
			format!("prepare pvf recv_request: failed to decode duration: {:?}", e),
		)
	})?;
	Ok((code, tmp_file, executor_params, preparation_timeout))
}

async fn send_response(
	stream: &mut UnixStream,
	result: PrepareResult,
	memory_stats: Option<MemoryStats>,
) -> io::Result<()> {
	framed_send(stream, &result.encode()).await?;
	framed_send(stream, &memory_stats.encode()).await
}

async fn recv_response(
	stream: &mut UnixStream,
	pid: u32,
) -> io::Result<(PrepareResult, Option<MemoryStats>)> {
	let result = framed_recv(stream).await?;
	let result = PrepareResult::decode(&mut &result[..]).map_err(|e| {
		// We received invalid bytes from the worker.
		let bound_bytes = &result[..result.len().min(4)];
		gum::warn!(
			target: LOG_TARGET,
			worker_pid = %pid,
			"received unexpected response from the prepare worker: {}",
			HexDisplay::from(&bound_bytes),
		);
		io::Error::new(
			io::ErrorKind::Other,
			format!("prepare pvf recv_response: failed to decode result: {:?}", e),
		)
	})?;
	let memory_stats = framed_recv(stream).await?;
	let memory_stats = Option::<MemoryStats>::decode(&mut &memory_stats[..]).map_err(|e| {
		io::Error::new(
			io::ErrorKind::Other,
			format!("prepare pvf recv_response: failed to decode memory stats: {:?}", e),
		)
	})?;
	Ok((result, memory_stats))
}

/// The entrypoint that the spawned prepare worker should start with. The `socket_path` specifies
/// the path to the socket used to communicate with the host.
///
/// # Flow
///
///	This runs the following in a loop:
///
///	1. Get the code and parameters for preparation from the host.
///
///	2. Start a memory tracker in a separate thread.
///
///	3. Start the CPU time monitor loop and the actual preparation in two separate threads.
///
///	4. Select on the two threads created in step 3. If the CPU timeout was hit, the CPU time monitor
///	   thread will trigger first.
///
///	5. Stop the memory tracker and get the stats.
///
/// 6. If compilation succeeded, write the compiled artifact into a temporary file.
///
///	7. Send the result of preparation back to the host. If any error occurred in the above steps, we
///	   send that in the `PrepareResult`.
pub fn worker_entrypoint(socket_path: &str) {
	worker_event_loop("prepare", socket_path, |rt_handle, mut stream| async move {
		loop {
			let (code, dest, executor_params, preparation_timeout) =
				recv_request(&mut stream).await?;
			gum::debug!(
				target: LOG_TARGET,
				worker_pid = %std::process::id(),
				"worker: preparing artifact",
			);

			let cpu_time_start = ProcessTime::now();

			// Run the memory tracker.
			#[cfg(any(target_os = "linux", feature = "jemalloc-allocator"))]
			let (memory_tracker_tx, memory_tracker_rx) = channel::<()>();
			#[cfg(any(target_os = "linux", feature = "jemalloc-allocator"))]
			let memory_tracker_fut = rt_handle.spawn_blocking(move || memory_tracker_loop(memory_tracker_rx));

			// Spawn a new thread that runs the CPU time monitor.
			let (cpu_time_monitor_tx, cpu_time_monitor_rx) = channel::<()>();
			let cpu_time_monitor_fut = rt_handle
				.spawn_blocking(move || {
					cpu_time_monitor_loop(cpu_time_start, preparation_timeout, cpu_time_monitor_rx)
				})
				.fuse();
			// Spawn another thread for preparation.
			let prepare_fut = rt_handle
				.spawn_blocking(move || {
					let prepare_result = prepare_artifact(&code, executor_params);

					// Get the `ru_maxrss` stat. If supported, call getrusage for the thread.
					let max_rss = get_max_rss_thread();

					(prepare_result, max_rss)
				})
				.fuse();

			pin_mut!(cpu_time_monitor_fut);
			pin_mut!(prepare_fut);

			let (result, memory_stats) = select_biased! {
				// If this future is not selected, the join handle is dropped and the thread will
				// finish in the background.
				join_res = cpu_time_monitor_fut => {
					let result = match join_res {
						Ok(Some(cpu_time_elapsed)) => {
							// Log if we exceed the timeout and the other thread hasn't finished.
							gum::warn!(
								target: LOG_TARGET,
								worker_pid = %std::process::id(),
								"prepare job took {}ms cpu time, exceeded prepare timeout {}ms",
								cpu_time_elapsed.as_millis(),
								preparation_timeout.as_millis(),
							);
							Err(PrepareError::TimedOut)
						},
						Ok(None) => Err(PrepareError::IoErr("error communicating over finished channel".into())),
						Err(err) => Err(PrepareError::IoErr(err.to_string())),
					};
					(result, None)
				},
				compilation_res = prepare_fut => {
					let cpu_time_elapsed = cpu_time_start.elapsed();
					let _ = cpu_time_monitor_tx.send(());

					match compilation_res.unwrap_or_else(|err| (Err(PrepareError::IoErr(err.to_string())), None)) {
						(Err(err), _) => {
							// Serialized error will be written into the socket.
							(Err(err), None)
						},
						(Ok(compiled_artifact), max_rss) => {
							// Stop the memory stats worker and get its observed memory stats.
							#[cfg(any(target_os = "linux", feature = "jemalloc-allocator"))]
							let memory_tracker_stats =
								get_memory_tracker_loop_stats(memory_tracker_fut, memory_tracker_tx).await;
							#[cfg(not(any(target_os = "linux", feature = "jemalloc-allocator")))]
							let memory_tracker_stats = None;
							let memory_stats = MemoryStats {
								memory_tracker_stats,
								max_rss: max_rss.map(|inner| inner.map_err(|e| e.to_string())),
							};

							// Write the serialized artifact into a temp file.
							//
							// PVF host only keeps artifacts statuses in its memory, successfully
							// compiled code gets stored on the disk (and consequently deserialized
							// by execute-workers). The prepare worker is only required to send `Ok`
							// to the pool to indicate the success.

							gum::debug!(
								target: LOG_TARGET,
								worker_pid = %std::process::id(),
								"worker: writing artifact to {}",
								dest.display(),
							);
							tokio::fs::write(&dest, &compiled_artifact).await?;

							(Ok(cpu_time_elapsed), Some(memory_stats))
						},
					}
				},
			};

			send_response(&mut stream, result, memory_stats).await?;
		}
	});
}

fn prepare_artifact(
	code: &[u8],
	executor_params: ExecutorParams,
) -> Result<CompiledArtifact, PrepareError> {
	panic::catch_unwind(|| {
		let blob = match crate::executor_intf::prevalidate(code) {
			Err(err) => return Err(PrepareError::Prevalidation(format!("{:?}", err))),
			Ok(b) => b,
		};

		match crate::executor_intf::prepare(blob, executor_params) {
			Ok(compiled_artifact) => Ok(CompiledArtifact::new(compiled_artifact)),
			Err(err) => Err(PrepareError::Preparation(format!("{:?}", err))),
		}
	})
	.map_err(|panic_payload| {
		PrepareError::Panic(crate::error::stringify_panic_payload(panic_payload))
	})
	.and_then(|inner_result| inner_result)
}
