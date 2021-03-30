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

#![cfg(feature = "real-overseer")]

//! A simple binary that allows mocking the behavior of the real worker.

fn main() {
	let args = std::env::args().collect::<Vec<_>>();
	if args.len() < 2 {
		panic!("wrong number of arguments");
	}

	let subcommand = &args[1];
	match subcommand.as_ref() {
		"prepare-worker" => {
			let socket_path = &args[2];
			polkadot_node_core_pvf::prepare_worker_entrypoint(socket_path);
		}
		"execute-worker" => {
			let socket_path = &args[2];
			polkadot_node_core_pvf::execute_worker_entrypoint(socket_path);
		}
		other => panic!("unknown subcommand: {}", other),
	}
}
