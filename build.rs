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
//!
//! NOTE: The crates for the workers should still be specified in `default-members` in order for
//! `cargo install` to work.
//!
//! # Testing
//!
//! The following scenarios have been tested. Run these tests after changes to this script.
//!
//! - [x] `cargo clean` then `cargo run` (workers should be built)
//!
//! - [x] `cargo run`, commit, `cargo run` again (workers should be rebuilt the second time)
//!
//! - [x] `cargo clean` then `cargo build` (workers should be built)
//!
//! - [x] `cargo +<specific-toolchain> run` (same as `cargo run`)
//!
//! - [ ] TODO: `cargo clean` then `cargo install` (latest workers should be installed)
//!
//! - [ ] TODO: `cargo clean`, `cargo run`, then `cargo install` with binaries already having been
//!       built.
//!
//! - [x] `cargo run` with a profile, like `--profile testnet` (same as `cargo run`)

use std::{env::var, path::Path, process::Command};

use polkadot_execute_worker::BINARY_NAME as EXECUTE_BINARY_NAME;
use polkadot_prepare_worker::BINARY_NAME as PREPARE_BINARY_NAME;

fn main() {
	// Always build PVF worker binaries whenever polkadot is built.
	//
	// This is needed because `default-members` does the same thing, but only for `cargo build` --
	// by itself, `default-members` does not work for `cargo run`.
	{
		let cargo = var("CARGO").expect("`CARGO` env variable is always set by cargo");
		let target = var("TARGET").expect("`TARGET` env variable is always set by cargo");
		let out_dir = var("OUT_DIR").expect("`OUT_DIR` env variable is always set by cargo");
		let target_dir = format!("{}/workers", out_dir);

		// HACK: Get the profile from OUT_DIR instead of the PROFILE env var. If we are using e.g.
		// testnet which inherits from release, then PROFILE will be release. The only way to get
		// the actual profile (e.g. testnet) is with this hacky parsing code. 🙈 We need to get the
		// actual profile to pass along settings like LTO (which is not even available to this build
		// script) and build the binaries as expected.
		let re = regex::Regex::new(r".*/target/(?<profile>\w+)/build/.*").unwrap();
		let caps = re
			.captures(&out_dir)
			.expect("regex broke, please contact your local regex repair facility.");
		let profile = &caps["profile"];

		// Construct the `cargo build ...` command.
		//
		// NOTE: We don't pass along features because the workers don't use any right now.
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
		// Needed to prevent an error.
		if profile != "debug" {
			args.push("--profile");
			args.push(&profile);
		}
		let mut build_cmd = Command::new(cargo);
		build_cmd.args(&args);

		eprintln!(
			"{}",
			colorize_info_message("Information that should be included in a bug report.")
		);
		eprintln!("{} {:?}", colorize_info_message("Executing build command:"), build_cmd);
		eprintln!(
			"{} {:?}",
			colorize_info_message("Using rustc version:"),
			rustc_version::version()
		);

		match build_cmd.status().map(|s| s.success()) {
			Ok(true) => (),
			// Use `process.exit(1)` to have a clean error output.
			_ => std::process::exit(1),
		}

		// Move the built worker binaries into the same location as the `polkadot` binary.
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
