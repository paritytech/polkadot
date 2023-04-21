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

mod common;
mod execute;
mod executor_intf;
mod memory_stats;
mod prepare;

#[doc(hidden)]
pub mod testing;

#[doc(hidden)]
pub use sp_tracing;

pub use execute::worker_entrypoint as execute_worker_entrypoint;
pub use prepare::worker_entrypoint as prepare_worker_entrypoint;

pub use executor_intf::{prepare, prevalidate};

// NOTE: Initializing logging in e.g. tests will not have an effect in the workers, as they are
//       separate spawned processes. Run with e.g. `RUST_LOG=parachain::pvf-worker=trace`.
const LOG_TARGET: &str = "parachain::pvf-worker";

/// Use this macro to declare a `fn main() {}` that will create an executable that can be used for
/// spawning the desired worker.
#[macro_export(local_inner_macros)]
macro_rules! decl_worker_main {
	($command:tt) => {
		fn main() {
			$crate::sp_tracing::try_init_simple();

			let args = std::env::args().collect::<Vec<_>>();

			let mut version = None;
			let mut socket_path: &str = "";

			for i in 1..args.len() {
				match args[i].as_ref() {
					"--socket-path" => socket_path = args[i + 1].as_str(),
					"--node-version" => version = Some(args[i + 1].as_str()),
					_ => (),
				}
			}

			decl_worker_main_command!($command, socket_path, version)
		}
	};
}

#[macro_export]
#[doc(hidden)]
macro_rules! decl_worker_main_command {
	(prepare, $socket_path:expr, $version: expr) => {
		$crate::prepare_worker_entrypoint(&$socket_path, $version)
	};
	(execute, $socket_path:expr, $version: expr) => {
		$crate::execute_worker_entrypoint(&$socket_path, $version)
	};
}
