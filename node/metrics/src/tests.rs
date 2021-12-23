// Copyright 2021 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.
#![cfg(feature = "runtime-metrics")]
use assert_cmd::cargo::cargo_bin;
use std::{convert::TryInto, process::Command, thread, time::Duration};
use tempfile::tempdir;

#[test]
#[cfg(unix)]
fn runtime_can_publish_metrics() {
	use hyper::{Client, Uri};
	use nix::{
		sys::signal::{kill, Signal::SIGINT},
		unistd::Pid,
	};
	use std::convert::TryFrom;

	const RUNTIME_METRIC_NAME: &str = "polkadot_parachain_inherent_data_bitfields_processed";
	const DEFAULT_PROMETHEUS_PORT: u16 = 9615;
	let metrics_uri = format!("http://localhost:{}/metrics", DEFAULT_PROMETHEUS_PORT);

	// Start the node with tracing enabled and forced wasm runtime execution.
	let cmd = Command::new(cargo_bin("polkadot"))
		// Runtime metrics require this trace target.
		.args(&["--tracing-targets", "wasm_tracing=trace"])
		.args(&["--execution", "wasm"])
		.args(&["--dev", "-d"])
		.arg(tempdir().expect("failed to create temp dir.").path())
		.spawn()
		.expect("failed to start the node process");

	// Enough time to author one block.
	thread::sleep(Duration::from_secs(10));

	let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

	runtime.block_on(async {
		let client = Client::new();

		let res = client
			.get(Uri::try_from(&metrics_uri).expect("bad URI"))
			.await
			.expect("get request failed");

		let body = String::from_utf8(
			hyper::body::to_bytes(res).await.expect("can't get body as bytes").to_vec(),
		)
		.expect("body is not an UTF8 string");

		// Time to die.
		kill(Pid::from_raw(cmd.id().try_into().unwrap()), SIGINT)
			.expect("failed to kill the node process");

		// If the node has authored at least 1 block this should pass.
		assert!(body.contains(&RUNTIME_METRIC_NAME));
	});
}
