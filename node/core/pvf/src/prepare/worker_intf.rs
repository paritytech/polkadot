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

//! Host interface to the prepare worker.

use crate::{
	error::{PrepareError, PrepareResult},
	metrics::Metrics,
	prepare::PrepareStats,
	pvf::PvfPrepData,
	worker_common::{
		framed_recv, framed_send, path_to_bytes, spawn_with_program_path, tmpfile_in, IdleWorker,
		SpawnErr, WorkerHandle, JOB_TIMEOUT_WALL_CLOCK_FACTOR,
	},
	LOG_TARGET,
};
use parity_scale_codec::{Decode, Encode};

use sp_core::hexdisplay::HexDisplay;
use std::{
	path::{Path, PathBuf},
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
	spawn_with_program_path(
		"prepare",
		program_path,
		&["prepare-worker", "--node-impl-version", env!("SUBSTRATE_CLI_IMPL_VERSION")],
		spawn_timeout,
	)
	.await
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
	pvf: PvfPrepData,
	cache_path: &Path,
	artifact_path: PathBuf,
) -> Outcome {
	let IdleWorker { stream, pid } = worker;

	gum::debug!(
		target: LOG_TARGET,
		worker_pid = %pid,
		"starting prepare for {}",
		artifact_path.display(),
	);

	with_tmp_file(stream, pid, cache_path, |tmp_file, mut stream| async move {
		let preparation_timeout = pvf.prep_timeout();
		if let Err(err) = send_request(&mut stream, pvf, &tmp_file).await {
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
			Ok(Ok(prepare_result)) =>
				handle_response(
					metrics,
					IdleWorker { stream, pid },
					prepare_result,
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
	worker_pid: u32,
	tmp_file: PathBuf,
	artifact_path: PathBuf,
	preparation_timeout: Duration,
) -> Outcome {
	let PrepareStats { cpu_time_elapsed, memory_stats } = match result.clone() {
		Ok(result) => result,
		// Timed out on the child. This should already be logged by the child.
		Err(PrepareError::TimedOut) => return Outcome::TimedOut,
		Err(_) => return Outcome::Concluded { worker, result },
	};

	if cpu_time_elapsed > preparation_timeout {
		// The job didn't complete within the timeout.
		gum::warn!(
			target: LOG_TARGET,
			%worker_pid,
			"prepare job took {}ms cpu time, exceeded preparation timeout {}ms. Clearing WIP artifact {}",
			cpu_time_elapsed.as_millis(),
			preparation_timeout.as_millis(),
			tmp_file.display(),
		);
		return Outcome::TimedOut
	}

	gum::debug!(
		target: LOG_TARGET,
		%worker_pid,
		"promoting WIP artifact {} to {}",
		tmp_file.display(),
		artifact_path.display(),
	);

	let outcome = match tokio::fs::rename(&tmp_file, &artifact_path).await {
		Ok(()) => Outcome::Concluded { worker, result },
		Err(err) => {
			gum::warn!(
				target: LOG_TARGET,
				%worker_pid,
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
	metrics.observe_preparation_memory_metrics(memory_stats);

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
	pvf: PvfPrepData,
	tmp_file: &Path,
) -> io::Result<()> {
	framed_send(stream, &pvf.encode()).await?;
	framed_send(stream, path_to_bytes(tmp_file)).await?;
	Ok(())
}

async fn recv_response(stream: &mut UnixStream, pid: u32) -> io::Result<PrepareResult> {
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
	Ok(result)
}
