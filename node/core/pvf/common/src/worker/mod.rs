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

//! Functionality common to both prepare and execute workers.

pub mod security;

use crate::LOG_TARGET;
use cpu_time::ProcessTime;
use futures::never::Never;
use std::{
	any::Any,
	path::PathBuf,
	sync::mpsc::{Receiver, RecvTimeoutError},
	time::Duration,
};
use tokio::{io, net::UnixStream, runtime::Runtime};

/// Use this macro to declare a `fn main() {}` that will create an executable that can be used for
/// spawning the desired worker.
#[macro_export]
macro_rules! decl_worker_main {
	($expected_command:expr, $entrypoint:expr, $worker_version:expr) => {
		fn print_help(expected_command: &str) {
			println!("{} {}", expected_command, $worker_version);
			println!();
			println!("PVF worker that is called by polkadot.");
		}

		fn main() {
			$crate::sp_tracing::try_init_simple();

			let args = std::env::args().collect::<Vec<_>>();
			if args.len() == 1 {
				print_help($expected_command);
				return
			}

			match args[1].as_ref() {
				"--help" | "-h" => {
					print_help($expected_command);
					return
				},
				"--version" | "-v" => {
					println!("{}", $worker_version);
					return
				},
				subcommand => {
					// Must be passed for compatibility with the single-binary test workers.
					if subcommand != $expected_command {
						panic!(
							"trying to run {} binary with the {} subcommand",
							$expected_command, subcommand
						)
					}
				},
			}

			let mut node_version = None;
			let mut socket_path: &str = "";

			for i in (2..args.len()).step_by(2) {
				match args[i].as_ref() {
					"--socket-path" => socket_path = args[i + 1].as_str(),
					"--node-impl-version" => node_version = Some(args[i + 1].as_str()),
					arg => panic!("Unexpected argument found: {}", arg),
				}
			}

			$entrypoint(&socket_path, node_version, Some($worker_version));
		}
	};
}

/// Some allowed overhead that we account for in the "CPU time monitor" thread's sleeps, on the
/// child process.
pub const JOB_TIMEOUT_OVERHEAD: Duration = Duration::from_millis(50);

/// Interprets the given bytes as a path. Returns `None` if the given bytes do not constitute a
/// a proper utf-8 string.
pub fn bytes_to_path(bytes: &[u8]) -> Option<PathBuf> {
	std::str::from_utf8(bytes).ok().map(PathBuf::from)
}

// The worker version must be passed in so that we accurately get the version of the worker, and not
// the version that this crate was compiled with.
pub fn worker_event_loop<F, Fut>(
	debug_id: &'static str,
	socket_path: &str,
	node_version: Option<&str>,
	worker_version: Option<&str>,
	mut event_loop: F,
) where
	F: FnMut(UnixStream) -> Fut,
	Fut: futures::Future<Output = io::Result<Never>>,
{
	let worker_pid = std::process::id();
	gum::debug!(target: LOG_TARGET, %worker_pid, "starting pvf worker ({})", debug_id);

	// Check for a mismatch between the node and worker versions.
	if let (Some(node_version), Some(worker_version)) = (node_version, worker_version) {
		if node_version != worker_version {
			gum::error!(
				target: LOG_TARGET,
				%worker_pid,
				%node_version,
				%worker_version,
				"Node and worker version mismatch, node needs restarting, forcing shutdown",
			);
			kill_parent_node_in_emergency();
			let err = io::Error::new(io::ErrorKind::Unsupported, "Version mismatch");
			worker_shutdown_message(debug_id, worker_pid, err);
			return
		}
	}

	remove_env_vars(debug_id);

	// Run the main worker loop.
	let rt = Runtime::new().expect("Creates tokio runtime. If this panics the worker will die and the host will detect that and deal with it.");
	let err = rt
		.block_on(async move {
			let stream = UnixStream::connect(socket_path).await?;
			let _ = tokio::fs::remove_file(socket_path).await;

			let result = event_loop(stream).await;

			result
		})
		// It's never `Ok` because it's `Ok(Never)`.
		.unwrap_err();

	worker_shutdown_message(debug_id, worker_pid, err);

	// We don't want tokio to wait for the tasks to finish. We want to bring down the worker as fast
	// as possible and not wait for stalled validation to finish. This isn't strictly necessary now,
	// but may be in the future.
	rt.shutdown_background();
}

/// Delete all env vars to prevent malicious code from accessing them.
fn remove_env_vars(debug_id: &'static str) {
	for (key, value) in std::env::vars_os() {
		// TODO: *theoretically* the value (or mere presence) of `RUST_LOG` can be a source of
		// randomness for malicious code. In the future we can remove it also and log in the host;
		// see <https://github.com/paritytech/polkadot/issues/7117>.
		if key == "RUST_LOG" {
			continue
		}

		// In case of a key or value that would cause [`env::remove_var` to
		// panic](https://doc.rust-lang.org/std/env/fn.remove_var.html#panics), we first log a
		// warning and then proceed to attempt to remove the env var.
		let mut err_reasons = vec![];
		let (key_str, value_str) = (key.to_str(), value.to_str());
		if key.is_empty() {
			err_reasons.push("key is empty");
		}
		if key_str.is_some_and(|s| s.contains('=')) {
			err_reasons.push("key contains '='");
		}
		if key_str.is_some_and(|s| s.contains('\0')) {
			err_reasons.push("key contains null character");
		}
		if value_str.is_some_and(|s| s.contains('\0')) {
			err_reasons.push("value contains null character");
		}
		if !err_reasons.is_empty() {
			gum::warn!(
				target: LOG_TARGET,
				%debug_id,
				?key,
				?value,
				"Attempting to remove badly-formatted env var, this may cause the PVF worker to crash. Please remove it yourself. Reasons: {:?}",
				err_reasons
			);
		}

		std::env::remove_var(key);
	}
}

/// Provide a consistent message on worker shutdown.
fn worker_shutdown_message(debug_id: &'static str, worker_pid: u32, err: io::Error) {
	gum::debug!(target: LOG_TARGET, %worker_pid, "quitting pvf worker ({}): {:?}", debug_id, err);
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
			// Sleep for the remaining CPU time, plus a bit to account for overhead. (And we don't
			// want to wake up too often -- so, since we just want to halt the worker thread if it
			// stalled, we can sleep longer than necessary.) Note that the sleep is wall clock time.
			// The CPU clock may be slower than the wall clock.
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

/// Attempt to convert an opaque panic payload to a string.
///
/// This is a best effort, and is not guaranteed to provide the most accurate value.
pub fn stringify_panic_payload(payload: Box<dyn Any + Send + 'static>) -> String {
	match payload.downcast::<&'static str>() {
		Ok(msg) => msg.to_string(),
		Err(payload) => match payload.downcast::<String>() {
			Ok(msg) => *msg,
			// At least we tried...
			Err(_) => "unknown panic payload".to_string(),
		},
	}
}

/// In case of node and worker version mismatch (as a result of in-place upgrade), send `SIGTERM`
/// to the node to tear it down and prevent it from raising disputes on valid candidates. Node
/// restart should be handled by the node owner. As node exits, Unix sockets opened to workers
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

/// Functionality related to threads spawned by the workers.
///
/// The motivation for this module is to coordinate worker threads without using async Rust.
pub mod thread {
	use std::{
		panic,
		sync::{Arc, Condvar, Mutex},
		thread,
		time::Duration,
	};

	/// Contains the outcome of waiting on threads, or `Pending` if none are ready.
	#[derive(Debug, Clone, Copy)]
	pub enum WaitOutcome {
		Finished,
		TimedOut,
		Pending,
	}

	impl WaitOutcome {
		pub fn is_pending(&self) -> bool {
			matches!(self, Self::Pending)
		}
	}

	/// Helper type.
	pub type Cond = Arc<(Mutex<WaitOutcome>, Condvar)>;

	/// Gets a condvar initialized to `Pending`.
	pub fn get_condvar() -> Cond {
		Arc::new((Mutex::new(WaitOutcome::Pending), Condvar::new()))
	}

	/// Runs a worker thread. Will first enable security features, and afterwards notify the threads
	/// waiting on the condvar. Catches panics during execution and resumes the panics after
	/// triggering the condvar, so that the waiting thread is notified on panics.
	///
	/// # Returns
	///
	/// Returns the thread's join handle. Calling `.join()` on it returns the result of executing
	/// `f()`, as well as whether we were able to enable sandboxing.
	pub fn spawn_worker_thread<F, R>(
		name: &str,
		f: F,
		cond: Cond,
		outcome: WaitOutcome,
	) -> std::io::Result<thread::JoinHandle<R>>
	where
		F: FnOnce() -> R,
		F: Send + 'static + panic::UnwindSafe,
		R: Send + 'static,
	{
		thread::Builder::new()
			.name(name.into())
			.spawn(move || cond_notify_on_done(f, cond, outcome))
	}

	/// Runs a worker thread with the given stack size. See [`spawn_worker_thread`].
	pub fn spawn_worker_thread_with_stack_size<F, R>(
		name: &str,
		f: F,
		cond: Cond,
		outcome: WaitOutcome,
		stack_size: usize,
	) -> std::io::Result<thread::JoinHandle<R>>
	where
		F: FnOnce() -> R,
		F: Send + 'static + panic::UnwindSafe,
		R: Send + 'static,
	{
		thread::Builder::new()
			.name(name.into())
			.stack_size(stack_size)
			.spawn(move || cond_notify_on_done(f, cond, outcome))
	}

	/// Runs a function, afterwards notifying the threads waiting on the condvar. Catches panics and
	/// resumes them after triggering the condvar, so that the waiting thread is notified on panics.
	fn cond_notify_on_done<F, R>(f: F, cond: Cond, outcome: WaitOutcome) -> R
	where
		F: FnOnce() -> R,
		F: panic::UnwindSafe,
	{
		let result = panic::catch_unwind(|| f());
		cond_notify_all(cond, outcome);
		match result {
			Ok(inner) => return inner,
			Err(err) => panic::resume_unwind(err),
		}
	}

	/// Helper function to notify all threads waiting on this condvar.
	fn cond_notify_all(cond: Cond, outcome: WaitOutcome) {
		let (lock, cvar) = &*cond;
		let mut flag = lock.lock().unwrap();
		if !flag.is_pending() {
			// Someone else already triggered the condvar.
			return
		}
		*flag = outcome;
		cvar.notify_all();
	}

	/// Block the thread while it waits on the condvar.
	pub fn wait_for_threads(cond: Cond) -> WaitOutcome {
		let (lock, cvar) = &*cond;
		let guard = cvar.wait_while(lock.lock().unwrap(), |flag| flag.is_pending()).unwrap();
		*guard
	}

	/// Block the thread while it waits on the condvar or on a timeout. If the timeout is hit,
	/// returns `None`.
	#[cfg_attr(not(any(target_os = "linux", feature = "jemalloc-allocator")), allow(dead_code))]
	pub fn wait_for_threads_with_timeout(cond: &Cond, dur: Duration) -> Option<WaitOutcome> {
		let (lock, cvar) = &**cond;
		let result = cvar
			.wait_timeout_while(lock.lock().unwrap(), dur, |flag| flag.is_pending())
			.unwrap();
		if result.1.timed_out() {
			None
		} else {
			Some(*result.0)
		}
	}

	#[cfg(test)]
	mod tests {
		use super::*;
		use assert_matches::assert_matches;

		#[test]
		fn get_condvar_should_be_pending() {
			let condvar = get_condvar();
			let outcome = *condvar.0.lock().unwrap();
			assert!(outcome.is_pending());
		}

		#[test]
		fn wait_for_threads_with_timeout_return_none_on_time_out() {
			let condvar = Arc::new((Mutex::new(WaitOutcome::Pending), Condvar::new()));
			let outcome = wait_for_threads_with_timeout(&condvar, Duration::from_millis(100));
			assert!(outcome.is_none());
		}

		#[test]
		fn wait_for_threads_with_timeout_returns_outcome() {
			let condvar = Arc::new((Mutex::new(WaitOutcome::Pending), Condvar::new()));
			let condvar2 = condvar.clone();
			cond_notify_all(condvar2, WaitOutcome::Finished);
			let outcome = wait_for_threads_with_timeout(&condvar, Duration::from_secs(2));
			assert_matches!(outcome.unwrap(), WaitOutcome::Finished);
		}

		#[test]
		fn spawn_worker_thread_should_notify_on_done() {
			let condvar = Arc::new((Mutex::new(WaitOutcome::Pending), Condvar::new()));
			let response =
				spawn_worker_thread("thread", || 2, condvar.clone(), WaitOutcome::TimedOut);
			let (lock, _) = &*condvar;
			let r = response.unwrap().join().unwrap();
			assert_eq!(r, 2);
			assert_matches!(*lock.lock().unwrap(), WaitOutcome::TimedOut);
		}

		#[test]
		fn spawn_worker_should_not_change_finished_outcome() {
			let condvar = Arc::new((Mutex::new(WaitOutcome::Finished), Condvar::new()));
			let response =
				spawn_worker_thread("thread", move || 2, condvar.clone(), WaitOutcome::TimedOut);

			let r = response.unwrap().join().unwrap();
			assert_eq!(r, 2);
			assert_matches!(*condvar.0.lock().unwrap(), WaitOutcome::Finished);
		}

		#[test]
		fn cond_notify_on_done_should_update_wait_outcome_when_panic() {
			let condvar = Arc::new((Mutex::new(WaitOutcome::Pending), Condvar::new()));
			let err = panic::catch_unwind(panic::AssertUnwindSafe(|| {
				cond_notify_on_done(|| panic!("test"), condvar.clone(), WaitOutcome::Finished)
			}));

			assert_matches!(*condvar.0.lock().unwrap(), WaitOutcome::Finished);
			assert!(err.is_err());
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::mpsc::channel;

	#[test]
	fn cpu_time_monitor_loop_should_return_time_elapsed() {
		let cpu_time_start = ProcessTime::now();
		let timeout = Duration::from_secs(0);
		let (_tx, rx) = channel();
		let result = cpu_time_monitor_loop(cpu_time_start, timeout, rx);
		assert_ne!(result, None);
	}

	#[test]
	fn cpu_time_monitor_loop_should_return_none() {
		let cpu_time_start = ProcessTime::now();
		let timeout = Duration::from_secs(10);
		let (tx, rx) = channel();
		tx.send(()).unwrap();
		let result = cpu_time_monitor_loop(cpu_time_start, timeout, rx);
		assert_eq!(result, None);
	}
}
