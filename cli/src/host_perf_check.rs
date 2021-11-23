// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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

use log::info;
use polkadot_node_core_pvf::{sc_executor_common, sp_maybe_compressed_blob};
use std::{fs::OpenOptions, path::Path, time::Duration};

#[allow(missing_docs)]
#[derive(thiserror::Error, Debug)]
pub enum PerfCheckError {
	#[error("This subcommand is only available in release mode")]
	DebugBuildNotSupported,

	#[error("Failed to decompress wasm code")]
	CodeDecompressionFailed,

	#[error(transparent)]
	Wasm(#[from] sc_executor_common::error::WasmError),

	#[error(
		"Performance check not passed: exceeded the {limit:?} time limit, elapsed: {elapsed:?}"
	)]
	TimeOut { elapsed: Duration, limit: Duration },
}

/// Runs a performance check via compiling sample wasm code with a timeout.
/// Should only be run in release build since the check would take too much time otherwise.
/// Returns `Ok` immediately if the check has been passed previously.
#[allow(dead_code)]
pub fn host_perf_check() -> Result<(), PerfCheckError> {
	const PERF_CHECK_TIME_LIMIT: Duration = Duration::from_secs(20);
	const CODE_SIZE_LIMIT: usize = 1024usize.pow(3);
	const WASM_CODE: &[u8] = include_bytes!(
		"../../target/release/wbuild/kusama-runtime/kusama_runtime.compact.compressed.wasm"
	);
	const CHECK_PASSED_FILE_NAME: &str = ".perf_check_passed";

	// We will try to save a dummy file to the same path as the polkadot binary
	// to make it independent from the current directory.
	let check_passed_path = std::env::current_exe()
		.map(|mut path| {
			path.pop();
			path
		})
		.unwrap_or_default()
		.join(CHECK_PASSED_FILE_NAME);

	// To avoid running the check on every launch we create a dummy dot-file on success.
	if Path::new(&check_passed_path).exists() {
		info!("Performance check skipped: already passed");
		return Ok(())
	}

	info!("Running the performance check...");
	let start = std::time::Instant::now();

	// Recreate the pipeline from the pvf prepare worker.
	let code = sp_maybe_compressed_blob::decompress(WASM_CODE, CODE_SIZE_LIMIT)
		.or(Err(PerfCheckError::CodeDecompressionFailed))?;
	let blob = polkadot_node_core_pvf::prevalidate(code.as_ref()).map_err(PerfCheckError::from)?;
	let _ = polkadot_node_core_pvf::prepare(blob).map_err(PerfCheckError::from)?;

	let elapsed = start.elapsed();
	if elapsed <= PERF_CHECK_TIME_LIMIT {
		info!("Performance check passed, elapsed: {:?}", start.elapsed());
		// `touch` a dummy file.
		let _ = OpenOptions::new().create(true).write(true).open(Path::new(&check_passed_path));
		Ok(())
	} else {
		Err(PerfCheckError::TimeOut { elapsed, limit: PERF_CHECK_TIME_LIMIT })
	}
}
