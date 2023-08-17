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

//! Various things for testing other crates.
//!
//! N.B. This is not guarded with some feature flag. Overexposing items here may affect the final
//!      artifact even for production builds.

pub use crate::worker_intf::{spawn_with_program_path, SpawnErr};

use polkadot_primitives::ExecutorParams;

/// A function that emulates the stitches together behaviors of the preparation and the execution
/// worker in a single synchronous function.
pub fn validate_candidate(
	code: &[u8],
	params: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
	use polkadot_node_core_pvf_execute_worker::Executor;
	use polkadot_node_core_pvf_prepare_worker::{prepare, prevalidate};

	let code = sp_maybe_compressed_blob::decompress(code, 10 * 1024 * 1024)
		.expect("Decompressing code failed");

	let blob = prevalidate(&code)?;
	let compiled_artifact_blob = prepare(blob, &ExecutorParams::default())?;

	let executor = Executor::new(ExecutorParams::default())?;
	let result = unsafe {
		// SAFETY: This is trivially safe since the artifact is obtained by calling `prepare`
		//         and is written into a temporary directory in an unmodified state.
		executor.execute(&compiled_artifact_blob, params)?
	};

	Ok(result)
}

/// Use this macro to declare a `fn main() {}` that will check the arguments and dispatch them to
/// the appropriate worker, making the executable that can be used for spawning workers.
#[macro_export]
macro_rules! decl_puppet_worker_main {
	() => {
		fn main() {
			$crate::sp_tracing::try_init_simple();

			let args = std::env::args().collect::<Vec<_>>();
			if args.len() == 1 {
				panic!("wrong number of arguments");
			}

			let entrypoint = match args[1].as_ref() {
				"exit" => {
					std::process::exit(1);
				},
				"sleep" => {
					std::thread::sleep(std::time::Duration::from_secs(5));
					return
				},
				"prepare-worker" => $crate::prepare_worker_entrypoint,
				"execute-worker" => $crate::execute_worker_entrypoint,
				other => panic!("unknown subcommand: {}", other),
			};

			let mut node_version = None;
			let mut socket_path: &str = "";

			for i in (2..args.len()).step_by(2) {
				match args[i].as_ref() {
					"--socket-path" => socket_path = args[i + 1].as_str(),
					"--node-impl-version" => node_version = Some(args[i + 1].as_str()),
					arg => panic!("Unexpected argument found: {}", arg),
				}
			}

			entrypoint(&socket_path, node_version, None);
		}
	};
}
