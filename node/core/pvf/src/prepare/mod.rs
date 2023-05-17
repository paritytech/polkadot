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

use crate::artifacts::ArtifactId;
use parity_scale_codec::{Decode, Encode};
use polkadot_parachain::primitives::ValidationCodeHash;
use polkadot_primitives::ExecutorParams;
use sp_core::blake2_256;
use std::{
	cmp::{Eq, PartialEq},
	fmt,
	sync::Arc,
	time::Duration,
};

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

/// The kind of prepare job.
#[derive(Copy, Clone, Debug, Encode, Decode)]
pub enum PrepareJobKind {
	/// Compilation triggered by a candidate validation request.
	Compilation,
	/// A prechecking job.
	Prechecking,
}

/// A struct that carries the exhaustive set of data to prepare an artifact out of plain
/// Wasm binary
///
/// Should be cheap to clone.
#[derive(Clone, Encode, Decode)]
pub struct PvfPrepData {
	/// Wasm code (uncompressed)
	code: Arc<Vec<u8>>,
	/// Wasm code hash
	code_hash: ValidationCodeHash,
	/// Executor environment parameters for the session for which artifact is prepared
	executor_params: Arc<ExecutorParams>,
	/// Preparation timeout
	prep_timeout: Duration,
	/// The kind of preparation job.
	prep_kind: PrepareJobKind,
}

impl PvfPrepData {
	/// Returns an instance of the PVF out of the given PVF code and executor params.
	pub fn from_code(
		code: Vec<u8>,
		executor_params: ExecutorParams,
		prep_timeout: Duration,
		prep_kind: PrepareJobKind,
	) -> Self {
		let code = Arc::new(code);
		let code_hash = blake2_256(&code).into();
		let executor_params = Arc::new(executor_params);
		Self { code, code_hash, executor_params, prep_timeout, prep_kind }
	}

	/// Returns artifact ID that corresponds to the PVF with given executor params
	pub(crate) fn as_artifact_id(&self) -> ArtifactId {
		ArtifactId::new(self.code_hash, self.executor_params.hash())
	}

	/// Returns validation code hash for the PVF
	pub(crate) fn code_hash(&self) -> ValidationCodeHash {
		self.code_hash
	}

	/// Returns PVF code
	pub fn code(&self) -> Arc<Vec<u8>> {
		self.code.clone()
	}

	/// Returns executor params
	pub fn executor_params(&self) -> Arc<ExecutorParams> {
		self.executor_params.clone()
	}

	/// Returns preparation timeout.
	pub fn prep_timeout(&self) -> Duration {
		self.prep_timeout
	}

	/// Returns preparation kind.
	pub fn prep_kind(&self) -> PrepareJobKind {
		self.prep_kind
	}

	/// Creates a structure for tests
	#[cfg(test)]
	pub(crate) fn from_discriminator_and_timeout(num: u32, timeout: Duration) -> Self {
		let descriminator_buf = num.to_le_bytes().to_vec();
		Self::from_code(
			descriminator_buf,
			ExecutorParams::default(),
			timeout,
			PrepareJobKind::Compilation,
		)
	}

	#[cfg(test)]
	pub(crate) fn from_discriminator(num: u32) -> Self {
		Self::from_discriminator_and_timeout(num, crate::host::tests::TEST_PREPARATION_TIMEOUT)
	}

	#[cfg(test)]
	pub(crate) fn from_discriminator_precheck(num: u32) -> Self {
		let mut pvf =
			Self::from_discriminator_and_timeout(num, crate::host::tests::TEST_PREPARATION_TIMEOUT);
		pvf.prep_kind = PrepareJobKind::Prechecking;
		pvf
	}
}

impl fmt::Debug for PvfPrepData {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"Pvf {{ code, code_hash: {:?}, executor_params: {:?}, prep_timeout: {:?} }}",
			self.code_hash, self.executor_params, self.prep_timeout
		)
	}
}

impl PartialEq for PvfPrepData {
	fn eq(&self, other: &Self) -> bool {
		self.code_hash == other.code_hash &&
			self.executor_params.hash() == other.executor_params.hash()
	}
}

impl Eq for PvfPrepData {}
