// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Tests for the integration between `PayOverXcm` and the salary pallet

use super::{super::*, *};

use frame_support::{assert_ok, macro_magic::use_attr};

#[use_attr]
use frame_support::derive_impl;

use frame_support::{
	construct_runtime, parameter_types,
	traits::{ConstU32, Everything},
};
use polkadot_test_runtime::xcm_config::{LocalOriginToLocation, XcmConfig};
use sp_runtime::AccountId32;
use xcm_executor::XcmExecutor;

pub type Address = sp_runtime::MultiAddress<AccountId, AccountIndex>;
pub type UncheckedExtrinsic =
	generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

type BlockNumber = u64;
type AccountId = u64;

construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = primitives::Block,
		UncheckedExtrinsic = UncheckedExtrinsic
	{
		System: frame_system,
		Balances: pallet_balances,
		Salary: pallet_salary,
		XcmPallet: pallet_xcm,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig as frame_system::DefaultConfig)]
impl frame_system::Config for Test {
	type BaseCallFilter = frame_support::traits::Everything;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	type RuntimeEvent = RuntimeEvent;
	type PalletInfo = PalletInfo;
	type OnSetCode = ();
}

pub type Balance = u128;

parameter_types! {
	pub const ExistentialDeposit: Balance = 1;
}

impl pallet_balances::Config for Test {
	type MaxLocks = ConstU32<0>;
	type Balance = Balance;
	type RuntimeEvent = RuntimeEvent;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	type RuntimeHoldReason = RuntimeHoldReason;
	type FreezeIdentifier = ();
	type MaxHolds = ConstU32<0>;
	type MaxFreezes = ConstU32<0>;
}

parameter_types! {
	pub const RelayLocation: MultiLocation = Here.into_location();
	pub const AnyNetwork: Option<NetworkId> = None;
	pub UniversalLocation: InteriorMultiLocation = Here;
	pub UnitWeightCost: u64 = 1_000;
	pub static AdvertisedXcmVersion: u32 = 3;
	pub const BaseXcmWeight: Weight = Weight::from_parts(1_000, 1_000);
	pub CurrencyPerSecondPerByte: (AssetId, u128, u128) = (Concrete(RelayLocation::get()), 1, 1);
	pub TrustedAssets: (MultiAssetFilter, MultiLocation) = (All.into(), Here.into());
	pub const MaxInstructions: u32 = 100;
	pub const MaxAssetsIntoHolding: u32 = 64;
}

impl pallet_xcm::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type SendXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmRouter = TestMessageSender;
	type ExecuteXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
	type XcmExecuteFilter = Everything;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything;
	type XcmReserveTransferFilter = Everything;
	type Weigher = FixedWeightBounds<BaseXcmWeight, RuntimeCall, MaxInstructions>;
	type UniversalLocation = UniversalLocation;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = AdvertisedXcmVersion;
	type TrustedLockers = ();
	type SovereignAccountOf = AccountId32Aliases<(), AccountId32>;
	type Currency = Balances;
	type CurrencyMatcher = IsConcrete<RelayLocation>;
	type MaxLockers = frame_support::traits::ConstU32<8>;
	type MaxRemoteLockConsumers = frame_support::traits::ConstU32<0>;
	type RemoteLockConsumerIdentifier = ();
	type WeightInfo = pallet_xcm::TestWeightInfo;
	#[cfg(feature = "runtime-benchmarks")]
	type ReachableDest = ReachableDest;
	type AdminOrigin = EnsureRoot<AccountId>;
}

parameter_types! {
	pub Interior: InteriorMultiLocation = Plurality { id: BodyId::Treasury, part: BodyPart::Voice }.into();
	pub Timeout: BlockNumber = 5;
}

type SalaryPayOverXcm = PayOverXcm<
	Interior,
	TestMessageSender,
	TestQueryHandler<TestConfig, BlockNumber>,
	Timeout,
	AccountId,
	AssetKind,
	LocatableAssetKindConverter,
	AliasesIntoAccountIndex64,
>;

type Rank = u64;

thread_local! {
	pub static CLUB: RefCell<BTreeMap<AccountId, Rank>> = RefCell::new(BTreeMap::new());
}

pub struct TestClub;
impl RankedMembers for TestClub {
	type AccountId = AccountId;
	type Rank = Rank;

	fn min_rank() -> Self::Rank {
		0
	}
	fn rank_of(who: &Self::AccountId) -> Option<Self::Rank> {
		CLUB.with(|club| club.borrow().get(who).cloned())
	}
	fn induct(who: &Self::AccountId) -> DispatchResult {
		CLUB.with(|club| club.borrow_mut().insert(*who, 0));
		Ok(())
	}
	fn promote(who: &Self::AccountId) -> DispatchResult {
		CLUB.with(|club| club.borrow_mut().entry(*who).and_modify(|rank| *rank += 1));
		Ok(())
	}
	fn demote(who: &Self::AccountId) -> DispatchResult {
		CLUB.with(|club| match club.borrow().get(who) {
			None => Err(sp_runtime::DispatchError::Unavailable),
			Some(&0) => {
				club.borrow_mut().remove(&who);
				Ok(())
			},
			Some(_) => {
				club.borrow_mut().entry(*who).and_modify(|rank| *rank += 1);
				Ok(())
			},
		})
	}
}

parameter_types! {
	pub const RegistrationPeriod: u64 = 2;
	pub const PayoutPeriod: u64 = 2;
	pub static Budget: u64 = 10;
}

impl pallet_salary::Config for Test {
	type WeightInfo = ();
	type RuntimeEvent = RuntimeEvent;
	type Paymaster = SalaryPayOverXcm;
	type Members = TestClub;
	type Salary = ConvertRank<Identity>;
	type RegistrationPeriod = RegistrationPeriod;
	type PayoutPeriod = PayoutPeriod;
	type Budget = Budget;
}

fn new_test_ext() -> sp_io::TestExternalities {
	let t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}

fn next_block() {
	System::set_block_number(System::block_number() + 1);
}

fn run_to(block_number: u64) {
	while System::block_number() < block_number {
		next_block();
	}
}

#[test]
fn salary_pay_over_xcm_works() {
	new_test_ext().execute_with(|| {
		assert_ok!(Salary::init(RuntimeOrigin::signed(1)));
		assert_ok!(Salary::induct(RuntimeOrigin::signed(1)));
		assert_ok!(Salary::register(RuntimeOrigin::signed(1)));
		run_to(3);
		assert_ok!(Salary::payout(RuntimeOrigin::signed(1)));
	});

	dbg!(&sent_xcm());
}
