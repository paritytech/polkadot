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

use crate::LOG_TARGET;
use cpu_time::ProcessTime;
use futures::never::Never;
use std::{
	path::PathBuf,
	sync::mpsc::{Receiver, RecvTimeoutError},
	time::Duration,
};
use tokio::{
	io,
	net::UnixStream,
	runtime::{Handle, Runtime},
};

/// Some allowed overhead that we account for in the "CPU time monitor" thread's sleeps, on the
/// child process.
pub const JOB_TIMEOUT_OVERHEAD: Duration = Duration::from_millis(50);

/// Interprets the given bytes as a path. Returns `None` if the given bytes do not constitute a
/// a proper utf-8 string.
pub fn bytes_to_path(bytes: &[u8]) -> Option<PathBuf> {
	std::str::from_utf8(bytes).ok().map(PathBuf::from)
}

pub fn worker_event_loop<F, Fut>(
	debug_id: &'static str,
	socket_path: &str,
	node_version: Option<&str>,
	mut event_loop: F,
) where
	F: FnMut(Handle, UnixStream) -> Fut,
	Fut: futures::Future<Output = io::Result<Never>>,
{
	let worker_pid = std::process::id();
	gum::debug!(target: LOG_TARGET, %worker_pid, "starting pvf worker ({})", debug_id);

	// Check for a mismatch between the node and worker versions.
	if let Some(version) = node_version {
		if version != env!("SUBSTRATE_CLI_IMPL_VERSION") {
			gum::error!(
				target: LOG_TARGET,
				%worker_pid,
				"Node and worker version mismatch, node needs restarting, forcing shutdown",
			);
			kill_parent_node_in_emergency();
			let err: io::Result<Never> =
				Err(io::Error::new(io::ErrorKind::Unsupported, "Version mismatch"));
			gum::debug!(target: LOG_TARGET, %worker_pid, "quitting pvf worker({}): {:?}", debug_id, err);
			return
		}
	}

	// Run the main worker loop.
	let rt = Runtime::new().expect("Creates tokio runtime. If this panics the worker will die and the host will detect that and deal with it.");
	let handle = rt.handle();
	let err = rt
		.block_on(async move {
			let stream = UnixStream::connect(socket_path).await?;
			let _ = tokio::fs::remove_file(socket_path).await;

			let result = event_loop(handle.clone(), stream).await;

			result
		})
		// It's never `Ok` because it's `Ok(Never)`.
		.unwrap_err();

	gum::debug!(target: LOG_TARGET, %worker_pid, "quitting pvf worker ({}): {:?}", debug_id, err);

	// We don't want tokio to wait for the tasks to finish. We want to bring down the worker as fast
	// as possible and not wait for stalled validation to finish. This isn't strictly necessary now,
	// but may be in the future.
	rt.shutdown_background();
}

/// Loop that runs in the CPU time monitor thread on prepare and execute jobs. Continuously wakes up
/// and then either blocks for the remaining CPU time, or returns if we exceed the CPU timeout.
///
/// Returning `Some` indicates that we should send a `TimedOut` error to the host. Will return
/// `None` if the other thread finishes first, without us timing out.
///
/// NOTE: Sending a `TimedOut` error to the host will cause the worker, whether preparation or
/// execution, to be killed by the host. We do not kill the process here because it would interfere
/// with the proper handling of this error.
pub fn cpu_time_monitor_loop(
	cpu_time_start: ProcessTime,
	timeout: Duration,
	finished_rx: Receiver<()>,
) -> Option<Duration> {
	loop {
		let cpu_time_elapsed = cpu_time_start.elapsed();

		// Treat the timeout as CPU time, which is less subject to variance due to load.
		if cpu_time_elapsed <= timeout {
			// Sleep for the remaining CPU time, plus a bit to account for overhead. Note that the sleep
			// is wall clock time. The CPU clock may be slower than the wall clock.
			let sleep_interval = timeout.saturating_sub(cpu_time_elapsed) + JOB_TIMEOUT_OVERHEAD;
			match finished_rx.recv_timeout(sleep_interval) {
				// Received finish signal.
				Ok(()) => return None,
				// Timed out, restart loop.
				Err(RecvTimeoutError::Timeout) => continue,
				Err(RecvTimeoutError::Disconnected) => return None,
			}
		}

		return Some(cpu_time_elapsed)
	}
}

/// In case of node and worker version mismatch (as a result of in-place upgrade), send `SIGTERM`
/// to the node to tear it down and prevent it from raising disputes on valid candidates. Node
/// restart should be handled by the node owner. As node exits, unix sockets opened to workers
/// get closed by the OS and other workers receive error on socket read and also exit. Preparation
/// jobs are written to the temporary files that are renamed to real artifacts on the node side, so
/// no leftover artifacts are possible.
fn kill_parent_node_in_emergency() {
	unsafe {
		// SAFETY: `getpid()` never fails but may return "no-parent" (0) or "parent-init" (1) in
		// some corner cases, which is checked. `kill()` never fails.
		let ppid = libc::getppid();
		if ppid > 1 {
			libc::kill(ppid, libc::SIGTERM);
		}
	}
}
