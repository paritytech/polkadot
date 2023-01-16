// Copyright 2021 Parity Technologies (UK) Ltd.
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

pub mod worker_common {
	pub use crate::worker_common::{spawn_with_program_path, SpawnErr};
}

/// A function that emulates the stitches together behaviors of the preparation and the execution
/// worker in a single synchronous function.
pub fn validate_candidate(
	code: &[u8],
	params: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
	use crate::executor_intf::{prepare, prevalidate, Executor};

	let code = sp_maybe_compressed_blob::decompress(code, 10 * 1024 * 1024)
		.expect("Decompressing code failed");

	let blob = prevalidate(&code)?;
	let artifact = prepare(blob)?;
	let tmpdir = tempfile::tempdir()?;
	let artifact_path = tmpdir.path().join("blob");
	std::fs::write(&artifact_path, &artifact)?;

	let executor = Executor::new()?;
	let result = unsafe {
		// SAFETY: This is trivially safe since the artifact is obtained by calling `prepare`
		//         and is written into a temporary directory in an unmodified state.
		executor.execute(&artifact_path, params)?
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
			if args.len() < 2 {
				panic!("wrong number of arguments");
			}

			let subcommand = &args[1];
			match subcommand.as_ref() {
				"sleep" => {
					std::thread::sleep(std::time::Duration::from_secs(5));
				},
				"prepare-worker" => {
					let socket_path = &args[2];
					$crate::prepare_worker_entrypoint(socket_path);
				},
				"execute-worker" => {
					let socket_path = &args[2];
					$crate::execute_worker_entrypoint(socket_path);
				},
				other => panic!("unknown subcommand: {}", other),
			}
		}
	};
}
