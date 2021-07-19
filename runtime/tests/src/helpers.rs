use remote_externalities::{Builder, Mode, OnlineConfig};
use sp_storage::well_known_keys;
use pallet_staking::{
	Nominators, Validators, CounterForValidators, CounterForNominators,
	voter_bags::{Bag, VoterList},
};
use frame_support::{assert_ok, traits::Get};
use sp_runtime::traits::Block as BlockT;

fn init_logger() {
	sp_tracing::try_init_simple();
}

const LOG_TARGET: &'static str = "runtime::test";

/// Test voter bags migration. `currency_unit` is the number of planks per the
/// the runtimes `UNITS` (i.e. number of decimal places per DOT, KSM etc)
pub (crate) async fn test_voter_bags_migration<
		Runtime: pallet_staking::Config,
		Block: BlockT
	>(currency_unit: u64) {
	use std::env;

	init_logger();

	let ws_url = match env::var("WS_RPC") {
		Ok(ws_url) => ws_url,
		Err(_) => panic!("Must set env var `WS_RPC=<ws-url>`"),
	};

	let mut ext = Builder::<Block>::new()
		.mode(Mode::Online(OnlineConfig {
			transport: ws_url.to_string().into(),
			modules: vec!["Staking".to_string()],
			at: None,
			state_snapshot: None,
		}))
		.inject_hashed_key(well_known_keys::CODE)
		.build()
		.await
		.unwrap();

	ext.execute_with(|| {
		// Get the nominator & validator count prior to migrating; these should be invariant.
		let pre_migrate_nominator_count = <Nominators<Runtime>>::iter().collect::<Vec<_>>().len();
		let pre_migrate_validator_count = <Validators<Runtime>>::iter().collect::<Vec<_>>().len();
		log::info!(target: LOG_TARGET, "Nominator count: {}", pre_migrate_nominator_count);
		log::info!(target: LOG_TARGET, "Validator count: {}", pre_migrate_validator_count);

		assert_ok!(pallet_staking::migrations::v8::pre_migrate::<Runtime>());

		// Run the actual migration.
		let migration_weight = pallet_staking::migrations::v8::migrate::<Runtime>();
		log::info!(target: LOG_TARGET, "Migration weight: {}", migration_weight);

		// `CountFor*` storage items are created during the migration, check them.
		assert_eq!(CounterForNominators::<Runtime>::get(), pre_migrate_nominator_count as u32);
		assert_eq!(CounterForValidators::<Runtime>::get(), pre_migrate_validator_count as u32);

		// Check that the count of validators and nominators did not change during the migration.
		let post_migrate_nominator_count = <Nominators<Runtime>>::iter().collect::<Vec<_>>().len();
		let post_migrate_validator_count = <Validators<Runtime>>::iter().collect::<Vec<_>>().len();
		assert_eq!(post_migrate_nominator_count, pre_migrate_nominator_count);
		assert_eq!(post_migrate_validator_count, pre_migrate_validator_count);

		// We can't access VoterCount from here, so we create it,
		let voter_count = post_migrate_nominator_count + post_migrate_validator_count;
		let voter_list_len = VoterList::<Runtime>::iter().collect::<Vec<_>>().len();
		// and confirm it is equal to the length of the `VoterList`.
		assert_eq!(voter_count, voter_list_len);

		// Go through every bag to track the total number of voters within bags
		// and get some info about how voters are distributed within the bags.
		let mut seen_in_bags = 0;
		for vote_weight_thresh in <Runtime as pallet_staking::Config>::VoterBagThresholds::get() {
			// Threshold in terms of UNITS (e.g. KSM, DOT etc)
			let vote_weight_thresh_as_unit = vote_weight_thresh / currency_unit;

			let bag = match Bag::<Runtime>::get(*vote_weight_thresh) {
				Some(bag) => bag,
				None => {
					log::info!(target: LOG_TARGET, "Threshold: {} UNITS. NO VOTERS.", vote_weight_thresh_as_unit);
					continue;
				},
			};

			let voters_in_bag_count = bag.iter().collect::<Vec<_>>().len();
			seen_in_bags += voters_in_bag_count;
			let percentage_of_voters =
				(voters_in_bag_count as f64 / voter_count as f64) * 100f64;

			log::info!(
				target: LOG_TARGET,
				"Threshold: {} UNITS. Voters: {} [%{:.3}]",
				vote_weight_thresh_as_unit, voters_in_bag_count, percentage_of_voters
			);
		}

		assert_eq!(seen_in_bags, voter_list_len);
	});
}
