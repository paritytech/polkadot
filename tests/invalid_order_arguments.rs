// Copyright (C) Parity Technologies (UK) Ltd.
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
use std::process::Command;
use tempfile::tempdir;

#[test]
#[cfg(unix)]
fn invalid_order_arguments() {
	let tmpdir = tempdir().expect("could not create temp dir");

	let status = Command::new(cargo_bin("polkadot"))
		.args(["--dev", "invalid_order_arguments", "-d"])
		.arg(tmpdir.path())
		.arg("-y")
		.status()
		.unwrap();
	assert!(!status.success());
}
