// Copyright 2022 Parity Technologies (UK) Ltd.
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

use assert_cmd::cargo::cargo_bin;
use std::{
	process::{Command, Stdio},
	result::Result,
};
use tempfile::tempdir;

static RUNTIMES: [&'static str; 4] = ["polkadot", "kusama", "westend", "rococo"];
static EXTRINSICS: [(&'static str, &'static str); 2] =
	[("system", "remark"), ("balances", "transfer_keep_alive")];

/// `benchmark extrinsic` works for all dev runtimes and some extrinsics.
#[test]
fn benchmark_extrinsic_works() {
	for (runtime, (pallet, extrinsic)) in RUNTIMES.iter().zip(EXTRINSICS.iter()) {
		let runtime = format!("{}-dev", runtime);
		assert!(benchmark_extrinsic(&runtime, pallet, extrinsic).is_ok());
	}
}

/// `benchmark extrinsic` rejects all non-dev runtimes.
#[test]
fn benchmark_extrinsic_rejects_non_dev_runtimes() {
	for (runtime, (pallet, extrinsic)) in RUNTIMES.iter().zip(EXTRINSICS.iter()) {
		assert!(benchmark_extrinsic(runtime, pallet, extrinsic).is_err());
	}
}

fn benchmark_extrinsic(runtime: &str, pallet: &str, extrinsic: &str) -> Result<(), String> {
	let tmp_dir = tempdir().expect("could not create a temp dir");
	let base_path = tmp_dir.path();

	let status = Command::new(cargo_bin("polkadot"))
		.args(["benchmark", "extrinsic", "--chain", &runtime])
		.args(&["--pallet", pallet, "--extrinsic", extrinsic])
		// Run with low repeats for faster execution.
		.args(["--repeat=10", "--warmup=10", "--max-ext-per-block", "5"])
		.status()
		.map_err(|e| format!("command failed: {:?}", e))?;

	if !status.success() {
		return Err("Command failed".into())
	}

	Ok(())
}
