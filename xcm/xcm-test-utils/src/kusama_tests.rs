
use kusama::XcmConfig;
use xcm_executor::XcmExecutor;
use MultiLocation::*;
use xcm::v0::ExecuteXcm;
use xcm::opaque::v0::Outcome;
use xcm::opaque::v0::prelude::*;
use sp_std::prelude::*;
use polkadot_primitives::v1::AccountId;
use polkadot_parachain::primitives::Id as ParaId;
use sp_runtime::traits::AccountIdConversion;

use kusama_runtime as kusama;

pub const ALICE: AccountId = AccountId::new([0u8; 32]);
pub const PARA_ID: u32 = 2000;
pub const INITIAL_BALANCE: u128 = 100_000_000_000;

pub fn kusama_ext() -> sp_io::TestExternalities {
	use kusama::{Runtime, System};

	let mut t = frame_system::GenesisConfig::default()
		.build_storage::<Runtime>()
		.unwrap();

	let parachain_acc: AccountId = ParaId::from(PARA_ID).into_account();

	pallet_balances::GenesisConfig::<Runtime> {
		balances: vec![
			(ALICE, INITIAL_BALANCE),
			(parachain_acc, INITIAL_BALANCE)
		],
	}
	.assimilate_storage(&mut t)
	.unwrap();

	// use polkadot_primitives::v1::{MAX_CODE_SIZE, MAX_POV_SIZE};
	// // default parachains host configuration from polkadot's `chain_spec.rs`
	// kusama::ParachainsConfigurationConfig {
	// 	config: polkadot_runtime_parachains::configuration::HostConfiguration {
	// 		validation_upgrade_frequency: 1u32,
	// 		validation_upgrade_delay: 1,
	// 		code_retention_period: 1200,
	// 		max_code_size: MAX_CODE_SIZE,
	// 		max_pov_size: MAX_POV_SIZE,
	// 		max_head_data_size: 32 * 1024,
	// 		group_rotation_frequency: 20,
	// 		chain_availability_period: 4,
	// 		thread_availability_period: 4,
	// 		max_upward_queue_count: 8,
	// 		max_upward_queue_size: 1024 * 1024,
	// 		max_downward_message_size: 1024,
	// 		// this is approximatelly 4ms.
	// 		//
	// 		// Same as `4 * frame_support::weights::WEIGHT_PER_MILLIS`. We don't bother with
	// 		// an import since that's a made up number and should be replaced with a constant
	// 		// obtained by benchmarking anyway.
	// 		ump_service_total_weight: 4 * 1_000_000_000,
	// 		max_upward_message_size: 1024 * 1024,
	// 		max_upward_message_num_per_candidate: 5,
	// 		hrmp_open_request_ttl: 5,
	// 		hrmp_sender_deposit: 0,
	// 		hrmp_recipient_deposit: 0,
	// 		hrmp_channel_max_capacity: 8,
	// 		hrmp_channel_max_total_size: 8 * 1024,
	// 		hrmp_max_parachain_inbound_channels: 4,
	// 		hrmp_max_parathread_inbound_channels: 4,
	// 		hrmp_channel_max_message_size: 1024 * 1024,
	// 		hrmp_max_parachain_outbound_channels: 4,
	// 		hrmp_max_parathread_outbound_channels: 4,
	// 		hrmp_max_message_num_per_candidate: 5,
	// 		dispute_period: 6,
	// 		no_show_slots: 2,
	// 		n_delay_tranches: 25,
	// 		needed_approvals: 2,
	// 		relay_vrf_modulo_samples: 2,
	// 		zeroth_delay_tranche_width: 0,
	// 		..Default::default()
	// 	},
	// }.assimilate_storage(&mut t).unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}

#[test]
fn kusama_executor_works() {
	use xcm::v0::Xcm;

	kusama_ext().execute_with(|| {
		let r = XcmExecutor::<XcmConfig>::execute_xcm(Parachain(PARA_ID).into(), Xcm::WithdrawAsset {
			assets: vec![ConcreteFungible { id: Null, amount: 10_000_000_000 }],
			effects: vec![
				Order::BuyExecution { fees: All, weight: 0, debt: 2_000_000_000, halt_on_error: false, xcm: vec![] }
			],
		}, 2_000_000_000);
		assert_eq!(r, Outcome::Complete(2_000_000_000));
	})
}
