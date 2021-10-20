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
	artifacts::Artifact,
	error::PrecheckError,
	precheck::PrecheckResult,
	worker_common::{
		bytes_to_path, framed_recv, framed_send, path_to_bytes, spawn_with_program_path,
		tmpfile_in, worker_event_loop, IdleWorker, SpawnErr, WorkerHandle,
	},
	Pvf, LOG_TARGET,
};
use async_std::{
	io,
	os::unix::net::UnixStream,
	path::{Path, PathBuf},
};
use futures::FutureExt as _;
use futures_timer::Delay;
use parity_scale_codec::{Decode, Encode};
use std::{sync::Arc, time::Duration};

const NICENESS_BACKGROUND: i32 = 10;
const NICENESS_FOREGROUND: i32 = 0;

const COMPILATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Spawns a new worker with the given program path that acts as the worker and the spawn timeout.
///
/// The program should be able to handle `<program-path> prepare-worker <socket-path>` invocation.
pub async fn spawn(
	program_path: &Path,
	spawn_timeout: Duration,
) -> Result<(IdleWorker, WorkerHandle), SpawnErr> {
	spawn_with_program_path("prepare", program_path, &["prepare-worker"], spawn_timeout).await
}

pub enum PrepareOutcome {
	/// The worker has finished the work assigned to it.
	Concluded(IdleWorker),
	/// The host tried to reach the worker but failed. This is most likely because the worked was
	/// killed by the system.
	Unreachable,
	/// The execution was interrupted abruptly and the worker is not available anymore. For example,
	/// this could've happen because the worker hadn't finished the work until the given deadline.
	///
	/// Note that in this case the artifact file is written (unless there was an error writing the
	/// the artifact).
	///
	/// This doesn't return an idle worker instance, thus this worker is no longer usable.
	DidntMakeIt,
}

pub enum PrecheckOutcome {
	/// Worker responded with a result of prechecking and is ready to accept the next job.
	Ok { precheck_result: PrecheckResult, idle_worker: IdleWorker },
	/// The compilation was interrupted due to the time limit.
	TimedOut,
	/// An I/O error happened during communication with the worker. This may mean that the worker
	/// process already died. The token is not returned in any case.
	IoErr(io::Error),
}

/// Given the idle token of a worker and parameters of work, communicates with the worker and
/// returns the outcome.
pub async fn start_prepare_work(
	worker: IdleWorker,
	code: Arc<Vec<u8>>,
	cache_path: &Path,
	artifact_path: PathBuf,
	background_priority: bool,
) -> PrepareOutcome {
	let IdleWorker { mut stream, pid } = worker;

	tracing::debug!(
		target: LOG_TARGET,
		worker_pid = %pid,
		%background_priority,
		"starting prepare for {}",
		artifact_path.display(),
	);

	if background_priority {
		renice(pid, NICENESS_BACKGROUND);
	}

	with_tmp_file(pid, cache_path, |tmp_file| async move {
		if let Err(err) = send_prepare_request(&mut stream, code, &tmp_file).await {
			tracing::warn!(
				target: LOG_TARGET,
				worker_pid = %pid,
				"failed to send a prepare request: {:?}",
				err,
			);
			return PrepareOutcome::Unreachable
		}

		// Wait for the result from the worker, keeping in mind that there may be a timeout, the
		// worker may get killed, or something along these lines.
		//
		// In that case we should handle these gracefully by writing the artifact file by ourselves.
		// We may potentially overwrite the artifact in rare cases where the worker didn't make
		// it to report back the result.

		#[derive(Debug)]
		enum Selected {
			Done,
			IoErr,
			Deadline,
		}

		let selected = futures::select! {
			res = framed_recv(&mut stream).fuse() => {
				match res {
					Ok(x) if x == &[1u8] => {
						tracing::debug!(
							target: LOG_TARGET,
							worker_pid = %pid,
							"promoting WIP artifact {} to {}",
							tmp_file.display(),
							artifact_path.display(),
						);

						async_std::fs::rename(&tmp_file, &artifact_path)
							.await
							.map(|_| Selected::Done)
							.unwrap_or_else(|err| {
								tracing::warn!(
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
					Ok(response_bytes) => {
						use sp_core::hexdisplay::HexDisplay;
						let bound_bytes =
							&response_bytes[..response_bytes.len().min(4)];
						tracing::warn!(
							target: LOG_TARGET,
							worker_pid = %pid,
							"received unexpected response from the prepare worker: {}",
							HexDisplay::from(&bound_bytes),
						);
						Selected::IoErr
					},
					Err(err) => {
						tracing::warn!(
							target: LOG_TARGET,
							worker_pid = %pid,
							"failed to recv a prepare response: {:?}",
							err,
						);
						Selected::IoErr
					}
				}
			},
			_ = Delay::new(COMPILATION_TIMEOUT).fuse() => Selected::Deadline,
		};

		match selected {
			Selected::Done => {
				renice(pid, NICENESS_FOREGROUND);
				PrepareOutcome::Concluded(IdleWorker { stream, pid })
			},
			Selected::IoErr | Selected::Deadline => {
				let bytes = Artifact::DidntMakeIt.serialize();
				// best effort: there is nothing we can do here if the write fails.
				if let Err(err) = async_std::fs::write(&artifact_path, &bytes).await {
					tracing::warn!(
						target: LOG_TARGET,
						worker_pid = %pid,
						"preparation didn't make it, because of `{:?}`: {:?}",
						selected,
						err,
					);
				}
				PrepareOutcome::DidntMakeIt
			},
		}
	})
	.await
}

pub async fn start_precheck_work(
	worker: IdleWorker,
	pvf: Pvf,
	precheck_timeout: Duration,
) -> PrecheckOutcome {
	let IdleWorker { mut stream, pid } = worker;

	tracing::debug!(
		target: LOG_TARGET,
		worker_pid = %pid,
		validation_code_hash = ?pvf.code_hash,
		"starting prechecking routine",
	);

	if let Err(error) = send_precheck_request(&mut stream, pvf.code).await {
		tracing::warn!(
			target: LOG_TARGET,
			worker_pid = %pid,
			validation_code_hash = ?pvf.code_hash,
			?error,
			"failed to send a precheck request",
		);
		return PrecheckOutcome::IoErr(error)
	}

	match async_std::future::timeout(precheck_timeout, recv_precheck_response(&mut stream)).await {
		Ok(Ok(precheck_result)) =>
			PrecheckOutcome::Ok { precheck_result, idle_worker: IdleWorker { stream, pid } },
		Ok(Err(err)) => PrecheckOutcome::IoErr(err),
		Err(_) => PrecheckOutcome::TimedOut,
	}
}

#[derive(Encode, Decode)]
enum Request {
	Prepare { code: Vec<u8>, tmp_file: Vec<u8> },
	Precheck { code: Vec<u8> },
}

/// Create a temporary file for an artifact at the given cache path and execute the given
/// future/closure passing the file path in.
///
/// The function will try best effort to not leave behind the temporary file.
async fn with_tmp_file<F, Fut>(pid: u32, cache_path: &Path, f: F) -> PrepareOutcome
where
	Fut: futures::Future<Output = PrepareOutcome>,
	F: FnOnce(PathBuf) -> Fut,
{
	let tmp_file = match tmpfile_in("prepare-artifact-", cache_path).await {
		Ok(f) => f,
		Err(err) => {
			tracing::warn!(
				target: LOG_TARGET,
				worker_pid = %pid,
				"failed to create a temp file for the artifact: {:?}",
				err,
			);
			return PrepareOutcome::DidntMakeIt
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
			tracing::warn!(
				target: LOG_TARGET,
				worker_pid = %pid,
				"failed to remove the tmp file: {:?}",
				err,
			);
		},
	}

	outcome
}

async fn send_prepare_request(
	stream: &mut UnixStream,
	code: Arc<Vec<u8>>,
	tmp_file: &Path,
) -> io::Result<()> {
	let request =
		Request::Prepare { code: code.to_vec(), tmp_file: path_to_bytes(tmp_file).into() };
	framed_send(stream, &request.encode()).await?;
	Ok(())
}

async fn send_precheck_request(stream: &mut UnixStream, code: Arc<Vec<u8>>) -> io::Result<()> {
	let request = Request::Precheck { code: code.to_vec() };
	framed_send(stream, &request.encode()).await?;
	Ok(())
}

async fn recv_precheck_response(stream: &mut UnixStream) -> io::Result<PrecheckResult> {
	let response_bytes = framed_recv(stream).await?;
	PrecheckResult::decode(&mut &response_bytes[..]).map_err(|e| {
		io::Error::new(
			io::ErrorKind::Other,
			format!("precheck pvf recv_response: decode error: {:?}", e),
		)
	})
}

async fn recv_request(stream: &mut UnixStream) -> io::Result<Request> {
	let response_bytes = framed_recv(stream).await?;
	Request::decode(&mut response_bytes.as_slice()).map_err(|e| {
		io::Error::new(
			io::ErrorKind::Other,
			format!("prepare worker recv_response: decode error: {:?}", e),
		)
	})
}

pub fn bump_priority(handle: &WorkerHandle) {
	let pid = handle.id();
	renice(pid, NICENESS_FOREGROUND);
}

fn renice(pid: u32, niceness: i32) {
	tracing::debug!(
		target: LOG_TARGET,
		worker_pid = %pid,
		"changing niceness to {}",
		niceness,
	);

	// Consider upstreaming this to the `nix` crate.
	unsafe {
		if -1 == libc::setpriority(libc::PRIO_PROCESS, pid, niceness) {
			let err = std::io::Error::last_os_error();
			tracing::warn!(target: LOG_TARGET, "failed to set the priority: {:?}", err);
		}
	}
}

/// The entrypoint that the spawned prepare worker should start with. The `socket_path` specifies
/// the path to the socket used to communicate with the host.
pub fn worker_entrypoint(socket_path: &str) {
	worker_event_loop("prepare", socket_path, |mut stream| async move {
		loop {
			let request = recv_request(&mut stream).await?;

			match request {
				Request::Prepare { code, tmp_file } => {
					let path = bytes_to_path(&tmp_file).ok_or_else(|| {
						io::Error::new(
							io::ErrorKind::Other,
							"prepare pvf recv_request: non utf-8 artifact path".to_string(),
						)
					})?;
					tracing::debug!(
						target: LOG_TARGET,
						worker_pid = %std::process::id(),
						"worker: preparing artifact",
					);
					let artifact_bytes = prepare_artifact(&code).serialize();

					// Write the serialized artifact into into a temp file.
					tracing::debug!(
						target: LOG_TARGET,
						worker_pid = %std::process::id(),
						"worker: writing artifact to {}",
						path.display(),
					);
					async_std::fs::write(&path, &artifact_bytes).await?;

					// Return back a byte that signals finishing the work.
					framed_send(&mut stream, &[1u8]).await?;
				},
				Request::Precheck { code } => {
					tracing::debug!(
						target: LOG_TARGET,
						worker_pid = %std::process::id(),
						"worker: prechecking pvf",
					);
					let precheck_result = precheck_pvf(code.as_slice());

					framed_send(&mut stream, &precheck_result.encode()).await?;
				},
			}
		}
	});
}

fn prepare_artifact(code: &[u8]) -> Artifact {
	let blob = match crate::executor_intf::prevalidate(code) {
		Err(err) => return Artifact::PrevalidationErr(format!("{:?}", err)),
		Ok(b) => b,
	};

	match crate::executor_intf::prepare(blob) {
		Ok(compiled_artifact) => Artifact::Compiled { compiled_artifact },
		Err(err) => Artifact::PreparationErr(format!("{:?}", err)),
	}
}

fn precheck_pvf(code: &[u8]) -> PrecheckResult {
	let blob = crate::executor_intf::prevalidate(code)
		.map_err(|err| PrecheckError::CompileError(format!("{:?}", err)))?;

	// Try to compile the artifact on a single thread in order to reduce
	// cpu number dependency and make this time easily reproducible.
	//
	// Also note that the artifact is not stored anywhere.
	match crate::executor_intf::prepare_single_threaded(blob) {
		Ok(_) => Ok(()),
		Err(err) => Err(PrecheckError::CompileError(format!("{:?}", err))),
	}
}
