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

//! Puppet worker used for integration tests.

use sp_tracing;

fn main() {
	sp_tracing::try_init_simple();

	let args = std::env::args().collect::<Vec<_>>();
	if args.len() < 3 {
		panic!("wrong number of arguments");
	}

	let subcommand = &args[1];
	match subcommand.as_ref() {
		"exit" => {
			std::process::exit(1);
		},
		"sleep" => {
			std::thread::sleep(std::time::Duration::from_secs(5));
		},
		other => panic!("unknown subcommand: {}", other),
	}
}
