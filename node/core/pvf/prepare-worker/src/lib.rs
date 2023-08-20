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

//! Contains the logic for preparing PVFs. Used by the polkadot-prepare-worker binary.

mod executor_intf;
mod memory_stats;

pub use executor_intf::{prepare, prevalidate};

// NOTE: Initializing logging in e.g. tests will not have an effect in the workers, as they are
//       separate spawned processes. Run with e.g. `RUST_LOG=parachain::pvf-prepare-worker=trace`.
const LOG_TARGET: &str = "parachain::pvf-prepare-worker";

#[cfg(target_os = "linux")]
use crate::memory_stats::max_rss_stat::{extract_max_rss_stat, get_max_rss_thread};
#[cfg(any(target_os = "linux", feature = "jemalloc-allocator"))]
use crate::memory_stats::memory_tracker::{get_memory_tracker_loop_stats, memory_tracker_loop};
use parity_scale_codec::{Decode, Encode};
use polkadot_node_core_pvf_common::{
	error::{PrepareError, PrepareResult},
	executor_intf::Executor,
	framed_recv, framed_send,
	prepare::{MemoryStats, PrepareJobKind, PrepareStats},
	pvf::PvfPrepData,
	worker::{
		bytes_to_path, cpu_time_monitor_loop,
		security::LandlockStatus,
		stringify_panic_payload,
		thread::{self, WaitOutcome},
		worker_event_loop,
	},
	ProcessTime,
};
use polkadot_primitives::ExecutorParams;
use std::{
	path::PathBuf,
	sync::{mpsc::channel, Arc},
	time::Duration,
};
use tokio::{io, net::UnixStream};

/// Contains the bytes for a successfully compiled artifact.
pub struct CompiledArtifact(Vec<u8>);

impl CompiledArtifact {
	/// Creates a `CompiledArtifact`.
	pub fn new(code: Vec<u8>) -> Self {
		Self(code)
	}
}

impl AsRef<[u8]> for CompiledArtifact {
	fn as_ref(&self) -> &[u8] {
		self.0.as_slice()
	}
}

async fn recv_request(stream: &mut UnixStream) -> io::Result<(PvfPrepData, PathBuf)> {
	let pvf = framed_recv(stream).await?;
	let pvf = PvfPrepData::decode(&mut &pvf[..]).map_err(|e| {
		io::Error::new(
			io::ErrorKind::Other,
			format!("prepare pvf recv_request: failed to decode PvfPrepData: {}", e),
		)
	})?;
	let tmp_file = framed_recv(stream).await?;
	let tmp_file = bytes_to_path(&tmp_file).ok_or_else(|| {
		io::Error::new(
			io::ErrorKind::Other,
			"prepare pvf recv_request: non utf-8 artifact path".to_string(),
		)
	})?;
	Ok((pvf, tmp_file))
}

async fn send_response(stream: &mut UnixStream, result: PrepareResult) -> io::Result<()> {
	framed_send(stream, &result.encode()).await
}

/// The entrypoint that the spawned prepare worker should start with.
///
/// # Parameters
///
/// The `socket_path` specifies the path to the socket used to communicate with the host. The
/// `node_version`, if `Some`, is checked against the worker version. A mismatch results in
/// immediate worker termination. `None` is used for tests and in other situations when version
/// check is not necessary.
///
/// # Flow
///
/// This runs the following in a loop:
///
/// 1. Get the code and parameters for preparation from the host.
///
/// 2. Start a memory tracker in a separate thread.
///
/// 3. Start the CPU time monitor loop and the actual preparation in two separate threads.
///
/// 4. Wait on the two threads created in step 3.
///
/// 5. Stop the memory tracker and get the stats.
///
/// 6. If compilation succeeded, write the compiled artifact into a temporary file.
///
/// 7. Send the result of preparation back to the host. If any error occurred in the above steps, we
///    send that in the `PrepareResult`.
pub fn worker_entrypoint(
	socket_path: &str,
	node_version: Option<&str>,
	worker_version: Option<&str>,
) {
	worker_event_loop(
		"prepare",
		socket_path,
		node_version,
		worker_version,
		|mut stream| async move {
			let worker_pid = std::process::id();

			loop {
				let (pvf, temp_artifact_dest) = recv_request(&mut stream).await?;
				gum::debug!(
					target: LOG_TARGET,
					%worker_pid,
					"worker: preparing artifact",
				);

				let preparation_timeout = pvf.prep_timeout();
				let prepare_job_kind = pvf.prep_kind();
				let executor_params = (*pvf.executor_params()).clone();

				// Conditional variable to notify us when a thread is done.
				let condvar = thread::get_condvar();

				// Run the memory tracker in a regular, non-worker thread.
				#[cfg(any(target_os = "linux", feature = "jemalloc-allocator"))]
				let condvar_memory = Arc::clone(&condvar);
				#[cfg(any(target_os = "linux", feature = "jemalloc-allocator"))]
				let memory_tracker_thread = std::thread::spawn(|| memory_tracker_loop(condvar_memory));

				let cpu_time_start = ProcessTime::now();

				// Spawn a new thread that runs the CPU time monitor.
				let (cpu_time_monitor_tx, cpu_time_monitor_rx) = channel::<()>();
				let cpu_time_monitor_thread = thread::spawn_worker_thread(
					"cpu time monitor thread",
					move || {
						cpu_time_monitor_loop(
							cpu_time_start,
							preparation_timeout,
							cpu_time_monitor_rx,
						)
					},
					Arc::clone(&condvar),
					WaitOutcome::TimedOut,
				)?;
				// Spawn another thread for preparation.
				let prepare_thread = thread::spawn_worker_thread(
					"prepare thread",
					move || {
						// Try to enable landlock.
						#[cfg(target_os = "linux")]
					let landlock_status = polkadot_node_core_pvf_common::worker::security::landlock::try_restrict_thread()
						.map(LandlockStatus::from_ruleset_status)
						.map_err(|e| e.to_string());
						#[cfg(not(target_os = "linux"))]
						let landlock_status: Result<LandlockStatus, String> = Ok(LandlockStatus::NotEnforced);

						#[allow(unused_mut)]
						let mut result = prepare_artifact(pvf, cpu_time_start);

						// Get the `ru_maxrss` stat. If supported, call getrusage for the thread.
						#[cfg(target_os = "linux")]
						let mut result = result
							.map(|(artifact, elapsed)| (artifact, elapsed, get_max_rss_thread()));

						// If we are pre-checking, check for runtime construction errors.
						//
						// As pre-checking is more strict than just preparation in terms of memory
						// and time, it is okay to do extra checks here. This takes negligible time
						// anyway.
						if let PrepareJobKind::Prechecking = prepare_job_kind {
							result = result.and_then(|output| {
								runtime_construction_check(output.0.as_ref(), executor_params)?;
								Ok(output)
							});
						}

						(result, landlock_status)
					},
					Arc::clone(&condvar),
					WaitOutcome::Finished,
				)?;

				let outcome = thread::wait_for_threads(condvar);

				let result = match outcome {
					WaitOutcome::Finished => {
						let _ = cpu_time_monitor_tx.send(());

						match prepare_thread.join().unwrap_or_else(|err| {
							(
								Err(PrepareError::Panic(stringify_panic_payload(err))),
								Ok(LandlockStatus::Unavailable),
							)
						}) {
							(Err(err), _) => {
								// Serialized error will be written into the socket.
								Err(err)
							},
							(Ok(ok), landlock_status) => {
								#[cfg(not(target_os = "linux"))]
								let (artifact, cpu_time_elapsed) = ok;
								#[cfg(target_os = "linux")]
								let (artifact, cpu_time_elapsed, max_rss) = ok;

								// Stop the memory stats worker and get its observed memory stats.
								#[cfg(any(target_os = "linux", feature = "jemalloc-allocator"))]
								let memory_tracker_stats = get_memory_tracker_loop_stats(memory_tracker_thread, worker_pid)
									.await;
								let memory_stats = MemoryStats {
									#[cfg(any(
										target_os = "linux",
										feature = "jemalloc-allocator"
									))]
									memory_tracker_stats,
									#[cfg(target_os = "linux")]
									max_rss: extract_max_rss_stat(max_rss, worker_pid),
								};

								// Log if landlock threw an error.
								if let Err(err) = landlock_status {
									gum::warn!(
										target: LOG_TARGET,
										%worker_pid,
										"error enabling landlock: {}",
										err
									);
								}

								// Write the serialized artifact into a temp file.
								//
								// PVF host only keeps artifacts statuses in its memory,
								// successfully compiled code gets stored on the disk (and
								// consequently deserialized by execute-workers). The prepare worker
								// is only required to send `Ok` to the pool to indicate the
								// success.

								gum::debug!(
									target: LOG_TARGET,
									%worker_pid,
									"worker: writing artifact to {}",
									temp_artifact_dest.display(),
								);
								tokio::fs::write(&temp_artifact_dest, &artifact).await?;

								Ok(PrepareStats { cpu_time_elapsed, memory_stats })
							},
						}
					},
					// If the CPU thread is not selected, we signal it to end, the join handle is
					// dropped and the thread will finish in the background.
					WaitOutcome::TimedOut => {
						match cpu_time_monitor_thread.join() {
							Ok(Some(cpu_time_elapsed)) => {
								// Log if we exceed the timeout and the other thread hasn't
								// finished.
								gum::warn!(
									target: LOG_TARGET,
									%worker_pid,
									"prepare job took {}ms cpu time, exceeded prepare timeout {}ms",
									cpu_time_elapsed.as_millis(),
									preparation_timeout.as_millis(),
								);
								Err(PrepareError::TimedOut)
							},
							Ok(None) => Err(PrepareError::IoErr(
								"error communicating over closed channel".into(),
							)),
							// Errors in this thread are independent of the PVF.
							Err(err) => Err(PrepareError::IoErr(stringify_panic_payload(err))),
						}
					},
					WaitOutcome::Pending => unreachable!(
						"we run wait_while until the outcome is no longer pending; qed"
					),
				};

				send_response(&mut stream, result).await?;
			}
		},
	);
}

fn prepare_artifact(
	pvf: PvfPrepData,
	cpu_time_start: ProcessTime,
) -> Result<(CompiledArtifact, Duration), PrepareError> {
	let blob = match prevalidate(&pvf.code()) {
		Err(err) => return Err(PrepareError::Prevalidation(format!("{:?}", err))),
		Ok(b) => b,
	};

	match prepare(blob, &pvf.executor_params()) {
		Ok(compiled_artifact) => Ok(CompiledArtifact::new(compiled_artifact)),
		Err(err) => Err(PrepareError::Preparation(format!("{:?}", err))),
	}
	.map(|artifact| (artifact, cpu_time_start.elapsed()))
}

/// Try constructing the runtime to catch any instantiation errors during pre-checking.
fn runtime_construction_check(
	artifact_bytes: &[u8],
	executor_params: ExecutorParams,
) -> Result<(), PrepareError> {
	let executor = Executor::new(executor_params)
		.map_err(|e| PrepareError::RuntimeConstruction(format!("cannot create executor: {}", e)))?;

	// SAFETY: We just compiled this artifact.
	let result = unsafe { executor.create_runtime_from_bytes(&artifact_bytes) };
	result
		.map(|_runtime| ())
		.map_err(|err| PrepareError::RuntimeConstruction(format!("{:?}", err)))
}
