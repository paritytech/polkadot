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

//! Build script to ensure that node/worker versions stay in sync and PVF worker binaries are always
//! built whenever polkadot is built.

use std::{env::var, path::Path, process::Command};

// TODO: fix
// use polkadot_node_core_pvf::{PREPARE_BINARY_NAME, EXECUTE_BINARY_NAME};
const PREPARE_BINARY_NAME: &str = "polkadot-prepare-worker";
const EXECUTE_BINARY_NAME: &str = "polkadot-execute-worker";

fn main() {
	// Always build PVF worker binaries whenever polkadot is built.
	//
	// This is needed because `default-members` does the same thing, but only for `cargo build` --
	// it does not work for `cargo run`.
	//
	// TODO: Test with `cargo +<specific-toolchain> ...`
	{
		let cargo = var("CARGO").expect("`CARGO` env variable is always set by cargo");
		let target = var("TARGET").expect("`TARGET` env variable is always set by cargo");
		let profile = var("PROFILE").expect("`PROFILE` env variable is always set by cargo");
		let out_dir = var("OUT_DIR").expect("`OUT_DIR` env variable is always set by cargo");
		let target_dir = format!("{}/workers", out_dir);

		// Settings like overflow-checks, opt-level, lto, etc. are correctly passed to cargo
		// subcommand through env vars, e.g. `CARGO_CFG_OVERFLOW_CHECKS`, which the subcommand
		// inherits. We don't pass along features because the workers don't use any right now.
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
		let mut build_cmd = Command::new(cargo);
		build_cmd.args(&args);

		println!(
			"{}",
			colorize_info_message("Information that should be included in a bug report.")
		);
		println!("{} {:?}", colorize_info_message("Executing build command:"), build_cmd);
		// println!("{} {}", colorize_info_message("Using rustc version:"), build_cmd.rustc_version());

		match build_cmd.status().map(|s| s.success()) {
			Ok(true) => (),
			// Use `process.exit(1)` to have a clean error output.
			_ => std::process::exit(1),
		}

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

/// Colorize an info message.
///
/// Returns the colorized message.
fn colorize_info_message(message: &str) -> String {
	ansi_term::Color::Yellow.bold().paint(message).to_string()
}
