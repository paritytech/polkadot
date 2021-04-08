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

use sp_std::sync::Arc;
use sp_io::TestExternalities;
use sp_core::{H256, crypto::KeyTypeId};
use sp_runtime::{
	ModuleId,
	traits::{
		BlakeTwo256, IdentityLookup, One,
	},
};
use sp_keystore::{KeystoreExt, testing::KeyStore};
use primitives::v1::{BlockNumber, Header, Id as ParaId, ValidationCode, HeadData};
use frame_support::{
	parameter_types, assert_ok, assert_noop,
	storage::StorageMap,
	traits::{Currency, OnInitialize, OnFinalize, KeyOwnerProofSystem},
};
use frame_system::EnsureRoot;
use runtime_parachains::{
	ParaLifecycle, Origin as ParaOrigin,
	paras, configuration, shared,
};
use frame_support_test::TestRandomness;
use crate::{
	auctions, crowdloan, slots, paras_registrar,
	traits::{
		Registrar as RegistrarT, Auctioneer,
	},
};

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

type AccountId = u32;
type Balance = u32;
type Moment = u32;

frame_support::construct_runtime!(
	pub enum Test where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		// System Stuff
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		Babe: pallet_babe::{Pallet, Call, Storage, Config, ValidateUnsigned},

		// Parachains Runtime
		Configuration: configuration::{Pallet, Call, Storage, Config<T>},
		Paras: paras::{Pallet, Origin, Call, Storage, Config<T>},

		// Para Onboarding Pallets
		Registrar: paras_registrar::{Pallet, Call, Storage, Event<T>},
		Auctions: auctions::{Pallet, Call, Storage, Event<T>},
		Crowdloan: crowdloan::{Pallet, Call, Storage, Event<T>},
		Slots: slots::{Pallet, Call, Storage, Event<T>},
	}
);

use crate::crowdloan::Error as CrowdloanError;

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
	type AccountData = pallet_balances::AccountData<Balance>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
}

parameter_types! {
	pub const EpochDuration: u64 = 10;
	pub const ExpectedBlockTime: Moment = 6_000;
	pub const ReportLongevity: u64 = 10;
}

impl pallet_babe::Config for Test {
	type EpochDuration = EpochDuration;
	type ExpectedBlockTime = ExpectedBlockTime;
	type EpochChangeTrigger = pallet_babe::ExternalTrigger;
	type KeyOwnerProofSystem = ();
	type KeyOwnerProof =
		<Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, pallet_babe::AuthorityId)>>::Proof;
	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		pallet_babe::AuthorityId,
	)>>::IdentificationTuple;
	type HandleEquivocation = ();
	type WeightInfo = ();
}

parameter_types! {
	pub const MinimumPeriod: Moment = 6_000 / 2;
}

impl pallet_timestamp::Config for Test {
	type Moment = Moment;
	type OnTimestampSet = ();
	type MinimumPeriod = MinimumPeriod;
	type WeightInfo = ();
}

parameter_types! {
	pub static ExistentialDeposit: Balance = 1;
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

impl shared::Config for Test { }

impl paras::Config for Test {
	type Origin = Origin;
}

parameter_types! {
	pub const ParaDeposit: Balance = 500;
	pub const DataDepositPerByte: Balance = 1;
	pub const MaxCodeSize: u32 = 200;
	pub const MaxHeadSize: u32 = 100;
}

impl paras_registrar::Config for Test {
	type Event = Event;
	type OnSwap = (Crowdloan, Slots);
	type ParaDeposit = ParaDeposit;
	type DataDepositPerByte = DataDepositPerByte;
	type MaxCodeSize = MaxCodeSize;
	type MaxHeadSize = MaxHeadSize;
	type Currency = Balances;
	type Origin = Origin;
	type WeightInfo = crate::paras_registrar::TestWeightInfo;
}

parameter_types! {
	pub const EndingPeriod: BlockNumber = 10;
	pub const SampleLength: BlockNumber = 1;
}

impl auctions::Config for Test {
	type Event = Event;
	type Leaser = Slots;
	type EndingPeriod = EndingPeriod;
	type SampleLength = SampleLength;
	type Randomness = TestRandomness<Self>;
	type InitiateOrigin = EnsureRoot<AccountId>;
	type WeightInfo = crate::auctions::TestWeightInfo;
}

parameter_types! {
	pub const LeasePeriod: BlockNumber = 100;
}

impl slots::Config for Test {
	type Event = Event;
	type Currency = Balances;
	type Registrar = Registrar;
	type LeasePeriod = LeasePeriod;
	type WeightInfo = crate::slots::TestWeightInfo;
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
	type WeightInfo = crate::crowdloan::TestWeightInfo;
}

/// Create a new set of test externalities.
pub fn new_test_ext() -> TestExternalities {
	let t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
	let keystore = KeyStore::new();
	let mut ext: sp_io::TestExternalities = t.into();
	ext.register_extension(KeystoreExt(Arc::new(keystore)));
	ext.execute_with(|| System::set_block_number(1));
	ext
}

const BLOCKS_PER_SESSION: u32 = 10;

fn maybe_new_session(n: u32) {
	if n % BLOCKS_PER_SESSION == 0 {
		shared::Module::<Test>::set_session_index(
			shared::Module::<Test>::session_index() + 1
		);
		Paras::test_on_new_session();
	}
}

fn test_genesis_head(size: usize) -> HeadData {
	HeadData(vec![0u8; size])
}

fn test_validation_code(size: usize) -> ValidationCode {
	let mut validation_code = vec![0u8; size as usize];
	// Replace first bytes of code with "WASM_MAGIC" to pass validation test.
	let _ = validation_code.splice(
		..crate::WASM_MAGIC.len(),
		crate::WASM_MAGIC.iter().cloned(),
	).collect::<Vec<_>>();
	ValidationCode(validation_code)
}

fn para_origin(id: u32) -> ParaOrigin {
	ParaOrigin::Parachain(id.into())
}

fn run_to_block(n: u32) {
	assert!(System::block_number() < n);
	while System::block_number() < n {
		let block_number = System::block_number();
		AllPallets::on_finalize(block_number);
		System::on_finalize(block_number);
		System::set_block_number(block_number + 1);
		System::on_initialize(block_number + 1);
		maybe_new_session(block_number + 1);
		AllPallets::on_initialize(block_number + 1);
	}
}

fn run_to_session(n: u32) {
	let block_number = BLOCKS_PER_SESSION * n;
	run_to_block(block_number);
}

fn last_event() -> Event {
	System::events().pop().expect("Event expected").event
}

#[test]
fn basic_end_to_end_works() {
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one());
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
		assert_ok!(Registrar::register(
			Origin::signed(2),
			ParaId::from(2),
			genesis_head,
			validation_code,
		));

		// Paras should be onboarding
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Onboarding));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Onboarding));

		// Start a new auction in the future
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// 2 sessions later they are parathreads
		run_to_session(2);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Parathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Parathread));

		// Para 1 will bid directly for slot 1, 2
		// Open a crowdloan for Para 2 for slot 3, 4
		assert_ok!(Crowdloan::create(
			Origin::signed(2),
			ParaId::from(2),
			1_000, // Cap
			lease_period_index_start + 2, // First Slot
			lease_period_index_start + 3, // Last Slot
			200, // Block End
			None,
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
			lease_period_index_start + 0, // First Slot
			lease_period_index_start + 1, // Last slot
			910, // Amount
		));

		// User 2 will be a contribute to crowdfund for parachain 2
		Balances::make_free_balance_be(&2, 1_000);
		assert_ok!(Crowdloan::contribute(Origin::signed(2), ParaId::from(2), 920, None));

		// Auction ends at block 110
		run_to_block(109);
		assert_eq!(
			last_event(),
			crowdloan::RawEvent::HandleBidResult(ParaId::from(2), Ok(())).into(),
		);
		run_to_block(110);
		assert_eq!(
			last_event(),
			auctions::RawEvent::AuctionClosed(1).into(),
		);

		// Paras should have won slots
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(1)),
			// -- 1 --- 2 --- 3 --------- 4 ------------ 5 --------
			vec![None, None, None, Some((10, 910)), Some((10, 910))],
		);
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(2)),
			// -- 1 --- 2 --- 3 --- 4 --- 5 ---------------- 6 --------------------------- 7 ----------------
			vec![None, None, None, None, None, Some((crowdloan_account, 920)), Some((crowdloan_account, 920))],
		);

		// New leases will start on block 400
		let lease_start_block = 400;
		run_to_block(lease_start_block);

		// First slot, Para 1 should be transitioning to Parachain
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::UpgradingParathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Parathread));

		// Two sessions later, it has upgraded
		run_to_block(lease_start_block + 20);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Parachain));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Parathread));

		// Second slot nothing happens :)
		run_to_block(lease_start_block + 100);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Parachain));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Parathread));

		// Third slot, Para 2 should be upgrading, and Para 1 is downgrading
		run_to_block(lease_start_block + 200);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::DowngradingParachain));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::UpgradingParathread));

		// Two sessions later, they have transitioned
		run_to_block(lease_start_block + 220);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Parathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Parachain));

		// Fourth slot nothing happens :)
		run_to_block(lease_start_block + 300);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Parathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Parachain));

		// Fifth slot, Para 2 is downgrading
		run_to_block(lease_start_block + 400);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Parathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::DowngradingParachain));

		// Two sessions later, Para 2 is downgraded
		run_to_block(lease_start_block + 420);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Parathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Parathread));
	});
}

#[test]
fn basic_errors_fail() {
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one());
		// Can't double register
		Balances::make_free_balance_be(&1, 1_000);
		Balances::make_free_balance_be(&2, 1_000);

		let genesis_head = Registrar::worst_head_data();
		let validation_code = Registrar::worst_validation_code();
		assert_ok!(Registrar::register(
			Origin::signed(1),
			ParaId::from(1),
			genesis_head.clone(),
			validation_code.clone(),
		));
		assert_noop!(Registrar::register(
			Origin::signed(2),
			ParaId::from(1),
			genesis_head,
			validation_code,
		), paras_registrar::Error::<Test>::AlreadyRegistered);

		// Start an auction
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// Cannot create a crowdloan if you do not own the para
		assert_noop!(Crowdloan::create(
			Origin::signed(2),
			ParaId::from(1),
			1_000, // Cap
			lease_period_index_start + 2, // First Slot
			lease_period_index_start + 3, // Last Slot
			200, // Block End
			None,
		), crowdloan::Error::<Test>::InvalidOrigin);
	});
}

#[test]
fn competing_slots() {
	// This test will verify that competing slots, from different sources will resolve appropriately.
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one());
		let max_bids = 10u32;

		// Create n paras and owners
		for n in 1 ..= max_bids {
			Balances::make_free_balance_be(&n, 1_000);
			let genesis_head = Registrar::worst_head_data();
			let validation_code = Registrar::worst_validation_code();
			assert_ok!(Registrar::register(
				Origin::signed(n),
				ParaId::from(n),
				genesis_head,
				validation_code,
			));
		}

		// Start a new auction in the future
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		for n in 1 ..= max_bids {
			// Increment block number
			run_to_block(n * 10);

			Balances::make_free_balance_be(&(n * 10), n * 1_000);

			let (start, end) = match n {
				1  => (0, 0),
				2  => (0, 1),
				3  => (0, 2),
				4  => (0, 3),
				5  => (1, 1),
				6  => (1, 2),
				7  => (1, 3),
				8  => (2, 2),
				9  => (2, 3),
				10 => (3, 3),
				_ => panic!("test not meant for this"),
			};

			// User 10 will bid directly for parachain 1
			assert_ok!(Auctions::bid(
				Origin::signed(n * 10),
				ParaId::from(n),
				1, // Auction Index
				lease_period_index_start + start, // First Slot
				lease_period_index_start + end, // Last slot
				n * 900, // Amount
			));
		}

		// All winner slots are filled by bids
		for winner in &auctions::Winning::<Test>::get(0).unwrap() {
			assert!(winner.is_some());
		}

		// Auction should be done
		run_to_block(110);

		// Appropriate Paras should have won slots
		// 900 + 4500 + 2x 8100 = 21,600
		// 900 + 4500 + 7200 + 9000 = 21,600
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(1)),
			// -- 1 --- 2 --- 3 ---------- 4 ------
			vec![None, None, None, Some((10, 900))],
		);
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(5)),
			// -- 1 --- 2 --- 3 --- 4 ---------- 5 -------
			vec![None, None, None, None, Some((50, 4500))],
		);
		// TODO: Is this right?
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(9)),
			// -- 1 --- 2 --- 3 --- 4 --- 5 ---------- 6 --------------- 7 -------
			vec![None, None, None, None, None, Some((90, 8100)), Some((90, 8100))],
		);
	});
}

#[test]
fn competing_bids() {
	// This test will verify that competing bids, from different sources will resolve appropriately.
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one());
		// Start a new auction in the future
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// Create 3 paras and owners
		for n in 1 ..= 3 {
			Balances::make_free_balance_be(&n, 1_000);
			let genesis_head = Registrar::worst_head_data();
			let validation_code = Registrar::worst_validation_code();
			assert_ok!(Registrar::register(
				Origin::signed(n),
				ParaId::from(n),
				genesis_head,
				validation_code,
			));

			// Create a crowdloan for each para
			assert_ok!(Crowdloan::create(
				Origin::signed(n),
				ParaId::from(n),
				100_000, // Cap
				lease_period_index_start + 2, // First Slot
				lease_period_index_start + 3, // Last Slot
				200, // Block End,
				None,
			));
		}

		for n in 1 ..= 9 {
			// Increment block number
			run_to_block(n * 10);

			Balances::make_free_balance_be(&(n * 10), n * 1_000);

			let para = n % 3 + 1;

			if n % 2 == 0 {
				// User 10 will bid directly for parachain 1
				assert_ok!(Auctions::bid(
					Origin::signed(n * 10),
					ParaId::from(para),
					1, // Auction Index
					lease_period_index_start + 0, // First Slot
					lease_period_index_start + 1, // Last slot
					n * 900, // Amount
				));
			} else {
				// User 20 will be a contribute to crowdfund for parachain 2
				assert_ok!(Crowdloan::contribute(
					Origin::signed(n * 10),
					ParaId::from(para),
					n + 900,
					None,
				));
			}
		}

		// Auction should be done
		run_to_block(110);

		// Appropriate Paras should have won slots
		let crowdloan_2 = Crowdloan::fund_account_id(ParaId::from(2));
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(1)),
			// -- 1 --- 2 --- 3 --- 4 --- 5 ------------- 6 ------------------------ 7 -------------
			vec![None, None, None, None, None, Some((crowdloan_2, 1812)), Some((crowdloan_2, 1812))],
		);
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(3)),
			// -- 1 --- 2 --- 3 ---------- 4 --------------- 5 -------
			vec![None, None, None, Some((80, 7200)), Some((80, 7200))],
		);
	});
}

#[test]
fn basic_swap_works() {
	// This test will test a swap between a parachain and parathread works successfully.
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one()); // So events are emitted
		// User 1 and 2 will own paras
		Balances::make_free_balance_be(&1, 1_000);
		Balances::make_free_balance_be(&2, 1_000);
		// First register 2 parathreads with different data
		assert_ok!(Registrar::register(
			Origin::signed(1),
			ParaId::from(1),
			test_genesis_head(10),
			test_validation_code(10),
		));
		assert_ok!(Registrar::register(
			Origin::signed(2),
			ParaId::from(2),
			test_genesis_head(20),
			test_validation_code(20),
		));

		// Paras should be onboarding
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Onboarding));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Onboarding));

		// Start a new auction in the future
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// 2 sessions later they are parathreads
		run_to_session(2);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Parathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Parathread));

		// Open a crowdloan for Para 1 for slots 0-3
		assert_ok!(Crowdloan::create(
			Origin::signed(1),
			ParaId::from(1),
			1_000_000, // Cap
			lease_period_index_start + 0, // First Slot
			lease_period_index_start + 3, // Last Slot
			200, // Block End
			None,
		));
		// TODO: Check why this is the same for all paras
		let crowdloan_account = Crowdloan::fund_account_id(ParaId::from(1));

		// Bunch of contributions
		let mut total = 0;
		for i in 10 .. 20 {
			Balances::make_free_balance_be(&i, 1_000);
			assert_ok!(Crowdloan::contribute(Origin::signed(i), ParaId::from(1), 900 - i, None));
			total += 900 - i;
		}
		assert!(total > 0);
		assert_eq!(Balances::free_balance(&crowdloan_account), total);

		// Go to end of auction where everyone won their slots
		run_to_block(200);

		// Deposit is appropriately taken
		// ----------------------------------------- para deposit --- crowdloan
		assert_eq!(Balances::reserved_balance(&1), (500 + 10 * 2 * 1) + 100);
		assert_eq!(Balances::reserved_balance(&2), 500 + 20 * 2 * 1);
		assert_eq!(Balances::reserved_balance(&crowdloan_account), total);
		// Crowdloan is appropriately set
		assert!(Crowdloan::funds(ParaId::from(1)).is_some());
		assert!(Crowdloan::funds(ParaId::from(2)).is_none());

		// New leases will start on block 400
		let lease_start_block = 400;
		run_to_block(lease_start_block);

		// Slots are won by Para 1
		assert!(!Slots::lease(ParaId::from(1)).is_empty());
		assert!(Slots::lease(ParaId::from(2)).is_empty());

		// 2 sessions later it is a parachain
		run_to_block(lease_start_block + 20);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Parachain));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Parathread));

		// Initiate a swap
		assert_ok!(Registrar::swap(para_origin(1).into(), ParaId::from(2)));
		assert_ok!(Registrar::swap(para_origin(2).into(), ParaId::from(1)));

		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::DowngradingParachain));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::UpgradingParathread));

		// 2 session later they have swapped
		run_to_block(lease_start_block + 40);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Parathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Parachain));

		// Deregister parathread
		assert_ok!(Registrar::deregister(para_origin(1).into(), ParaId::from(1)));
		// Correct deposit is unreserved
		assert_eq!(Balances::reserved_balance(&1), 100); // crowdloan deposit left over
		assert_eq!(Balances::reserved_balance(&2), 500 + 20 * 2 * 1);
		// Crowdloan ownership is swapped
		assert!(Crowdloan::funds(ParaId::from(1)).is_none());
		assert!(Crowdloan::funds(ParaId::from(2)).is_some());
		// Slot is swapped
		assert!(Slots::lease(ParaId::from(1)).is_empty());
		assert!(!Slots::lease(ParaId::from(2)).is_empty());

		// Cant dissolve
		assert_noop!(Crowdloan::dissolve(Origin::signed(1), ParaId::from(1)), CrowdloanError::<Test>::InvalidParaId);
		assert_noop!(Crowdloan::dissolve(Origin::signed(2), ParaId::from(2)), CrowdloanError::<Test>::NotReadyToDissolve);

		// Go way in the future when the para is offboarded
		run_to_block(lease_start_block + 1000);

		// Withdraw of contributions works
		assert_eq!(Balances::free_balance(&crowdloan_account), total);
		for i in 10 .. 20 {
			assert_ok!(Crowdloan::withdraw(Origin::signed(i), i, ParaId::from(2)));
		}
		assert_eq!(Balances::free_balance(&crowdloan_account), 0);

		// Dissolve returns the balance of the person who put a deposit for crowdloan
		assert_ok!(Crowdloan::dissolve(Origin::signed(1), ParaId::from(2)));
		assert_eq!(Balances::reserved_balance(&1), 0);
		assert_eq!(Balances::reserved_balance(&2), 500 + 20 * 2 * 1);

		// Final deregister sets everything back to the start
		assert_ok!(Registrar::deregister(para_origin(2).into(), ParaId::from(2)));
		assert_eq!(Balances::reserved_balance(&2), 0);
	})
}

#[test]
fn crowdloan_ending_period_bid() {
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one()); // So events are emitted
		// User 1 and 2 will own paras
		Balances::make_free_balance_be(&1, 1_000);
		Balances::make_free_balance_be(&2, 1_000);
		// First register 2 parathreads
		assert_ok!(Registrar::register(
			Origin::signed(1),
			ParaId::from(1),
			test_genesis_head(10),
			test_validation_code(10),
		));
		assert_ok!(Registrar::register(
			Origin::signed(2),
			ParaId::from(2),
			test_genesis_head(20),
			test_validation_code(20),
		));

		// Paras should be onboarding
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Onboarding));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Onboarding));

		// Start a new auction in the future
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// 2 sessions later they are parathreads
		run_to_session(2);
		assert_eq!(Paras::lifecycle(ParaId::from(1)), Some(ParaLifecycle::Parathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2)), Some(ParaLifecycle::Parathread));

		// Open a crowdloan for Para 1 for slots 0-3
		assert_ok!(Crowdloan::create(
			Origin::signed(1),
			ParaId::from(1),
			1_000_000, // Cap
			lease_period_index_start + 0, // First Slot
			lease_period_index_start + 3, // Last Slot
			200, // Block End
			None,
		));
		// TODO: Check why this is the same for all paras
		let crowdloan_account = Crowdloan::fund_account_id(ParaId::from(1));

		// Bunch of contributions
		let mut total = 0;
		for i in 10 .. 20 {
			Balances::make_free_balance_be(&i, 1_000);
			assert_ok!(Crowdloan::contribute(Origin::signed(i), ParaId::from(1), 900 - i, None));
			total += 900 - i;
		}
		assert!(total > 0);
		assert_eq!(Balances::free_balance(&crowdloan_account), total);

		// Bid for para 2 directly
		Balances::make_free_balance_be(&2, 1_000);
		assert_ok!(Auctions::bid(
			Origin::signed(2),
			ParaId::from(2),
			1, // Auction Index
			lease_period_index_start + 0, // First Slot
			lease_period_index_start + 1, // Last slot
			900, // Amount
		));

		// Go to beginning of ending period
		run_to_block(100);

		assert_eq!(Auctions::is_ending(100), Some(0));

		assert_eq!(Auctions::winning(0), Some(
			[
				None, // 0-0
				Some((2, ParaId::from(2), 900)), // 0-1
				None, // 0-2
				Some((crowdloan_account, ParaId::from(1), total)), // 0-3
				None, // 1-1
				None, // 1-2
				None, // 1-3
				None, // 2-2
				None, // 2-3
				None, // 3-3
			]
		));

		run_to_block(101);

		Balances::make_free_balance_be(&1234, 1_000);
		assert_ok!(Crowdloan::contribute(Origin::signed(1234), ParaId::from(1), 900, None));

		// Data propagates correctly
		run_to_block(102);
		assert_eq!(Auctions::winning(2), Some(
			[
				None, // 0-0
				Some((2, ParaId::from(2), 900)), // 0-1
				None, // 0-2
				Some((crowdloan_account, ParaId::from(1), total + 900)), // 0-3
				None, // 1-1
				None, // 1-2
				None, // 1-3
				None, // 2-2
				None, // 2-3
				None, // 3-3
			]
		));
	})
}
