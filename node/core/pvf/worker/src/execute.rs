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

use crate::{
	common::{
		bytes_to_path, cpu_time_monitor_loop, stringify_panic_payload,
		thread::{self, WaitOutcome},
		worker_event_loop,
	},
	executor_intf::{Executor, EXECUTE_THREAD_STACK_SIZE},
	LOG_TARGET,
};
use cpu_time::ProcessTime;
use parity_scale_codec::{Decode, Encode};
use polkadot_node_core_pvf::{
	framed_recv, framed_send, ExecuteHandshake as Handshake, ExecuteResponse as Response,
	InternalValidationError,
};
use polkadot_parachain::primitives::ValidationResult;
use std::{
	path::PathBuf,
	sync::{mpsc::channel, Arc},
	time::Duration,
};
use tokio::{io, net::UnixStream};

async fn recv_handshake(stream: &mut UnixStream) -> io::Result<Handshake> {
	let handshake_enc = framed_recv(stream).await?;
	let handshake = Handshake::decode(&mut &handshake_enc[..]).map_err(|_| {
		io::Error::new(
			io::ErrorKind::Other,
			"execute pvf recv_handshake: failed to decode Handshake".to_owned(),
		)
	})?;
	Ok(handshake)
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

/// The entrypoint that the spawned execute worker should start with.
///
/// # Parameters
///
/// The `socket_path` specifies the path to the socket used to communicate with the host. The
/// `node_version`, if `Some`, is checked against the worker version. A mismatch results in
/// immediate worker termination. `None` is used for tests and in other situations when version
/// check is not necessary.
pub fn worker_entrypoint(socket_path: &str, node_version: Option<&str>) {
	worker_event_loop("execute", socket_path, node_version, |mut stream| async move {
		let worker_pid = std::process::id();

		let handshake = recv_handshake(&mut stream).await?;
		let executor = Executor::new(handshake.executor_params).map_err(|e| {
			io::Error::new(io::ErrorKind::Other, format!("cannot create executor: {}", e))
		})?;

		loop {
			let (artifact_path, params, execution_timeout) = recv_request(&mut stream).await?;
			gum::debug!(
				target: LOG_TARGET,
				%worker_pid,
				"worker: validating artifact {}",
				artifact_path.display(),
			);

			// Get the artifact bytes.
			//
			// We do this outside the thread so that we can lock down filesystem access there.
			let compiled_artifact_blob = match std::fs::read(artifact_path) {
				Ok(bytes) => bytes,
				Err(err) => {
					let response = Response::InternalError(
						InternalValidationError::CouldNotOpenFile(err.to_string()),
					);
					send_response(&mut stream, response).await?;
					continue
				},
			};

			// Conditional variable to notify us when a thread is done.
			let condvar = thread::get_condvar();

			let cpu_time_start = ProcessTime::now();

			// Spawn a new thread that runs the CPU time monitor.
			let (cpu_time_monitor_tx, cpu_time_monitor_rx) = channel::<()>();
			let cpu_time_monitor_thread = thread::spawn_worker_thread(
				"cpu time monitor thread",
				move || {
					cpu_time_monitor_loop(cpu_time_start, execution_timeout, cpu_time_monitor_rx)
				},
				Arc::clone(&condvar),
				WaitOutcome::TimedOut,
			)?;
			let executor_2 = executor.clone();
			let execute_thread = thread::spawn_worker_thread_with_stack_size(
				"execute thread",
				move || {
					validate_using_artifact(
						&compiled_artifact_blob,
						&params,
						executor_2,
						cpu_time_start,
					)
				},
				Arc::clone(&condvar),
				WaitOutcome::Finished,
				EXECUTE_THREAD_STACK_SIZE,
			)?;

			let outcome = thread::wait_for_threads(condvar);

			let response = match outcome {
				WaitOutcome::Finished => {
					let _ = cpu_time_monitor_tx.send(());
					execute_thread
						.join()
						.unwrap_or_else(|e| Response::Panic(stringify_panic_payload(e)))
				},
				// If the CPU thread is not selected, we signal it to end, the join handle is
				// dropped and the thread will finish in the background.
				WaitOutcome::TimedOut => {
					match cpu_time_monitor_thread.join() {
						Ok(Some(cpu_time_elapsed)) => {
							// Log if we exceed the timeout and the other thread hasn't finished.
							gum::warn!(
								target: LOG_TARGET,
								%worker_pid,
								"execute job took {}ms cpu time, exceeded execute timeout {}ms",
								cpu_time_elapsed.as_millis(),
								execution_timeout.as_millis(),
							);
							Response::TimedOut
						},
						Ok(None) =>
							Response::InternalError(InternalValidationError::CpuTimeMonitorThread(
								"error communicating over finished channel".into(),
							)),
						Err(e) =>
							Response::InternalError(InternalValidationError::CpuTimeMonitorThread(
								stringify_panic_payload(e),
							)),
					}
				},
				WaitOutcome::Pending =>
					unreachable!("we run wait_while until the outcome is no longer pending; qed"),
			};

			send_response(&mut stream, response).await?;
		}
	});
}

fn validate_using_artifact(
	compiled_artifact_blob: &[u8],
	params: &[u8],
	executor: Executor,
	cpu_time_start: ProcessTime,
) -> Response {
	let descriptor_bytes = match unsafe {
		// SAFETY: this should be safe since the compiled artifact passed here comes from the
		//         file created by the prepare workers. These files are obtained by calling
		//         [`executor_intf::prepare`].
		executor.execute(compiled_artifact_blob, params)
	} {
		Err(err) => return Response::format_invalid("execute", &err),
		Ok(d) => d,
	};

	let result_descriptor = match ValidationResult::decode(&mut &descriptor_bytes[..]) {
		Err(err) =>
			return Response::format_invalid("validation result decoding failed", &err.to_string()),
		Ok(r) => r,
	};

	// Include the decoding in the measured time, to prevent any potential attacks exploiting some
	// bug in decoding.
	let duration = cpu_time_start.elapsed();

	Response::Ok { result_descriptor, duration }
}
