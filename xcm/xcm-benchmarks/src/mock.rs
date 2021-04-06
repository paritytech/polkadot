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

//! A mock environment for xcm-benchmarks.

use crate as xcm_benchmarks;
use sp_core::H256;
use frame_support::parameter_types;
use sp_runtime::{
	AccountId32,
	traits::{BlakeTwo256, IdentityLookup}, testing::Header,
};
use frame_system as system;

// Parachains Stuff
use polkadot_parachain::primitives::Id as ParaId;
use xcm::v0::{MultiLocation, NetworkId};
use xcm_executor::traits::IsConcrete;
use xcm_builder::{
	AccountId32Aliases, ChildParachainConvertsVia, SovereignSignedViaLocation,
	CurrencyAdapter as XcmCurrencyAdapter,
	SignedAccountId32AsNative, ChildSystemParachainAsSuperuser, LocationInverter,
};

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

// Configure a mock runtime to test the pallet.
frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Event<T>},
		XcmBenchmarks: xcm_benchmarks::{Pallet},
	}
);

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub const SS58Prefix: u8 = 42;
}

type AccountId = AccountId32;

impl system::Config for Test {
	type BaseCallFilter = ();
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type Origin = Origin;
	type Call = Call;
	type Index = u64;
	type BlockNumber = u64;
	type Hash = H256;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId;
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
	type SS58Prefix = SS58Prefix;
	type OnSetCode = ();
}

parameter_types! {
	pub const ExistentialDeposit: u64 = 1;
}

impl pallet_balances::Config for Test {
	type Balance = u64;
	type Event = Event;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type MaxLocks = ();
	type WeightInfo = ();
}

parameter_types! {
	pub const RocLocation: MultiLocation = MultiLocation::Null;
	pub const RococoNetwork: NetworkId = NetworkId::Polkadot;
	pub const Ancestry: MultiLocation = MultiLocation::Null;
}

pub type LocationConverter = (
	ChildParachainConvertsVia<ParaId, AccountId>,
	AccountId32Aliases<RococoNetwork, AccountId>,
);

pub type LocalAssetTransactor =
	XcmCurrencyAdapter<
		// Use this currency:
		Balances,
		// Use this currency when it is a fungible asset matching the given location or name:
		IsConcrete<RocLocation>,
		// We can convert the MultiLocations with our converter above:
		LocationConverter,
		// Our chain's account ID type (we can't get away without mentioning it explicitly):
		AccountId,
	>;

type LocalOriginConverter = (
	SovereignSignedViaLocation<LocationConverter, Origin>,
	SignedAccountId32AsNative<RococoNetwork, Origin>,
	ChildSystemParachainAsSuperuser<ParaId, Origin>,
);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type Call = Call;
	type XcmSender = ();// TODO
	type AssetTransactor = LocalAssetTransactor;
	type OriginConverter = LocalOriginConverter;
	type IsReserve = ();
	type IsTeleporter = ();
	type LocationInverter = LocationInverter<Ancestry>;
}

impl xcm_benchmarks::Config for Test {
	type XcmConfig = XcmConfig;
	type Balances = Balances;
}

// Build genesis storage according to the mock runtime.
#[allow(unused)]
pub fn new_test_ext() -> sp_io::TestExternalities {
	system::GenesisConfig::default().build_storage::<Test>().unwrap().into()
}
