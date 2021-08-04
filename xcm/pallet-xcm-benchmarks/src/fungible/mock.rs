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

//! A mock runtime for xcm benchmarking.

use crate::{fungible as xcm_balances_benchmark, mock::*};
use frame_support::{parameter_types, traits::All};
use polkadot_primitives::v1::Id as ParaId;
use sp_core::H256;
use sp_runtime::{
	testing::Header,
	traits::{BlakeTwo256, IdentityLookup},
	BuildStorage,
};
use xcm::v0::{Junction, MultiAsset, MultiLocation, NetworkId};
use xcm_builder::{
	AllowBenchmarks, AllowTopLevelPaidExecutionFrom, AllowUnpaidExecutionFrom,
	IsChildSystemParachain, TakeWeightCredit,
};

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

// For testing the pallet, we construct a mock runtime.
frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		XcmBalancesBenchmark: xcm_balances_benchmark::{Pallet},
	}
);

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub BlockWeights: frame_system::limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(1024);
}
impl frame_system::Config for Test {
	type BaseCallFilter = frame_support::traits::AllowAll;
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type Origin = Origin;
	type Index = u64;
	type BlockNumber = u64;
	type Hash = H256;
	type Call = Call;
	type Hashing = BlakeTwo256;
	type AccountId = u64;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<u64>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
}

parameter_types! {
	pub const ExistentialDeposit: u64 = 7;
}

impl pallet_balances::Config for Test {
	type MaxLocks = ();
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	type Balance = u64;
	type DustRemoval = ();
	type Event = Event;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
}

parameter_types! {
	pub const AssetDeposit: u64 = 100 * ExistentialDeposit::get();
	pub const ApprovalDeposit: u64 = 1 * ExistentialDeposit::get();
	pub const StringLimit: u32 = 50;
	pub const MetadataDepositBase: u64 = 10 * ExistentialDeposit::get();
	pub const MetadataDepositPerByte: u64 = 1 * ExistentialDeposit::get();
}

pub struct MatchAnyFungible;
impl xcm_executor::traits::MatchesFungible<u64> for MatchAnyFungible {
	fn matches_fungible(m: &MultiAsset) -> Option<u64> {
		use sp_runtime::traits::SaturatedConversion;
		match m {
			MultiAsset::ConcreteFungible { amount, .. } => Some((*amount).saturated_into::<u64>()),
			_ => None,
		}
	}
}

parameter_types! {
	pub const CheckedAccount: Option<u64> = Some(100);
	pub const ValidDestination: MultiLocation = MultiLocation::X1(Junction::AccountId32 {
		network: NetworkId::Any,
		id: [0u8; 32],
	});
}

// Use balances as the asset transactor.
pub type AssetTransactor = xcm_builder::CurrencyAdapter<
	Balances,
	MatchAnyFungible,
	AccountIdConverter,
	u64,
	CheckedAccount,
>;

/// The barriers one of which must be passed for an XCM message to be executed.
pub type Barrier = (
	// Weight that is paid for may be consumed.
	TakeWeightCredit,
	// If the message is one that immediately attemps to pay for execution, then allow it.
	AllowTopLevelPaidExecutionFrom<All<MultiLocation>>,
	// Messages coming from system parachains need not pay for execution.
	AllowUnpaidExecutionFrom<IsChildSystemParachain<ParaId>>,
	AllowBenchmarks,
);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type Call = Call;
	type XcmSender = DevNull;
	type AssetTransactor = AssetTransactor;
	type OriginConverter = ();
	type IsReserve = ();
	type IsTeleporter = ();
	type LocationInverter = xcm_builder::LocationInverter<Ancestry>;
	type Barrier = Barrier;
	type Weigher = xcm_builder::FixedWeightBounds<UnitWeightCost, Call>;
	type Trader = xcm_builder::FixedRateOfConcreteFungible<WeightPrice, ()>;
	type ResponseHandler = DevNull;
}

impl crate::Config for Test {
	type XcmConfig = XcmConfig;
	type AccountIdConverter = AccountIdConverter;
}

impl xcm_balances_benchmark::Config for Test {
	type TransactAsset = Balances;
	type CheckedAccount = CheckedAccount;
	type ValidDestination = ValidDestination;
	fn get_multi_asset() -> MultiAsset {
		let amount =
			<Balances as frame_support::traits::fungible::Inspect<u64>>::minimum_balance() as u128;
		MultiAsset::ConcreteFungible { id: MultiLocation::Null, amount }
	}
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let t = GenesisConfig { ..Default::default() }.build_storage().unwrap();
	sp_tracing::try_init_simple();
	t.into()
}
