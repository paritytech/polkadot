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

//! Preparation part of pipeline
//!
//! The validation host spins up two processes: the queue (by running [`start_queue`]) and the pool
//! (by running [`start_pool`]).
//!
//! The pool will spawn workers in new processes and those should execute pass control to
//! `polkadot_node_core_pvf_worker::prepare_worker_entrypoint`.

mod pool;
mod queue;
mod worker_intf;

pub use pool::start as start_pool;
pub use queue::{start as start_queue, FromQueue, ToQueue};

use parity_scale_codec::{Decode, Encode};

/// Preparation statistics, including the CPU time and memory taken.
#[derive(Debug, Clone, Default, Encode, Decode)]
pub struct PrepareStats {
	/// The CPU time that elapsed for the preparation job.
	pub cpu_time_elapsed: std::time::Duration,
	/// The observed memory statistics for the preparation job.
	pub memory_stats: MemoryStats,
}

/// Helper struct to contain all the memory stats, including `MemoryAllocationStats` and, if
/// supported by the OS, `ru_maxrss`.
#[derive(Clone, Debug, Default, Encode, Decode)]
pub struct MemoryStats {
	/// Memory stats from `tikv_jemalloc_ctl`.
	#[cfg(any(target_os = "linux", feature = "jemalloc-allocator"))]
	pub memory_tracker_stats: Option<MemoryAllocationStats>,
	/// `ru_maxrss` from `getrusage`. `None` if an error occurred.
	#[cfg(target_os = "linux")]
	pub max_rss: Option<i64>,
}

/// Statistics of collected memory metrics.
#[cfg(any(target_os = "linux", feature = "jemalloc-allocator"))]
#[derive(Clone, Debug, Default, Encode, Decode)]
pub struct MemoryAllocationStats {
	/// Total resident memory, in bytes.
	pub resident: u64,
	/// Total allocated memory, in bytes.
	pub allocated: u64,
}
