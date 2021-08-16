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

//! Memory tracking statistics.
//!
//! Many subsystems have common interests such as canceling a bunch of spawned jobs,
//! or determining what their validator ID is. These common interests are factored into
//! this module.
//!
//! This crate also reexports Prometheus metric types which are expected to be implemented by subsystems.

// #[cfg(not(feature = "memory-stats"))]
// use std::convert::Infallible;

use jemalloc_ctl::{epoch, stats, Result};

/// Accessor to the allocator internals.
#[derive(Clone)]
pub struct MemoryAllocationTracker {
	epoch: jemalloc_ctl::epoch_mib,
	allocated: stats::allocated_mib,
	resident: stats::resident_mib,
}

impl MemoryAllocationTracker {
	/// Create an instance of an allocation tracker.
	pub fn new() -> Result<Self> {
		Ok(Self {
			epoch: epoch::mib()?,
			allocated: stats::allocated::mib()?,
			resident: stats::resident::mib()?,
		})
	}

	/// Create an allocation snapshot.
	pub fn snapshot(&self) -> Result<MemoryAllocationSnapshot> {
		// update stats by advancing the allocation epoch
		self.epoch.advance()?;

		let allocated: u64 = self.allocated.read()? as _;
		let resident: u64 = self.resident.read()? as _;
		Ok(MemoryAllocationSnapshot { allocated, resident })
	}
}

/// Snapshot of collected memory metrics.
#[derive(Debug, Clone)]
pub struct MemoryAllocationSnapshot {
	/// Total resident memory, in bytes.
	pub resident: u64,
	/// Total allocated memory, in bytes.
	pub allocated: u64,
}
