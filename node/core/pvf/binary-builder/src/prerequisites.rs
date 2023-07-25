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

use crate::{write_file_if_changed, CargoCommand, CargoCommandVersioned, is_musl_target};

use std::{fs, path::Path};

use ansi_term::Color;
use tempfile::tempdir;

/// Print an error message.
fn print_error_message(message: &str) -> String {
	if super::color_output_enabled() {
		Color::Red.bold().paint(message).to_string()
	} else {
		message.into()
	}
}

/// Checks that all prerequisites are installed.
///
/// Returns the versioned cargo command on success.
pub(crate) fn check(target: &str) -> Result<CargoCommandVersioned, String> {
	let cargo_command = crate::get_cargo_command();

	if !cargo_command.supports_env() {
		// TODO: are there actual prerequisites for musl?
		return Err(print_error_message(&format!(
			"Cannot compile the {} runtime: no compatible Rust compiler found!",
			target
		)))
	}

	check_target_installed(cargo_command, target)
}

/// Create the project that will be used to check that the required target is installed and to
/// extract the rustc version.
fn create_check_target_project(project_dir: &Path, target: &str) {
	let lib_rs_file = project_dir.join("src/lib.rs");
	let main_rs_file = project_dir.join("src/main.rs");
	let build_rs_file = project_dir.join("build.rs");
	let manifest_path = project_dir.join("Cargo.toml");

	// TODO: Make this generalizable.
	//       See <https://github.com/paritytech/substrate/issues/13982>.
	let crate_type = if is_musl_target(target) { "staticlib" } else { "cdylib" };

	write_file_if_changed(
		&manifest_path,
		r#"
			[package]
			name = "builder-test"
			version = "1.0.0"
			edition = "2021"
			build = "build.rs"

			[lib]
			name = "builder_test"
			crate-type = ["{}"]

			[workspace]
		"#
		.to_string()
		.replace("{}", crate_type),
	);
	write_file_if_changed(lib_rs_file, "pub fn test() {}");

	// We want to know the rustc version of the rustc that is being used by our cargo command.
	// The cargo command is determined by some *very* complex algorithm to find the cargo command
	// that supports nightly.
	// The best solution would be if there is a `cargo rustc --version` command, which sadly
	// doesn't exists. So, the only available way of getting the rustc version is to build a project
	// and capture the rustc version in this build process. This `build.rs` is exactly doing this.
	// It gets the rustc version by calling `rustc --version` and exposing it in the `RUSTC_VERSION`
	// environment variable.
	write_file_if_changed(
		build_rs_file,
		r#"
			fn main() {
				let rustc_cmd = std::env::var("RUSTC").ok().unwrap_or_else(|| "rustc".into());

				let rustc_version = std::process::Command::new(rustc_cmd)
					.arg("--version")
					.output()
					.ok()
					.and_then(|o| String::from_utf8(o.stdout).ok());

				println!(
					"cargo:rustc-env=RUSTC_VERSION={}",
					rustc_version.unwrap_or_else(|| "unknown rustc version".into()),
				);
			}
		"#,
	);
	// Just prints the `RUSTC_VERSION` environment variable that is being created by the
	// `build.rs` script.
	write_file_if_changed(
		main_rs_file,
		r#"
			fn main() {
				println!("{}", env!("RUSTC_VERSION"));
			}
		"#,
	);
}

fn check_target_installed(
	cargo_command: CargoCommand,
	target: &str,
) -> Result<CargoCommandVersioned, String> {
	let temp = tempdir().expect("Creating temp dir does not fail; qed");
	fs::create_dir_all(temp.path().join("src")).expect("Creating src dir does not fail; qed");
	create_check_target_project(temp.path(), target);

	let err_msg =
		print_error_message(&format!("could not build with {} target, please make sure it is installed and check below for more info", target));
	let manifest_path = temp.path().join("Cargo.toml").display().to_string();

	let mut build_cmd = cargo_command.command();
	// Chdir to temp to avoid including project's .cargo/config.toml
	// by accident - it can happen in some CI environments.
	build_cmd.current_dir(&temp);
	build_cmd.args(&["build", &format!("--target={}", target), "--manifest-path", &manifest_path]);

	if super::color_output_enabled() {
		build_cmd.arg("--color=always");
	}

	let mut run_cmd = cargo_command.command();
	// Chdir to temp to avoid including project's .cargo/config.toml
	// by accident - it can happen in some CI environments.
	run_cmd.current_dir(&temp);
	run_cmd.args(&["run", "--manifest-path", &manifest_path]);

	// Unset the `CARGO_TARGET_DIR` to prevent a cargo deadlock
	build_cmd.env_remove("CARGO_TARGET_DIR");
	run_cmd.env_remove("CARGO_TARGET_DIR");

	// Make sure the host's flags aren't used here, e.g. if an alternative linker is specified
	// in the RUSTFLAGS then the check we do here will break unless we clear these.
	build_cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");
	run_cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");
	build_cmd.env_remove("RUSTFLAGS");
	run_cmd.env_remove("RUSTFLAGS");

	build_cmd.output().map_err(|_| err_msg.clone()).and_then(|s| {
		if s.status.success() {
			let version = run_cmd.output().ok().and_then(|o| String::from_utf8(o.stdout).ok());
			Ok(CargoCommandVersioned::new(
				cargo_command,
				version.unwrap_or_else(|| "unknown rustc version".into()),
			))
		} else {
			match String::from_utf8(s.stderr) {
				Ok(ref err) if err.contains("linker `rust-lld` not found") =>
					Err(print_error_message("`rust-lld` not found, please install it!")),
				Ok(ref err) => Err(format!(
					"{}\n\n{}\n{}\n{}{}\n",
					err_msg,
					Color::Yellow.bold().paint("Further error information:"),
					Color::Yellow.bold().paint("-".repeat(60)),
					err,
					Color::Yellow.bold().paint("-".repeat(60)),
				)),
				Err(_) => Err(err_msg),
			}
		}
	})
}
