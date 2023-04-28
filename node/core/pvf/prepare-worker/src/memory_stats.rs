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

//! Memory stats for preparation.
//!
//! Right now we gather three measurements:
//!
//! - `ru_maxrss` (resident set size) from `getrusage`.
//! - `resident` memory stat provided by `tikv-malloc-ctl`.
//! - `allocated` memory stat also from `tikv-malloc-ctl`.
//!
//! Currently we are only logging these for the purposes of gathering data. In the future, we may
//! use these stats to reject PVFs during pre-checking. See
//! <https://github.com/paritytech/polkadot/issues/6472#issuecomment-1381941762> for more
//! background.

/// Module for the memory tracker. The memory tracker runs in its own thread, where it polls memory
/// usage at an interval.
///
/// NOTE: Requires jemalloc enabled.
#[cfg(any(target_os = "linux", feature = "jemalloc-allocator"))]
pub mod memory_tracker {
	use crate::LOG_TARGET;
	use polkadot_node_core_pvf::MemoryAllocationStats;
	use std::{
		sync::mpsc::{Receiver, RecvTimeoutError, Sender},
		time::Duration,
	};
	use tikv_jemalloc_ctl::{epoch, stats, Error};
	use tokio::task::JoinHandle;

	#[derive(Clone)]
	struct MemoryAllocationTracker {
		epoch: tikv_jemalloc_ctl::epoch_mib,
		allocated: stats::allocated_mib,
		resident: stats::resident_mib,
	}

	impl MemoryAllocationTracker {
		pub fn new() -> Result<Self, Error> {
			Ok(Self {
				epoch: epoch::mib()?,
				allocated: stats::allocated::mib()?,
				resident: stats::resident::mib()?,
			})
		}

		pub fn snapshot(&self) -> Result<MemoryAllocationStats, Error> {
			// update stats by advancing the allocation epoch
			self.epoch.advance()?;

			// Convert to `u64`, as `usize` is not `Encode`able.
			let allocated = self.allocated.read()? as u64;
			let resident = self.resident.read()? as u64;
			Ok(MemoryAllocationStats { allocated, resident })
		}
	}

	/// Runs a thread in the background that observes memory statistics. The goal is to try to get
	/// accurate stats during preparation.
	///
	/// # Algorithm
	///
	/// 1. Create the memory tracker.
	///
	/// 2. Sleep for some short interval. Whenever we wake up, take a snapshot by updating the
	///    allocation epoch.
	///
	/// 3. When we receive a signal that preparation has completed, take one last snapshot and return
	///    the maximum observed values.
	///
	/// # Errors
	///
	/// For simplicity, any errors are returned as a string. As this is not a critical component, errors
	/// are used for informational purposes (logging) only.
	pub fn memory_tracker_loop(finished_rx: Receiver<()>) -> Result<MemoryAllocationStats, String> {
		// This doesn't need to be too fine-grained since preparation currently takes 3-10s or more.
		// Apart from that, there is not really a science to this number.
		const POLL_INTERVAL: Duration = Duration::from_millis(100);

		let tracker = MemoryAllocationTracker::new().map_err(|err| err.to_string())?;
		let mut max_stats = MemoryAllocationStats::default();

		let mut update_stats = || -> Result<(), String> {
			let current_stats = tracker.snapshot().map_err(|err| err.to_string())?;
			if current_stats.resident > max_stats.resident {
				max_stats.resident = current_stats.resident;
			}
			if current_stats.allocated > max_stats.allocated {
				max_stats.allocated = current_stats.allocated;
			}
			Ok(())
		};

		loop {
			// Take a snapshot and update the max stats.
			update_stats()?;

			// Sleep.
			match finished_rx.recv_timeout(POLL_INTERVAL) {
				// Received finish signal.
				Ok(()) => {
					update_stats()?;
					return Ok(max_stats)
				},
				// Timed out, restart loop.
				Err(RecvTimeoutError::Timeout) => continue,
				Err(RecvTimeoutError::Disconnected) =>
					return Err("memory_tracker_loop: finished_rx disconnected".into()),
			}
		}
	}

	/// Helper function to terminate the memory tracker thread and get the stats. Helps isolate all this
	/// error handling.
	pub async fn get_memory_tracker_loop_stats(
		fut: JoinHandle<Result<MemoryAllocationStats, String>>,
		tx: Sender<()>,
		worker_pid: u32,
	) -> Option<MemoryAllocationStats> {
		// Signal to the memory tracker thread to terminate.
		if let Err(err) = tx.send(()) {
			gum::warn!(
				target: LOG_TARGET,
				%worker_pid,
				"worker: error sending signal to memory tracker_thread: {}",
				err
			);
			None
		} else {
			// Join on the thread handle.
			match fut.await {
				Ok(Ok(stats)) => Some(stats),
				Ok(Err(err)) => {
					gum::warn!(
						target: LOG_TARGET,
						%worker_pid,
						"worker: error occurred in the memory tracker thread: {}", err
					);
					None
				},
				Err(err) => {
					gum::warn!(
						target: LOG_TARGET,
						%worker_pid,
						"worker: error joining on memory tracker thread: {}", err
					);
					None
				},
			}
		}
	}
}

/// Module for dealing with the `ru_maxrss` (peak resident memory) stat from `getrusage`.
///
/// NOTE: `getrusage` with the `RUSAGE_THREAD` parameter is only supported on Linux. `RUSAGE_SELF`
/// works on MacOS, but we need to get the max rss only for the preparation thread. Gettng it for
/// the current process would conflate the stats of previous jobs run by the process.
#[cfg(target_os = "linux")]
pub mod max_rss_stat {
	use crate::LOG_TARGET;
	use core::mem::MaybeUninit;
	use libc::{getrusage, rusage, RUSAGE_THREAD};
	use std::io;

	/// Get the rusage stats for the current thread.
	fn getrusage_thread() -> io::Result<rusage> {
		let mut result: MaybeUninit<rusage> = MaybeUninit::zeroed();

		// SAFETY: `result` is a valid pointer, so calling this is safe.
		if unsafe { getrusage(RUSAGE_THREAD, result.as_mut_ptr()) } == -1 {
			return Err(io::Error::last_os_error())
		}

		// SAFETY: `result` was successfully initialized by `getrusage`.
		unsafe { Ok(result.assume_init()) }
	}

	/// Gets the `ru_maxrss` for the current thread.
	pub fn get_max_rss_thread() -> io::Result<i64> {
		// `c_long` is either `i32` or `i64` depending on architecture. `i64::from` always works.
		getrusage_thread().map(|rusage| i64::from(rusage.ru_maxrss))
	}

	/// Extracts the max_rss stat and logs any error.
	pub fn extract_max_rss_stat(max_rss: io::Result<i64>, worker_pid: u32) -> Option<i64> {
		max_rss
			.map_err(|err| {
				gum::warn!(
					target: LOG_TARGET,
					%worker_pid,
					"error getting `ru_maxrss` in preparation thread: {}",
					err
				);
				err
			})
			.ok()
	}
}
