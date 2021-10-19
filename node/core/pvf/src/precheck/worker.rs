// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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
	worker_common::{
		framed_recv, framed_send, spawn_with_program_path, worker_event_loop, IdleWorker, SpawnErr,
		WorkerHandle,
	},
	PrecheckError, PrecheckResult, Pvf, LOG_TARGET,
};
use async_std::{io, os::unix::net::UnixStream, path::Path};
use parity_scale_codec::{Decode, Encode};
use std::time::Duration;

/// Spawns a new worker with the given program path that acts as the worker and the spawn timeout.
///
/// The program should be able to handle `<program-path> precheck-worker <socket-path>` invocation.
pub async fn spawn(
	program_path: &Path,
	spawn_timeout: Duration,
) -> Result<(IdleWorker, WorkerHandle), SpawnErr> {
	spawn_with_program_path("precheck", program_path, &["precheck-worker"], spawn_timeout).await
}

/// Outcome of PVF prechecking.
pub enum Outcome {
	/// Worker responded with a result of prechecking and is ready to accept the next job.
	Ok { precheck_result: PrecheckResult, idle_worker: IdleWorker },
	/// The compilation was interrupted due to the time limit.
	TimedOut,
	/// An I/O error happened during communication with the worker. This may mean that the worker
	/// process already died. The token is not returned in any case.
	IoErr(io::Error),
}

pub async fn start_work(worker: IdleWorker, pvf: Pvf, precheck_timeout: Duration) -> Outcome {
	let IdleWorker { mut stream, pid } = worker;

	tracing::debug!(
		target: LOG_TARGET,
		worker_pid = %pid,
		validation_code_hash = ?pvf.code_hash,
		"starting prechecking routine",
	);

	if let Err(error) = send_request(&mut stream, pvf.code.as_ref()).await {
		tracing::warn!(
			target: LOG_TARGET,
			worker_pid = %pid,
			validation_code_hash = ?pvf.code_hash,
			?error,
			"failed to send a precheck request",
		);
		return Outcome::IoErr(error)
	}

	match async_std::future::timeout(precheck_timeout, recv_response(&mut stream)).await {
		Ok(receive_result) => match receive_result {
			Ok(precheck_result) =>
				Outcome::Ok { precheck_result, idle_worker: IdleWorker { stream, pid } },
			Err(err) => Outcome::IoErr(err),
		},
		Err(_) => Outcome::TimedOut,
	}
}

async fn send_request(stream: &mut UnixStream, code: &[u8]) -> io::Result<()> {
	framed_send(stream, code).await
}

async fn recv_request(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
	framed_recv(stream).await
}

async fn send_response(stream: &mut UnixStream, response: PrecheckResult) -> io::Result<()> {
	framed_send(stream, &response.encode()).await
}

async fn recv_response(stream: &mut UnixStream) -> io::Result<PrecheckResult> {
	let response_bytes = framed_recv(stream).await?;
	Decode::decode(&mut &response_bytes[..]).map_err(|e| {
		io::Error::new(
			io::ErrorKind::Other,
			format!("precheck pvf recv_response: decode error: {:?}", e),
		)
	})
}

/// The entrypoint that the spawned precheck worker should start with. The `socket_path` specifies
/// the path to the socket used to communicate with the host.
pub fn worker_entrypoint(socket_path: &str) {
	worker_event_loop("precheck", socket_path, |mut stream| async move {
		loop {
			let code = recv_request(&mut stream).await?;

			tracing::debug!(
				target: LOG_TARGET,
				worker_pid = %std::process::id(),
				"worker: prechecking pvf",
			);
			let precheck_result = precheck_pvf(code.as_slice());

			send_response(&mut stream, precheck_result).await?
		}
	});
}

fn precheck_pvf(code: &[u8]) -> PrecheckResult {
	let blob = match crate::executor_intf::prevalidate(code) {
		Err(err) => return Err(PrecheckError::CompileError(format!("{:?}", err))),
		Ok(b) => b,
	};

	match crate::executor_intf::prepare_single_threaded(blob) {
		Ok(_) => Ok(()),
		Err(err) => Err(PrecheckError::CompileError(format!("{:?}", err))),
	}
}
