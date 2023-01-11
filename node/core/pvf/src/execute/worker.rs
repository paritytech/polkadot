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
	artifacts::ArtifactPathId,
	executor_intf::Executor,
	worker_common::{
		bytes_to_path, cpu_time_monitor_loop, framed_recv, framed_send, path_to_bytes,
		spawn_with_program_path, worker_event_loop, IdleWorker, SpawnErr, WorkerHandle,
		JOB_TIMEOUT_WALL_CLOCK_FACTOR,
	},
	LOG_TARGET,
};
use cpu_time::ProcessTime;
use futures::{pin_mut, select_biased, FutureExt};
use futures_timer::Delay;
use parity_scale_codec::{Decode, Encode};
use polkadot_parachain::primitives::ValidationResult;
use std::{
	path::{Path, PathBuf},
	sync::{mpsc::channel, Arc},
	time::Duration,
};
use tokio::{io, net::UnixStream};

/// Spawns a new worker with the given program path that acts as the worker and the spawn timeout.
///
/// The program should be able to handle `<program-path> execute-worker <socket-path>` invocation.
pub async fn spawn(
	program_path: &Path,
	spawn_timeout: Duration,
) -> Result<(IdleWorker, WorkerHandle), SpawnErr> {
	spawn_with_program_path("execute", program_path, &["execute-worker"], spawn_timeout).await
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

async fn recv_request(stream: &mut UnixStream) -> io::Result<(PathBuf, Vec<u8>, Duration)> {
	let artifact_path = framed_recv(stream).await?;
	let artifact_path = bytes_to_path(&artifact_path).ok_or_else(|| {
		io::Error::new(
			io::ErrorKind::Other,
			"execute pvf recv_request: non utf-8 artifact path".to_string(),
		)
	})?;
	let params = framed_recv(stream).await?;
	let execution_timeout = framed_recv(stream).await?;
	let execution_timeout = Duration::decode(&mut &execution_timeout[..]).map_err(|_| {
		io::Error::new(
			io::ErrorKind::Other,
			"execute pvf recv_request: failed to decode duration".to_string(),
		)
	})?;
	Ok((artifact_path, params, execution_timeout))
}

async fn send_response(stream: &mut UnixStream, response: Response) -> io::Result<()> {
	framed_send(stream, &response.encode()).await
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

#[derive(Encode, Decode)]
pub enum Response {
	Ok { result_descriptor: ValidationResult, duration: Duration },
	InvalidCandidate(String),
	TimedOut,
	InternalError(String),
}

impl Response {
	fn format_invalid(ctx: &'static str, msg: &str) -> Self {
		if msg.is_empty() {
			Self::InvalidCandidate(ctx.to_string())
		} else {
			Self::InvalidCandidate(format!("{}: {}", ctx, msg))
		}
	}
}

/// The entrypoint that the spawned execute worker should start with. The `socket_path` specifies
/// the path to the socket used to communicate with the host.
pub fn worker_entrypoint(socket_path: &str) {
	worker_event_loop("execute", socket_path, |rt_handle, mut stream| async move {
		let executor = Arc::new(Executor::new().map_err(|e| {
			io::Error::new(io::ErrorKind::Other, format!("cannot create executor: {}", e))
		})?);

		loop {
			let (artifact_path, params, execution_timeout) = recv_request(&mut stream).await?;
			gum::debug!(
				target: LOG_TARGET,
				worker_pid = %std::process::id(),
				"worker: validating artifact {}",
				artifact_path.display(),
			);

			// Used to signal to the cpu time monitor thread that it can finish.
			let (finished_tx, finished_rx) = channel::<()>();
			let cpu_time_start = ProcessTime::now();

			// Spawn a new thread that runs the CPU time monitor.
			let thread_fut = rt_handle
				.spawn_blocking(move || {
					cpu_time_monitor_loop(cpu_time_start, execution_timeout, finished_rx)
				})
				.fuse();
			let executor_2 = executor.clone();
			let execute_fut = rt_handle
				.spawn_blocking(move || {
					validate_using_artifact(&artifact_path, &params, executor_2, cpu_time_start)
				})
				.fuse();

			pin_mut!(thread_fut);
			pin_mut!(execute_fut);

			let response = select_biased! {
				// If this future is not selected, the join handle is dropped and the thread will
				// finish in the background.
				join_res = thread_fut => {
					match join_res {
						Ok(Some(cpu_time_elapsed)) => {
							// Log if we exceed the timeout and the other thread hasn't finished.
							gum::warn!(
								target: LOG_TARGET,
								worker_pid = %std::process::id(),
								"execute job took {}ms cpu time, exceeded execute timeout {}ms",
								cpu_time_elapsed.as_millis(),
								execution_timeout.as_millis(),
							);
							Response::TimedOut
						},
						Ok(None) => Response::InternalError("error communicating over finished channel".into()),
						Err(e) => Response::InternalError(format!("{}", e)),
					}
				},
				execute_res = execute_fut => {
					let _ = finished_tx.send(());
					execute_res.unwrap_or_else(|e| Response::InternalError(format!("{}", e)))
				},
			};

			send_response(&mut stream, response).await?;
		}
	});
}

fn validate_using_artifact(
	artifact_path: &Path,
	params: &[u8],
	executor: Arc<Executor>,
	cpu_time_start: ProcessTime,
) -> Response {
	let descriptor_bytes = match unsafe {
		// SAFETY: this should be safe since the compiled artifact passed here comes from the
		//         file created by the prepare workers. These files are obtained by calling
		//         [`executor_intf::prepare`].
		executor.execute(artifact_path.as_ref(), params)
	} {
		Err(err) => return Response::format_invalid("execute", &err),
		Ok(d) => d,
	};

	let duration = cpu_time_start.elapsed();

	let result_descriptor = match ValidationResult::decode(&mut &descriptor_bytes[..]) {
		Err(err) =>
			return Response::InvalidCandidate(format!("validation result decoding failed: {}", err)),
		Ok(r) => r,
	};

	Response::Ok { result_descriptor, duration }
}
