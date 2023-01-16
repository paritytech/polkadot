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
