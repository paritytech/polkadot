use frame_support::{
	assert_ok, construct_runtime, parameter_types,
	traits::{All, AllowAll},
	weights::Weight,
};
use sp_core::H256;
use sp_runtime::traits::AccountIdConversion;
use sp_runtime::{testing::Header, traits::IdentityLookup, AccountId32};

use crate::{
	AccountId32Aliases, AllowUnpaidExecutionFrom, ChildParachainAsNative,
	ChildParachainConvertsVia, ChildSystemParachainAsSuperuser,
	CurrencyAdapter as XcmCurrencyAdapter, FixedRateOfConcreteFungible, FixedWeightBounds,
	IsConcrete, LocationInverter, SignedAccountId32AsNative, SignedToAccountId32,
	SovereignSignedViaLocation,
};
use polkadot_parachain::primitives::Id as ParaId;
use polkadot_runtime_parachains::{configuration, origin, shared, ump};
use xcm::opaque::v0::MultiAsset;
use xcm::v0::{MultiLocation, NetworkId, Order};
use xcm_executor::XcmExecutor;

use crate as xcm_builder;

pub type AccountId = AccountId32;
pub type Balance = u128;

parameter_types! {
	pub const BlockHashCount: u64 = 250;
}

impl frame_system::Config for Runtime {
	type Origin = Origin;
	type Call = Call;
	type Index = u64;
	type BlockNumber = u64;
	type Hash = H256;
	type Hashing = ::sp_runtime::traits::BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type BlockWeights = ();
	type BlockLength = ();
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<Balance>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type DbWeight = ();
	type BaseCallFilter = AllowAll;
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
}

parameter_types! {
	pub ExistentialDeposit: Balance = 1;
	pub const MaxLocks: u32 = 50;
	pub const MaxReserves: u32 = 50;
}

impl pallet_balances::Config for Runtime {
	type MaxLocks = MaxLocks;
	type Balance = Balance;
	type Event = Event;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
}

impl shared::Config for Runtime {}

impl configuration::Config for Runtime {}

parameter_types! {
	pub const KsmLocation: MultiLocation = MultiLocation::Null;
	pub const KusamaNetwork: NetworkId = NetworkId::Kusama;
	pub const AnyNetwork: NetworkId = NetworkId::Any;
	pub Ancestry: MultiLocation = MultiLocation::Null;
	pub UnitWeightCost: Weight = 1_000;
}

pub type SovereignAccountOf =
	(ChildParachainConvertsVia<ParaId, AccountId>, AccountId32Aliases<KusamaNetwork, AccountId>);

pub type LocalAssetTransactor =
	XcmCurrencyAdapter<Balances, IsConcrete<KsmLocation>, SovereignAccountOf, AccountId, ()>;

type LocalOriginConverter = (
	SovereignSignedViaLocation<SovereignAccountOf, Origin>,
	ChildParachainAsNative<origin::Origin, Origin>,
	SignedAccountId32AsNative<KusamaNetwork, Origin>,
	ChildSystemParachainAsSuperuser<ParaId, Origin>,
);

parameter_types! {
	pub const BaseXcmWeight: Weight = 1_000;
	pub KsmPerSecond: (MultiLocation, u128) = (KsmLocation::get(), 1);
}

pub type Barrier = AllowUnpaidExecutionFrom<All<MultiLocation>>;

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type Call = Call;
	type XcmSender = crate::mock::TestSendXcm;
	type AssetTransactor = LocalAssetTransactor;
	type OriginConverter = LocalOriginConverter;
	type IsReserve = ();
	type IsTeleporter = ();
	type LocationInverter = LocationInverter<Ancestry>;
	type Barrier = Barrier;
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call>;
	type Trader = FixedRateOfConcreteFungible<KsmPerSecond, ()>;
	type ResponseHandler = ();
}

pub type LocalOriginToLocation = SignedToAccountId32<Origin, AccountId, KusamaNetwork>;

impl pallet_xcm::Config for Runtime {
	type Event = Event;
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	type XcmRouter = crate::mock::TestSendXcm;
	// Anyone can execute XCM messages locally...
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	type XcmExecuteFilter = ();
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = All<(MultiLocation, Vec<MultiAsset>)>;
	type XcmReserveTransferFilter = All<(MultiLocation, Vec<MultiAsset>)>;
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call>;
}

parameter_types! {
	pub const FirstMessageFactorPercent: u64 = 100;
}

impl ump::Config for Runtime {
	type Event = Event;
	type UmpSink = ump::XcmSink<XcmExecutor<XcmConfig>, Runtime>;
	type FirstMessageFactorPercent = FirstMessageFactorPercent;
}

impl origin::Config for Runtime {}

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Runtime>;
type Block = frame_system::mocking::MockBlock<Runtime>;

construct_runtime!(
	pub enum Runtime where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Storage, Config, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		ParasOrigin: origin::{Pallet, Origin},
		ParasUmp: ump::{Pallet, Call, Storage, Event},
		XcmPallet: pallet_xcm::{Pallet, Call, Storage, Event<T>},
	}
);

pub const ALICE: AccountId = AccountId::new([0u8; 32]);
pub const PARA_ID: u32 = 2000;
pub const INITIAL_BALANCE: u128 = 100_000_000_000;

pub fn kusama_like_ext() -> sp_io::TestExternalities {
	let mut t = frame_system::GenesisConfig::default().build_storage::<Runtime>().unwrap();

	let parachain_acc: AccountId = ParaId::from(PARA_ID).into_account();

	pallet_balances::GenesisConfig::<Runtime> {
		balances: vec![(ALICE, INITIAL_BALANCE), (parachain_acc, INITIAL_BALANCE)],
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

fn buy_execution<C>(debt: Weight) -> Order<C> {
	use xcm::opaque::v0::prelude::*;
	Order::BuyExecution {
		fees: All,
		weight: 0,
		debt,
		halt_on_error: false,
		xcm: vec![],
	}
}

#[test]
fn withdraw_and_deposit_works() {
	use xcm::opaque::v0::prelude::*;
	use xcm::v0::Xcm;
	use MultiLocation::*;

	kusama_like_ext().execute_with(|| {
		let other_para_id = 3000;
		let amount = 5_000_000;
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(0),
					Order::DepositAsset {
						assets: vec![All],
						dest: Parachain(other_para_id).into(),
					},
				],
			},
			2_000_000,
		);
		assert_eq!(r, Outcome::Complete(3_000));
		let other_para_acc: AccountId = ParaId::from(other_para_id).into_account();
		assert_eq!(Balances::free_balance(other_para_acc), amount);
	});
}

#[test]
fn reserve_transfer_assets_works() {
	use xcm::opaque::v0::prelude::*;
	use xcm::v0::{Junction, Xcm};
	use MultiLocation::*;

	kusama_like_ext().execute_with(|| {
		let amount = 5_000_000;
		let dest_weight = 2_000_000;
		assert_ok!(XcmPallet::reserve_transfer_assets(
			Origin::signed(ALICE),
			Parachain(PARA_ID).into(),
			Junction::AccountId32 { network: NetworkId::Kusama, id: ALICE.into() }.into(),
			vec![ConcreteFungible { id: Null, amount }],
			dest_weight
		));

		assert_eq!(Balances::free_balance(ALICE), INITIAL_BALANCE - amount);
		let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE + amount);
		assert_eq!(
			crate::mock::sent_xcm(),
			vec![(
				Parachain(PARA_ID).into(),
				Xcm::ReserveAssetDeposit {
					assets: vec![ConcreteFungible { id: Parent.into(), amount }],
					effects: vec![
						buy_execution(dest_weight),
						DepositAsset {
							assets: vec![All],
							dest: Junction::AccountId32 {
								network: NetworkId::Kusama,
								id: ALICE.into()
							}
							.into()
						},
					]
				}
			)]
		);
	});
}

#[test]
fn query_holding_works() {
	use xcm::opaque::v0::prelude::*;
	use xcm::opaque::v0::Response;
	use xcm::v0::Xcm;
	use MultiLocation::*;

	kusama_like_ext().execute_with(|| {
		let other_para_id = 3000;
		let amount = 5_000_000;
		let query_id = 1234;
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(0),
					Order::QueryHolding {
						query_id,
						dest: Parachain(PARA_ID).into(),
						assets: vec![All],
					},
					Order::DepositAsset {
						assets: vec![All],
						dest: Parachain(other_para_id).into(),
					},
				],
			},
			2_000_000,
		);
		assert_eq!(r, Outcome::Complete(4_000));
		let other_para_acc: AccountId = ParaId::from(other_para_id).into_account();
		assert_eq!(Balances::free_balance(other_para_acc), amount);
		let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - amount);
		assert_eq!(
			crate::mock::sent_xcm(),
			vec![(
				Parachain(PARA_ID).into(),
				Xcm::QueryResponse {
					query_id,
					response: Response::Assets(vec![ConcreteFungible {
						id: Parent.into(),
						amount
					}])
				}
			)]
		);
	});
}

#[test]
fn teleport_to_statemine_works() {
	use xcm::opaque::v0::prelude::*;
	use xcm::v0::Xcm;
	use MultiLocation::*;

	kusama_like_ext().execute_with(|| {
		let statemine_id = 1000;
		let amount = 5_000_000;
		let teleport_effects = vec![
			buy_execution(0),
			Order::DepositAsset { assets: vec![All], dest: Parachain(PARA_ID).into() },
		];
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(0),
					Order::InitiateTeleport {
						assets: vec![All],
						dest: Parachain(statemine_id).into(),
						effects: teleport_effects.clone(),
					},
				],
			},
			2_000_000,
		);
		assert_eq!(r, Outcome::Complete(3_000));
		let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - amount);
		assert_eq!(
			crate::mock::sent_xcm(),
			vec![(
				Parachain(statemine_id).into(),
				Xcm::TeleportAsset {
					assets: vec![ConcreteFungible { id: Parent.into(), amount }],
					effects: teleport_effects,
				}
			)]
		);
	});
}
