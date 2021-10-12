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

//! Remote tests for bags-list pallet.

use clap::arg_enum;
use pallet_election_provider_multi_phase as EPM;
use structopt::StructOpt;

const LOG_TARGET: &'static str = "remote-ext-tests::bags-list";

mod migration_test;
mod sanity_check;

arg_enum! {
	#[derive(Debug)]
	enum Command {
		CheckMigration,
		SanityChecks,
	}
}

arg_enum! {
	#[derive(Debug)]
	enum Runtime {
		Kusama,
		Westend,
	}
}

#[derive(StructOpt)]
struct Cli {
	#[structopt(long, short, default_value = "wss://kusama-rpc.polkadot.io")]
	uri: String,
	#[structopt(long, short, case_insensitive = true, possible_values = &Runtime::variants(), default_value = "kusama")]
	runtime: Runtime,
	#[structopt(long, short, case_insensitive = true, possible_values = &Command::variants(), default_value = "check-migration")]
	command: Command,
}

pub(crate) trait RuntimeT:
	pallet_staking::Config + pallet_bags_list::Config + EPM::Config + frame_system::Config
{
}
impl<T: pallet_staking::Config + pallet_bags_list::Config + EPM::Config + frame_system::Config>
	RuntimeT for T
{
}

#[tokio::main]
async fn main() {
	let options = Cli::from_args();
	sp_tracing::try_init_simple();

	match (options.runtime, options.command) {
		(Runtime::Kusama, Command::CheckMigration) => {
			use kusama_runtime::{constants::currency::UNITS, Block, Runtime};
			migration_test::execute::<Runtime, Block>(UNITS as u64, options.uri.clone()).await;
		},
		(Runtime::Kusama, Command::SanityChecks) => {
			use kusama_runtime::{Block, Runtime};
			sanity_check::execute::<Runtime, Block>(options.uri.clone()).await;
		},

		(Runtime::Westend, Command::CheckMigration) => {
			use westend_runtime::{constants::currency::UNITS, Block, Runtime};
			migration_test::execute::<Runtime, Block>(UNITS as u64, options.uri.clone()).await;
		},
		(Runtime::Westend, Command::SanityChecks) => {
			use westend_runtime::{Block, Runtime};
			sanity_check::execute::<Runtime, Block>(options.uri.clone()).await;
		},
	}
}
