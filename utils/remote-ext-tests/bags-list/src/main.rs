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

//! Remote tests.

use structopt::StructOpt;

mod voter_bags;

#[derive(StructOpt)]
enum Runtime {
	Kusama,
	Polkadot,
}

impl std::str::FromStr for Runtime {
	type Err = &'static str;
	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_lowercase().as_str() {
			"kusama" => Ok(Runtime::Kusama),
			"polkadot" => Ok(Runtime::Polkadot),
			_ => Err("wrong Runtime: can be 'polkadot' or 'kusama'."),
		}
	}
}

#[derive(StructOpt)]
struct Cli {
	#[structopt(long, default_value = "wss://rpc.polkadot.io")]
	uri: String,
	#[structopt(long, short, default_value = "polkadot")]
	runtime: Runtime,
}

#[tokio::main]
async fn main() {
	let options = Cli::from_args();
	match options.runtime {
		Runtime::Kusama => {
			use kusama_runtime::{constants::currency::UNITS, Block, Runtime};
			voter_bags::test_voter_bags_migration::<Runtime, Block>(
				UNITS as u64,
				options.uri.clone(),
			)
			.await;
		},
		Runtime::Polkadot => {
			use polkadot_runtime::{constants::currency::UNITS, Block, Runtime};
			voter_bags::test_voter_bags_migration::<Runtime, Block>(
				UNITS as u64,
				options.uri.clone(),
			)
			.await;
		},
	}
}
