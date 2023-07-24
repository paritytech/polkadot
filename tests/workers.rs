// Copyright (C) Parity Technologies (UK) Ltd.
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

use polkadot_cli::NODE_VERSION;
use std::process::Command;

const PREPARE_WORKER_EXE: &str = env!("CARGO_BIN_EXE_polkadot-prepare-worker");
const EXECUTE_WORKER_EXE: &str = env!("CARGO_BIN_EXE_polkadot-execute-worker");

#[test]
fn worker_binaries_have_same_version_as_node() {
	let prep_worker_version =
		Command::new(&PREPARE_WORKER_EXE).args(["--version"]).output().unwrap().stdout;
	let prep_worker_version = std::str::from_utf8(&prep_worker_version)
		.expect("version is printed as a string; qed")
		.trim();
	assert_eq!(prep_worker_version, NODE_VERSION);

	let exec_worker_version =
		Command::new(&EXECUTE_WORKER_EXE).args(["--version"]).output().unwrap().stdout;
	let exec_worker_version = std::str::from_utf8(&exec_worker_version)
		.expect("version is printed as a string; qed")
		.trim();
	assert_eq!(exec_worker_version, NODE_VERSION);
}
