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

//! Host interface to the execute worker.

use crate::{
	artifacts::ArtifactPathId,
	worker_common::{
		framed_recv, framed_send, path_to_bytes, spawn_with_program_path, IdleWorker, SpawnErr,
		WorkerHandle, JOB_TIMEOUT_WALL_CLOCK_FACTOR,
	},
	LOG_TARGET,
};
use futures::FutureExt;
use futures_timer::Delay;
use parity_scale_codec::{Decode, Encode};

use polkadot_parachain::primitives::ValidationResult;
use polkadot_primitives::ExecutorParams;
use std::{path::Path, time::Duration};
use tokio::{io, net::UnixStream};

/// Spawns a new worker with the given program path that acts as the worker and the spawn timeout.
/// Sends a handshake message to the worker as soon as it is spawned.
///
/// The program should be able to handle `<program-path> execute-worker <socket-path>` invocation.
pub async fn spawn(
	program_path: &Path,
	executor_params: ExecutorParams,
	spawn_timeout: Duration,
) -> Result<(IdleWorker, WorkerHandle), SpawnErr> {
	let (mut idle_worker, worker_handle) = spawn_with_program_path(
		"execute",
		program_path,
		&["execute-worker", "--node-impl-version", env!("SUBSTRATE_CLI_IMPL_VERSION")],
		spawn_timeout,
	)
	.await?;
	send_handshake(&mut idle_worker.stream, Handshake { executor_params })
		.await
		.map_err(|error| {
			gum::warn!(
				target: LOG_TARGET,
				worker_pid = %idle_worker.pid,
				?error,
				"failed to send a handshake to the spawned worker",
			);
			SpawnErr::Handshake
		})?;
	Ok((idle_worker, worker_handle))
}

/// Outcome of PVF execution.
pub enum Outcome {
	/// PVF execution completed successfully and the result is returned. The worker is ready for
	/// another job.
	Ok { result_descriptor: ValidationResult, duration: Duration, idle_worker: IdleWorker },
	/// The candidate validation failed. It may be for example because the wasm execution triggered a trap.
	/// Errors related to the preparation process are not expected to be encountered by the execution workers.
	InvalidCandidate { err: String, idle_worker: IdleWorker },
	/// An internal error happened during the validation. Such an error is most likely related to
	/// some transient glitch.
	InternalError { err: String, idle_worker: IdleWorker },
	/// The execution time exceeded the hard limit. The worker is terminated.
	HardTimeout,
	/// An I/O error happened during communication with the worker. This may mean that the worker
	/// process already died. The token is not returned in any case.
	IoErr,
}

/// Given the idle token of a worker and parameters of work, communicates with the worker and
/// returns the outcome.
///
/// NOTE: Returning the `HardTimeout` or `IoErr` errors will trigger the child process being killed.
pub async fn start_work(
	worker: IdleWorker,
	artifact: ArtifactPathId,
	execution_timeout: Duration,
	validation_params: Vec<u8>,
) -> Outcome {
	let IdleWorker { mut stream, pid } = worker;

	gum::debug!(
		target: LOG_TARGET,
		worker_pid = %pid,
		validation_code_hash = ?artifact.id.code_hash,
		"starting execute for {}",
		artifact.path.display(),
	);

	if let Err(error) =
		send_request(&mut stream, &artifact.path, &validation_params, execution_timeout).await
	{
		gum::warn!(
			target: LOG_TARGET,
			worker_pid = %pid,
			validation_code_hash = ?artifact.id.code_hash,
			?error,
			"failed to send an execute request",
		);
		return Outcome::IoErr
	}

	// We use a generous timeout here. This is in addition to the one in the child process, in
	// case the child stalls. We have a wall clock timeout here in the host, but a CPU timeout
	// in the child. We want to use CPU time because it varies less than wall clock time under
	// load, but the CPU resources of the child can only be measured from the parent after the
	// child process terminates.
	let timeout = execution_timeout * JOB_TIMEOUT_WALL_CLOCK_FACTOR;
	let response = futures::select! {
		response = recv_response(&mut stream).fuse() => {
			match response {
				Err(error) => {
					gum::warn!(
						target: LOG_TARGET,
						worker_pid = %pid,
						validation_code_hash = ?artifact.id.code_hash,
						?error,
						"failed to recv an execute response",
					);
					return Outcome::IoErr
				},
				Ok(response) => {
					if let Response::Ok{duration, ..} = response {
						if duration > execution_timeout {
							// The job didn't complete within the timeout.
							gum::warn!(
								target: LOG_TARGET,
								worker_pid = %pid,
								"execute job took {}ms cpu time, exceeded execution timeout {}ms.",
								duration.as_millis(),
								execution_timeout.as_millis(),
							);

							// Return a timeout error.
							return Outcome::HardTimeout;
						}
					}

					response
				},
			}
		},
		_ = Delay::new(timeout).fuse() => {
			gum::warn!(
				target: LOG_TARGET,
				worker_pid = %pid,
				validation_code_hash = ?artifact.id.code_hash,
				"execution worker exceeded lenient timeout for execution, child worker likely stalled",
			);
			Response::TimedOut
		},
	};

	match response {
		Response::Ok { result_descriptor, duration } =>
			Outcome::Ok { result_descriptor, duration, idle_worker: IdleWorker { stream, pid } },
		Response::InvalidCandidate(err) =>
			Outcome::InvalidCandidate { err, idle_worker: IdleWorker { stream, pid } },
		Response::TimedOut => Outcome::HardTimeout,
		Response::InternalError(err) =>
			Outcome::InternalError { err, idle_worker: IdleWorker { stream, pid } },
	}
}

async fn send_handshake(stream: &mut UnixStream, handshake: Handshake) -> io::Result<()> {
	framed_send(stream, &handshake.encode()).await
}

async fn send_request(
	stream: &mut UnixStream,
	artifact_path: &Path,
	validation_params: &[u8],
	execution_timeout: Duration,
) -> io::Result<()> {
	framed_send(stream, path_to_bytes(artifact_path)).await?;
	framed_send(stream, validation_params).await?;
	framed_send(stream, &execution_timeout.encode()).await
}

async fn recv_response(stream: &mut UnixStream) -> io::Result<Response> {
	let response_bytes = framed_recv(stream).await?;
	Response::decode(&mut &response_bytes[..]).map_err(|e| {
		io::Error::new(
			io::ErrorKind::Other,
			format!("execute pvf recv_response: decode error: {:?}", e),
		)
	})
}

/// The payload of the one-time handshake that is done when a worker process is created. Carries
/// data from the host to the worker.
#[derive(Encode, Decode)]
pub struct Handshake {
	/// The executor parameters.
	pub executor_params: ExecutorParams,
}

/// The response from an execution job on the worker.
#[derive(Encode, Decode)]
pub enum Response {
	/// The job completed successfully.
	Ok {
		/// The result of parachain validation.
		result_descriptor: ValidationResult,
		/// The amount of CPU time taken by the job.
		duration: Duration,
	},
	/// The candidate is invalid.
	InvalidCandidate(String),
	/// The job timed out.
	TimedOut,
	/// Some internal error occurred. Should only be used for errors independent of the candidate.
	InternalError(String),
}

impl Response {
	/// Creates an invalid response from a context `ctx` and a message `msg` (which can be empty).
	pub fn format_invalid(ctx: &'static str, msg: &str) -> Self {
		if msg.is_empty() {
			Self::InvalidCandidate(ctx.to_string())
		} else {
			Self::InvalidCandidate(format!("{}: {}", ctx, msg))
		}
	}
	/// Creates an internal response from a context `ctx` and a message `msg` (which can be empty).
	pub fn format_internal(ctx: &'static str, msg: &str) -> Self {
		if msg.is_empty() {
			Self::InternalError(ctx.to_string())
		} else {
			Self::InternalError(format!("{}: {}", ctx, msg))
		}
	}
}
