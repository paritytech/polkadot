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

//! Generic remote tests for the voter bags module.

use frame_election_provider_support::SortedListProvider;
use frame_support::traits::Get;
use pallet_staking::Nominators;
use remote_externalities::{Builder, Mode, OnlineConfig, SnapshotConfig};
use sp_runtime::traits::Block as BlockT;
use sp_storage::well_known_keys;
use sp_core::hashing::twox_128;
use sp_std::{convert::TryInto};

const LOG_TARGET: &'static str = "remote-ext-tests";

/// Test voter bags migration. `currency_unit` is the number of planks per the
/// the runtimes `UNITS` (i.e. number of decimal places per DOT, KSM etc)
pub(crate) async fn test_voter_bags_migration<
	Runtime: pallet_staking::Config + pallet_bags_list::Config,
	Block: BlockT,
>(
	currency_unit: u64,
) {
	use std::env;

	sp_tracing::try_init_simple();

	let ws_url = match env::var("WS_RPC") {
		Ok(ws_url) => ws_url,
		Err(_) => panic!("Must set env var `WS_RPC=<ws-url>`"),
	};

	let mut ext = Builder::<Block>::new()
		.mode(Mode::Online(OnlineConfig {
			transport: ws_url.to_string().into(),
			modules: vec!["Staking".to_string(), "System".to_string()],
			at: None,
			state_snapshot: None,
		}))
		.inject_hashed_key(well_known_keys::CODE)
		// TODO: this query fails
		// .inject_hashed_key(&[twox_128(b"System"), twox_128(b"SS58Prefix")].concat())
		.build()
		.await
		.unwrap();

	ext.execute_with(|| {
		// set the ss58 prefix so addresses printed below are human friendly.
		sp_core::crypto::set_default_ss58_version(Runtime::SS58Prefix::get().try_into().unwrap());

		// get the nominator & validator count prior to migrating; these should be invariant.
		let pre_migrate_nominator_count = <Nominators<Runtime>>::iter().count() as u32;
		log::info!(target: LOG_TARGET, "Nominator count: {}", pre_migrate_nominator_count);

		// run the actual migration,
		let moved = <Runtime as pallet_staking::Config>::SortedListProvider::regenerate(
			pallet_staking::Nominators::<Runtime>::iter().map(|(n, _)| n),
			pallet_staking::Pallet::<Runtime>::weight_of_fn(),
		);
		log::info!(target: LOG_TARGET, "Moved {} nominators", moved);

		let voter_list_len =
			<Runtime as pallet_staking::Config>::SortedListProvider::iter().count() as u32;
		let voter_list_count = <Runtime as pallet_staking::Config>::SortedListProvider::count();
		// and confirm it is equal to the length of the `VoterList`.
		assert_eq!(pre_migrate_nominator_count, voter_list_len);
		assert_eq!(pre_migrate_nominator_count, voter_list_count);


		// go through every bag to track the total number of voters within bags
		// and get some info about how voters are distributed within the bags.
		let mut seen_in_bags = 0;
		for vote_weight_thresh in <Runtime as pallet_bags_list::Config>::BagThresholds::get() {
			// threshold in terms of UNITS (e.g. KSM, DOT etc)
			let vote_weight_thresh_as_unit = *vote_weight_thresh as f64 / currency_unit as f64;
			let pretty_thresh = format!("Threshold: {}.", vote_weight_thresh_as_unit);

			let bag = match pallet_bags_list::ListBags::<Runtime>::get(*vote_weight_thresh) {
				Some(bag) => bag,
				None => {
					log::info!(target: LOG_TARGET, "{} NO VOTERS.", pretty_thresh);
					continue;
				}
			};

			let voters_in_bag = bag.iter().count() as u32;

			// update our overall counter
			seen_in_bags += voters_in_bag;

			// percentage of all voters (nominators and voters)
			let percent_of_voters = percent(voters_in_bag, voter_list_count);


			log::info!(
				target: LOG_TARGET,
				"{} Nominators: {} [%{:.3}]",
				pretty_thresh,
				voters_in_bag,
				percent_of_voters,
			);
		}

		assert_eq!(seen_in_bags, voter_list_count);
	});
}

fn percent(portion: u32, total: u32) -> f64 {
	(portion as f64 / total as f64) * 100f64
}
