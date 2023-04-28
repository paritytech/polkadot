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

use crate::{copy_file_if_changed, write_file_if_changed, CargoCommandVersioned, OFFLINE};

use build_helper::rerun_if_changed;
use cargo_metadata::{CargoOpt, Metadata, MetadataCommand};
use std::{
	borrow::ToOwned,
	collections::HashSet,
	env, fs,
	hash::{Hash, Hasher},
	ops::Deref,
	path::{Path, PathBuf},
	process,
};
use strum::{EnumIter, IntoEnumIterator};
use toml::value::{Array, Table};
use walkdir::WalkDir;

/// Colorize an info message.
///
/// Returns the colorized message.
fn colorize_info_message(message: &str) -> String {
	if super::color_output_enabled() {
		ansi_term::Color::Yellow.bold().paint(message).to_string()
	} else {
		message.into()
	}
}

/// Holds the path to the bloaty binary.
pub struct BinaryBloaty(PathBuf);

impl BinaryBloaty {
	/// Returns the escaped path to the bloaty binary.
	pub fn binary_bloaty_path_escaped(&self) -> String {
		self.0.display().to_string().escape_default().to_string()
	}

	/// Returns the path to the binary.
	pub fn binary_bloaty_path(&self) -> &Path {
		&self.0
	}
}

/// Holds the path to the binary.
#[derive(Debug)]
pub struct Binary(PathBuf);

impl Binary {
	/// Returns the path to the binary.
	pub fn binary_path(&self) -> &Path {
		&self.0
	}

	/// Returns the escaped path to the binary.
	pub fn binary_path_escaped(&self) -> String {
		self.0.display().to_string().escape_default().to_string()
	}
}

fn crate_metadata(cargo_manifest: &Path) -> Metadata {
	let mut cargo_lock = cargo_manifest.to_path_buf();
	cargo_lock.set_file_name("Cargo.lock");

	let cargo_lock_existed = cargo_lock.exists();

	// If we can find a `Cargo.lock`, we assume that this is the workspace root and there exists a
	// `Cargo.toml` that we can use for getting the metadata.
	let cargo_manifest = if let Some(mut cargo_lock) = find_cargo_lock(cargo_manifest) {
		cargo_lock.set_file_name("Cargo.toml");
		cargo_lock
	} else {
		cargo_manifest.to_path_buf()
	};

	let mut crate_metadata_command = create_metadata_command(cargo_manifest);
	crate_metadata_command.features(CargoOpt::AllFeatures);

	let crate_metadata = crate_metadata_command
		.exec()
		.expect("`cargo metadata` can not fail on project `Cargo.toml`; qed");
	// If the `Cargo.lock` didn't exist, we need to remove it after
	// calling `cargo metadata`. This is required to ensure that we don't change
	// the build directory outside of the `target` folder. Commands like
	// `cargo publish` require this.
	if !cargo_lock_existed {
		let _ = fs::remove_file(&cargo_lock);
	}

	crate_metadata
}

/// Creates the project, compiles the binary and compacts the binary.
///
/// # Returns
///
/// The path to the compact binary and the bloaty binary.
pub(crate) fn create_and_compile(
	project_cargo_toml: &Path,
	default_rustflags: &str,
	cargo_cmd: CargoCommandVersioned,
	features_to_enable: Vec<String>,
	binary_name: Option<String>,
	target: &str,
) -> (Option<Binary>, BinaryBloaty) {
	let workspace_root = get_workspace_root();
	let workspace = workspace_root.join("wbuild");

	let crate_metadata = crate_metadata(project_cargo_toml);

	let project = create_project(
		project_cargo_toml,
		&workspace,
		&crate_metadata,
		crate_metadata.workspace_root.as_ref(),
		features_to_enable,
	);

	let profile = build_project(&project, default_rustflags, cargo_cmd, target);
	let (binary, binary_compressed, bloaty) =
		compact_file(&project, profile, project_cargo_toml, binary_name, target);

	binary
		.as_ref()
		.map(|binary| copy_binary_to_target_directory(project_cargo_toml, binary));

	binary_compressed.as_ref().map(|binary_compressed| {
		copy_binary_to_target_directory(project_cargo_toml, binary_compressed)
	});

	let final_binary = binary_compressed.or(binary);

	generate_rerun_if_changed_instructions(
		project_cargo_toml,
		&project,
		&workspace,
		final_binary.as_ref(),
		&bloaty,
	);

	if let Err(err) = adjust_mtime(&bloaty, final_binary.as_ref()) {
		build_helper::warning!("Error while adjusting the mtime of the binaries: {}", err)
	}

	(final_binary, bloaty)
}

/// Adjust the mtime of the bloaty and compressed/compact files.
///
/// We add the bloaty and the compressed/compact file to the `rerun-if-changed` files.
/// Cargo/Rustc determines based on the timestamp of the `invoked.timestamp` file that can be found
/// in the `OUT_DIR/..`, if it needs to rerun a `build.rs` script. The problem is that this
/// `invoked.timestamp` is created when the `build.rs` is executed and the binaries are created
/// later. This leads to them having a later mtime than the `invoked.timestamp` file and thus,
/// cargo/rustc always re-executes the `build.rs` script. To hack around this, we copy the mtime of
/// the `invoked.timestamp` to the binaries.
fn adjust_mtime(
	bloaty_binary: &BinaryBloaty,
	compressed_or_compact_binary: Option<&Binary>,
) -> std::io::Result<()> {
	let out_dir = build_helper::out_dir();
	let invoked_timestamp = out_dir.join("../invoked.timestamp");

	// Get the mtime of the `invoked.timestamp`
	let metadata = fs::metadata(invoked_timestamp)?;
	let mtime = filetime::FileTime::from_last_modification_time(&metadata);

	filetime::set_file_mtime(bloaty_binary.binary_bloaty_path(), mtime)?;
	if let Some(binary) = compressed_or_compact_binary.as_ref() {
		filetime::set_file_mtime(binary.binary_path(), mtime)?;
	}

	Ok(())
}

/// Find the `Cargo.lock` relative to the `OUT_DIR` environment variable.
///
/// If the `Cargo.lock` cannot be found, we emit a warning and return `None`.
fn find_cargo_lock(cargo_manifest: &Path) -> Option<PathBuf> {
	fn find_impl(mut path: PathBuf) -> Option<PathBuf> {
		loop {
			if path.join("Cargo.lock").exists() {
				return Some(path.join("Cargo.lock"))
			}

			if !path.pop() {
				return None
			}
		}
	}

	if let Ok(workspace) = env::var(crate::BUILD_WORKSPACE_HINT) {
		let path = PathBuf::from(workspace);

		if path.join("Cargo.lock").exists() {
			return Some(path.join("Cargo.lock"))
		} else {
			build_helper::warning!(
				"`{}` env variable doesn't point to a directory that contains a `Cargo.lock`.",
				crate::BUILD_WORKSPACE_HINT,
			);
		}
	}

	if let Some(path) = find_impl(build_helper::out_dir()) {
		return Some(path)
	}

	build_helper::warning!(
		"Could not find `Cargo.lock` for `{}`, while searching from `{}`. \
		 To fix this, point the `{}` env variable to the directory of the workspace being compiled.",
		cargo_manifest.display(),
		build_helper::out_dir().display(),
		crate::BUILD_WORKSPACE_HINT,
	);

	None
}

/// Extract the crate name from the given `Cargo.toml`.
fn get_crate_name(cargo_manifest: &Path) -> String {
	let cargo_toml: Table = toml::from_str(
		&fs::read_to_string(cargo_manifest).expect("File exists as checked before; qed"),
	)
	.expect("Cargo manifest is a valid toml file; qed");

	let package = cargo_toml
		.get("package")
		.and_then(|t| t.as_table())
		.expect("`package` key exists in valid `Cargo.toml`; qed");

	package
		.get("name")
		.and_then(|p| p.as_str())
		.map(ToOwned::to_owned)
		.expect("Package name exists; qed")
}

/// Returns the name for the binary.
fn get_binary_name(cargo_manifest: &Path) -> String {
	get_crate_name(cargo_manifest).replace('-', "_")
}

/// Returns the root path of the workspace.
fn get_workspace_root() -> PathBuf {
	let mut out_dir = build_helper::out_dir();

	loop {
		match out_dir.parent() {
			Some(parent) if out_dir.ends_with("build") => return parent.to_path_buf(),
			_ =>
				if !out_dir.pop() {
					break
				},
		}
	}

	panic!("Could not find target dir in: {}", build_helper::out_dir().display())
}

fn create_project_cargo_toml(
	workspace: &Path,
	workspace_root_path: &Path,
	crate_name: &str,
	crate_path: &Path,
	binary: &str,
	enabled_features: impl Iterator<Item = String>,
) {
	// For the PVF workers we want unwinding panics (we have panic handlers in place).
	// TODO: Allow customizing this for the generalized builder.
	let panic_setting = "unwind";

	let mut root_workspace_toml: Table = toml::from_str(
		&fs::read_to_string(workspace_root_path.join("Cargo.toml"))
			.expect("Workspace root `Cargo.toml` exists; qed"),
	)
	.expect("Workspace root `Cargo.toml` is a valid toml file; qed");

	let mut workspace_toml = Table::new();

	// Add different profiles which are selected by setting `BUILD_TYPE`.
	let mut release_profile = Table::new();
	release_profile.insert("panic".into(), panic_setting.into());
	release_profile.insert("lto".into(), "thin".into());

	let mut production_profile = Table::new();
	production_profile.insert("inherits".into(), "release".into());
	production_profile.insert("lto".into(), "fat".into());
	production_profile.insert("codegen-units".into(), 1.into());

	let mut dev_profile = Table::new();
	dev_profile.insert("panic".into(), panic_setting.into());

	let mut profile = Table::new();
	profile.insert("release".into(), release_profile.into());
	profile.insert("production".into(), production_profile.into());
	profile.insert("dev".into(), dev_profile.into());

	workspace_toml.insert("profile".into(), profile.into());

	// Add patch section from the project root `Cargo.toml`
	while let Some(mut patch) =
		root_workspace_toml.remove("patch").and_then(|p| p.try_into::<Table>().ok())
	{
		// Iterate over all patches and make the patch path absolute from the workspace root path.
		patch
			.iter_mut()
			.filter_map(|p| {
				p.1.as_table_mut().map(|t| t.iter_mut().filter_map(|t| t.1.as_table_mut()))
			})
			.flatten()
			.for_each(|p| {
				p.iter_mut().filter(|(k, _)| k == &"path").for_each(|(_, v)| {
					if let Some(path) = v.as_str().map(PathBuf::from) {
						if path.is_relative() {
							*v = workspace_root_path.join(path).display().to_string().into();
						}
					}
				})
			});

		workspace_toml.insert("patch".into(), patch.into());
	}

	let mut package = Table::new();
	package.insert("name".into(), format!("{}", crate_name).into());
	package.insert("version".into(), "1.0.0".into());
	package.insert("edition".into(), "2021".into());

	workspace_toml.insert("package".into(), package.into());

	// For PVF workers use `bin` instead of wasm-builder's lib.
	// TODO: Allow choose between bin/lib for the generalized builder.
	let mut bin_array = Array::new();
	let mut bin = Table::new();
	bin.insert("name".into(), binary.into());
	bin.insert("path".into(), "src/main.rs".into());
	bin_array.insert(0, bin.into());

	workspace_toml.insert("bin".into(), bin_array.into());

	// let mut lib = Table::new();
	// lib.insert("name".into(), binary.into());
	// lib.insert("crate-type".into(), vec!["cdylib".to_string()].into());

	// workspace_toml.insert("lib".into(), lib.into());

	let mut dependencies = Table::new();

	let mut project = Table::new();
	project.insert("package".into(), crate_name.into());
	project.insert("path".into(), crate_path.display().to_string().into());
	project.insert("default-features".into(), false.into());
	project.insert("features".into(), enabled_features.collect::<Vec<_>>().into());

	dependencies.insert("project".into(), project.into());

	workspace_toml.insert("dependencies".into(), dependencies.into());

	workspace_toml.insert("workspace".into(), Table::new().into());

	write_file_if_changed(
		workspace.join("Cargo.toml"),
		toml::to_string_pretty(&workspace_toml).expect("workspace toml is valid; qed"),
	);
}

/// Find a package by the given `manifest_path` in the metadata. In case it can't be found by its
/// manifest_path, fallback to finding it by name; this is necessary during publish because the
/// package's manifest path will be *generated* within a specific packaging directory, thus it won't
/// be found by its original path anymore.
///
/// Panics if the package could not be found.
fn find_package_by_manifest_path<'a>(
	pkg_name: &str,
	manifest_path: &Path,
	crate_metadata: &'a cargo_metadata::Metadata,
) -> &'a cargo_metadata::Package {
	if let Some(pkg) = crate_metadata.packages.iter().find(|p| p.manifest_path == manifest_path) {
		return pkg
	}

	let pkgs_by_name = crate_metadata
		.packages
		.iter()
		.filter(|p| p.name == pkg_name)
		.collect::<Vec<_>>();

	if let Some(pkg) = pkgs_by_name.first() {
		if pkgs_by_name.len() > 1 {
			panic!(
				"Found multiple packages matching the name {pkg_name} ({manifest_path:?}): {:?}",
				pkgs_by_name
			);
		} else {
			return pkg
		}
	} else {
		panic!("Failed to find entry for package {pkg_name} ({manifest_path:?}).");
	}
}

/// Get a list of enabled features for the project.
fn project_enabled_features(
	pkg_name: &str,
	cargo_manifest: &Path,
	crate_metadata: &cargo_metadata::Metadata,
) -> Vec<String> {
	let package = find_package_by_manifest_path(pkg_name, cargo_manifest, crate_metadata);

	let std_enabled = package.features.get("std");

	let mut enabled_features = package
		.features
		.iter()
		.filter(|(f, v)| {
			let mut feature_env = f.replace("-", "_");
			feature_env.make_ascii_uppercase();

			// If this is a feature that corresponds only to an optional dependency
			// and this feature is enabled by the `std` feature, we assume that this
			// is only done through the `std` feature. This is a bad heuristic and should
			// be removed after namespaced features are landed:
			// https://doc.rust-lang.org/cargo/reference/unstable.html#namespaced-features
			// Then we can just express this directly in the `Cargo.toml` and do not require
			// this heuristic anymore. However, for the transition phase between now and namespaced
			// features already being present in nightly, we need this code to make
			// runtimes compile with all the possible rustc versions.
			if v.len() == 1 &&
				v.get(0).map_or(false, |v| *v == format!("dep:{}", f)) &&
				std_enabled.as_ref().map(|e| e.iter().any(|ef| ef == *f)).unwrap_or(false)
			{
				return false
			}

			// TODO: Generalize this?
			// We don't want to enable the `std`/`default` feature for the wasm build and
			// we need to check if the feature is enabled by checking the env variable.
			// *f != "std" && *f != "default" &&
			env::var(format!("CARGO_FEATURE_{}", feature_env))
				.map(|v| v == "1")
				.unwrap_or_default()
		})
		.map(|d| d.0.clone())
		.collect::<Vec<_>>();

	// Assert that the "builder" feature is present but disabled.
	assert!(
		package.features.contains_key("builder") &&
			!enabled_features.contains(&"builder".to_string())
	);
	// Enable the builder feature so that it is only present when building the binary.
	enabled_features.push("builder".into());

	enabled_features.sort();
	enabled_features
}

// TODO: Generalize this?
// /// Returns if the project has the `runtime-wasm` feature
// fn has_runtime_wasm_feature_declared(
// 	pkg_name: &str,
// 	cargo_manifest: &Path,
// 	crate_metadata: &cargo_metadata::Metadata,
// ) -> bool {
// 	let package = find_package_by_manifest_path(pkg_name, cargo_manifest, crate_metadata);

// 	package.features.keys().any(|k| k == "runtime-wasm")
// }

/// Create the project used to build the binary.
///
/// # Returns
///
/// The path to the created project.
fn create_project(
	project_cargo_toml: &Path,
	workspace: &Path,
	crate_metadata: &Metadata,
	workspace_root_path: &Path,
	features_to_enable: Vec<String>,
) -> PathBuf {
	let crate_name = get_crate_name(project_cargo_toml);
	let crate_path = project_cargo_toml.parent().expect("Parent path exists; qed");
	let binary = get_binary_name(project_cargo_toml);
	let project_folder = workspace.join(&crate_name);

	fs::create_dir_all(project_folder.join("src")).expect("project dir create can not fail; qed");

	let enabled_features =
		project_enabled_features(&crate_name, project_cargo_toml, crate_metadata);

	// TODO: Generalize this?
	// if has_runtime_wasm_feature_declared(&crate_name, project_cargo_toml, crate_metadata) {
	// 	enabled_features.push("runtime-wasm".into());
	// }

	let mut enabled_features = enabled_features.into_iter().collect::<HashSet<_>>();
	enabled_features.extend(features_to_enable.into_iter());

	create_project_cargo_toml(
		&project_folder,
		workspace_root_path,
		&crate_name,
		crate_path,
		&binary,
		enabled_features.into_iter(),
	);

	write_file_if_changed(project_folder.join("src/lib.rs"), "#![no_std] pub use project::*;");

	let main_path = crate_path.join("src/main.rs");
	if main_path.exists() {
		copy_file_if_changed(main_path, project_folder.join("src/main.rs"));
	}

	if let Some(crate_lock_file) = find_cargo_lock(project_cargo_toml) {
		// Use the `Cargo.lock` of the main project.
		copy_file_if_changed(crate_lock_file, project_folder.join("Cargo.lock"));
	}

	project_folder
}

/// The cargo profile that is used to build the project.
#[derive(Debug, EnumIter)]
enum Profile {
	/// The `--profile dev` profile.
	Debug,
	/// The `--profile release` profile.
	Release,
	/// The `--profile production` profile.
	Production,
}

impl Profile {
	/// Create a profile by detecting which profile is used for the main build.
	///
	/// We cannot easily determine the profile that is used by the main cargo invocation
	/// because the `PROFILE` environment variable won't contain any custom profiles like
	/// "production". It would only contain the builtin profile where the custom profile
	/// inherits from. This is why we inspect the build path to learn which profile is used.
	///
	/// # Note
	///
	/// Can be overriden by setting [`crate::BUILD_TYPE_ENV`].
	fn detect(project: &Path) -> Profile {
		let (name, overriden) = if let Ok(name) = env::var(crate::BUILD_TYPE_ENV) {
			(name, true)
		} else {
			// First go backwards to the beginning of the target directory.
			// Then go forwards to find the "wbuild" directory.
			// We need to go backwards first because when starting from the root there
			// might be a chance that someone has a "wbuild" directory somewhere in the path.
			let name = project
				.components()
				.rev()
				.take_while(|c| c.as_os_str() != "target")
				.collect::<Vec<_>>()
				.iter()
				.rev()
				.take_while(|c| c.as_os_str() != "wbuild")
				.last()
				.expect("We put the project within a `target/.../wbuild` path; qed")
				.as_os_str()
				.to_str()
				.expect("All our profile directory names are ascii; qed")
				.to_string();
			(name, false)
		};
		match (Profile::iter().find(|p| p.directory() == name), overriden) {
			// When not overriden by a env variable we default to using the `Release` profile
			// for the build even when the main build uses the debug build. This
			// is because the `Debug` profile is too slow for normal development activities.
			(Some(Profile::Debug), false) => Profile::Release,
			// For any other profile or when overriden we take it at face value.
			(Some(profile), _) => profile,
			// For non overriden unknown profiles we fall back to `Release`.
			// This allows us to continue building when a custom profile is used for the
			// main builds cargo. When explicitly passing a profile via env variable we are
			// not doing a fallback.
			(None, false) => {
				let profile = Profile::Release;
				build_helper::warning!(
					"Unknown cargo profile `{}`. Defaulted to `{:?}` for the runtime build.",
					name,
					profile,
				);
				profile
			},
			// Invalid profile specified.
			(None, true) => {
				// We use println! + exit instead of a panic in order to have a cleaner output.
				println!(
					"Unexpected profile name: `{}`. One of the following is expected: {:?}",
					name,
					Profile::iter().map(|p| p.directory()).collect::<Vec<_>>(),
				);
				process::exit(1);
			},
		}
	}

	/// The name of the profile as supplied to the cargo `--profile` cli option.
	fn name(&self) -> &'static str {
		match self {
			Self::Debug => "dev",
			Self::Release => "release",
			Self::Production => "production",
		}
	}

	/// The sub directory within `target` where cargo places the build output.
	///
	/// # Note
	///
	/// Usually this is the same as [`Self::name`] with the exception of the debug
	/// profile which is called `dev`.
	fn directory(&self) -> &'static str {
		match self {
			Self::Debug => "debug",
			_ => self.name(),
		}
	}

	/// Whether the resulting binary should be compacted and compressed.
	fn wants_compact(&self) -> bool {
		!matches!(self, Self::Debug)
	}
}

/// Check environment whether we should build without network
fn offline_build() -> bool {
	env::var(OFFLINE).map_or(false, |v| v == "true")
}

/// Build the project to create the binary.
fn build_project(
	project: &Path,
	default_rustflags: &str,
	cargo_cmd: CargoCommandVersioned,
	target: &str,
) -> Profile {
	let manifest_path = project.join("Cargo.toml");
	let mut build_cmd = cargo_cmd.command();

	// TODO: If wasm-builder is ever refactored to use this generalized builder, it needs the
	// following default flags:
	//
	// ```
	// -C target-cpu=mvp -C target-feature=-sign-ext -C link-arg=--export-table
	// ```
	//
	// See <https://github.com/paritytech/substrate/issues/13982>.
	let rustflags = format!(
		"{} {}",
		default_rustflags,
		env::var(crate::BUILD_RUSTFLAGS_ENV).unwrap_or_default(),
	);

	build_cmd
		.args(&["rustc", &format!("--target={}", target)])
		.arg(format!("--manifest-path={}", manifest_path.display()))
		.env("RUSTFLAGS", rustflags)
		// Unset the `CARGO_TARGET_DIR` to prevent a cargo deadlock (cargo locks a target dir
		// exclusive). The runner project is created in `CARGO_TARGET_DIR` and executing it will
		// create a sub target directory inside of `CARGO_TARGET_DIR`.
		.env_remove("CARGO_TARGET_DIR")
		// As we are being called inside a build-script, this env variable is set. However, we set
		// our own `RUSTFLAGS` and thus, we need to remove this. Otherwise cargo favors this
		// env variable.
		.env_remove("CARGO_ENCODED_RUSTFLAGS")
		// We don't want to call ourselves recursively
		.env(crate::SKIP_BUILD_ENV, "");

	if super::color_output_enabled() {
		build_cmd.arg("--color=always");
	}

	let profile = Profile::detect(project);
	build_cmd.arg("--profile");
	build_cmd.arg(profile.name());

	if offline_build() {
		build_cmd.arg("--offline");
	}

	println!("{}", colorize_info_message("Information that should be included in a bug report."));
	println!("{} {:?}", colorize_info_message("Executing build command:"), build_cmd);
	println!("{} {}", colorize_info_message("Using rustc version:"), cargo_cmd.rustc_version());

	match build_cmd.status().map(|s| s.success()) {
		Ok(true) => profile,
		// Use `process.exit(1)` to have a clean error output.
		_ => process::exit(1),
	}
}

/// Compact the binary and compress it using zstd.
fn compact_file(
	project: &Path,
	profile: Profile,
	cargo_manifest: &Path,
	out_name: Option<String>,
	target: &str,
) -> (Option<Binary>, Option<Binary>, BinaryBloaty) {
	let default_out_name = get_binary_name(cargo_manifest);
	let out_name = out_name.unwrap_or_else(|| default_out_name.clone());
	let in_path = project
		.join(format!("target/{}", target))
		.join(profile.directory())
		.join(format!("{}", default_out_name));

	let (compact_path, compact_compressed_path) = if profile.wants_compact() {
		// TODO: For a generalized builder we may want to support passing in a function to compact the
		// binary.

		let compact_path = project.join(format!("{}.compact", out_name));
		// TODO: Wasm compaction goes here.
		// TODO: For other targets, compaction is a noop so the compact
		//       and compressed binaries are the same right now.
		fs::copy(&in_path, &compact_path).expect("Copying the binary to the project dir.");

		let compact_compressed_path = project.join(format!("{}.compact.compressed", out_name));
		if compress(&compact_path, &compact_compressed_path) {
			(Some(Binary(compact_path)), Some(Binary(compact_compressed_path)))
		} else {
			(Some(Binary(compact_path)), None)
		}
	} else {
		(None, None)
	};

	let bloaty_path = project.join(format!("{}", out_name));
	fs::copy(in_path, &bloaty_path).expect("Copying the bloaty file to the project dir.");

	(compact_path, compact_compressed_path, BinaryBloaty(bloaty_path))
}

fn compress(binary_path: &Path, compressed_binary_out_path: &Path) -> bool {
	use sp_maybe_compressed_blob::CODE_BLOB_BOMB_LIMIT;

	let data = fs::read(binary_path).expect("Failed to read binary");
	if let Some(compressed) = sp_maybe_compressed_blob::compress(&data, CODE_BLOB_BOMB_LIMIT) {
		fs::write(compressed_binary_out_path, &compressed[..])
			.expect("Failed to write compressed binary");

		true
	} else {
		build_helper::warning!(
			"Writing uncompressed binary. Exceeded maximum size {}",
			CODE_BLOB_BOMB_LIMIT,
		);

		false
	}
}

/// Custom wrapper for a [`cargo_metadata::Package`] to store it in
/// a `HashSet`.
#[derive(Debug)]
struct DeduplicatePackage<'a> {
	package: &'a cargo_metadata::Package,
	identifier: String,
}

impl<'a> From<&'a cargo_metadata::Package> for DeduplicatePackage<'a> {
	fn from(package: &'a cargo_metadata::Package) -> Self {
		Self {
			package,
			identifier: format!("{}{}{:?}", package.name, package.version, package.source),
		}
	}
}

impl<'a> Hash for DeduplicatePackage<'a> {
	fn hash<H: Hasher>(&self, state: &mut H) {
		self.identifier.hash(state);
	}
}

impl<'a> PartialEq for DeduplicatePackage<'a> {
	fn eq(&self, other: &Self) -> bool {
		self.identifier == other.identifier
	}
}

impl<'a> Eq for DeduplicatePackage<'a> {}

impl<'a> Deref for DeduplicatePackage<'a> {
	type Target = cargo_metadata::Package;

	fn deref(&self) -> &Self::Target {
		self.package
	}
}

fn create_metadata_command(path: impl Into<PathBuf>) -> MetadataCommand {
	let mut metadata_command = MetadataCommand::new();
	metadata_command.manifest_path(path);

	if offline_build() {
		metadata_command.other_options(vec!["--offline".to_owned()]);
	}
	metadata_command
}

/// Generate the `rerun-if-changed` instructions for cargo to make sure that the binary is
/// rebuilt when needed.
fn generate_rerun_if_changed_instructions(
	cargo_manifest: &Path,
	project_folder: &Path,
	workspace: &Path,
	compressed_or_compact: Option<&Binary>,
	bloaty_binary: &BinaryBloaty,
) {
	// Rerun `build.rs` if the `Cargo.lock` changes
	if let Some(cargo_lock) = find_cargo_lock(cargo_manifest) {
		rerun_if_changed(cargo_lock);
	}

	let metadata = create_metadata_command(project_folder.join("Cargo.toml"))
		.exec()
		.expect("`cargo metadata` can not fail!");

	let package = metadata
		.packages
		.iter()
		.find(|p| p.manifest_path == cargo_manifest)
		.expect("The crate package is contained in its own metadata; qed");

	// Start with the dependencies of the crate we want to compile.
	let mut dependencies = package.dependencies.iter().collect::<Vec<_>>();

	// Collect all packages by follow the dependencies of all packages we find.
	let mut packages = HashSet::new();
	packages.insert(DeduplicatePackage::from(package));

	while let Some(dependency) = dependencies.pop() {
		let path_or_git_dep =
			dependency.source.as_ref().map(|s| s.starts_with("git+")).unwrap_or(true);

		let package = metadata
			.packages
			.iter()
			.filter(|p| !p.manifest_path.starts_with(workspace))
			.find(|p| {
				// Check that the name matches and that the version matches or this is
				// a git or path dep. A git or path dependency can only occur once, so we don't
				// need to check the version.
				(path_or_git_dep || dependency.req.matches(&p.version)) && dependency.name == p.name
			});

		if let Some(package) = package {
			if packages.insert(DeduplicatePackage::from(package)) {
				dependencies.extend(package.dependencies.iter());
			}
		}
	}

	// Make sure that if any file/folder of a dependency change, we need to rerun the `build.rs`
	packages.iter().for_each(package_rerun_if_changed);

	compressed_or_compact.map(|w| rerun_if_changed(w.binary_path()));
	rerun_if_changed(bloaty_binary.binary_bloaty_path());

	// Register our env variables
	println!("cargo:rerun-if-env-changed={}", crate::SKIP_BUILD_ENV);
	println!("cargo:rerun-if-env-changed={}", crate::BUILD_TYPE_ENV);
	println!("cargo:rerun-if-env-changed={}", crate::BUILD_RUSTFLAGS_ENV);
	println!("cargo:rerun-if-env-changed={}", crate::TARGET_DIRECTORY);
	println!("cargo:rerun-if-env-changed={}", crate::BUILD_TOOLCHAIN);
}

/// Track files and paths related to the given package to rerun `build.rs` on any relevant change.
fn package_rerun_if_changed(package: &DeduplicatePackage) {
	let mut manifest_path = package.manifest_path.clone();
	if manifest_path.ends_with("Cargo.toml") {
		manifest_path.pop();
	}

	WalkDir::new(&manifest_path)
		.into_iter()
		.filter_entry(|p| {
			// Ignore this entry if it is a directory that contains a `Cargo.toml` that is not the
			// `Cargo.toml` related to the current package. This is done to ignore sub-crates of a
			// crate. If such a sub-crate is a dependency, it will be processed independently
			// anyway.
			p.path() == manifest_path || !p.path().is_dir() || !p.path().join("Cargo.toml").exists()
		})
		.filter_map(|p| p.ok().map(|p| p.into_path()))
		.filter(|p| {
			p.is_dir() || p.extension().map(|e| e == "rs" || e == "toml").unwrap_or_default()
		})
		.for_each(rerun_if_changed);
}

/// Copy the binary to the target directory set in `TARGET_DIRECTORY` environment
/// variable. If the variable is not set, this is a no-op.
fn copy_binary_to_target_directory(cargo_manifest: &Path, binary: &Binary) {
	let target_dir = match env::var(crate::TARGET_DIRECTORY) {
		Ok(path) => PathBuf::from(path),
		Err(_) => return,
	};

	if !target_dir.is_absolute() {
		// We use println! + exit instead of a panic in order to have a cleaner output.
		println!(
			"Environment variable `{}` with `{}` is not an absolute path!",
			crate::TARGET_DIRECTORY,
			target_dir.display(),
		);
		process::exit(1);
	}

	fs::create_dir_all(&target_dir).expect("Creates `TARGET_DIRECTORY`.");

	fs::copy(binary.binary_path(), target_dir.join(format!("{}", get_binary_name(cargo_manifest))))
		.expect("Copies binary to `TARGET_DIRECTORY`.");
}
