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
	common::{bytes_to_path, cpu_time_monitor_loop, worker_event_loop},
	executor_intf::Executor,
	LOG_TARGET,
};
use cpu_time::ProcessTime;
use futures::{pin_mut, select_biased, FutureExt};
use parity_scale_codec::{Decode, Encode};
use polkadot_node_core_pvf::{
	framed_recv, framed_send, ExecuteHandshake as Handshake, ExecuteResponse as Response,
};
use polkadot_parachain::primitives::ValidationResult;
use std::{
	path::{Path, PathBuf},
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

/// The entrypoint that the spawned execute worker should start with. The `socket_path` specifies
/// the path to the socket used to communicate with the host. The `node_version`, if `Some`,
/// is checked against the worker version. A mismatch results in immediate worker termination.
/// `None` is used for tests and in other situations when version check is not necessary.
pub fn worker_entrypoint(socket_path: &str, node_version: Option<&str>) {
	worker_event_loop("execute", socket_path, node_version, |rt_handle, mut stream| async move {
		let worker_pid = std::process::id();

		let handshake = recv_handshake(&mut stream).await?;
		let executor = Arc::new(Executor::new(handshake.executor_params).map_err(|e| {
			io::Error::new(io::ErrorKind::Other, format!("cannot create executor: {}", e))
		})?);

		loop {
			let (artifact_path, params, execution_timeout) = recv_request(&mut stream).await?;
			gum::debug!(
				target: LOG_TARGET,
				%worker_pid,
				"worker: validating artifact {}",
				artifact_path.display(),
			);

			// Used to signal to the cpu time monitor thread that it can finish.
			let (finished_tx, finished_rx) = channel::<()>();
			let cpu_time_start = ProcessTime::now();

			// Spawn a new thread that runs the CPU time monitor.
			let cpu_time_monitor_fut = rt_handle
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

			pin_mut!(cpu_time_monitor_fut);
			pin_mut!(execute_fut);

			let response = select_biased! {
				// If this future is not selected, the join handle is dropped and the thread will
				// finish in the background.
				cpu_time_monitor_res = cpu_time_monitor_fut => {
					match cpu_time_monitor_res {
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
						Ok(None) => Response::InternalError("error communicating over finished channel".into()),
						Err(e) => Response::format_internal("cpu time monitor thread error", &e.to_string()),
					}
				},
				execute_res = execute_fut => {
					let _ = finished_tx.send(());
					execute_res.unwrap_or_else(|e| Response::format_internal("execute thread error", &e.to_string()))
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
	// Check here if the file exists, because the error from Substrate is not match-able.
	// TODO: Re-evaluate after <https://github.com/paritytech/substrate/issues/13860>.
	let file_metadata = std::fs::metadata(artifact_path);
	if let Err(err) = file_metadata {
		return Response::format_internal("execute: could not find or open file", &err.to_string())
	}

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
			return Response::format_invalid("validation result decoding failed", &err.to_string()),
		Ok(r) => r,
	};

	Response::Ok { result_descriptor, duration }
}
