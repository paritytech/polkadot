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

use crate::{
	auctions, crowdloan, paras_registrar,
	slot_range::SlotRange,
	slots,
	traits::{AuctionStatus, Auctioneer, Leaser, Registrar as RegistrarT},
};
use frame_support::{
	assert_noop, assert_ok, parameter_types,
	traits::{Currency, GenesisBuild, KeyOwnerProofSystem, OnFinalize, OnInitialize},
	PalletId,
};
use frame_support_test::TestRandomness;
use frame_system::EnsureRoot;
use parity_scale_codec::Encode;
use primitives::v2::{
	BlockNumber, HeadData, Header, Id as ParaId, ValidationCode, LOWEST_PUBLIC_ID,
};
use runtime_parachains::{
	configuration, origin, paras, shared, Origin as ParaOrigin, ParaLifecycle,
};
use sp_core::{crypto::KeyTypeId, H256};
use sp_io::TestExternalities;
use sp_keystore::{testing::KeyStore, KeystoreExt};
use sp_runtime::{
	traits::{BlakeTwo256, IdentityLookup, One},
	transaction_validity::TransactionPriority,
	AccountId32,
};
use sp_std::{convert::TryInto, sync::Arc};

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

type AccountId = AccountId32;
type Balance = u32;
type Moment = u32;

fn account_id(i: u32) -> AccountId32 {
	let b4 = i.encode();
	let b32 = [&b4[..], &b4[..], &b4[..], &b4[..], &b4[..], &b4[..], &b4[..], &b4[..]].concat();
	let array: [u8; 32] = b32.try_into().unwrap();
	array.into()
}

fn signed(i: u32) -> Origin {
	let account_id = account_id(i);
	Origin::signed(account_id)
}

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
		Paras: paras::{Pallet, Call, Storage, Event, Config},
		ParasShared: shared::{Pallet, Call, Storage},
		ParachainsOrigin: origin::{Pallet, Origin},

		// Para Onboarding Pallets
		Registrar: paras_registrar::{Pallet, Call, Storage, Event<T>},
		Auctions: auctions::{Pallet, Call, Storage, Event<T>},
		Crowdloan: crowdloan::{Pallet, Call, Storage, Event<T>},
		Slots: slots::{Pallet, Call, Storage, Event<T>},
	}
);

impl<C> frame_system::offchain::SendTransactionTypes<C> for Test
where
	Call: From<C>,
{
	type Extrinsic = UncheckedExtrinsic;
	type OverarchingCall = Call;
}

use crate::{auctions::Error as AuctionsError, crowdloan::Error as CrowdloanError};

parameter_types! {
	pub const BlockHashCount: u32 = 250;
	pub BlockWeights: frame_system::limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(4 * 1024 * 1024);
}

impl frame_system::Config for Test {
	type BaseCallFilter = frame_support::traits::Everything;
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
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

parameter_types! {
	pub const EpochDuration: u64 = 10;
	pub const ExpectedBlockTime: Moment = 6_000;
	pub const ReportLongevity: u64 = 10;
	pub const MaxAuthorities: u32 = 100_000;
}

impl pallet_babe::Config for Test {
	type EpochDuration = EpochDuration;
	type ExpectedBlockTime = ExpectedBlockTime;
	type EpochChangeTrigger = pallet_babe::ExternalTrigger;
	type DisabledValidators = ();
	type KeyOwnerProofSystem = ();
	type KeyOwnerProof = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		pallet_babe::AuthorityId,
	)>>::Proof;
	type KeyOwnerIdentification = <Self::KeyOwnerProofSystem as KeyOwnerProofSystem<(
		KeyTypeId,
		pallet_babe::AuthorityId,
	)>>::IdentificationTuple;
	type HandleEquivocation = ();
	type WeightInfo = ();
	type MaxAuthorities = MaxAuthorities;
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
	pub const MaxReserves: u32 = 50;
}

impl pallet_balances::Config for Test {
	type MaxLocks = ();
	type Balance = Balance;
	type Event = Event;
	type DustRemoval = ();
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type WeightInfo = ();
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
}

impl configuration::Config for Test {
	type WeightInfo = configuration::TestWeightInfo;
}

impl shared::Config for Test {}

impl origin::Config for Test {}

parameter_types! {
	pub const ParasUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
}

impl paras::Config for Test {
	type Event = Event;
	type WeightInfo = paras::TestWeightInfo;
	type UnsignedPriority = ParasUnsignedPriority;
	type NextSessionRotation = crate::mock::TestNextSessionRotation;
}

parameter_types! {
	pub const ParaDeposit: Balance = 500;
	pub const DataDepositPerByte: Balance = 1;
}

impl paras_registrar::Config for Test {
	type Event = Event;
	type OnSwap = (Crowdloan, Slots);
	type ParaDeposit = ParaDeposit;
	type DataDepositPerByte = DataDepositPerByte;
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
	type Registrar = Registrar;
	type EndingPeriod = EndingPeriod;
	type SampleLength = SampleLength;
	type Randomness = TestRandomness<Self>;
	type InitiateOrigin = EnsureRoot<AccountId>;
	type WeightInfo = crate::auctions::TestWeightInfo;
}

parameter_types! {
	pub const LeasePeriod: BlockNumber = 100;
	pub static LeaseOffset: BlockNumber = 0;
}

impl slots::Config for Test {
	type Event = Event;
	type Currency = Balances;
	type Registrar = Registrar;
	type LeasePeriod = LeasePeriod;
	type LeaseOffset = LeaseOffset;
	type ForceOrigin = EnsureRoot<AccountId>;
	type WeightInfo = crate::slots::TestWeightInfo;
}

parameter_types! {
	pub const CrowdloanId: PalletId = PalletId(*b"py/cfund");
	pub const SubmissionDeposit: Balance = 100;
	pub const MinContribution: Balance = 1;
	pub const RemoveKeysLimit: u32 = 100;
	pub const MaxMemoLength: u8 = 32;
}

impl crowdloan::Config for Test {
	type Event = Event;
	type PalletId = CrowdloanId;
	type SubmissionDeposit = SubmissionDeposit;
	type MinContribution = MinContribution;
	type RemoveKeysLimit = RemoveKeysLimit;
	type Registrar = Registrar;
	type Auctioneer = Auctions;
	type MaxMemoLength = MaxMemoLength;
	type WeightInfo = crate::crowdloan::TestWeightInfo;
}

/// Create a new set of test externalities.
pub fn new_test_ext() -> TestExternalities {
	let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
	GenesisBuild::<Test>::assimilate_storage(
		&configuration::GenesisConfig {
			config: configuration::HostConfiguration {
				max_code_size: 2 * 1024 * 1024,      // 2 MB
				max_head_data_size: 1 * 1024 * 1024, // 1 MB
				..Default::default()
			},
		},
		&mut t,
	)
	.unwrap();
	let keystore = KeyStore::new();
	let mut ext: sp_io::TestExternalities = t.into();
	ext.register_extension(KeystoreExt(Arc::new(keystore)));
	ext.execute_with(|| System::set_block_number(1));
	ext
}

#[cfg(feature = "runtime-benchmarks")]
pub fn new_test_ext_with_offset(n: BlockNumber) -> TestExternalities {
	LeaseOffset::set(n);
	new_test_ext()
}

const BLOCKS_PER_SESSION: u32 = 10;

fn maybe_new_session(n: u32) {
	if n % BLOCKS_PER_SESSION == 0 {
		shared::Pallet::<Test>::set_session_index(shared::Pallet::<Test>::session_index() + 1);
		Paras::test_on_new_session();
	}
}

fn test_genesis_head(size: usize) -> HeadData {
	HeadData(vec![0u8; size])
}

fn test_validation_code(size: usize) -> ValidationCode {
	let validation_code = vec![0u8; size as usize];
	ValidationCode(validation_code)
}

fn para_origin(id: u32) -> ParaOrigin {
	ParaOrigin::Parachain(id.into())
}

fn run_to_block(n: u32) {
	assert!(System::block_number() < n);
	while System::block_number() < n {
		let block_number = System::block_number();
		AllPalletsWithSystem::on_finalize(block_number);
		System::set_block_number(block_number + 1);
		maybe_new_session(block_number + 1);
		AllPalletsWithSystem::on_initialize(block_number + 1);
	}
}

fn run_to_session(n: u32) {
	let block_number = BLOCKS_PER_SESSION * n;
	run_to_block(block_number);
}

fn last_event() -> Event {
	System::events().pop().expect("Event expected").event
}

fn contains_event(event: Event) -> bool {
	System::events().iter().any(|x| x.event == event)
}

// Runs an end to end test of the auction, crowdloan, slots, and onboarding process over varying
// lease period offsets.
#[test]
fn basic_end_to_end_works() {
	for offset in [0u32, 50, 100, 200].iter() {
		LeaseOffset::set(*offset);
		new_test_ext().execute_with(|| {
			let para_1 = LOWEST_PUBLIC_ID;
			let para_2 = LOWEST_PUBLIC_ID + 1;
			assert!(System::block_number().is_one());
			// User 1 and 2 will own parachains
			Balances::make_free_balance_be(&account_id(1), 1_000_000_000);
			Balances::make_free_balance_be(&account_id(2), 1_000_000_000);
			// First register 2 parathreads
			let genesis_head = Registrar::worst_head_data();
			let validation_code = Registrar::worst_validation_code();
			assert_ok!(Registrar::reserve(signed(1)));
			assert_ok!(Registrar::register(
				signed(1),
				ParaId::from(para_1),
				genesis_head.clone(),
				validation_code.clone(),
			));
			assert_ok!(Registrar::reserve(signed(2)));
			assert_ok!(Registrar::register(
				signed(2),
				ParaId::from(2001),
				genesis_head,
				validation_code,
			));

			// Paras should be onboarding
			assert_eq!(Paras::lifecycle(ParaId::from(para_1)), Some(ParaLifecycle::Onboarding));
			assert_eq!(Paras::lifecycle(ParaId::from(para_2)), Some(ParaLifecycle::Onboarding));

			// Start a new auction in the future
			let duration = 99u32 + offset;
			let lease_period_index_start = 4u32;
			assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

			// 2 sessions later they are parathreads
			run_to_session(2);
			assert_eq!(Paras::lifecycle(ParaId::from(para_1)), Some(ParaLifecycle::Parathread));
			assert_eq!(Paras::lifecycle(ParaId::from(para_2)), Some(ParaLifecycle::Parathread));

			// Para 1 will bid directly for slot 1, 2
			// Open a crowdloan for Para 2 for slot 3, 4
			assert_ok!(Crowdloan::create(
				signed(2),
				ParaId::from(para_2),
				1_000,                        // Cap
				lease_period_index_start + 2, // First Slot
				lease_period_index_start + 3, // Last Slot
				200 + offset,                 // Block End
				None,
			));
			let fund_2 = Crowdloan::funds(ParaId::from(para_2)).unwrap();
			let crowdloan_account = Crowdloan::fund_account_id(fund_2.fund_index);

			// Auction ending begins on block 100 + offset, so we make a bid before then.
			run_to_block(90 + offset);

			Balances::make_free_balance_be(&account_id(10), 1_000_000_000);
			Balances::make_free_balance_be(&account_id(20), 1_000_000_000);

			// User 10 will bid directly for parachain 1
			assert_ok!(Auctions::bid(
				signed(10),
				ParaId::from(para_1),
				1,                            // Auction Index
				lease_period_index_start + 0, // First Slot
				lease_period_index_start + 1, // Last slot
				910,                          // Amount
			));

			// User 2 will be a contribute to crowdloan for parachain 2
			Balances::make_free_balance_be(&account_id(2), 1_000_000_000);
			assert_ok!(Crowdloan::contribute(signed(2), ParaId::from(para_2), 920, None));

			// Auction ends at block 110 + offset
			run_to_block(109 + offset);
			assert!(contains_event(
				crowdloan::Event::<Test>::HandleBidResult(ParaId::from(para_2), Ok(())).into()
			));
			run_to_block(110 + offset);
			assert_eq!(last_event(), auctions::Event::<Test>::AuctionClosed(1).into());

			// Paras should have won slots
			assert_eq!(
				slots::Leases::<Test>::get(ParaId::from(para_1)),
				// -- 1 --- 2 --- 3 --------- 4 ------------ 5 --------
				vec![None, None, None, Some((account_id(10), 910)), Some((account_id(10), 910))],
			);
			assert_eq!(
				slots::Leases::<Test>::get(ParaId::from(para_2)),
				// -- 1 --- 2 --- 3 --- 4 --- 5 ---------------- 6 --------------------------- 7 ----------------
				vec![
					None,
					None,
					None,
					None,
					None,
					Some((crowdloan_account.clone(), 920)),
					Some((crowdloan_account.clone(), 920))
				],
			);

			// Should not be able to contribute to a winning crowdloan
			Balances::make_free_balance_be(&account_id(3), 1_000_000_000);
			assert_noop!(
				Crowdloan::contribute(signed(3), ParaId::from(2001), 10, None),
				CrowdloanError::<Test>::BidOrLeaseActive
			);

			// New leases will start on block 400
			let lease_start_block = 400 + offset;
			run_to_block(lease_start_block);

			// First slot, Para 1 should be transitioning to Parachain
			assert_eq!(
				Paras::lifecycle(ParaId::from(para_1)),
				Some(ParaLifecycle::UpgradingParathread)
			);
			assert_eq!(Paras::lifecycle(ParaId::from(para_2)), Some(ParaLifecycle::Parathread));

			// Two sessions later, it has upgraded
			run_to_block(lease_start_block + 20);
			assert_eq!(Paras::lifecycle(ParaId::from(para_1)), Some(ParaLifecycle::Parachain));
			assert_eq!(Paras::lifecycle(ParaId::from(para_2)), Some(ParaLifecycle::Parathread));

			// Second slot nothing happens :)
			run_to_block(lease_start_block + 100);
			assert_eq!(Paras::lifecycle(ParaId::from(para_1)), Some(ParaLifecycle::Parachain));
			assert_eq!(Paras::lifecycle(ParaId::from(para_2)), Some(ParaLifecycle::Parathread));

			// Third slot, Para 2 should be upgrading, and Para 1 is downgrading
			run_to_block(lease_start_block + 200);
			assert_eq!(
				Paras::lifecycle(ParaId::from(para_1)),
				Some(ParaLifecycle::DowngradingParachain)
			);
			assert_eq!(
				Paras::lifecycle(ParaId::from(para_2)),
				Some(ParaLifecycle::UpgradingParathread)
			);

			// Two sessions later, they have transitioned
			run_to_block(lease_start_block + 220);
			assert_eq!(Paras::lifecycle(ParaId::from(para_1)), Some(ParaLifecycle::Parathread));
			assert_eq!(Paras::lifecycle(ParaId::from(para_2)), Some(ParaLifecycle::Parachain));

			// Fourth slot nothing happens :)
			run_to_block(lease_start_block + 300);
			assert_eq!(Paras::lifecycle(ParaId::from(para_1)), Some(ParaLifecycle::Parathread));
			assert_eq!(Paras::lifecycle(ParaId::from(para_2)), Some(ParaLifecycle::Parachain));

			// Fifth slot, Para 2 is downgrading
			run_to_block(lease_start_block + 400);
			assert_eq!(Paras::lifecycle(ParaId::from(para_1)), Some(ParaLifecycle::Parathread));
			assert_eq!(
				Paras::lifecycle(ParaId::from(para_2)),
				Some(ParaLifecycle::DowngradingParachain)
			);

			// Two sessions later, Para 2 is downgraded
			run_to_block(lease_start_block + 420);
			assert_eq!(Paras::lifecycle(ParaId::from(para_1)), Some(ParaLifecycle::Parathread));
			assert_eq!(Paras::lifecycle(ParaId::from(para_2)), Some(ParaLifecycle::Parathread));
		});
	}
}

#[test]
fn basic_errors_fail() {
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one());
		let para_id = LOWEST_PUBLIC_ID;
		// Can't double register
		Balances::make_free_balance_be(&account_id(1), 1_000_000_000);
		Balances::make_free_balance_be(&account_id(2), 1_000_000_000);

		let genesis_head = Registrar::worst_head_data();
		let validation_code = Registrar::worst_validation_code();
		assert_ok!(Registrar::reserve(signed(1)));
		assert_ok!(Registrar::register(
			signed(1),
			para_id,
			genesis_head.clone(),
			validation_code.clone(),
		));
		assert_ok!(Registrar::reserve(signed(2)));
		assert_noop!(
			Registrar::register(signed(2), para_id, genesis_head, validation_code,),
			paras_registrar::Error::<Test>::NotOwner
		);

		// Start an auction
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// Cannot create a crowdloan if you do not own the para
		assert_noop!(
			Crowdloan::create(
				signed(2),
				para_id,
				1_000,                        // Cap
				lease_period_index_start + 2, // First Slot
				lease_period_index_start + 3, // Last Slot
				200,                          // Block End
				None,
			),
			crowdloan::Error::<Test>::InvalidOrigin
		);
	});
}

#[test]
fn competing_slots() {
	// This test will verify that competing slots, from different sources will resolve appropriately.
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one());
		let max_bids = 10u32;
		let para_id = LOWEST_PUBLIC_ID;

		// Create n paras and owners
		for n in 1..=max_bids {
			Balances::make_free_balance_be(&account_id(n), 1_000_000_000);
			let genesis_head = Registrar::worst_head_data();
			let validation_code = Registrar::worst_validation_code();
			assert_ok!(Registrar::reserve(signed(n)));
			assert_ok!(Registrar::register(
				signed(n),
				para_id + n - 1,
				genesis_head,
				validation_code,
			));
		}

		// Start a new auction in the future
		let duration = 149u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// Paras should be onboarded
		run_to_block(20); // session 2

		for n in 1..=max_bids {
			// Increment block number
			run_to_block(System::block_number() + 10);

			Balances::make_free_balance_be(&account_id(n * 10), n * 1_000);

			let (start, end) = match n {
				1 => (0, 0),
				2 => (0, 1),
				3 => (0, 2),
				4 => (0, 3),
				5 => (1, 1),
				6 => (1, 2),
				7 => (1, 3),
				8 => (2, 2),
				9 => (2, 3),
				10 => (3, 3),
				_ => panic!("test not meant for this"),
			};

			// Users will bid directly for parachain
			assert_ok!(Auctions::bid(
				signed(n * 10),
				para_id + n - 1,
				1,                                // Auction Index
				lease_period_index_start + start, // First Slot
				lease_period_index_start + end,   // Last slot
				n * 900,                          // Amount
			));
		}

		// Auction should be done after ending period
		run_to_block(160);

		// Appropriate Paras should have won slots
		// 900 + 4500 + 2x 8100 = 21,600
		// 900 + 4500 + 7200 + 9000 = 21,600
		assert_eq!(
			slots::Leases::<Test>::get(para_id),
			// -- 1 --- 2 --- 3 ---------- 4 ------
			vec![None, None, None, Some((account_id(10), 900))],
		);
		assert_eq!(
			slots::Leases::<Test>::get(para_id + 4),
			// -- 1 --- 2 --- 3 --- 4 ---------- 5 -------
			vec![None, None, None, None, Some((account_id(50), 4500))],
		);
		// TODO: Is this right?
		assert_eq!(
			slots::Leases::<Test>::get(para_id + 8),
			// -- 1 --- 2 --- 3 --- 4 --- 5 ---------- 6 --------------- 7 -------
			vec![
				None,
				None,
				None,
				None,
				None,
				Some((account_id(90), 8100)),
				Some((account_id(90), 8100))
			],
		);
	});
}

#[test]
fn competing_bids() {
	// This test will verify that competing bids, from different sources will resolve appropriately.
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one());

		let start_para = LOWEST_PUBLIC_ID - 1;
		// Create 3 paras and owners
		for n in 1..=3 {
			Balances::make_free_balance_be(&account_id(n), 1_000_000_000);
			let genesis_head = Registrar::worst_head_data();
			let validation_code = Registrar::worst_validation_code();
			assert_ok!(Registrar::reserve(signed(n)));
			assert_ok!(Registrar::register(
				signed(n),
				ParaId::from(start_para + n),
				genesis_head,
				validation_code,
			));
		}

		// Finish registration of paras.
		run_to_session(2);

		// Start a new auction in the future
		let starting_block = System::block_number();
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		for n in 1..=3 {
			// Create a crowdloan for each para
			assert_ok!(Crowdloan::create(
				signed(n),
				ParaId::from(start_para + n),
				100_000,                      // Cap
				lease_period_index_start + 2, // First Slot
				lease_period_index_start + 3, // Last Slot
				200,                          // Block End,
				None,
			));
		}

		for n in 1..=9 {
			// Increment block number
			run_to_block(starting_block + n * 10);

			Balances::make_free_balance_be(&account_id(n * 10), n * 1_000);

			let para = start_para + n % 3 + 1;

			if n % 2 == 0 {
				// User 10 will bid directly for parachain 1
				assert_ok!(Auctions::bid(
					signed(n * 10),
					ParaId::from(para),
					1,                            // Auction Index
					lease_period_index_start + 0, // First Slot
					lease_period_index_start + 1, // Last slot
					n * 900,                      // Amount
				));
			} else {
				// User 20 will be a contribute to crowdloan for parachain 2
				assert_ok!(Crowdloan::contribute(
					signed(n * 10),
					ParaId::from(para),
					n + 900,
					None,
				));
			}
		}

		// Auction should be done
		run_to_block(starting_block + 110);

		// Appropriate Paras should have won slots
		let fund_1 = Crowdloan::funds(ParaId::from(2000)).unwrap();
		let crowdloan_1 = Crowdloan::fund_account_id(fund_1.fund_index);
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(2000)),
			// -- 1 --- 2 --- 3 --- 4 --- 5 ------------- 6 ------------------------ 7 -------------
			vec![
				None,
				None,
				None,
				None,
				None,
				Some((crowdloan_1.clone(), 1812)),
				Some((crowdloan_1.clone(), 1812))
			],
		);
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(2002)),
			// -- 1 --- 2 --- 3 ---------- 4 --------------- 5 -------
			vec![None, None, None, Some((account_id(80), 7200)), Some((account_id(80), 7200))],
		);
	});
}

#[test]
fn basic_swap_works() {
	// This test will test a swap between a parachain and parathread works successfully.
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one()); /* So events are emitted */
		// User 1 and 2 will own paras
		Balances::make_free_balance_be(&account_id(1), 1_000_000_000);
		Balances::make_free_balance_be(&account_id(2), 1_000_000_000);
		// First register 2 parathreads with different data
		assert_ok!(Registrar::reserve(signed(1)));
		assert_ok!(Registrar::register(
			signed(1),
			ParaId::from(2000),
			test_genesis_head(10),
			test_validation_code(10),
		));
		assert_ok!(Registrar::reserve(signed(2)));
		assert_ok!(Registrar::register(
			signed(2),
			ParaId::from(2001),
			test_genesis_head(20),
			test_validation_code(20),
		));

		// Paras should be onboarding
		assert_eq!(Paras::lifecycle(ParaId::from(2000)), Some(ParaLifecycle::Onboarding));
		assert_eq!(Paras::lifecycle(ParaId::from(2001)), Some(ParaLifecycle::Onboarding));

		// Start a new auction in the future
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// 2 sessions later they are parathreads
		run_to_session(2);
		assert_eq!(Paras::lifecycle(ParaId::from(2000)), Some(ParaLifecycle::Parathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2001)), Some(ParaLifecycle::Parathread));

		// Open a crowdloan for Para 1 for slots 0-3
		assert_ok!(Crowdloan::create(
			signed(1),
			ParaId::from(2000),
			1_000_000,                    // Cap
			lease_period_index_start + 0, // First Slot
			lease_period_index_start + 3, // Last Slot
			200,                          // Block End
			None,
		));
		let fund = Crowdloan::funds(ParaId::from(2000)).unwrap();
		let crowdloan_account = Crowdloan::fund_account_id(fund.fund_index);

		// Bunch of contributions
		let mut total = 0;
		for i in 10..20 {
			Balances::make_free_balance_be(&account_id(i), 1_000_000_000);
			assert_ok!(Crowdloan::contribute(signed(i), ParaId::from(2000), 900 - i, None));
			total += 900 - i;
		}
		assert!(total > 0);
		assert_eq!(Balances::free_balance(&crowdloan_account), total);

		// Go to end of auction where everyone won their slots
		run_to_block(200);

		// Deposit is appropriately taken
		// ----------------------------------------- para deposit --- crowdloan
		assert_eq!(Balances::reserved_balance(&account_id(1)), (500 + 10 * 2 * 1) + 100);
		assert_eq!(Balances::reserved_balance(&account_id(2)), 500 + 20 * 2 * 1);
		assert_eq!(Balances::reserved_balance(&crowdloan_account), total);
		// Crowdloan is appropriately set
		assert!(Crowdloan::funds(ParaId::from(2000)).is_some());
		assert!(Crowdloan::funds(ParaId::from(2001)).is_none());

		// New leases will start on block 400
		let lease_start_block = 400;
		run_to_block(lease_start_block);

		// Slots are won by Para 1
		assert!(!Slots::lease(ParaId::from(2000)).is_empty());
		assert!(Slots::lease(ParaId::from(2001)).is_empty());

		// 2 sessions later it is a parachain
		run_to_block(lease_start_block + 20);
		assert_eq!(Paras::lifecycle(ParaId::from(2000)), Some(ParaLifecycle::Parachain));
		assert_eq!(Paras::lifecycle(ParaId::from(2001)), Some(ParaLifecycle::Parathread));

		// Initiate a swap
		assert_ok!(Registrar::swap(
			para_origin(2000).into(),
			ParaId::from(2000),
			ParaId::from(2001)
		));
		assert_ok!(Registrar::swap(
			para_origin(2001).into(),
			ParaId::from(2001),
			ParaId::from(2000)
		));

		assert_eq!(Paras::lifecycle(ParaId::from(2000)), Some(ParaLifecycle::DowngradingParachain));
		assert_eq!(Paras::lifecycle(ParaId::from(2001)), Some(ParaLifecycle::UpgradingParathread));

		// 2 session later they have swapped
		run_to_block(lease_start_block + 40);
		assert_eq!(Paras::lifecycle(ParaId::from(2000)), Some(ParaLifecycle::Parathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2001)), Some(ParaLifecycle::Parachain));

		// Deregister parathread
		assert_ok!(Registrar::deregister(para_origin(2000).into(), ParaId::from(2000)));
		// Correct deposit is unreserved
		assert_eq!(Balances::reserved_balance(&account_id(1)), 100); // crowdloan deposit left over
		assert_eq!(Balances::reserved_balance(&account_id(2)), 500 + 20 * 2 * 1);
		// Crowdloan ownership is swapped
		assert!(Crowdloan::funds(ParaId::from(2000)).is_none());
		assert!(Crowdloan::funds(ParaId::from(2001)).is_some());
		// Slot is swapped
		assert!(Slots::lease(ParaId::from(2000)).is_empty());
		assert!(!Slots::lease(ParaId::from(2001)).is_empty());

		// Cant dissolve
		assert_noop!(
			Crowdloan::dissolve(signed(1), ParaId::from(2000)),
			CrowdloanError::<Test>::InvalidParaId
		);
		assert_noop!(
			Crowdloan::dissolve(signed(2), ParaId::from(2001)),
			CrowdloanError::<Test>::NotReadyToDissolve
		);

		// Go way in the future when the para is offboarded
		run_to_block(lease_start_block + 1000);

		// Withdraw of contributions works
		assert_eq!(Balances::free_balance(&crowdloan_account), total);
		for i in 10..20 {
			assert_ok!(Crowdloan::withdraw(signed(i), account_id(i), ParaId::from(2001)));
		}
		assert_eq!(Balances::free_balance(&crowdloan_account), 0);

		// Dissolve returns the balance of the person who put a deposit for crowdloan
		assert_ok!(Crowdloan::dissolve(signed(1), ParaId::from(2001)));
		assert_eq!(Balances::reserved_balance(&account_id(1)), 0);
		assert_eq!(Balances::reserved_balance(&account_id(2)), 500 + 20 * 2 * 1);

		// Final deregister sets everything back to the start
		assert_ok!(Registrar::deregister(para_origin(2001).into(), ParaId::from(2001)));
		assert_eq!(Balances::reserved_balance(&account_id(2)), 0);
	})
}

#[test]
fn parachain_swap_works() {
	// This test will test a swap between two parachains works successfully.
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one()); /* So events are emitted */
		// User 1 and 2 will own paras
		Balances::make_free_balance_be(&account_id(1), 1_000_000_000);
		Balances::make_free_balance_be(&account_id(2), 1_000_000_000);
		// First register 2 parathreads with different data
		assert_ok!(Registrar::reserve(signed(1)));
		assert_ok!(Registrar::register(
			signed(1),
			ParaId::from(2000),
			test_genesis_head(10),
			test_validation_code(10),
		));
		assert_ok!(Registrar::reserve(signed(2)));
		assert_ok!(Registrar::register(
			signed(2),
			ParaId::from(2001),
			test_genesis_head(20),
			test_validation_code(20),
		));

		// Paras should be onboarding
		assert_eq!(Paras::lifecycle(ParaId::from(2000)), Some(ParaLifecycle::Onboarding));
		assert_eq!(Paras::lifecycle(ParaId::from(2001)), Some(ParaLifecycle::Onboarding));

		assert_eq!(
			Balances::total_balance(&Crowdloan::fund_account_id(Crowdloan::next_fund_index())),
			0
		);

		// Start a new auction in the future
		let start_auction = |lease_period_index_start, winner, end| {
			let unique_id = winner - 1999u32;
			let starting_block = System::block_number();
			let duration = 99u32;
			assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

			// 2 sessions later they are parathreads
			run_to_block(starting_block + 20);
			assert_eq!(Paras::lifecycle(ParaId::from(winner)), Some(ParaLifecycle::Parathread));

			// Open a crowdloan for Para 1 for slots 0-3
			assert_ok!(Crowdloan::create(
				signed(unique_id),
				ParaId::from(winner),
				1_000_000,                    // Cap
				lease_period_index_start + 0, // First Slot
				lease_period_index_start + 7, // Last Slot
				end,                          // Block End
				None,
			));
			let winner_fund = Crowdloan::funds(ParaId::from(winner)).unwrap();
			let crowdloan_account = Crowdloan::fund_account_id(winner_fund.fund_index);

			// Bunch of contributions
			let mut total = 0;
			for i in (unique_id * 10)..(unique_id + 1) * 10 {
				Balances::make_free_balance_be(&account_id(i), 1_000_000_000);
				assert_ok!(Crowdloan::contribute(signed(i), ParaId::from(winner), 900 - i, None));
				total += 900 - i;
			}
			assert!(total > 0);
			assert_eq!(Balances::free_balance(&crowdloan_account), total);

			// Go to end of auction where everyone won their slots
			run_to_block(end);

			// Crowdloan is appropriately set
			assert!(Crowdloan::funds(ParaId::from(winner)).is_some());

			// New leases will start on block lease period index * 100
			let lease_start_block = lease_period_index_start * 100;
			run_to_block(lease_start_block);
		};

		start_auction(4u32, 2000, 200);
		// Slots are won by Para 1
		assert!(!Slots::lease(ParaId::from(2000)).is_empty());
		assert!(Slots::lease(ParaId::from(2001)).is_empty());

		// 2 sessions later it is a parachain
		run_to_block(4 * 100 + 20);
		assert_eq!(Paras::lifecycle(ParaId::from(2000)), Some(ParaLifecycle::Parachain));
		assert_eq!(Paras::lifecycle(ParaId::from(2001)), Some(ParaLifecycle::Parathread));

		// Let's repeat the process now for another parachain.
		start_auction(6u32, 2001, 500);
		// Slots are won by Para 1
		assert!(!Slots::lease(ParaId::from(2000)).is_empty());
		assert!(!Slots::lease(ParaId::from(2001)).is_empty());

		// 2 sessions later it is a parachain
		run_to_block(6 * 100 + 20);
		assert_eq!(Paras::lifecycle(ParaId::from(2000)), Some(ParaLifecycle::Parachain));
		assert_eq!(Paras::lifecycle(ParaId::from(2001)), Some(ParaLifecycle::Parachain));

		// Currently we are on lease 6
		assert_eq!(
			<Slots as Leaser<_>>::lease_period_index(System::block_number()),
			Some((6u32, false))
		);

		// This means that parachain 1 should only have 6 slots left, and parachain 2 has all 8.
		assert_eq!(slots::Leases::<Test>::get(ParaId::from(2000)).len(), 6);
		assert_eq!(slots::Leases::<Test>::get(ParaId::from(2001)).len(), 8);

		let fund_2000 = Crowdloan::funds(ParaId::from(2000)).unwrap();
		assert_eq!(fund_2000.fund_index, 0);
		assert_eq!(
			Balances::reserved_balance(&Crowdloan::fund_account_id(fund_2000.fund_index)),
			fund_2000.raised
		);

		let fund_2001 = Crowdloan::funds(ParaId::from(2001)).unwrap();
		assert_eq!(fund_2001.fund_index, 1);
		assert_eq!(
			Balances::reserved_balance(&Crowdloan::fund_account_id(fund_2001.fund_index)),
			fund_2001.raised
		);

		assert_eq!(Slots::lease(ParaId::from(2000)).len(), 6);
		assert_eq!(Slots::lease(ParaId::from(2001)).len(), 8);

		// Now we swap them.
		assert_ok!(Registrar::swap(
			para_origin(2000).into(),
			ParaId::from(2000),
			ParaId::from(2001)
		));
		assert_ok!(Registrar::swap(
			para_origin(2001).into(),
			ParaId::from(2001),
			ParaId::from(2000)
		));

		// Crowdloan Swapped
		let fund_2000 = Crowdloan::funds(ParaId::from(2000)).unwrap();
		assert_eq!(fund_2000.fund_index, 1);
		assert_eq!(
			Balances::reserved_balance(&Crowdloan::fund_account_id(fund_2000.fund_index)),
			fund_2000.raised
		);

		let fund_2001 = Crowdloan::funds(ParaId::from(2001)).unwrap();
		assert_eq!(fund_2001.fund_index, 0);
		assert_eq!(
			Balances::reserved_balance(&Crowdloan::fund_account_id(fund_2001.fund_index)),
			fund_2001.raised
		);

		// Slots Swapped
		assert_eq!(Slots::lease(ParaId::from(2000)).len(), 8);
		assert_eq!(Slots::lease(ParaId::from(2001)).len(), 6);
	})
}

#[test]
fn crowdloan_ending_period_bid() {
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one()); /* So events are emitted */
		// User 1 and 2 will own paras
		Balances::make_free_balance_be(&account_id(1), 1_000_000_000);
		Balances::make_free_balance_be(&account_id(2), 1_000_000_000);
		// First register 2 parathreads
		assert_ok!(Registrar::reserve(signed(1)));
		assert_ok!(Registrar::register(
			signed(1),
			ParaId::from(2000),
			test_genesis_head(10),
			test_validation_code(10),
		));
		assert_ok!(Registrar::reserve(signed(2)));
		assert_ok!(Registrar::register(
			signed(2),
			ParaId::from(2001),
			test_genesis_head(20),
			test_validation_code(20),
		));

		// Paras should be onboarding
		assert_eq!(Paras::lifecycle(ParaId::from(2000)), Some(ParaLifecycle::Onboarding));
		assert_eq!(Paras::lifecycle(ParaId::from(2001)), Some(ParaLifecycle::Onboarding));

		// Start a new auction in the future
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// 2 sessions later they are parathreads
		run_to_session(2);
		assert_eq!(Paras::lifecycle(ParaId::from(2000)), Some(ParaLifecycle::Parathread));
		assert_eq!(Paras::lifecycle(ParaId::from(2001)), Some(ParaLifecycle::Parathread));

		// Open a crowdloan for Para 1 for slots 0-3
		assert_ok!(Crowdloan::create(
			signed(1),
			ParaId::from(2000),
			1_000_000,                    // Cap
			lease_period_index_start + 0, // First Slot
			lease_period_index_start + 3, // Last Slot
			200,                          // Block End
			None,
		));
		let fund = Crowdloan::funds(ParaId::from(2000)).unwrap();
		let crowdloan_account = Crowdloan::fund_account_id(fund.fund_index);

		// Bunch of contributions
		let mut total = 0;
		for i in 10..20 {
			Balances::make_free_balance_be(&account_id(i), 1_000_000_000);
			assert_ok!(Crowdloan::contribute(signed(i), ParaId::from(2000), 900 - i, None));
			total += 900 - i;
		}
		assert!(total > 0);
		assert_eq!(Balances::free_balance(&crowdloan_account), total);

		// Bid for para 2 directly
		Balances::make_free_balance_be(&account_id(2), 1_000_000_000);
		assert_ok!(Auctions::bid(
			signed(2),
			ParaId::from(2001),
			1,                            // Auction Index
			lease_period_index_start + 0, // First Slot
			lease_period_index_start + 1, // Last slot
			900,                          // Amount
		));

		// Go to beginning of ending period
		run_to_block(100);

		assert_eq!(Auctions::auction_status(100), AuctionStatus::<u32>::EndingPeriod(0, 0));
		let mut winning = [(); SlotRange::SLOT_RANGE_COUNT].map(|_| None);

		winning[SlotRange::ZeroOne as u8 as usize] = Some((account_id(2), ParaId::from(2001), 900));
		winning[SlotRange::ZeroThree as u8 as usize] =
			Some((crowdloan_account.clone(), ParaId::from(2000), total));

		assert_eq!(Auctions::winning(0), Some(winning));

		run_to_block(101);

		Balances::make_free_balance_be(&account_id(1234), 1_000_000_000);
		assert_ok!(Crowdloan::contribute(signed(1234), ParaId::from(2000), 900, None));

		// Data propagates correctly
		run_to_block(102);
		let mut winning = [(); SlotRange::SLOT_RANGE_COUNT].map(|_| None);
		winning[SlotRange::ZeroOne as u8 as usize] = Some((account_id(2), ParaId::from(2001), 900));
		winning[SlotRange::ZeroThree as u8 as usize] =
			Some((crowdloan_account.clone(), ParaId::from(2000), total + 900));
		assert_eq!(Auctions::winning(2), Some(winning));
	})
}

#[test]
fn auction_bid_requires_registered_para() {
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one()); /* So events are emitted */

		// Start a new auction in the future
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// Can't bid with non-registered paras
		Balances::make_free_balance_be(&account_id(1), 1_000_000_000);
		assert_noop!(
			Auctions::bid(
				signed(1),
				ParaId::from(2000),
				1,                            // Auction Index
				lease_period_index_start + 0, // First Slot
				lease_period_index_start + 1, // Last slot
				900,                          // Amount
			),
			AuctionsError::<Test>::ParaNotRegistered
		);

		// Now we register the para
		assert_ok!(Registrar::reserve(signed(1)));
		assert_ok!(Registrar::register(
			signed(1),
			ParaId::from(2000),
			test_genesis_head(10),
			test_validation_code(10),
		));

		// Still can't bid until it is fully onboarded
		assert_noop!(
			Auctions::bid(
				signed(1),
				ParaId::from(2000),
				1,                            // Auction Index
				lease_period_index_start + 0, // First Slot
				lease_period_index_start + 1, // Last slot
				900,                          // Amount
			),
			AuctionsError::<Test>::ParaNotRegistered
		);

		// Onboarded on Session 2
		run_to_session(2);

		// Success
		Balances::make_free_balance_be(&account_id(1), 1_000_000_000);
		assert_ok!(Auctions::bid(
			signed(1),
			ParaId::from(2000),
			1,                            // Auction Index
			lease_period_index_start + 0, // First Slot
			lease_period_index_start + 1, // Last slot
			900,                          // Amount
		));
	});
}

#[test]
fn gap_bids_work() {
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one()); /* So events are emitted */

		// Start a new auction in the future
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));
		Balances::make_free_balance_be(&account_id(1), 1_000_000_000);
		Balances::make_free_balance_be(&account_id(2), 1_000_000_000);

		// Now register 2 paras
		assert_ok!(Registrar::reserve(signed(1)));
		assert_ok!(Registrar::register(
			signed(1),
			ParaId::from(2000),
			test_genesis_head(10),
			test_validation_code(10),
		));
		assert_ok!(Registrar::reserve(signed(2)));
		assert_ok!(Registrar::register(
			signed(2),
			ParaId::from(2001),
			test_genesis_head(10),
			test_validation_code(10),
		));

		// Onboarded on Session 2
		run_to_session(2);

		// Make bids
		Balances::make_free_balance_be(&account_id(10), 1_000_000_000);
		Balances::make_free_balance_be(&account_id(20), 1_000_000_000);
		// Slot 1 for 100 from 10
		assert_ok!(Auctions::bid(
			signed(10),
			ParaId::from(2000),
			1,                            // Auction Index
			lease_period_index_start + 0, // First Slot
			lease_period_index_start + 0, // Last slot
			100,                          // Amount
		));
		// Slot 4 for 400 from 10
		assert_ok!(Auctions::bid(
			signed(10),
			ParaId::from(2000),
			1,                            // Auction Index
			lease_period_index_start + 3, // First Slot
			lease_period_index_start + 3, // Last slot
			400,                          // Amount
		));

		// A bid for another para is counted separately.
		assert_ok!(Auctions::bid(
			signed(10),
			ParaId::from(2001),
			1,                            // Auction Index
			lease_period_index_start + 1, // First Slot
			lease_period_index_start + 1, // Last slot
			555,                          // Amount
		));
		assert_eq!(Balances::reserved_balance(&account_id(10)), 400 + 555);

		// Slot 2 for 800 from 20, overtaking 10's bid
		assert_ok!(Auctions::bid(
			signed(20),
			ParaId::from(2000),
			1,                            // Auction Index
			lease_period_index_start + 1, // First Slot
			lease_period_index_start + 1, // Last slot
			800,                          // Amount
		));
		// Slot 3 for 200 from 20
		assert_ok!(Auctions::bid(
			signed(20),
			ParaId::from(2000),
			1,                            // Auction Index
			lease_period_index_start + 2, // First Slot
			lease_period_index_start + 2, // Last slot
			200,                          // Amount
		));

		// Finish the auction
		run_to_block(110);

		// Should have won the lease periods
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(2000)),
			// -- 1 --- 2 --- 3 ---------- 4 -------------- 5 -------------- 6 -------------- 7 -------
			vec![
				None,
				None,
				None,
				Some((account_id(10), 100)),
				Some((account_id(20), 800)),
				Some((account_id(20), 200)),
				Some((account_id(10), 400))
			],
		);
		// Appropriate amount is reserved (largest of the values)
		assert_eq!(Balances::reserved_balance(&account_id(10)), 400);
		// Appropriate amount is reserved (largest of the values)
		assert_eq!(Balances::reserved_balance(&account_id(20)), 800);

		// Progress through the leases and note the correct amount of balance is reserved.

		run_to_block(400);
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(2000)),
			// --------- 4 -------------- 5 -------------- 6 -------------- 7 -------
			vec![
				Some((account_id(10), 100)),
				Some((account_id(20), 800)),
				Some((account_id(20), 200)),
				Some((account_id(10), 400))
			],
		);
		// Nothing changed.
		assert_eq!(Balances::reserved_balance(&account_id(10)), 400);
		assert_eq!(Balances::reserved_balance(&account_id(20)), 800);

		// Lease period 4 is done, but nothing is unreserved since user 1 has a debt on lease 7
		run_to_block(500);
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(2000)),
			// --------- 5 -------------- 6 -------------- 7 -------
			vec![
				Some((account_id(20), 800)),
				Some((account_id(20), 200)),
				Some((account_id(10), 400))
			],
		);
		// Nothing changed.
		assert_eq!(Balances::reserved_balance(&account_id(10)), 400);
		assert_eq!(Balances::reserved_balance(&account_id(20)), 800);

		// Lease period 5 is done, and 20 will unreserve down to 200.
		run_to_block(600);
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(2000)),
			// --------- 6 -------------- 7 -------
			vec![Some((account_id(20), 200)), Some((account_id(10), 400))],
		);
		assert_eq!(Balances::reserved_balance(&account_id(10)), 400);
		assert_eq!(Balances::reserved_balance(&account_id(20)), 200);

		// Lease period 6 is done, and 20 will unreserve everything.
		run_to_block(700);
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(2000)),
			// --------- 7 -------
			vec![Some((account_id(10), 400))],
		);
		assert_eq!(Balances::reserved_balance(&account_id(10)), 400);
		assert_eq!(Balances::reserved_balance(&account_id(20)), 0);

		// All leases are done. Everything is unreserved.
		run_to_block(800);
		assert_eq!(slots::Leases::<Test>::get(ParaId::from(2000)), vec![]);
		assert_eq!(Balances::reserved_balance(&account_id(10)), 0);
		assert_eq!(Balances::reserved_balance(&account_id(20)), 0);
	});
}

// This test verifies that if a parachain already has won some lease periods, that it cannot bid for
// any of those same lease periods again.
#[test]
fn cant_bid_on_existing_lease_periods() {
	new_test_ext().execute_with(|| {
		assert!(System::block_number().is_one()); /* So events are emitted */
		Balances::make_free_balance_be(&account_id(1), 1_000_000_000);
		// First register a parathread
		assert_ok!(Registrar::reserve(signed(1)));
		assert_ok!(Registrar::register(
			signed(1),
			ParaId::from(2000),
			test_genesis_head(10),
			test_validation_code(10),
		));

		// Start a new auction in the future
		let starting_block = System::block_number();
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// 2 sessions later they are parathreads
		run_to_session(2);

		// Open a crowdloan for Para 1 for slots 0-3
		assert_ok!(Crowdloan::create(
			signed(1),
			ParaId::from(2000),
			1_000_000,                    // Cap
			lease_period_index_start + 0, // First Slot
			lease_period_index_start + 1, // Last Slot
			400,                          // Long block end
			None,
		));
		let fund = Crowdloan::funds(ParaId::from(2000)).unwrap();
		let crowdloan_account = Crowdloan::fund_account_id(fund.fund_index);

		// Bunch of contributions
		let mut total = 0;
		for i in 10..20 {
			Balances::make_free_balance_be(&account_id(i), 1_000_000_000);
			assert_ok!(Crowdloan::contribute(signed(i), ParaId::from(2000), 900 - i, None));
			total += 900 - i;
		}
		assert!(total > 0);
		assert_eq!(Balances::free_balance(&crowdloan_account), total);

		// Finish the auction.
		run_to_block(starting_block + 110);

		// Appropriate Paras should have won slots
		assert_eq!(
			slots::Leases::<Test>::get(ParaId::from(2000)),
			// -- 1 --- 2 --- 3 ------------- 4 ------------------------ 5 -------------
			vec![
				None,
				None,
				None,
				Some((crowdloan_account.clone(), 8855)),
				Some((crowdloan_account.clone(), 8855))
			],
		);

		// Let's start another auction for the same range
		let starting_block = System::block_number();
		let duration = 99u32;
		let lease_period_index_start = 4u32;
		assert_ok!(Auctions::new_auction(Origin::root(), duration, lease_period_index_start));

		// Poke the crowdloan into `NewRaise`
		assert_ok!(Crowdloan::poke(signed(1), ParaId::from(2000)));
		assert_eq!(Crowdloan::new_raise(), vec![ParaId::from(2000)]);

		// Beginning of ending block.
		run_to_block(starting_block + 100);

		// Bids cannot be made which intersect
		assert_noop!(
			Auctions::bid(
				Origin::signed(crowdloan_account.clone()),
				ParaId::from(2000),
				2,
				lease_period_index_start + 0,
				lease_period_index_start + 1,
				100,
			),
			AuctionsError::<Test>::AlreadyLeasedOut,
		);

		assert_noop!(
			Auctions::bid(
				Origin::signed(crowdloan_account.clone()),
				ParaId::from(2000),
				2,
				lease_period_index_start + 1,
				lease_period_index_start + 2,
				100,
			),
			AuctionsError::<Test>::AlreadyLeasedOut,
		);

		assert_noop!(
			Auctions::bid(
				Origin::signed(crowdloan_account.clone()),
				ParaId::from(2000),
				2,
				lease_period_index_start - 1,
				lease_period_index_start + 0,
				100,
			),
			AuctionsError::<Test>::AlreadyLeasedOut,
		);

		assert_noop!(
			Auctions::bid(
				Origin::signed(crowdloan_account.clone()),
				ParaId::from(2000),
				2,
				lease_period_index_start + 0,
				lease_period_index_start + 0,
				100,
			),
			AuctionsError::<Test>::AlreadyLeasedOut,
		);

		assert_noop!(
			Auctions::bid(
				Origin::signed(crowdloan_account.clone()),
				ParaId::from(2000),
				2,
				lease_period_index_start + 1,
				lease_period_index_start + 1,
				100,
			),
			AuctionsError::<Test>::AlreadyLeasedOut,
		);

		assert_noop!(
			Auctions::bid(
				Origin::signed(crowdloan_account.clone()),
				ParaId::from(2000),
				2,
				lease_period_index_start - 1,
				lease_period_index_start + 5,
				100,
			),
			AuctionsError::<Test>::AlreadyLeasedOut,
		);

		// Will work when not overlapping
		assert_ok!(Auctions::bid(
			Origin::signed(crowdloan_account.clone()),
			ParaId::from(2000),
			2,
			lease_period_index_start + 2,
			lease_period_index_start + 3,
			100,
		));
	});
}
