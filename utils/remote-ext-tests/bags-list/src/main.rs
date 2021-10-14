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
use sp_std::convert::TryInto;
use structopt::StructOpt;

const LOG_TARGET: &'static str = "remote-ext-tests::bags-list";

mod migration_test;
mod sanity_check;

arg_enum! {
	#[derive(Debug)]
	enum Command {
		CheckMigration,
		SanityCheck,
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
	#[structopt(long, short, case_insensitive = true, possible_values = &Command::variants(), default_value = "SanityCheck")]
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

fn percent(portion: u32, total: u32) -> f64 {
	(portion as f64 / total as f64) * 100f64
}

pub(crate) fn display_and_check_bags<Runtime: RuntimeT>(
	currency_unit: u64,
	currency_name: &'static str,
) {
	use frame_election_provider_support::SortedListProvider;
	use frame_support::traits::Get;

	let min_nominator_bond = <pallet_staking::MinNominatorBond<Runtime>>::get();
	log::info!(target: LOG_TARGET, "min nominator bond is {:?}", min_nominator_bond);

	let voter_list_count = <Runtime as pallet_staking::Config>::SortedListProvider::count();

	// go through every bag to track the total number of voters within bags and log some info about
	// how voters are distributed within the bags.
	let mut seen_in_bags = 0;
	for vote_weight_thresh in <Runtime as pallet_bags_list::Config>::BagThresholds::get() {
		// threshold in terms of UNITS (e.g. KSM, DOT etc)
		let vote_weight_thresh_as_unit = *vote_weight_thresh as f64 / currency_unit as f64;
		let pretty_thresh = format!("Threshold: {}. {}", vote_weight_thresh_as_unit, currency_name);

		let bag = match pallet_bags_list::Pallet::<Runtime>::list_bags_get(*vote_weight_thresh) {
			Some(bag) => bag,
			None => {
				log::info!(target: LOG_TARGET, "{} NO VOTERS.", pretty_thresh);
				continue
			},
		};

		let voters_in_bag = bag.std_iter().count() as u32;

		for id in bag.std_iter().map(|node| node.std_id().clone()) {
			let vote_weight = pallet_staking::Pallet::<Runtime>::weight_of(&id);
			let vote_weight_as_balance: pallet_staking::BalanceOf<Runtime> =
				vote_weight.try_into().map_err(|_| "can't convert").unwrap();

			if vote_weight_as_balance < min_nominator_bond {
				log::warn!(
					target: LOG_TARGET,
					"{} Account found below min bond: {:?}.",
					pretty_thresh,
					id
				);
			}

			let node =
				pallet_bags_list::Node::<Runtime>::get(&id).expect("node in bag must exist.");
			if node.is_misplaced(vote_weight) {
				log::warn!(
					target: LOG_TARGET,
					"Account {:?} can be rebagged from {:?} to {:?}",
					id,
					vote_weight_thresh_as_unit,
					pallet_bags_list::notional_bag_for::<Runtime>(vote_weight) as f64 /
						currency_unit as f64
				);
			}
		}

		// update our overall counter
		seen_in_bags += voters_in_bag;

		// percentage of all nominators
		let percent_of_voters = percent(voters_in_bag, voter_list_count);

		log::info!(
			target: LOG_TARGET,
			"{} Nominators: {} [%{:.3}]",
			pretty_thresh,
			voters_in_bag,
			percent_of_voters,
		);
	}

	if seen_in_bags != voter_list_count {
		log::error!(
			target: LOG_TARGET,
			"bags list population ({}) not on par whoever is voter_list ({})",
			seen_in_bags,
			voter_list_count,
		)
	}
}

#[tokio::main]
async fn main() {
	let options = Cli::from_args();
	sp_tracing::try_init_simple();

	log::info!(
		target: LOG_TARGET,
		"using runtime {:?} / command: {:?}",
		options.runtime,
		options.command
	);

	match (options.runtime, options.command) {
		(Runtime::Kusama, Command::CheckMigration) => {
			use kusama_runtime::{constants::currency::UNITS, Block, Runtime};
			migration_test::execute::<Runtime, Block>(UNITS as u64, "KSM", options.uri.clone())
				.await;
		},
		(Runtime::Kusama, Command::SanityCheck) => {
			use kusama_runtime::{constants::currency::UNITS, Block, Runtime};
			sanity_check::execute::<Runtime, Block>(UNITS as u64, "KSM", options.uri.clone()).await;
		},

		(Runtime::Westend, Command::CheckMigration) => {
			use westend_runtime::{constants::currency::UNITS, Block, Runtime};
			migration_test::execute::<Runtime, Block>(UNITS as u64, "WND", options.uri.clone())
				.await;
		},
		(Runtime::Westend, Command::SanityCheck) => {
			use westend_runtime::{constants::currency::UNITS, Block, Runtime};
			sanity_check::execute::<Runtime, Block>(UNITS as u64, "WND", options.uri.clone()).await;
		},
	}
}
