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

use polkadot_node_core_pvf::sc_executor_common;
use std::time::Duration;

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

#[cfg(build_type = "release")]
fn dummy_file_path() -> std::io::Result<std::path::PathBuf> {
	use std::{fs, io, path::Path};

	let home_dir =
		std::env::var("HOME").map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
	let path = Path::new(&home_dir).join(".polkadot");

	if let Err(err) = fs::create_dir(&path) {
		if err.kind() != io::ErrorKind::AlreadyExists {
			return Err(err)
		}
	}

	Ok(path.join("perf_check_passed"))
}

#[cfg(build_type = "release")]
fn check_dummy_file(path: &std::path::Path) -> std::io::Result<bool> {
	use nix::unistd;
	use std::{
		fs,
		io::{self, Read},
	};

	let host_name_max_len = unistd::SysconfVar::HOST_NAME_MAX as usize;

	let mut host_name = vec![0u8; host_name_max_len];
	let mut buf = host_name.clone();
	unistd::gethostname(&mut host_name).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

	let dummy_file = match fs::File::open(path) {
		Ok(file) => file,
		Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
		Err(err) => return Err(err),
	};
	let mut reader = io::BufReader::new(dummy_file);

	reader.read_exact(&mut buf)?;

	Ok(host_name == buf)
}

#[cfg(build_type = "release")]
fn save_dummy_file(path: &std::path::Path) -> std::io::Result<()> {
	use nix::unistd;
	use std::{
		fs::OpenOptions,
		io::{self, Write},
	};

	let host_name_max_len = unistd::SysconfVar::HOST_NAME_MAX as usize;
	let mut host_name = vec![0u8; host_name_max_len];
	unistd::gethostname(&mut host_name).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

	let mut dummy_file = OpenOptions::new().truncate(true).create(true).write(true).open(path)?;

	dummy_file.write(&host_name)?;

	Ok(())
}

/// Runs a performance check via compiling sample wasm code with a timeout.
/// Should only be run in release build since the check would take too much time otherwise.
/// Returns `Ok` immediately if the check has been passed previously.
pub fn host_perf_check() -> Result<(), PerfCheckError> {
	#[cfg(build_type = "debug")]
	{
		Err(PerfCheckError::DebugBuildNotSupported)
	}
	#[cfg(build_type = "release")]
	{
		use polkadot_node_core_pvf::sp_maybe_compressed_blob;

		const PERF_CHECK_TIME_LIMIT: Duration = Duration::from_secs(20);
		const CODE_SIZE_LIMIT: usize = 1024usize.pow(3);
		const WASM_CODE: &[u8] = include_bytes!(
			"../../target/release/wbuild/kusama-runtime/kusama_runtime.compact.compressed.wasm"
		);

		// We will try to save a dummy file at $HOME/.polkadot/perf_check_passed.
		let dummy_file_path =
			dummy_file_path()
				.map_err(|err| {
					log::info!("Performance check result is not going to be persisted due to an error: {:?}", err)
				})
				.ok();

		if let Some(ref path) = dummy_file_path {
			if let Ok(true) = check_dummy_file(path) {
				log::info!("Performance check skipped: already passed");
				return Ok(())
			}
		}

		log::info!("Running the performance check...");
		let start = std::time::Instant::now();

		// Recreate the pipeline from the pvf prepare worker.
		let code = sp_maybe_compressed_blob::decompress(WASM_CODE, CODE_SIZE_LIMIT)
			.or(Err(PerfCheckError::CodeDecompressionFailed))?;
		let blob =
			polkadot_node_core_pvf::prevalidate(code.as_ref()).map_err(PerfCheckError::from)?;
		let _ = polkadot_node_core_pvf::prepare(blob).map_err(PerfCheckError::from)?;

		let elapsed = start.elapsed();
		if elapsed <= PERF_CHECK_TIME_LIMIT {
			log::info!("Performance check passed, elapsed: {:?}", start.elapsed());
			// Save a dummy file.
			dummy_file_path.map(|path| save_dummy_file(&path));
			Ok(())
		} else {
			Err(PerfCheckError::TimeOut { elapsed, limit: PERF_CHECK_TIME_LIMIT })
		}
	}
	#[cfg(not(any(build_type = "debug", build_type = "release")))]
	{
		log::info!("Performance check skipped: unknown build type");
		Ok(())
	}
}
