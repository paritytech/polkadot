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

//! Various things for testing other crates.
//!
//! N.B. This is not guarded with some feature flag. Overexposing items here may affect the final
//!      artifact even for production builds.

use polkadot_primitives::ExecutorParams;

pub mod worker_common {
	pub use crate::worker_common::{spawn_with_program_path, SpawnErr};
}

/// A function that emulates the stitches together behaviors of the preparation and the execution
/// worker in a single synchronous function.
pub fn validate_candidate(
	code: &[u8],
	params: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
	use crate::executor_intf::{prepare, prevalidate, Executor};

	let code = sp_maybe_compressed_blob::decompress(code, 10 * 1024 * 1024)
		.expect("Decompressing code failed");

	let blob = prevalidate(&code)?;
	let artifact = prepare(blob, &ExecutorParams::default())?;
	let tmpdir = tempfile::tempdir()?;
	let artifact_path = tmpdir.path().join("blob");
	std::fs::write(&artifact_path, &artifact)?;

	let executor = Executor::new(ExecutorParams::default())?;
	let result = unsafe {
		// SAFETY: This is trivially safe since the artifact is obtained by calling `prepare`
		//         and is written into a temporary directory in an unmodified state.
		executor.execute(&artifact_path, params)?
	};

	Ok(result)
}
