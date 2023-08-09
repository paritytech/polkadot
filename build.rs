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

//! Build script to ensure that PVF worker binaries are always built whenever polkadot is built.
//!
//! This is needed because `default-members` does the same thing, but only for `cargo build` -- it
//! does not work for `cargo run`.

use std::{env::var, path::Path, process::Command};

use polkadot_node_core_pvf::{EXECUTE_BINARY_NAME, PREPARE_BINARY_NAME};

// TODO: unskip
#[rustfmt::skip]
fn main() {
	// Build additional artifacts if a single package build is explicitly requested.
	{
		let cargo   = dbg!(var("CARGO").expect("`CARGO` env variable is always set by cargo"));
		let target  = dbg!(var("TARGET").expect("`TARGET` env variable is always set by cargo"));
		let profile = dbg!(var("PROFILE").expect("`PROFILE` env variable is always set by cargo"));
		let out_dir = dbg!(var("OUT_DIR").expect("`OUT_DIR` env variable is always set by cargo"));
		let target_dir = format!("{}/workers", out_dir);

		// assert!(false);

		// TODO: opt-level, debug, features, etc.
		let mut args = vec![
			"build",
			"-p",
			EXECUTE_BINARY_NAME,
			"-p",
			PREPARE_BINARY_NAME,
			"--target",
			&target,
			"--target-dir",
			&target_dir,
		];
		if profile != "debug" {
			args.push("--profile");
			args.push(&profile);
		}

		Command::new(cargo).args(&args).status().unwrap();
		std::fs::rename(
			Path::new(&format!("{target_dir}/{target}/{profile}/{EXECUTE_BINARY_NAME}")),
			Path::new(&format!("{target_dir}/../../../../{EXECUTE_BINARY_NAME}")),
		)
			.unwrap();
		std::fs::rename(
			Path::new(&format!("{target_dir}/{target}/{profile}/{PREPARE_BINARY_NAME}")),
			Path::new(&format!("{target_dir}/../../../../{PREPARE_BINARY_NAME}")),
		)
			.unwrap();
	}

	// TODO: is this needed here?
	substrate_build_script_utils::generate_cargo_keys();
	// For the node/worker version check, make sure we always rebuild the node and binary workers
	// when the version changes.
	substrate_build_script_utils::rerun_if_git_head_changed();
}
