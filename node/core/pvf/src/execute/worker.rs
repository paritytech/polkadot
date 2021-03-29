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

use crate::{artifacts::Artifact, executor_intf::TaskExecutor, worker_common::{
		IdleWorker, SpawnErr, WorkerHandle, bytes_to_path, framed_recv, framed_send, path_to_bytes,
		spawn_with_program_path, worker_event_loop,
	}};
use std::time::{Duration, Instant};
use async_std::{
	io,
	os::unix::net::UnixStream,
	path::{Path, PathBuf},
};
use futures::FutureExt;
use futures_timer::Delay;
use polkadot_parachain::primitives::ValidationResult;
use parity_scale_codec::{Encode, Decode};

/// Spawns a new worker with the given program path that acts as the worker and the spawn timeout.
///
/// The program should be able to handle `<program-path> execute-worker <socket-path>` invocation.
pub async fn spawn(
	program_path: &Path,
	spawn_timeout_secs: u64,
) -> Result<(IdleWorker, WorkerHandle), SpawnErr> {
	let program_path = program_path.to_string_lossy();
	spawn_with_program_path(
		"execute",
		&program_path,
		&["execute-worker"],
		spawn_timeout_secs,
	)
	.await
}

/// Outcome of PVF execution.
pub enum Outcome {
	/// PVF execution completed successfully and the result is returned. The worker is ready for
	/// another job.
	Ok {
		result_descriptor: ValidationResult,
		duration_ms: u64,
		idle_worker: IdleWorker,
	},
	/// The candidate validation failed. It may be for example because the preparation processe
	/// produced an error or the wasm execution triggered a trap.
	InvalidCandidate {
		err: String,
		idle_worker: IdleWorker,
	},
	/// The execution time exceeded the hard limit. The worker is terminated.
	HardTimeout,
	/// An I/O error happened during communication with the worker. This may mean that the worker
	/// process already died. The token is not returned in any case.
	IoErr,
}

pub async fn start_work(
	worker: IdleWorker,
	artifact_path: PathBuf,
	validation_params: Vec<u8>,
) -> Outcome {
	let IdleWorker { mut stream, pid } = worker;

	if let Err(_err) = send_request(&mut stream, &artifact_path, &validation_params).await {
		return Outcome::IoErr;
	}

	let response = futures::select! {
		response = recv_response(&mut stream).fuse() => {
			match response {
				Err(_err) => return Outcome::IoErr,
				Ok(response) => response,
			}
		},
		_ = Delay::new(Duration::from_secs(3)).fuse() => return Outcome::HardTimeout,
	};

	match response {
		Response::Ok {
			result_descriptor,
			duration_ms,
		} => Outcome::Ok {
			result_descriptor,
			duration_ms,
			idle_worker: IdleWorker { stream, pid },
		},
		Response::InvalidCandidate(err) => Outcome::InvalidCandidate {
			err,
			idle_worker: IdleWorker { stream, pid },
		},
	}
}

async fn send_request(
	stream: &mut UnixStream,
	artifact_path: &Path,
	validation_params: &[u8],
) -> io::Result<()> {
	framed_send(stream, path_to_bytes(artifact_path)).await?;
	framed_send(stream, validation_params).await?;
	Ok(())
}

async fn recv_request(stream: &mut UnixStream) -> io::Result<(PathBuf, Vec<u8>)> {
	let artifact_path = framed_recv(stream).await?;
	let artifact_path = bytes_to_path(&artifact_path).ok_or_else(|| {
		io::Error::new(
			io::ErrorKind::Other,
			format!("execute pvf recv_request: non utf-8 artifact path"),
		)
	})?;
	let params = framed_recv(stream).await?;
	Ok((artifact_path, params))
}

async fn send_response(stream: &mut UnixStream, response: Response) -> io::Result<()> {
	framed_send(stream, &response.encode()).await?;
	Ok(())
}

async fn recv_response(stream: &mut UnixStream) -> io::Result<Response> {
	let response_bytes = framed_recv(stream).await?;
	let response = Response::decode(&mut &response_bytes[..]).map_err(|e| {
		io::Error::new(
			io::ErrorKind::Other,
			format!("execute pvf recv_response: decode error: {:?}", e),
		)
	})?;
	Ok(response)
}

#[derive(Encode, Decode)]
enum Response {
	Ok {
		result_descriptor: ValidationResult,
		duration_ms: u64,
	},
	InvalidCandidate(String),
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

/// The entrypoint that the spawned execute worker should start with. The socket_path specifies
/// the path to the socket used to communicate with the host.
pub fn worker_entrypoint(socket_path: &str) {
	worker_event_loop("execute", socket_path, |mut stream| async move {
		let executor = TaskExecutor::new().map_err(|e| {
			io::Error::new(
				io::ErrorKind::Other,
				format!("cannot create task executor: {}", e),
			)
		})?;
		loop {
			let (artifact_path, params) = recv_request(&mut stream).await?;
			let artifact_bytes = async_std::fs::read(&artifact_path).await?;
			let artifact = Artifact::deserialize(&artifact_bytes).map_err(|e| {
				io::Error::new(
					io::ErrorKind::Other,
					format!("artifact deserialization error: {}", e),
				)
			})?;
			let response = validate_using_artifact(&artifact, &params, &executor);
			send_response(&mut stream, response).await?;
		}
	});
}

fn validate_using_artifact(artifact: &Artifact, params: &[u8], spawner: &TaskExecutor) -> Response {
	let compiled_artifact = match artifact {
		Artifact::PrevalidationErr(msg) => {
			return Response::format_invalid("prevalidation", msg);
		}
		Artifact::PreparationErr(msg) => {
			return Response::format_invalid("preparation", msg);
		}
		Artifact::DidntMakeIt => {
			return Response::format_invalid("preparation timeout", "");
		}

		Artifact::Compiled { compiled_artifact } => compiled_artifact,
	};

	let validation_started_at = Instant::now();
	let descriptor_bytes =
		match crate::executor_intf::execute(compiled_artifact, params, spawner.clone()) {
			Err(err) => {
				return Response::format_invalid("execute", &err.to_string());
			}
			Ok(d) => d,
		};

	let duration_ms = validation_started_at.elapsed().as_millis() as u64;

	let result_descriptor = match ValidationResult::decode(&mut &descriptor_bytes[..]) {
		Err(err) => {
			return Response::InvalidCandidate(format!(
				"validation result decoding failed: {}",
				err
			))
		}
		Ok(r) => r,
	};

	Response::Ok {
		result_descriptor,
		duration_ms,
	}
}
