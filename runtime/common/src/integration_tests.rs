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
	curve::PiecewiseLinear,
	traits::{
		BlakeTwo256, IdentityLookup,
	},
};
use primitives::v1::{Balance, BlockNumber, Header, Id as ParaId};
use frame_support::{
	parameter_types, assert_ok, assert_noop,
	storage::StorageMap,
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
		// System Stuff
		System: frame_system::{Module, Call, Config, Storage, Event<T>},
		//Session: pallet_session::{Module, Call, Storage, Event, Config<T>},
		Balances: pallet_balances::{Module, Call, Storage, Config<T>, Event<T>},

		// Parachains Runtime
		Configuration: configuration::{Module, Call, Storage, Config<T>},
		Parachains: paras::{Module, Origin, Call, Storage, Config<T>},

		// Para Onboarding Pallets
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

pallet_staking_reward_curve::build! {
	const REWARD_CURVE: PiecewiseLinear<'static> = curve!(
		min_inflation: 0_025_000,
		max_inflation: 0_100_000,
		ideal_stake: 0_500_000,
		falloff: 0_050_000,
		max_piece_count: 40,
		test_precision: 0_005_000,
	);
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

fn maybe_new_session(_n: u32) {
	// TODO: Handle any session changes
}

fn run_to_block(n: u32) {
	while System::block_number() < n {
		let block_number = System::block_number();
		AllModules::on_finalize(block_number);
		System::on_finalize(block_number);
		System::set_block_number(block_number + 1);
		System::on_initialize(block_number + 1);
		maybe_new_session(block_number + 1);
		AllModules::on_initialize(block_number + 1);
	}
}

fn last_event() -> Event {
	System::events().pop().expect("Event expected").event
}

#[test]
fn basic_end_to_end_works() {
	new_test_ext().execute_with(|| {
		// User 1 and 2 will own parachains
		Balances::make_free_balance_be(&1, 1_000);
		Balances::make_free_balance_be(&2, 1_000);
		// First register 2 parathreads
		let genesis_head = Registrar::worst_head_data();
		let validation_code = Registrar::worst_validation_code();
		assert_ok!(Registrar::register(
			Origin::signed(1),
			ParaId::from(1),
			genesis_head.clone(),
			validation_code.clone(),
		));
		assert_ok!(Registrar::register(Origin::signed(2), ParaId::from(2), genesis_head, validation_code));
		// Start a new auction
		let duration = 100u32;
		let lease_period_index = 1u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index));
		// Para 1 will bid directly for slot 1, 2
		// Open a crowdloan for Para 2 for slot 3, 4
		assert_ok!(Crowdloan::create(
			Origin::signed(2),
			ParaId::from(2),
			1_000, // Cap
			3, // First Slot
			4, // Last Slot
			200, // Block End
		));
		let crowdloan_account = Crowdloan::fund_account_id(ParaId::from(2));

		// Auction ending begins on block 100, so we make a bid before then.
		run_to_block(90);

		Balances::make_free_balance_be(&10, 1_000);
		Balances::make_free_balance_be(&20, 1_000);

		// User 10 will bid directly for parachain 1
		assert_ok!(Auctions::bid(
			Origin::signed(10),
			ParaId::from(1),
			1, // Auction Index
			1, // First Slot
			2, // Last slot
			910, // Amount
		));

		// User 20 will be a contribute to crowdfund for parachain 2
		Balances::make_free_balance_be(&2, 1_000);
		assert_ok!(Crowdloan::contribute(Origin::signed(2), ParaId::from(2), 920));

		// Auction ends at block 110
		run_to_block(109);
		assert_eq!(
			last_event(),
			crowdloan::RawEvent::HandleBidResult(ParaId::from(2), Ok(())).into(),
		);
		run_to_block(110);
		assert_eq!(
			last_event(),
			slots::RawEvent::Leased(
				ParaId::from(2),
				crowdloan_account,
				3, 2, 920, 920,
			).into(),
		);

		// Paras should have won slots
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(1)),
			vec![Some((10, 910)), Some((10, 910))],
		);
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(2)),
			vec![None, None, Some((crowdloan_account, 920)), Some((crowdloan_account, 920))],
		);

	});
}
