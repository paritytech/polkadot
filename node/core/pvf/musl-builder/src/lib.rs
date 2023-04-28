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

// TODO: Extract common parts into a generalized builder that wasm/musl builders are based on.

// TODO: Make sure we build with O2 and LTO.

mod builder;
mod prerequisites;
mod project;
mod version;

pub use builder::{Builder, BuilderSelectProject};

use std::{
	env, fs,
	io::BufRead,
	path::{Path, PathBuf},
	process::Command,
};
use version::Version;

/// Environment variable that tells us to skip building the binary.
const SKIP_BUILD_ENV: &str = "BUILDER_SKIP_BUILD";

/// Environment variable that tells us whether we should avoid network requests.
const OFFLINE: &str = "CARGO_NET_OFFLINE";

/// Environment variable to force a certain build type when building the binary.
/// Expects "debug", "release" or "production" as value.
///
/// When unset the binary uses the same build type as the main cargo build with
/// the exception of a debug build: In this case the build defaults to `release` in
/// order to avoid a slowdown when not explicitly requested.
const BUILD_TYPE_ENV: &str = "BUILDER_BUILD_TYPE";

/// Environment variable to extend the `RUSTFLAGS` variable given to the build.
const BUILD_RUSTFLAGS_ENV: &str = "BUILDER_BUILD_RUSTFLAGS";

/// Environment variable to set the target directory to copy the final binary.
///
/// The directory needs to be an absolute path.
const TARGET_DIRECTORY: &str = "BUILDER_TARGET_DIRECTORY";

/// Environment variable to disable color output of the build.
const BUILD_NO_COLOR: &str = "BUILDER_BUILD_NO_COLOR";

/// Environment variable to set the toolchain used to compile the binary.
const BUILD_TOOLCHAIN: &str = "BUILDER_BUILD_TOOLCHAIN";

/// Environment variable that makes sure the build is triggered.
const FORCE_BUILD_ENV: &str = "BUILDER_FORCE_BUILD";

/// Environment variable that hints the workspace we are building.
const BUILD_WORKSPACE_HINT: &str = "BUILDER_BUILD_WORKSPACE_HINT";

/// Write to the given `file` if the `content` is different.
fn write_file_if_changed(file: impl AsRef<Path>, content: impl AsRef<str>) {
	if fs::read_to_string(file.as_ref()).ok().as_deref() != Some(content.as_ref()) {
		fs::write(file.as_ref(), content.as_ref())
			.unwrap_or_else(|_| panic!("Writing `{}` can not fail!", file.as_ref().display()));
	}
}

/// Copy `src` to `dst` if the `dst` does not exist or is different.
fn copy_file_if_changed(src: PathBuf, dst: PathBuf) {
	let src_file = fs::read_to_string(&src).ok();
	let dst_file = fs::read_to_string(&dst).ok();

	if src_file != dst_file {
		fs::copy(&src, &dst).unwrap_or_else(|_| {
			panic!("Copying `{}` to `{}` can not fail; qed", src.display(), dst.display())
		});
	}
}

/// Get a cargo command that should be used to invoke the compilation.
fn get_cargo_command() -> CargoCommand {
	let env_cargo =
		CargoCommand::new(&env::var("CARGO").expect("`CARGO` env variable is always set by cargo"));
	let default_cargo = CargoCommand::new("cargo");
	let toolchain = env::var(BUILD_TOOLCHAIN).ok();

	// First check if the user requested a specific toolchain
	if let Some(cmd) =
		toolchain.map(|t| CargoCommand::new_with_args("rustup", &["run", &t, "cargo"]))
	{
		cmd
	} else if env_cargo.supports_env() {
		env_cargo
	} else if default_cargo.supports_env() {
		default_cargo
	} else {
		// If no command before provided us with a cargo that supports our Substrate env, we
		// try to search one with rustup. If that fails as well, we return the default cargo and let
		// the prequisities check fail.
		get_rustup_command().unwrap_or(default_cargo)
	}
}

/// Get the newest rustup command that supports our Substrate env.
///
/// Stable versions are always favored over nightly versions even if the nightly versions are
/// newer.
fn get_rustup_command() -> Option<CargoCommand> {
	let host = format!("-{}", env::var("HOST").expect("`HOST` is always set by cargo"));

	let output = Command::new("rustup").args(&["toolchain", "list"]).output().ok()?.stdout;
	let lines = output.as_slice().lines();

	let mut versions = Vec::new();
	for line in lines.filter_map(|l| l.ok()) {
		let rustup_version = line.trim_end_matches(&host);

		let cmd = CargoCommand::new_with_args("rustup", &["run", &rustup_version, "cargo"]);

		if !cmd.supports_env() {
			continue
		}

		let Some(cargo_version) = cmd.version() else { continue; };

		versions.push((cargo_version, rustup_version.to_string()));
	}

	// Sort by the parsed version to get the latest version (greatest version) at the end of the
	// vec.
	versions.sort_by_key(|v| v.0);
	let version = &versions.last()?.1;

	Some(CargoCommand::new_with_args("rustup", &["run", &version, "cargo"]))
}

/// Wraps a specific command which represents a cargo invocation.
#[derive(Debug)]
struct CargoCommand {
	program: String,
	args: Vec<String>,
	version: Option<Version>,
}

impl CargoCommand {
	fn new(program: &str) -> Self {
		let version = Self::extract_version(program, &[]);

		CargoCommand { program: program.into(), args: Vec::new(), version }
	}

	fn new_with_args(program: &str, args: &[&str]) -> Self {
		let version = Self::extract_version(program, args);

		CargoCommand {
			program: program.into(),
			args: args.iter().map(ToString::to_string).collect(),
			version,
		}
	}

	fn command(&self) -> Command {
		let mut cmd = Command::new(&self.program);
		cmd.args(&self.args);
		cmd
	}

	fn extract_version(program: &str, args: &[&str]) -> Option<Version> {
		let version = Command::new(program)
			.args(args)
			.arg("--version")
			.output()
			.ok()
			.and_then(|o| String::from_utf8(o.stdout).ok())?;

		Version::extract(&version)
	}

	/// Returns the version of this cargo command or `None` if it failed to extract the version.
	fn version(&self) -> Option<Version> {
		self.version
	}

	/// Check if the supplied cargo command supports our environment.
	///
	/// Assumes that cargo version matches the rustc version.
	fn supports_env(&self) -> bool {
		// TODO: Just a stub for now -- not sure this is needed for musl-builder.
		true
	}
}

/// Wraps a [`CargoCommand`] and the version of `rustc` the cargo command uses.
struct CargoCommandVersioned {
	command: CargoCommand,
	version: String,
}

impl CargoCommandVersioned {
	fn new(command: CargoCommand, version: String) -> Self {
		Self { command, version }
	}

	/// Returns the `rustc` version.
	fn rustc_version(&self) -> &str {
		&self.version
	}
}

impl std::ops::Deref for CargoCommandVersioned {
	type Target = CargoCommand;

	fn deref(&self) -> &CargoCommand {
		&self.command
	}
}

/// Returns `true` when color output is enabled.
fn color_output_enabled() -> bool {
	env::var(crate::BUILD_NO_COLOR).is_err()
}
