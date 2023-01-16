// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

use assert_cmd::cargo::cargo_bin;
use std::{
	process::{self, Command},
	time::Duration,
};
use tempfile::tempdir;

pub mod common;

#[tokio::test]
#[cfg(unix)]
async fn purge_chain_rocksdb_works() {
	use nix::{
		sys::signal::{kill, Signal::SIGINT},
		unistd::Pid,
	};

	let tmpdir = tempdir().expect("could not create temp dir");

	let mut cmd = Command::new(cargo_bin("polkadot"))
		.stdout(process::Stdio::piped())
		.stderr(process::Stdio::piped())
		.args(["--dev", "-d"])
		.arg(tmpdir.path())
		.arg("--port")
		.arg("33034")
		.arg("--no-hardware-benchmarks")
		.spawn()
		.unwrap();

	let (ws_url, _) = common::find_ws_url_from_output(cmd.stderr.take().unwrap());

	// Let it produce 1 block.
	common::wait_n_finalized_blocks(1, Duration::from_secs(60), &ws_url)
		.await
		.unwrap();

	// Send SIGINT to node.
	kill(Pid::from_raw(cmd.id().try_into().unwrap()), SIGINT).unwrap();
	// Wait for the node to handle it and exit.
	assert!(common::wait_for(&mut cmd, 30).map(|x| x.success()).unwrap_or_default());
	assert!(tmpdir.path().join("chains/dev").exists());
	assert!(tmpdir.path().join("chains/dev/db/full").exists());
	assert!(tmpdir.path().join("chains/dev/db/full/parachains").exists());

	// Purge chain
	let status = Command::new(cargo_bin("polkadot"))
		.args(["purge-chain", "--dev", "-d"])
		.arg(tmpdir.path())
		.arg("-y")
		.status()
		.unwrap();
	assert!(status.success());

	// Make sure that the chain folder exists, but `db/full` is deleted.
	assert!(tmpdir.path().join("chains/dev").exists());
	assert!(!tmpdir.path().join("chains/dev/db/full").exists());
}

#[tokio::test]
#[cfg(unix)]
async fn purge_chain_paritydb_works() {
	use nix::{
		sys::signal::{kill, Signal::SIGINT},
		unistd::Pid,
	};

	let tmpdir = tempdir().expect("could not create temp dir");

	let mut cmd = Command::new(cargo_bin("polkadot"))
		.stdout(process::Stdio::piped())
		.stderr(process::Stdio::piped())
		.args(["--dev", "-d"])
		.arg(tmpdir.path())
		.arg("--database")
		.arg("paritydb-experimental")
		.arg("--no-hardware-benchmarks")
		.spawn()
		.unwrap();

	let (ws_url, _) = common::find_ws_url_from_output(cmd.stderr.take().unwrap());

	// Let it produce 1 block.
	common::wait_n_finalized_blocks(1, Duration::from_secs(60), &ws_url)
		.await
		.unwrap();

	// Send SIGINT to node.
	kill(Pid::from_raw(cmd.id().try_into().unwrap()), SIGINT).unwrap();
	// Wait for the node to handle it and exit.
	assert!(common::wait_for(&mut cmd, 30).map(|x| x.success()).unwrap_or_default());
	assert!(tmpdir.path().join("chains/dev").exists());
	assert!(tmpdir.path().join("chains/dev/paritydb/full").exists());
	assert!(tmpdir.path().join("chains/dev/paritydb/parachains").exists());

	// Purge chain
	let status = Command::new(cargo_bin("polkadot"))
		.args(["purge-chain", "--dev", "-d"])
		.arg(tmpdir.path())
		.arg("--database")
		.arg("paritydb-experimental")
		.arg("-y")
		.status()
		.unwrap();
	assert!(status.success());

	// Make sure that the chain folder exists, but `db/full` is deleted.
	assert!(tmpdir.path().join("chains/dev").exists());
	assert!(!tmpdir.path().join("chains/dev/paritydb/full").exists());
	// Parachains removal requires calling "purge-chain --parachains".
	assert!(tmpdir.path().join("chains/dev/paritydb/parachains").exists());
}
