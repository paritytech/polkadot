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

	pub fn snapshot(&self) -> Result<MemoryAllocationSnapshot, Error> {
		// update stats by advancing the allocation epoch
		self.epoch.advance()?;

		let allocated: u64 = self.allocated.read()? as _;
		let resident: u64 = self.resident.read()? as _;
		Ok(MemoryAllocationSnapshot { allocated, resident })
	}
}

/// Snapshot of collected memory metrics.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct MemoryAllocationSnapshot {
	/// Total resident memory, in bytes.
	pub resident: u64,
	/// Total allocated memory, in bytes.
	pub allocated: u64,
}
