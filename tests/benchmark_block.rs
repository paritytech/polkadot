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

// Unix only since it uses signals.
#![cfg(unix)]

use assert_cmd::cargo::cargo_bin;
use nix::{
	sys::signal::{kill, Signal::SIGINT},
	unistd::Pid,
};
use std::{
	path::Path,
	process::{self, Command},
	result::Result,
	time::Duration,
};
use tempfile::tempdir;

pub mod common;

static RUNTIMES: [&str; 4] = ["polkadot", "kusama", "westend", "rococo"];

/// `benchmark block` works for all dev runtimes using the wasm executor.
#[tokio::test]
async fn benchmark_block_works() {
	for runtime in RUNTIMES {
		let tmp_dir = tempdir().expect("could not create a temp dir");
		let base_path = tmp_dir.path();
		let runtime = format!("{}-dev", runtime);

		// Build a chain with a single block.
		build_chain(&runtime, base_path).await.unwrap();
		// Benchmark the one block.
		benchmark_block(&runtime, base_path, 1).unwrap();
	}
}

/// Builds a chain with one block for the given runtime and base path.
async fn build_chain(runtime: &str, base_path: &Path) -> Result<(), String> {
	let mut cmd = Command::new(cargo_bin("polkadot"))
		.stdout(process::Stdio::piped())
		.stderr(process::Stdio::piped())
		.args(["--chain", runtime, "--force-authoring", "--alice"])
		.arg("-d")
		.arg(base_path)
		.arg("--no-hardware-benchmarks")
		.spawn()
		.unwrap();

	let (ws_url, _) = common::find_ws_url_from_output(cmd.stderr.take().unwrap());

	// Wait for the chain to produce one block.
	let ok = common::wait_n_finalized_blocks(1, Duration::from_secs(60), &ws_url).await;
	// Send SIGINT to node.
	kill(Pid::from_raw(cmd.id().try_into().unwrap()), SIGINT).unwrap();
	// Wait for the node to handle it and exit.
	assert!(common::wait_for(&mut cmd, 30).map(|x| x.success()).unwrap_or_default());

	ok.map_err(|e| format!("Node did not build the chain: {:?}", e))
}

/// Benchmarks the given block with the wasm executor.
fn benchmark_block(runtime: &str, base_path: &Path, block: u32) -> Result<(), String> {
	// Invoke `benchmark block` with all options to make sure that they are valid.
	let status = Command::new(cargo_bin("polkadot"))
		.args(["benchmark", "block", "--chain", runtime])
		.arg("-d")
		.arg(base_path)
		.args(["--from", &block.to_string(), "--to", &block.to_string()])
		.args(["--repeat", "1"])
		.args(["--execution", "wasm", "--wasm-execution", "compiled"])
		.status()
		.map_err(|e| format!("command failed: {:?}", e))?;

	if !status.success() {
		return Err("Command failed".into())
	}

	Ok(())
}
