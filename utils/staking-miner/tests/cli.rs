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

use assert_cmd::{cargo::cargo_bin, Command};
use serde_json::{Result, Value};

#[test]
fn cli_version_works() {
	let crate_name = env!("CARGO_PKG_NAME");
	let output = Command::new(cargo_bin(crate_name)).arg("--version").output().unwrap();

	assert!(output.status.success(), "command returned with non-success exit code");
	let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();

	assert_eq!(version, format!("{} {}", crate_name, env!("CARGO_PKG_VERSION")));
}

#[test]
fn cli_info_works() {
	let crate_name = env!("CARGO_PKG_NAME");
	let output = Command::new(cargo_bin(crate_name))
		.arg("info")
		.arg("--json")
		.env("RUST_LOG", "none")
		.output()
		.unwrap();

	assert!(output.status.success(), "command returned with non-success exit code");
	let info = String::from_utf8_lossy(&output.stdout).trim().to_owned();
	let v: Result<Value> = serde_json::from_str(&info);
	let v = v.unwrap();
	assert!(!v["builtin"].to_string().is_empty());
	assert!(!v["builtin"]["spec_name"].to_string().is_empty());
	assert!(!v["builtin"]["spec_version"].to_string().is_empty());
	assert!(!v["remote"].to_string().is_empty());
}
