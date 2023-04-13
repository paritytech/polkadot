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

use tikv_jemalloc_ctl::stats;

#[derive(Clone)]
pub struct MemoryAllocationTracker {
	epoch: tikv_jemalloc_ctl::epoch_mib,
	allocated: stats::allocated_mib,
	resident: stats::resident_mib,
}

impl MemoryAllocationTracker {
	pub fn new() -> Result<Self, tikv_jemalloc_ctl::Error> {
		Ok(Self {
			epoch: tikv_jemalloc_ctl::epoch::mib()?,
			allocated: stats::allocated::mib()?,
			resident: stats::resident::mib()?,
		})
	}

	pub fn snapshot(&self) -> Result<MemoryAllocationSnapshot, tikv_jemalloc_ctl::Error> {
		// update stats by advancing the allocation epoch
		self.epoch.advance()?;

		let allocated = self.allocated.read()?;
		let resident = self.resident.read()?;
		Ok(MemoryAllocationSnapshot { allocated, resident })
	}
}

/// Snapshot of collected memory metrics.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct MemoryAllocationSnapshot {
	/// Total resident memory, in bytes.
	pub resident: usize,
	/// Total allocated memory, in bytes.
	pub allocated: usize,
}
