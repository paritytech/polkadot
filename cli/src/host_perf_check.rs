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
use polkadot_node_core_pvf::sp_maybe_compressed_blob;
use std::{
	fs::{self, OpenOptions},
	io::{self, Read, Write},
	path::{Path, PathBuf},
	time::{Duration, Instant},
};

fn dummy_file_path() -> io::Result<PathBuf> {
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

fn check_dummy_file(path: &Path) -> io::Result<bool> {
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

fn save_dummy_file(path: &Path) -> io::Result<()> {
	let host_name_max_len = unistd::SysconfVar::HOST_NAME_MAX as usize;
	let mut host_name = vec![0u8; host_name_max_len];
	unistd::gethostname(&mut host_name).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

	let mut dummy_file = OpenOptions::new().truncate(true).create(true).write(true).open(path)?;

	dummy_file.write(&host_name)?;

	Ok(())
}

pub fn host_perf_check() -> Result<(), PerfCheckError> {
	const PERF_CHECK_TIME_LIMIT: Duration = Duration::from_secs(20);
	const CODE_SIZE_LIMIT: usize = 1024usize.pow(3);
	const WASM_CODE: &[u8] = include_bytes!(
		"../../target/release/wbuild/kusama-runtime/kusama_runtime.compact.compressed.wasm"
	);

	// We will try to save a dummy file at $HOME/.polkadot/perf_check_passed.
	let dummy_file_path = dummy_file_path()
		.map_err(|err| {
			info!(
				"Performance check result is not going to be persisted due to an error: {:?}",
				err
			)
		})
		.ok();

	if let Some(ref path) = dummy_file_path {
		if let Ok(true) = check_dummy_file(path) {
			info!("Performance check skipped: already passed");
			return Ok(())
		}
	}

	info!("Running the performance check...");
	let start = Instant::now();

	// Recreate the pipeline from the pvf prepare worker.
	let code = sp_maybe_compressed_blob::decompress(WASM_CODE, CODE_SIZE_LIMIT)
		.or(Err(PerfCheckError::CodeDecompressionFailed))?;
	let blob = polkadot_node_core_pvf::prevalidate(code.as_ref()).map_err(PerfCheckError::from)?;
	let _ = polkadot_node_core_pvf::prepare(blob).map_err(PerfCheckError::from)?;

	let elapsed = start.elapsed();
	if elapsed <= PERF_CHECK_TIME_LIMIT {
		info!("Performance check passed, elapsed: {:?}", start.elapsed());
		// Save a dummy file.
		dummy_file_path.map(|path| save_dummy_file(&path));
		Ok(())
	} else {
		Err(PerfCheckError::TimeOut { elapsed, limit: PERF_CHECK_TIME_LIMIT })
	}
}
