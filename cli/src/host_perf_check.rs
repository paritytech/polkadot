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
use polkadot_node_core_pvf::sp_maybe_compressed_blob;
use polkadot_performance_test::{
	measure_erasure_coding, measure_pvf_prepare, PerfCheckError, ERASURE_CODING_N_VALIDATORS,
	ERASURE_CODING_TIME_LIMIT, PVF_PREPARE_TIME_LIMIT, VALIDATION_CODE_BOMB_LIMIT,
};
use std::time::Duration;

pub fn host_perf_check() -> Result<(), PerfCheckError> {
	let pvf_prepare_time_limit = time_limit_from_baseline(PVF_PREPARE_TIME_LIMIT);
	let erasure_coding_time_limit = time_limit_from_baseline(ERASURE_CODING_TIME_LIMIT);
	let wasm_code =
		polkadot_performance_test::WASM_BINARY.ok_or(PerfCheckError::WasmBinaryMissing)?;

	// Decompress the code before running checks.
	let code = sp_maybe_compressed_blob::decompress(wasm_code, VALIDATION_CODE_BOMB_LIMIT)
		.or(Err(PerfCheckError::CodeDecompressionFailed))?;

	info!("Running the performance checks...");

	perf_check("PVF-prepare", pvf_prepare_time_limit, || measure_pvf_prepare(code.as_ref()))?;

	perf_check("Erasure-coding", erasure_coding_time_limit, || {
		measure_erasure_coding(ERASURE_CODING_N_VALIDATORS, code.as_ref())
	})?;

	Ok(())
}

/// Returns a no-warning threshold for the given time limit.
fn green_threshold(duration: Duration) -> Duration {
	duration * 4 / 5
}

/// Returns an extended time limit to be used for the actual check.
fn time_limit_from_baseline(duration: Duration) -> Duration {
	duration * 3 / 2
}

fn perf_check(
	test_name: &str,
	time_limit: Duration,
	test: impl Fn() -> Result<Duration, PerfCheckError>,
) -> Result<(), PerfCheckError> {
	let elapsed = test()?;

	if elapsed < green_threshold(time_limit) {
		info!("🟢 {} performance check passed, elapsed: {:?}", test_name, elapsed);
		Ok(())
	} else if elapsed <= time_limit {
		info!(
			"🟡 {} performance check passed, {:?} limit almost exceeded, elapsed: {:?}",
			test_name, time_limit, elapsed
		);
		Ok(())
	} else {
		Err(PerfCheckError::TimeOut { elapsed, limit: time_limit })
	}
}
