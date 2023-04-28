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

use std::{
	env,
	path::{Path, PathBuf},
	process,
};

/// Returns the manifest dir from the `CARGO_MANIFEST_DIR` env.
fn get_manifest_dir() -> PathBuf {
	env::var("CARGO_MANIFEST_DIR")
		.expect("`CARGO_MANIFEST_DIR` is always set for `build.rs` files; qed")
		.into()
}

/// First step of the [`Builder`] to select the project to build.
pub struct BuilderSelectProject {
	/// This parameter just exists to make it impossible to construct
	/// this type outside of this crate.
	_ignore: (),
}

impl BuilderSelectProject {
	/// Use the current project as project for building the binary.
	///
	/// # Panics
	///
	/// Panics if the `CARGO_MANIFEST_DIR` variable is not set. This variable
	/// is always set by `Cargo` in `build.rs` files.
	pub fn with_current_project(self) -> BuilderSelectTarget {
		BuilderSelectTarget {
			_ignore: (),
			project_cargo_toml: get_manifest_dir().join("Cargo.toml"),
		}
	}

	/// Use the given `path` as project for building the binary.
	///
	/// Returns an error if the given `path` does not points to a `Cargo.toml`.
	pub fn with_project(
		self,
		path: impl Into<PathBuf>,
	) -> Result<BuilderSelectTarget, &'static str> {
		let path = path.into();

		if path.ends_with("Cargo.toml") && path.exists() {
			Ok(BuilderSelectTarget { _ignore: (), project_cargo_toml: path })
		} else {
			Err("Project path must point to the `Cargo.toml` of the project")
		}
	}
}

/// Second step of the [`Builder`] to select the target to build with.
pub struct BuilderSelectTarget {
	/// This parameter just exists to make it impossible to construct
	/// this type outside of this crate.
	_ignore: (),
	project_cargo_toml: PathBuf,
}

impl BuilderSelectTarget {
	/// Use the current Rust target for building the binary.
	pub fn with_current_target(self) -> Builder {
		self.with_target(env::var("TARGET").expect("this is set by cargo! qed"))
	}

	/// Use the given Rust target for building the binary.
	pub fn with_target(self, target: impl Into<String>) -> Builder {
		Builder {
			rust_flags: Vec::new(),
			file_name: None,
			constant_name: None,
			project_cargo_toml: self.project_cargo_toml,
			features_to_enable: Vec::new(),
			target: target.into(),
		}
	}
}

/// The builder for building a binary.
///
/// The builder itself is separated into multiple structs to make the setup type safe.
///
/// Building a binary:
///
/// 1. Call [`Builder::new`] to create a new builder.
/// 2. Select the project to build using the methods of [`BuilderSelectProject`].
/// 3. Set additional `RUST_FLAGS` or a different name for the file containing the code
///    using methods of [`Builder`].
/// 4. Build the binary using [`Self::build`].
pub struct Builder {
	/// Flags that should be appended to `RUST_FLAGS` env variable.
	rust_flags: Vec<String>,
	/// The name of the file that is being generated in `OUT_DIR`.
	///
	/// Defaults to `binary.rs`.
	file_name: Option<String>,
	/// The name of the Rust constant that is generated which contains the binary bytes.
	///
	/// Defaults to `BINARY`.
	constant_name: Option<String>,
	/// The path to the `Cargo.toml` of the project that should be built.
	project_cargo_toml: PathBuf,
	/// Features that should be enabled when building the binary.
	features_to_enable: Vec<String>,
	/// The Rust target to use.
	target: String,
}

impl Builder {
	/// Create a new instance of the builder.
	pub fn new() -> BuilderSelectProject {
		BuilderSelectProject { _ignore: () }
	}

	/// Enable exporting `__heap_base` as global variable in the binary.
	///
	/// This adds `-Clink-arg=--export=__heap_base` to `RUST_FLAGS`.
	pub fn export_heap_base(mut self) -> Self {
		self.rust_flags.push("-Clink-arg=--export=__heap_base".into());
		self
	}

	/// Set the name of the file that will be generated in `OUT_DIR`.
	///
	/// This file needs to be included to get access to the build binary.
	///
	/// If this function is not called, `file_name` defaults to `binary.rs`
	pub fn set_file_name(mut self, file_name: impl Into<String>) -> Self {
		self.file_name = Some(file_name.into());
		self
	}

	/// Set the name of the constant that will be generated in the Rust code to include!.
	///
	/// If this function is not called, `constant_name` defaults to `BINARY`
	pub fn set_constant_name(mut self, constant_name: impl Into<String>) -> Self {
		self.constant_name = Some(constant_name.into());
		self
	}

	/// Instruct the linker to import the memory into the binary.
	///
	/// This adds `-C link-arg=--import-memory` to `RUST_FLAGS`.
	pub fn import_memory(mut self) -> Self {
		self.rust_flags.push("-C link-arg=--import-memory".into());
		self
	}

	/// Append the given `flag` to `RUST_FLAGS`.
	///
	/// `flag` is appended as is, so it needs to be a valid flag.
	pub fn append_to_rust_flags(mut self, flag: impl Into<String>) -> Self {
		self.rust_flags.push(flag.into());
		self
	}

	/// Enable the given feature when building the binary.
	///
	/// `feature` needs to be a valid feature that is defined in the project `Cargo.toml`.
	pub fn enable_feature(mut self, feature: impl Into<String>) -> Self {
		self.features_to_enable.push(feature.into());
		self
	}

	/// Build the binary.
	pub fn build(self) {
		let out_dir = PathBuf::from(env::var("OUT_DIR").expect("`OUT_DIR` is set by cargo!"));
		let file_path = out_dir.join(self.file_name.clone().unwrap_or_else(|| "binary.rs".into()));
		let constant_name = self.constant_name.clone().unwrap_or_else(|| "BINARY".into());

		if check_skip_build() {
			// If we skip the build, we still want to make sure to be called when an env variable
			// changes
			generate_rerun_if_changed_instructions();

			provide_dummy_binary_if_not_exist(&file_path);

			return
		}

		build_project(
			file_path,
			constant_name,
			self.project_cargo_toml,
			self.rust_flags.into_iter().map(|f| format!("{} ", f)).collect(),
			self.features_to_enable,
			self.file_name,
			self.target,
		);

		// As last step we need to generate our `rerun-if-changed` stuff. If a build fails, we don't
		// want to spam the output!
		generate_rerun_if_changed_instructions();
	}
}

/// Generate the name of the skip build environment variable for the current crate.
fn generate_crate_skip_build_env_name() -> String {
	format!(
		"BUILDER_SKIP_{}_BUILD",
		env::var("CARGO_PKG_NAME")
			.expect("Package name is set")
			.to_uppercase()
			.replace('-', "_"),
	)
}

/// Checks if the build of the binary should be skipped.
fn check_skip_build() -> bool {
	env::var(crate::SKIP_BUILD_ENV).is_ok() ||
		env::var(generate_crate_skip_build_env_name()).is_ok()
}

/// Provide a dummy binary if there doesn't exist one.
fn provide_dummy_binary_if_not_exist(file_path: &Path) {
	if !file_path.exists() {
		crate::write_file_if_changed(
			file_path,
			"pub const BINARY: Option<&[u8]> = None;\
			 pub const BINARY_BLOATY: Option<&[u8]> = None;",
		);
	}
}

/// Generate the `rerun-if-changed` instructions for cargo to make sure that the binary is
/// rebuilt when needed.
fn generate_rerun_if_changed_instructions() {
	// Make sure that the `build.rs` is called again if one of the following env variables changes.
	println!("cargo:rerun-if-env-changed={}", crate::SKIP_BUILD_ENV);
	println!("cargo:rerun-if-env-changed={}", crate::FORCE_BUILD_ENV);
	println!("cargo:rerun-if-env-changed={}", generate_crate_skip_build_env_name());
}

/// Build the currently built project as binary.
///
/// The current project is determined by using the `CARGO_MANIFEST_DIR` environment variable.
///
/// `file_name` - The name + path of the file being generated. The file contains the
/// constant `BINARY`, which contains the built binary.
///
/// `project_cargo_toml` - The path to the `Cargo.toml` of the project that should be built.
///
/// `default_rustflags` - Default `RUSTFLAGS` that will always be set for the build.
///
/// `features_to_enable` - Features that should be enabled for the project.
///
/// `binary_name` - The optional binary name that is extended with
///
/// `target` - The binary target.
fn build_project(
	file_name: PathBuf,
	constant_name: String,
	project_cargo_toml: PathBuf,
	default_rustflags: String,
	features_to_enable: Vec<String>,
	binary_name: Option<String>,
	target: String,
) {
	let cargo_cmd = match crate::prerequisites::check(&target) {
		Ok(cmd) => cmd,
		Err(err_msg) => {
			eprintln!("{}", err_msg);
			process::exit(1);
		},
	};

	let (binary, bloaty) = crate::project::create_and_compile(
		&project_cargo_toml,
		&default_rustflags,
		cargo_cmd,
		features_to_enable,
		binary_name,
		&target,
	);

	let binary = if let Some(binary) = binary {
		binary.binary_path_escaped()
	} else {
		bloaty.binary_bloaty_path_escaped()
	};

	// NOTE: Don't write bloaty binary, as opposed to wasm-builder which always writes it (why?).
	crate::write_file_if_changed(
		file_name,
		format!(
			r#"
				pub const {constant_name}: Option<&[u8]> = Some(include_bytes!("{binary}"));
			"#,
		),
	);
}
