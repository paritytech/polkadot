use remote_externalities::{Builder, Mode, OnlineConfig};
use sp_storage::well_known_keys;
use pallet_staking::{
	Nominators, Validators, CounterForValidators, CounterForNominators,
	voter_bags::{Bag, VoterList},
};
use frame_support::{assert_ok, traits::Get};
use sp_runtime::traits::Block as BlockT;

pub (crate) async fn test_voter_bags_migration<
		Runtime: pallet_staking::Config,
		Block: BlockT
	>() {
	use std::env;

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
		let pre_migrate_nominator_count = <Nominators<Runtime>>::iter().collect::<Vec<_>>().len() as u32;
		let pre_migrate_validator_count = <Validators<Runtime>>::iter().collect::<Vec<_>>().len() as u32;
		println!("Nominator count: {}", pre_migrate_nominator_count);
		println!("Validator count: {}", pre_migrate_validator_count);

		assert_ok!(pallet_staking::migrations::v8::pre_migrate::<Runtime>());

		let migration_weight = pallet_staking::migrations::v8::migrate::<Runtime>();
		println!("Migration weight: {}", migration_weight);

		assert_eq!(CounterForNominators::<Runtime>::get(), pre_migrate_nominator_count);
		assert_eq!(CounterForValidators::<Runtime>::get(), pre_migrate_validator_count);

		let post_migrate_nominator_count = <Nominators<Runtime>>::iter().collect::<Vec<_>>().len() as u32;
		let post_migrate_validator_count = <Validators<Runtime>>::iter().collect::<Vec<_>>().len() as u32;
		assert_eq!(post_migrate_nominator_count, pre_migrate_nominator_count);
		assert_eq!(post_migrate_validator_count, pre_migrate_validator_count);

		// We can't access VoterCount from here, so we create it.
		let voter_count = post_migrate_nominator_count + post_migrate_validator_count;
		let voter_list_len = VoterList::<Runtime>::iter().collect::<Vec<_>>().len();

		let mut seen_in_bags = 0;
		for vote_weight_thresh in <Runtime as pallet_staking::Config>::VoterBagThresholds::get() {
			let bag = match Bag::<Runtime>::get(*vote_weight_thresh) {
				Some(bag) => bag,
				None => {
					println!("Threshold: {}. NO VOTERS.", vote_weight_thresh);
					continue;
				},
			};

			let voters_in_bag_count = bag.iter().collect::<Vec<_>>().len();
			seen_in_bags += voters_in_bag_count;
			let percentage_of_voters =
				(voter_count as f64 / voters_in_bag_count as f64) * 100f64;
			println!(
				"Threshold: {}. Voters: {} (%{:.3} voters)",
				vote_weight_thresh, voter_count, percentage_of_voters
			);
		}

		assert_eq!(seen_in_bags, voter_list_len);
	});
}
