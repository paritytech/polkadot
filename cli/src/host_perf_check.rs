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

	let mut host_name = vec![0u8; host_name_max_len];
	let mut buf = host_name.clone();
	// Makes a call to FFI which is available on both Linux and MacOS.
	unistd::gethostname(&mut host_name).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

	let file = match fs::File::open(path) {
		Ok(file) => file,
		Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
		Err(err) => return Err(err),
	};
	let mut reader = io::BufReader::new(file);

	reader.read_exact(&mut buf)?;

	Ok(host_name == buf)
}

fn save_check_passed_file(path: &Path) -> io::Result<()> {
	let host_name_max_len = unistd::SysconfVar::HOST_NAME_MAX as usize;
	let mut host_name = vec![0u8; host_name_max_len];
	unistd::gethostname(&mut host_name).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

	let mut file = OpenOptions::new().truncate(true).create(true).write(true).open(path)?;

	file.write(&host_name)?;

	Ok(())
}

pub fn host_perf_check(result_cache_path: &Path) -> Result<(), PerfCheckError> {
	const CODE_SIZE_LIMIT: usize = 1024usize.pow(3);
	const CHECK_PASSED_FILE_NAME: &str = ".perf_check_passed";
	let wasm_code = kusama_runtime::WASM_BINARY.ok_or(PerfCheckError::WasmBinaryMissing)?;

	let check_passed_file_path = result_cache_path.join(CHECK_PASSED_FILE_NAME);

	if let Ok(true) = is_perf_check_done(&check_passed_file_path) {
		info!("Performance check skipped: already passed (cached at {:?})", check_passed_file_path);
		return Ok(())
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

fn pvf_perf_check(code: &[u8]) -> Result<(), PerfCheckError> {
	const PREPARE_TIME_LIMIT: Duration = Duration::from_secs(20);

	let start = Instant::now();

	// Recreate the pipeline from the pvf prepare worker.
	let blob = polkadot_node_core_pvf::prevalidate(code.as_ref()).map_err(PerfCheckError::from)?;
	polkadot_node_core_pvf::prepare(blob).map_err(PerfCheckError::from)?;

	let elapsed = start.elapsed();
	if elapsed <= PREPARE_TIME_LIMIT {
		info!("PVF performance check passed, elapsed: {:?}", elapsed);

		Ok(())
	} else {
		Err(PerfCheckError::TimeOut { elapsed, limit: PREPARE_TIME_LIMIT })
	}
}

fn erasure_coding_perf_check(data: &[u8]) -> Result<(), PerfCheckError> {
	const ERASURE_CODING_TIME_LIMIT: Duration = Duration::from_secs(1);
	const N: usize = 1024;

	let start = Instant::now();

	let chunks = obtain_chunks(N, &data).expect("The payload is not empty; qed");
	let indexed_chunks = chunks.iter().enumerate().map(|(i, chunk)| (chunk.as_slice(), i));

	let _: Vec<u8> =
		reconstruct(N, indexed_chunks).expect("Chunks were obtained above, cannot fail; qed");

	let elapsed = start.elapsed();
	if elapsed <= ERASURE_CODING_TIME_LIMIT {
		info!("Erasure-coding performance check passed, elapsed: {:?}", elapsed);

		Ok(())
	} else {
		Err(PerfCheckError::TimeOut { elapsed, limit: ERASURE_CODING_TIME_LIMIT })
	}
}
