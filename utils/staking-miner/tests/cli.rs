use assert_cmd::{cargo::cargo_bin, Command};

#[test]
fn cli_version_works() {
	let crate_name = env!("CARGO_PKG_NAME");
	let output = Command::new(cargo_bin(crate_name)).arg("--version").output().unwrap();

	assert!(output.status.success(), "command returned with non-success exit code");
	let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();

	assert_eq!(version, format!("{} {}", crate_name, env!("CARGO_PKG_VERSION")));
}
