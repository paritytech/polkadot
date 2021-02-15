// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

//! Mocking utilities for testing with real pallets.

use sp_io::TestExternalities;
use sp_core::H256;
use sp_runtime::{
	ModuleId,
	traits::{
		BlakeTwo256, IdentityLookup,
	},
};
use primitives::v1::{Balance, BlockNumber, Header, Id as ParaId};
use frame_support::{
	parameter_types, assert_ok, assert_noop,
	traits::{
		TestRandomness, Currency, OnInitialize, OnFinalize,
	}
};
use frame_system::EnsureRoot;
use runtime_parachains::{paras, configuration};
use crate::{
	auctions, crowdloan, slots, paras_registrar,
	traits::{
		Registrar as RegistrarT
	},
};

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

type AccountId = u64;

frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Module, Call, Config, Storage, Event<T>},
		Balances: pallet_balances::{Module, Call, Storage, Config<T>, Event<T>},
		Paras: paras::{Module, Origin, Call, Storage, Config<T>},
		Configuration: configuration::{Module, Call, Storage, Config<T>},
		Registrar: paras_registrar::{Module, Call, Storage, Event<T>},
		Auctions: auctions::{Module, Call, Storage, Event<T>},
		Crowdloan: crowdloan::{Module, Call, Storage, Event<T>},
		Slots: slots::{Module, Call, Storage, Event<T>},
	}
);

parameter_types! {
	pub const BlockHashCount: u32 = 250;
	pub BlockWeights: frame_system::limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(4 * 1024 * 1024);
}

impl frame_system::Config for Test {
	type BaseCallFilter = ();
	type BlockWeights = BlockWeights;
	type BlockLength = ();
	type DbWeight = ();
	type Origin = Origin;
	type Call = Call;
	type Index = u64;
	type BlockNumber = BlockNumber;
	type Hash = H256;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<AccountId>;
	type Header = Header;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<u128>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
}

parameter_types! {
	pub static ExistentialDeposit: u64 = 0;
}

impl pallet_balances::Config for Test {
	type MaxLocks = ();
	type Balance = Balance;
	type Event = Event;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
}

impl configuration::Config for Test { }

impl paras::Config for Test {
	type Origin = Origin;
}

parameter_types! {
	pub const ParaDeposit: Balance = 500;
	pub const MaxCodeSize: u32 = 200;
	pub const MaxHeadSize: u32 = 100;
}

impl paras_registrar::Config for Test {
	type Event = Event;
	type OnSwap = ();
	type ParaDeposit = ParaDeposit;
	type MaxCodeSize = MaxCodeSize;
	type MaxHeadSize = MaxHeadSize;
	type Currency = Balances;
	type Origin = Origin;
	type ParachainCleanup = ();
}

parameter_types! {
	pub const EndingPeriod: BlockNumber = 10;
}

impl auctions::Config for Test {
	type Event = Event;
	type Leaser = Slots;
	type EndingPeriod = EndingPeriod;
	type Randomness = TestRandomness;
	type InitiateOrigin = EnsureRoot<AccountId>;
}

parameter_types! {
	pub const LeasePeriod: BlockNumber = 100;
}

impl slots::Config for Test {
	type Event = Event;
	type Currency = Balances;
	type Registrar = Registrar;
	type LeasePeriod = LeasePeriod;
}

parameter_types! {
	pub const CrowdloanId: ModuleId = ModuleId(*b"py/cfund");
	pub const SubmissionDeposit: Balance = 100;
	pub const MinContribution: Balance = 1;
	pub const RetirementPeriod: BlockNumber = 10;
	pub const RemoveKeysLimit: u32 = 100;

}

impl crowdloan::Config for Test {
	type Event = Event;
	type ModuleId = CrowdloanId;
	type SubmissionDeposit = SubmissionDeposit;
	type MinContribution = MinContribution;
	type RetirementPeriod = RetirementPeriod;
	type OrphanedFunds = ();
	type RemoveKeysLimit = RemoveKeysLimit;
	type Registrar = Registrar;
	type Auctioneer = Auctions;
}

/// Create a new set of test externalities.
pub fn new_test_ext() -> TestExternalities {
	let t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
	t.into()
}

fn run_to_block(n: u32) {
	while System::block_number() < n {
		Slots::on_finalize(System::block_number());
		Crowdloan::on_finalize(System::block_number());
		Auctions::on_finalize(System::block_number());
		Registrar::on_finalize(System::block_number());
		Configuration::on_finalize(System::block_number());
		Paras::on_finalize(System::block_number());
		Balances::on_finalize(System::block_number());
		System::on_finalize(System::block_number());
		System::set_block_number(System::block_number() + 1);
		System::on_initialize(System::block_number());
		Balances::on_initialize(System::block_number());
		Paras::on_initialize(System::block_number());
		Configuration::on_initialize(System::block_number());
		Registrar::on_initialize(System::block_number());
		Auctions::on_initialize(System::block_number());
		Crowdloan::on_initialize(System::block_number());
		Slots::on_initialize(System::block_number());
	}
}

#[test]
fn basic_end_to_end_works() {
	new_test_ext().execute_with(|| {
		// User 1 will be our caller
		Balances::make_free_balance_be(&1, 1_000);
		// First register a parathread
		let genesis_head = Registrar::worst_head_data();
		let validation_code = Registrar::worst_validation_code();
		assert_ok!(Registrar::register(Origin::signed(1), ParaId::from(1), genesis_head, validation_code));
		// Start a new auction
		let duration = 100u32;
		let lease_period_index = 1u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index));
		// Open a crowdloan for the auction
		let cap = 1_000;
		let first_slot = 0;
		let last_slot = 3;
		let crowdloan_end = 200u32;
		assert_ok!(Crowdloan::create(
			Origin::signed(1),
			ParaId::from(1),
			cap,
			first_slot,
			last_slot,
			crowdloan_end,
		));

	});
}
