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
use nix::unistd;
use polkadot_node_core_pvf::sp_maybe_compressed_blob;
use polkadot_performance_test::{
	measure_erasure_coding, measure_pvf_prepare, PerfCheckError, ERASURE_CODING_N_VALIDATORS,
	ERASURE_CODING_TIME_LIMIT, PVF_PREPARE_TIME_LIMIT, VALIDATION_CODE_BOMB_LIMIT,
};
use std::{fs, io, path::Path, time::Duration};

const HOSTNAME_MAX_LEN: usize = unistd::SysconfVar::HOST_NAME_MAX as usize;

fn is_perf_check_done(path: &Path) -> io::Result<bool> {
	let mut hostname_buf = vec![0u8; HOSTNAME_MAX_LEN];
	// Makes a call to FFI which is available on both Linux and MacOS.
	let hostname = unistd::gethostname(&mut hostname_buf)
		.map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

	let buf = match fs::read(path) {
		Ok(buf) => buf,
		Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
		Err(err) => return Err(err),
	};

	Ok(hostname.to_bytes() == buf.as_slice())
}

fn save_check_passed_file(path: &Path) -> io::Result<()> {
	let mut hostname_buf = vec![0u8; HOSTNAME_MAX_LEN];
	let hostname = unistd::gethostname(&mut hostname_buf)
		.map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

	fs::write(path, hostname.to_bytes())
}

pub fn host_perf_check(result_cache_path: &Path, force: bool) -> Result<(), PerfCheckError> {
	const CHECK_PASSED_FILE_NAME: &str = ".perf_check_passed";

	let wasm_code =
		polkadot_performance_test::WASM_BINARY.ok_or(PerfCheckError::WasmBinaryMissing)?;

	let check_passed_file_path = result_cache_path.join(CHECK_PASSED_FILE_NAME);

	if !force {
		if let Ok(true) = is_perf_check_done(&check_passed_file_path) {
			info!(
				"Performance check skipped: already passed (cached at {:?})",
				check_passed_file_path
			);
			return Ok(())
		}
	}

	// Decompress the code before running checks.
	let code = sp_maybe_compressed_blob::decompress(wasm_code, VALIDATION_CODE_BOMB_LIMIT)
		.or(Err(PerfCheckError::CodeDecompressionFailed))?;

	info!("Running the performance checks...");

	perf_check("PVF-prepare", PVF_PREPARE_TIME_LIMIT, || measure_pvf_prepare(code.as_ref()))?;

	perf_check("Erasure-coding", ERASURE_CODING_TIME_LIMIT, || {
		measure_erasure_coding(ERASURE_CODING_N_VALIDATORS, code.as_ref())
	})?;

	// Persist successful result.
	if let Err(err) = save_check_passed_file(&check_passed_file_path) {
		info!("Couldn't persist check result at {:?}: {}", check_passed_file_path, err.to_string());
	}

	Ok(())
}

fn green_threshold(duration: Duration) -> Duration {
	duration * 4 / 5
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
