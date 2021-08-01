use frame_support::{construct_runtime, parameter_types, traits::{All, AllowAll}, weights::Weight};
use sp_core::H256;
use sp_runtime::{testing::Header, traits::IdentityLookup, AccountId32};
pub use sp_std::{fmt::Debug, marker::PhantomData, cell::RefCell};
use polkadot_parachain::primitives::{Id as ParaId, AccountIdConversion};
use polkadot_runtime_parachains::origin;
use xcm::v0::{MultiLocation, NetworkId};
use xcm::opaque::v0::MultiAsset;
use xcm_builder::{
	AccountId32Aliases, AllowTopLevelPaidExecutionFrom, ChildParachainAsNative, ChildParachainConvertsVia,
	ChildSystemParachainAsSuperuser, CurrencyAdapter as XcmCurrencyAdapter, FixedRateOfConcreteFungible,
	FixedWeightBounds, IsConcrete, LocationInverter, SignedAccountId32AsNative, SignedToAccountId32,
	SovereignSignedViaLocation, TakeWeightCredit,
};
use xcm_executor::XcmExecutor;
use xcm::opaque::v0::{Xcm, SendXcm, Result as XcmResult};

use crate as pallet_xcm;

pub type AccountId = AccountId32;
pub type Balance = u128;
type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Storage, Config, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		ParasOrigin: origin::{Pallet, Origin},
		XcmPallet: pallet_xcm::{Pallet, Call, Storage, Event<T>},
	}
);

thread_local! {
	pub static SENT_XCM: RefCell<Vec<(MultiLocation, Xcm)>> = RefCell::new(Vec::new());
}
pub fn sent_xcm() -> Vec<(MultiLocation, Xcm)> {
	SENT_XCM.with(|q| (*q.borrow()).clone())
}
pub struct TestSendXcm;
impl SendXcm for TestSendXcm {
	fn send_xcm(dest: MultiLocation, msg: Xcm) -> XcmResult {
		SENT_XCM.with(|q| q.borrow_mut().push((dest, msg)));
		Ok(())
	}
}

parameter_types! {
	pub const BlockHashCount: u64 = 250;
}

impl frame_system::Config for Test {
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

impl pallet_balances::Config for Test {
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

parameter_types! {
	pub const KsmLocation: MultiLocation = MultiLocation::Null;
	pub const KusamaNetwork: NetworkId = NetworkId::Kusama;
	pub const AnyNetwork: NetworkId = NetworkId::Any;
	pub Ancestry: MultiLocation = MultiLocation::Null;
	pub UnitWeightCost: Weight = 1_000;
}

pub type SovereignAccountOf = (
	ChildParachainConvertsVia<ParaId, AccountId>,
	AccountId32Aliases<KusamaNetwork, AccountId>,
);

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

// TODO: stricter than All for inner AllowTopLevelPaidExecutionFrom
pub type Barrier = (TakeWeightCredit, AllowTopLevelPaidExecutionFrom<All<MultiLocation>>);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type Call = Call;
	type XcmSender = TestSendXcm;
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

impl pallet_xcm::Config for Test {
	type Event = Event;
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	type XcmRouter = TestSendXcm;
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	type XcmExecuteFilter = All<(MultiLocation, xcm::v0::Xcm<Call>)>;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = All<(MultiLocation, Vec<MultiAsset>)>;
	type XcmReserveTransferFilter = All<(MultiLocation, Vec<MultiAsset>)>;
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call>;
}

impl origin::Config for Test {}

pub const ALICE: AccountId = AccountId::new([0u8; 32]);
pub const PARA_ID: u32 = 2000;
pub const INITIAL_BALANCE: u128 = 100_000_000_000;

pub(crate) fn last_event() -> Event {
	System::events().pop().expect("Event expected").event
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let mut t = frame_system::GenesisConfig::default()
		.build_storage::<Test>()
		.unwrap();

	let parachain_acc: AccountId = ParaId::from(PARA_ID).into_account();

	pallet_balances::GenesisConfig::<Test> {
		balances: vec![
			(ALICE, INITIAL_BALANCE),
			(parachain_acc, INITIAL_BALANCE)
		],
	}
	.assimilate_storage(&mut t)
	.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}