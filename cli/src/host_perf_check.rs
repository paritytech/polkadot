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

use crate::error::PerfCheckError;
use log::info;
use nix::unistd;
use polkadot_erasure_coding::{obtain_chunks, reconstruct};
use polkadot_node_core_pvf::sp_maybe_compressed_blob;
use service::kusama_runtime;
use std::{
	fs::{self, OpenOptions},
	io::{self, Read, Write},
	path::Path,
	time::{Duration, Instant},
};

fn is_perf_check_done(path: &Path) -> io::Result<bool> {
	let host_name_max_len = unistd::SysconfVar::HOST_NAME_MAX as usize;

	let mut hostname_buf = vec![0u8; host_name_max_len];
	// Makes a call to FFI which is available on both Linux and MacOS.
	let hostname = unistd::gethostname(&mut hostname_buf)
		.map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

	let file = match fs::File::open(path) {
		Ok(file) => file,
		Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
		Err(err) => return Err(err),
	};
	let mut reader = io::BufReader::new(file);

	let mut buf = Vec::new();
	reader.read_to_end(&mut buf)?;

	Ok(hostname.to_bytes() == buf.as_slice())
}

fn save_check_passed_file(path: &Path) -> io::Result<()> {
	let hostname_max_len = unistd::SysconfVar::HOST_NAME_MAX as usize;
	let mut hostname_buf = vec![0u8; hostname_max_len];
	let hostname = unistd::gethostname(&mut hostname_buf)
		.map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

	let mut file = OpenOptions::new().truncate(true).create(true).write(true).open(path)?;

	file.write(hostname.to_bytes())?;

	Ok(())
}

pub fn host_perf_check(result_cache_path: &Path, force: bool) -> Result<(), PerfCheckError> {
	const CODE_SIZE_LIMIT: usize = 1024usize.pow(3);
	const CHECK_PASSED_FILE_NAME: &str = ".perf_check_passed";
	let wasm_code = kusama_runtime::WASM_BINARY.ok_or(PerfCheckError::WasmBinaryMissing)?;

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
	let code = sp_maybe_compressed_blob::decompress(wasm_code, CODE_SIZE_LIMIT)
		.or(Err(PerfCheckError::CodeDecompressionFailed))?;

	info!("Running the performance checks...");

	pvf_perf_check(code.as_ref())?;

	erasure_coding_perf_check(code.as_ref())?;

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
	test: impl Fn() -> Result<(), PerfCheckError>,
) -> Result<(), PerfCheckError> {
	let start = Instant::now();

	test()?;

	let elapsed = start.elapsed();
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

fn pvf_perf_check(code: &[u8]) -> Result<(), PerfCheckError> {
	const PREPARE_TIME_LIMIT: Duration = Duration::from_secs(20);

	perf_check("PVF-prepare", PREPARE_TIME_LIMIT, || {
		// Recreate the pipeline from the pvf prepare worker.
		let blob =
			polkadot_node_core_pvf::prevalidate(code.as_ref()).map_err(PerfCheckError::from)?;
		polkadot_node_core_pvf::prepare(blob).map_err(PerfCheckError::from)?;
		Ok(())
	})
}

fn erasure_coding_perf_check(data: &[u8]) -> Result<(), PerfCheckError> {
	const ERASURE_CODING_TIME_LIMIT: Duration = Duration::from_secs(1);
	const N: usize = 1024;

	perf_check("Erasure-coding", ERASURE_CODING_TIME_LIMIT, || {
		let chunks = obtain_chunks(N, &data).expect("The payload is not empty; qed");
		let indexed_chunks = chunks.iter().enumerate().map(|(i, chunk)| (chunk.as_slice(), i));

		let _: Vec<u8> =
			reconstruct(N, indexed_chunks).expect("Chunks were obtained above, cannot fail; qed");

		Ok(())
	})
}
