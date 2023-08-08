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
use polkadot_execute_worker::BINARY_NAME as EXECUTE_WORKER_EXE;
use polkadot_prepare_worker::BINARY_NAME as PREPARE_WORKER_EXE;
use std::process::Command;

#[test]
fn worker_binaries_have_same_version_as_node() {
	// We're in `deps/` directory, target directory is one level up
	let mut path = std::env::current_exe().unwrap();
	path.pop();
	path.pop();
	path.push(&PREPARE_WORKER_EXE);
	let prep_worker_version =
		Command::new(path.clone()).args(["--version"]).output().unwrap().stdout;
	let prep_worker_version = std::str::from_utf8(&prep_worker_version)
		.expect("version is printed as a string; qed")
		.trim();
	assert_eq!(prep_worker_version, NODE_VERSION);

	path.pop();
	path.push(&EXECUTE_WORKER_EXE);
	let exec_worker_version = Command::new(path).args(["--version"]).output().unwrap().stdout;
	let exec_worker_version = std::str::from_utf8(&exec_worker_version)
		.expect("version is printed as a string; qed")
		.trim();
	assert_eq!(exec_worker_version, NODE_VERSION);
}
