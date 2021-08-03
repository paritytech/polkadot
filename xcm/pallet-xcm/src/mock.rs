// Copyright 2020 Parity Technologies (UK) Ltd.
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

use frame_support::{
	construct_runtime, parameter_types,
	traits::{All, AllowAll},
	weights::Weight,
};
use polkadot_parachain::primitives::Id as ParaId;
use polkadot_runtime_parachains::origin;
use sp_core::H256;
use sp_runtime::{testing::Header, traits::IdentityLookup, AccountId32};
pub use sp_std::{cell::RefCell, fmt::Debug, marker::PhantomData};
use xcm::{
	opaque::v0::{Error as XcmError, MultiAsset, Result as XcmResult, SendXcm, Xcm},
	v0::{MultiLocation, NetworkId, Order},
};
use xcm_builder::{
	AccountId32Aliases, AllowTopLevelPaidExecutionFrom, ChildParachainAsNative,
	ChildParachainConvertsVia, ChildSystemParachainAsSuperuser,
	CurrencyAdapter as XcmCurrencyAdapter, FixedRateOfConcreteFungible, FixedWeightBounds,
	IsConcrete, LocationInverter, SignedAccountId32AsNative, SignedToAccountId32,
	SovereignSignedViaLocation, TakeWeightCredit,
};
use xcm_executor::XcmExecutor;

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
/// Sender that never returns error, always sends
pub struct TestSendXcm;
impl SendXcm for TestSendXcm {
	fn send_xcm(dest: MultiLocation, msg: Xcm) -> XcmResult {
		SENT_XCM.with(|q| q.borrow_mut().push((dest, msg)));
		Ok(())
	}
}
/// Sender that returns error if `X8` junction and stops routing
pub struct TestSendXcmErrX8;
impl SendXcm for TestSendXcmErrX8 {
	fn send_xcm(dest: MultiLocation, msg: Xcm) -> XcmResult {
		if let MultiLocation::X8(..) = dest {
			Err(XcmError::Undefined)
		} else {
			SENT_XCM.with(|q| q.borrow_mut().push((dest, msg)));
			Ok(())
		}
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
	pub const RelayLocation: MultiLocation = MultiLocation::Null;
	pub const AnyNetwork: NetworkId = NetworkId::Any;
	pub Ancestry: MultiLocation = MultiLocation::Null;
	pub UnitWeightCost: Weight = 1_000;
}

pub type SovereignAccountOf =
	(ChildParachainConvertsVia<ParaId, AccountId>, AccountId32Aliases<AnyNetwork, AccountId>);

pub type LocalAssetTransactor =
	XcmCurrencyAdapter<Balances, IsConcrete<RelayLocation>, SovereignAccountOf, AccountId, ()>;

type LocalOriginConverter = (
	SovereignSignedViaLocation<SovereignAccountOf, Origin>,
	ChildParachainAsNative<origin::Origin, Origin>,
	SignedAccountId32AsNative<AnyNetwork, Origin>,
	ChildSystemParachainAsSuperuser<ParaId, Origin>,
);

parameter_types! {
	pub const BaseXcmWeight: Weight = 1_000;
	pub CurrencyPerSecond: (MultiLocation, u128) = (RelayLocation::get(), 1);
}

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
	type Trader = FixedRateOfConcreteFungible<CurrencyPerSecond, ()>;
	type ResponseHandler = ();
}

pub type LocalOriginToLocation = SignedToAccountId32<Origin, AccountId, AnyNetwork>;

impl pallet_xcm::Config for Test {
	type Event = Event;
	type SendXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	type XcmRouter = (TestSendXcmErrX8, TestSendXcm);
	type ExecuteXcmOrigin = xcm_builder::EnsureXcmOrigin<Origin, LocalOriginToLocation>;
	type XcmExecuteFilter = All<(MultiLocation, xcm::v0::Xcm<Call>)>;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = All<(MultiLocation, Vec<MultiAsset>)>;
	type XcmReserveTransferFilter = All<(MultiLocation, Vec<MultiAsset>)>;
	type Weigher = FixedWeightBounds<BaseXcmWeight, Call>;
}

impl origin::Config for Test {}

pub(crate) fn last_event() -> Event {
	System::events().pop().expect("Event expected").event
}

pub(crate) fn buy_execution<C>(debt: Weight) -> Order<C> {
	use xcm::opaque::v0::prelude::*;
	Order::BuyExecution { fees: All, weight: 0, debt, halt_on_error: false, xcm: vec![] }
}

pub(crate) fn new_test_ext_with_balances(
	balances: Vec<(AccountId, Balance)>,
) -> sp_io::TestExternalities {
	let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();

	pallet_balances::GenesisConfig::<Test> { balances }
		.assimilate_storage(&mut t)
		.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}
