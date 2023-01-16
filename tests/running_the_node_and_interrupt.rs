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
async fn running_the_node_works_and_can_be_interrupted() {
	use nix::{
		sys::signal::{
			kill,
			Signal::{self, SIGINT, SIGTERM},
		},
		unistd::Pid,
	};

	async fn run_command_and_kill(signal: Signal) {
		let tmpdir = tempdir().expect("coult not create temp dir");

		let mut cmd = Command::new(cargo_bin("polkadot"))
			.stdout(process::Stdio::piped())
			.stderr(process::Stdio::piped())
			.args(["--dev", "-d"])
			.arg(tmpdir.path())
			.arg("--no-hardware-benchmarks")
			.spawn()
			.unwrap();

		let (ws_url, _) = common::find_ws_url_from_output(cmd.stderr.take().unwrap());

		// Let it produce three blocks.
		common::wait_n_finalized_blocks(3, Duration::from_secs(60), &ws_url)
			.await
			.unwrap();

		assert!(cmd.try_wait().unwrap().is_none(), "the process should still be running");
		kill(Pid::from_raw(cmd.id().try_into().unwrap()), signal).unwrap();
		assert_eq!(
			common::wait_for(&mut cmd, 30).map(|x| x.success()),
			Some(true),
			"the process must exit gracefully after signal {}",
			signal,
		);
	}

	run_command_and_kill(SIGINT).await;
	run_command_and_kill(SIGTERM).await;
}
