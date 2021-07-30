// This file is part of Substrate.

// Copyright (C) 2019-2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! A mock runtime for xcm benchmarking.
#![allow(dead_code, unused_imports)] // TODO: remove this later.

use crate as pallet_xcm_benchmarks;
use crate::*;
use frame_support::parameter_types;
use sp_core::H256;
use sp_runtime::{
	testing::Header,
	traits::{BlakeTwo256, IdentityLookup},
	BuildStorage,
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
		XcmPalletBenchmarks: pallet_xcm_benchmarks::{Pallet},
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
	pub const ExistentialDeposit: u64 = 1;
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

use frame_support::weights::Weight;
use xcm::opaque::v0::{prelude::XcmResult, Junction, MultiAsset, MultiLocation, Response, Xcm};

// An xcm sender/receiver akin to > /dev/null
pub struct DevNull;
impl xcm::opaque::v0::SendXcm for DevNull {
	fn send_xcm(_: MultiLocation, _: Xcm) -> XcmResult {
		Ok(())
	}
}

impl xcm_executor::traits::OnResponse for DevNull {
	fn expecting_response(_: &MultiLocation, _: u64) -> bool {
		false
	}
	fn on_response(_: MultiLocation, _: u64, _: Response) -> Weight {
		0
	}
}

parameter_types! {
	pub const CheckedAccount: Option<u64> = Some(100);
	pub Ancestry: MultiLocation = MultiLocation::X1(Junction::Parachain(101));
	pub UnitWeightCost: Weight = 10;
	pub WeightPrice: (MultiLocation, u128) = (MultiLocation::Null, 1_000_000_000_000);
}

pub struct TestMatcher;
impl xcm_executor::traits::MatchesFungible<u64> for TestMatcher {
	fn matches_fungible(_: &MultiAsset) -> Option<u64> {
		// TODO
		None
	}
}

pub struct AccountIdConverter;
impl xcm_executor::traits::Convert<MultiLocation, u64> for AccountIdConverter {
	fn convert(ml: MultiLocation) -> Result<u64, MultiLocation> {
		Err(ml)
	}

	fn reverse(acc: u64) -> Result<MultiLocation, u64> {
		Err(acc)
	}
}

// Use balances as the asset transactor.
pub type AssetTransactor =
	xcm_builder::CurrencyAdapter<Balances, TestMatcher, AccountIdConverter, u64, CheckedAccount>;

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type Call = Call;
	type XcmSender = DevNull;
	type AssetTransactor = AssetTransactor;
	type OriginConverter = (); // TODO:
	type IsReserve = (); // TODO:
	type IsTeleporter = (); // no one can teleport.
	type LocationInverter = xcm_builder::LocationInverter<Ancestry>;
	type Barrier = (); // No barriers -- everything is allowed.
	type Weigher = xcm_builder::FixedWeightBounds<UnitWeightCost, Call>;
	type Trader = xcm_builder::FixedRateOfConcreteFungible<WeightPrice, ()>;
	type ResponseHandler = DevNull;
}

impl pallet_xcm_benchmarks::Config for Test {
	type XcmConfig = XcmConfig;
}

// This function basically just builds a genesis storage key/value store according to
// our desired mockup.
pub fn new_test_ext() -> sp_io::TestExternalities {
	let t = GenesisConfig { ..Default::default() }.build_storage().unwrap();
	t.into()
}
