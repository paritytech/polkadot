// Copyright 2023 Parity Technologies (UK) Ltd.
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

use parity_scale_codec::{Decode, Encode};
use tikv_jemalloc_ctl::{epoch, stats, Error};

#[derive(Clone)]
pub struct MemoryAllocationTracker {
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

		let allocated: u64 = self.allocated.read()? as _;
		let resident: u64 = self.resident.read()? as _;
		Ok(MemoryAllocationStats { allocated, resident })
	}
}

/// Statistics of collected memory metrics.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Encode, Decode)]
pub struct MemoryAllocationStats {
	/// Total resident memory, in bytes.
	pub resident: u64,
	/// Total allocated memory, in bytes.
	pub allocated: u64,
}
