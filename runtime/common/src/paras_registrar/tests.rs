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

use super::*;
use crate::{mock::conclude_pvf_checking, paras_registrar, traits::Registrar as RegistrarTrait};
use frame_support::{
	assert_noop, assert_ok,
	error::BadOrigin,
	parameter_types,
	traits::{ConstU32, OnFinalize, OnInitialize},
};
use frame_system::limits;
use pallet_balances::Error as BalancesError;
use primitives::{Balance, BlockNumber, SessionIndex};
use runtime_parachains::{configuration, origin, shared};
use sp_core::H256;
use sp_io::TestExternalities;
use sp_keyring::Sr25519Keyring;
use sp_runtime::{
	traits::{BlakeTwo256, IdentityLookup},
	transaction_validity::TransactionPriority,
	BuildStorage, Perbill,
};
use sp_std::collections::btree_map::BTreeMap;

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlockU32<Test>;

frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system::{Pallet, Call, Config<T>, Storage, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
		Configuration: configuration::{Pallet, Call, Storage, Config<T>},
		Parachains: paras::{Pallet, Call, Storage, Config<T>, Event},
		ParasShared: shared::{Pallet, Call, Storage},
		Registrar: paras_registrar::{Pallet, Call, Storage, Event<T>},
		ParachainsOrigin: origin::{Pallet, Origin},
	}
);

impl<C> frame_system::offchain::SendTransactionTypes<C> for Test
where
	RuntimeCall: From<C>,
{
	type Extrinsic = UncheckedExtrinsic;
	type OverarchingCall = RuntimeCall;
}

const NORMAL_RATIO: Perbill = Perbill::from_percent(75);
parameter_types! {
	pub const BlockHashCount: u32 = 250;
	pub BlockWeights: limits::BlockWeights =
		frame_system::limits::BlockWeights::simple_max(Weight::from_parts(1024, u64::MAX));
	pub BlockLength: limits::BlockLength =
		limits::BlockLength::max_with_normal_ratio(4 * 1024 * 1024, NORMAL_RATIO);
}

impl frame_system::Config for Test {
	type BaseCallFilter = frame_support::traits::Everything;
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeCall = RuntimeCall;
	type Nonce = u64;
	type Hash = H256;
	type Hashing = BlakeTwo256;
	type AccountId = u64;
	type Lookup = IdentityLookup<u64>;
	type Block = Block;
	type RuntimeEvent = RuntimeEvent;
	type BlockHashCount = BlockHashCount;
	type DbWeight = ();
	type BlockWeights = BlockWeights;
	type BlockLength = BlockLength;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = pallet_balances::AccountData<u128>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

parameter_types! {
	pub const ExistentialDeposit: Balance = 1;
}

impl pallet_balances::Config for Test {
	type Balance = u128;
	type DustRemoval = ();
	type RuntimeEvent = RuntimeEvent;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = System;
	type MaxLocks = ();
	type MaxReserves = ();
	type ReserveIdentifier = [u8; 8];
	type WeightInfo = ();
	type RuntimeHoldReason = RuntimeHoldReason;
	type FreezeIdentifier = ();
	type MaxHolds = ConstU32<1>;
	type MaxFreezes = ConstU32<1>;
}

impl shared::Config for Test {}

impl origin::Config for Test {}

parameter_types! {
	pub const ParasUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
}

impl paras::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = paras::TestWeightInfo;
	type UnsignedPriority = ParasUnsignedPriority;
	type QueueFootprinter = ();
	type NextSessionRotation = crate::mock::TestNextSessionRotation;
	type OnNewHead = ();
}

impl configuration::Config for Test {
	type WeightInfo = configuration::TestWeightInfo;
}

parameter_types! {
	pub const ParaDeposit: Balance = 10;
	pub const DataDepositPerByte: Balance = 1;
	pub const MaxRetries: u32 = 3;
}

impl Config for Test {
	type RuntimeOrigin = RuntimeOrigin;
	type RuntimeEvent = RuntimeEvent;
	type Currency = Balances;
	type OnSwap = MockSwap;
	type ParaDeposit = ParaDeposit;
	type DataDepositPerByte = DataDepositPerByte;
	type WeightInfo = TestWeightInfo;
}

pub fn new_test_ext() -> TestExternalities {
	let mut t = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();

	configuration::GenesisConfig::<Test> {
		config: configuration::HostConfiguration {
			max_code_size: 2 * 1024 * 1024,      // 2 MB
			max_head_data_size: 1 * 1024 * 1024, // 1 MB
			..Default::default()
		},
	}
	.assimilate_storage(&mut t)
	.unwrap();

	pallet_balances::GenesisConfig::<Test> {
		balances: vec![(1, 10_000_000), (2, 10_000_000), (3, 10_000_000)],
	}
	.assimilate_storage(&mut t)
	.unwrap();

	t.into()
}

parameter_types! {
	pub static SwapData: BTreeMap<ParaId, u64> = BTreeMap::new();
}

pub struct MockSwap;
impl OnSwap for MockSwap {
	fn on_swap(one: ParaId, other: ParaId) {
		let mut swap_data = SwapData::get();
		let one_data = swap_data.remove(&one).unwrap_or_default();
		let other_data = swap_data.remove(&other).unwrap_or_default();
		swap_data.insert(one, other_data);
		swap_data.insert(other, one_data);
		SwapData::set(swap_data);
	}
}

const BLOCKS_PER_SESSION: u32 = 3;

const VALIDATORS: &[Sr25519Keyring] = &[
	Sr25519Keyring::Alice,
	Sr25519Keyring::Bob,
	Sr25519Keyring::Charlie,
	Sr25519Keyring::Dave,
	Sr25519Keyring::Ferdie,
];

fn run_to_block(n: BlockNumber) {
	// NOTE that this function only simulates modules of interest. Depending on new pallet may
	// require adding it here.
	assert!(System::block_number() < n);
	while System::block_number() < n {
		let b = System::block_number();

		if System::block_number() > 1 {
			System::on_finalize(System::block_number());
		}
		// Session change every 3 blocks.
		if (b + 1) % BLOCKS_PER_SESSION == 0 {
			let session_index = shared::Pallet::<Test>::session_index() + 1;
			let validators_pub_keys = VALIDATORS.iter().map(|v| v.public().into()).collect();

			shared::Pallet::<Test>::set_session_index(session_index);
			shared::Pallet::<Test>::set_active_validators_ascending(validators_pub_keys);

			Parachains::test_on_new_session();
		}
		System::set_block_number(b + 1);
		System::on_initialize(System::block_number());
	}
}

fn run_to_session(n: BlockNumber) {
	let block_number = n * BLOCKS_PER_SESSION;
	run_to_block(block_number);
}

fn test_genesis_head(size: usize) -> HeadData {
	HeadData(vec![0u8; size])
}

fn test_validation_code(size: usize) -> ValidationCode {
	let validation_code = vec![0u8; size as usize];
	ValidationCode(validation_code)
}

fn para_origin(id: ParaId) -> RuntimeOrigin {
	runtime_parachains::Origin::Parachain(id).into()
}

fn max_code_size() -> u32 {
	Configuration::config().max_code_size
}

fn max_head_size() -> u32 {
	Configuration::config().max_head_data_size
}

#[test]
fn basic_setup_works() {
	new_test_ext().execute_with(|| {
		assert_eq!(PendingSwap::<Test>::get(&ParaId::from(0u32)), None);
		assert_eq!(Paras::<Test>::get(&ParaId::from(0u32)), None);
	});
}

#[test]
fn end_to_end_scenario_works() {
	new_test_ext().execute_with(|| {
		let para_id = LOWEST_PUBLIC_ID;

		const START_SESSION_INDEX: SessionIndex = 1;
		run_to_session(START_SESSION_INDEX);

		// first para is not yet registered
		assert!(!Parachains::is_parathread(para_id));
		// We register the Para ID
		let validation_code = test_validation_code(32);
		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
		assert_ok!(Registrar::register(
			RuntimeOrigin::signed(1),
			para_id,
			test_genesis_head(32),
			validation_code.clone(),
		));
		conclude_pvf_checking::<Test>(&validation_code, VALIDATORS, START_SESSION_INDEX);

		run_to_session(START_SESSION_INDEX + 2);
		// It is now a parathread.
		assert!(Parachains::is_parathread(para_id));
		assert!(!Parachains::is_parachain(para_id));
		// Some other external process will elevate parathread to parachain
		assert_ok!(Registrar::make_parachain(para_id));
		run_to_session(START_SESSION_INDEX + 4);
		// It is now a parachain.
		assert!(!Parachains::is_parathread(para_id));
		assert!(Parachains::is_parachain(para_id));
		// Turn it back into a parathread
		assert_ok!(Registrar::make_parathread(para_id));
		run_to_session(START_SESSION_INDEX + 6);
		assert!(Parachains::is_parathread(para_id));
		assert!(!Parachains::is_parachain(para_id));
		// Deregister it
		assert_ok!(Registrar::deregister(RuntimeOrigin::root(), para_id,));
		run_to_session(START_SESSION_INDEX + 8);
		// It is nothing
		assert!(!Parachains::is_parathread(para_id));
		assert!(!Parachains::is_parachain(para_id));
	});
}

#[test]
fn register_works() {
	new_test_ext().execute_with(|| {
		const START_SESSION_INDEX: SessionIndex = 1;
		run_to_session(START_SESSION_INDEX);

		let para_id = LOWEST_PUBLIC_ID;
		assert!(!Parachains::is_parathread(para_id));

		let validation_code = test_validation_code(32);
		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
		assert_eq!(Balances::reserved_balance(&1), <Test as Config>::ParaDeposit::get());
		assert_ok!(Registrar::register(
			RuntimeOrigin::signed(1),
			para_id,
			test_genesis_head(32),
			validation_code.clone(),
		));
		conclude_pvf_checking::<Test>(&validation_code, VALIDATORS, START_SESSION_INDEX);

		run_to_session(START_SESSION_INDEX + 2);
		assert!(Parachains::is_parathread(para_id));
		assert_eq!(
			Balances::reserved_balance(&1),
			<Test as Config>::ParaDeposit::get() + 64 * <Test as Config>::DataDepositPerByte::get()
		);
	});
}

#[test]
fn register_handles_basic_errors() {
	new_test_ext().execute_with(|| {
		let para_id = LOWEST_PUBLIC_ID;

		assert_noop!(
			Registrar::register(
				RuntimeOrigin::signed(1),
				para_id,
				test_genesis_head(max_head_size() as usize),
				test_validation_code(max_code_size() as usize),
			),
			Error::<Test>::NotReserved
		);

		// Successfully register para
		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));

		assert_noop!(
			Registrar::register(
				RuntimeOrigin::signed(2),
				para_id,
				test_genesis_head(max_head_size() as usize),
				test_validation_code(max_code_size() as usize),
			),
			Error::<Test>::NotOwner
		);

		assert_ok!(Registrar::register(
			RuntimeOrigin::signed(1),
			para_id,
			test_genesis_head(max_head_size() as usize),
			test_validation_code(max_code_size() as usize),
		));
		// Can skip pre-check and deregister para which's still onboarding.
		run_to_session(2);

		assert_ok!(Registrar::deregister(RuntimeOrigin::root(), para_id));

		// Can't do it again
		assert_noop!(
			Registrar::register(
				RuntimeOrigin::signed(1),
				para_id,
				test_genesis_head(max_head_size() as usize),
				test_validation_code(max_code_size() as usize),
			),
			Error::<Test>::NotReserved
		);

		// Head Size Check
		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(2)));
		assert_noop!(
			Registrar::register(
				RuntimeOrigin::signed(2),
				para_id + 1,
				test_genesis_head((max_head_size() + 1) as usize),
				test_validation_code(max_code_size() as usize),
			),
			Error::<Test>::HeadDataTooLarge
		);

		// Code Size Check
		assert_noop!(
			Registrar::register(
				RuntimeOrigin::signed(2),
				para_id + 1,
				test_genesis_head(max_head_size() as usize),
				test_validation_code((max_code_size() + 1) as usize),
			),
			Error::<Test>::CodeTooLarge
		);

		// Needs enough funds for deposit
		assert_noop!(
			Registrar::reserve(RuntimeOrigin::signed(1337)),
			BalancesError::<Test, _>::InsufficientBalance
		);
	});
}

#[test]
fn deregister_works() {
	new_test_ext().execute_with(|| {
		const START_SESSION_INDEX: SessionIndex = 1;
		run_to_session(START_SESSION_INDEX);

		let para_id = LOWEST_PUBLIC_ID;
		assert!(!Parachains::is_parathread(para_id));

		let validation_code = test_validation_code(32);
		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
		assert_ok!(Registrar::register(
			RuntimeOrigin::signed(1),
			para_id,
			test_genesis_head(32),
			validation_code.clone(),
		));
		conclude_pvf_checking::<Test>(&validation_code, VALIDATORS, START_SESSION_INDEX);

		run_to_session(START_SESSION_INDEX + 2);
		assert!(Parachains::is_parathread(para_id));
		assert_ok!(Registrar::deregister(RuntimeOrigin::root(), para_id,));
		run_to_session(START_SESSION_INDEX + 4);
		assert!(paras::Pallet::<Test>::lifecycle(para_id).is_none());
		assert_eq!(Balances::reserved_balance(&1), 0);
	});
}

#[test]
fn deregister_handles_basic_errors() {
	new_test_ext().execute_with(|| {
		const START_SESSION_INDEX: SessionIndex = 1;
		run_to_session(START_SESSION_INDEX);

		let para_id = LOWEST_PUBLIC_ID;
		assert!(!Parachains::is_parathread(para_id));

		let validation_code = test_validation_code(32);
		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
		assert_ok!(Registrar::register(
			RuntimeOrigin::signed(1),
			para_id,
			test_genesis_head(32),
			validation_code.clone(),
		));
		conclude_pvf_checking::<Test>(&validation_code, VALIDATORS, START_SESSION_INDEX);

		run_to_session(START_SESSION_INDEX + 2);
		assert!(Parachains::is_parathread(para_id));
		// Owner check
		assert_noop!(Registrar::deregister(RuntimeOrigin::signed(2), para_id,), BadOrigin);
		assert_ok!(Registrar::make_parachain(para_id));
		run_to_session(START_SESSION_INDEX + 4);
		// Cant directly deregister parachain
		assert_noop!(
			Registrar::deregister(RuntimeOrigin::root(), para_id,),
			Error::<Test>::NotParathread
		);
	});
}

#[test]
fn swap_works() {
	new_test_ext().execute_with(|| {
		const START_SESSION_INDEX: SessionIndex = 1;
		run_to_session(START_SESSION_INDEX);

		// Successfully register first two parachains
		let para_1 = LOWEST_PUBLIC_ID;
		let para_2 = LOWEST_PUBLIC_ID + 1;

		let validation_code = test_validation_code(max_code_size() as usize);
		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
		assert_ok!(Registrar::register(
			RuntimeOrigin::signed(1),
			para_1,
			test_genesis_head(max_head_size() as usize),
			validation_code.clone(),
		));
		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(2)));
		assert_ok!(Registrar::register(
			RuntimeOrigin::signed(2),
			para_2,
			test_genesis_head(max_head_size() as usize),
			validation_code.clone(),
		));
		conclude_pvf_checking::<Test>(&validation_code, VALIDATORS, START_SESSION_INDEX);

		run_to_session(START_SESSION_INDEX + 2);

		// Upgrade para 1 into a parachain
		assert_ok!(Registrar::make_parachain(para_1));

		// Set some mock swap data.
		let mut swap_data = SwapData::get();
		swap_data.insert(para_1, 69);
		swap_data.insert(para_2, 1337);
		SwapData::set(swap_data);

		run_to_session(START_SESSION_INDEX + 4);

		// Roles are as we expect
		assert!(Parachains::is_parachain(para_1));
		assert!(!Parachains::is_parathread(para_1));
		assert!(!Parachains::is_parachain(para_2));
		assert!(Parachains::is_parathread(para_2));

		// Both paras initiate a swap
		// Swap between parachain and parathread
		assert_ok!(Registrar::swap(para_origin(para_1), para_1, para_2,));
		assert_ok!(Registrar::swap(para_origin(para_2), para_2, para_1,));
		System::assert_last_event(RuntimeEvent::Registrar(paras_registrar::Event::Swapped {
			para_id: para_2,
			other_id: para_1,
		}));

		run_to_session(START_SESSION_INDEX + 6);

		// Roles are swapped
		assert!(!Parachains::is_parachain(para_1));
		assert!(Parachains::is_parathread(para_1));
		assert!(Parachains::is_parachain(para_2));
		assert!(!Parachains::is_parathread(para_2));

		// Data is swapped
		assert_eq!(SwapData::get().get(&para_1).unwrap(), &1337);
		assert_eq!(SwapData::get().get(&para_2).unwrap(), &69);

		// Both paras initiate a swap
		// Swap between parathread and parachain
		assert_ok!(Registrar::swap(para_origin(para_1), para_1, para_2,));
		assert_ok!(Registrar::swap(para_origin(para_2), para_2, para_1,));
		System::assert_last_event(RuntimeEvent::Registrar(paras_registrar::Event::Swapped {
			para_id: para_2,
			other_id: para_1,
		}));

		// Data is swapped
		assert_eq!(SwapData::get().get(&para_1).unwrap(), &69);
		assert_eq!(SwapData::get().get(&para_2).unwrap(), &1337);

		// Parachain to parachain swap
		let para_3 = LOWEST_PUBLIC_ID + 2;
		let validation_code = test_validation_code(max_code_size() as usize);
		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(3)));
		assert_ok!(Registrar::register(
			RuntimeOrigin::signed(3),
			para_3,
			test_genesis_head(max_head_size() as usize),
			validation_code.clone(),
		));
		conclude_pvf_checking::<Test>(&validation_code, VALIDATORS, START_SESSION_INDEX + 6);

		run_to_session(START_SESSION_INDEX + 8);

		// Upgrade para 3 into a parachain
		assert_ok!(Registrar::make_parachain(para_3));

		// Set some mock swap data.
		let mut swap_data = SwapData::get();
		swap_data.insert(para_3, 777);
		SwapData::set(swap_data);

		run_to_session(START_SESSION_INDEX + 10);

		// Both are parachains
		assert!(Parachains::is_parachain(para_3));
		assert!(!Parachains::is_parathread(para_3));
		assert!(Parachains::is_parachain(para_1));
		assert!(!Parachains::is_parathread(para_1));

		// Both paras initiate a swap
		// Swap between parachain and parachain
		assert_ok!(Registrar::swap(para_origin(para_1), para_1, para_3,));
		assert_ok!(Registrar::swap(para_origin(para_3), para_3, para_1,));
		System::assert_last_event(RuntimeEvent::Registrar(paras_registrar::Event::Swapped {
			para_id: para_3,
			other_id: para_1,
		}));

		// Data is swapped
		assert_eq!(SwapData::get().get(&para_3).unwrap(), &69);
		assert_eq!(SwapData::get().get(&para_1).unwrap(), &777);
	});
}

#[test]
fn para_lock_works() {
	new_test_ext().execute_with(|| {
		run_to_block(1);

		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
		let para_id = LOWEST_PUBLIC_ID;
		assert_ok!(Registrar::register(
			RuntimeOrigin::signed(1),
			para_id,
			vec![1; 3].into(),
			vec![1, 2, 3].into(),
		));

		assert_noop!(Registrar::add_lock(RuntimeOrigin::signed(2), para_id), BadOrigin);
		// Once they begin onboarding, we lock them in.
		assert_ok!(Registrar::add_lock(RuntimeOrigin::signed(1), para_id));
		// Owner cannot pass origin check when checking lock
		assert_noop!(
			Registrar::ensure_root_para_or_owner(RuntimeOrigin::signed(1), para_id),
			BadOrigin
		);
		// Owner cannot remove lock.
		assert_noop!(Registrar::remove_lock(RuntimeOrigin::signed(1), para_id), BadOrigin);
		// Para can.
		assert_ok!(Registrar::remove_lock(para_origin(para_id), para_id));
		// Owner can pass origin check again
		assert_ok!(Registrar::ensure_root_para_or_owner(RuntimeOrigin::signed(1), para_id));
	});
}

#[test]
fn swap_handles_bad_states() {
	new_test_ext().execute_with(|| {
		const START_SESSION_INDEX: SessionIndex = 1;
		run_to_session(START_SESSION_INDEX);

		let para_1 = LOWEST_PUBLIC_ID;
		let para_2 = LOWEST_PUBLIC_ID + 1;

		// paras are not yet registered
		assert!(!Parachains::is_parathread(para_1));
		assert!(!Parachains::is_parathread(para_2));

		// Cannot even start a swap
		assert_noop!(
			Registrar::swap(RuntimeOrigin::root(), para_1, para_2),
			Error::<Test>::NotRegistered
		);

		// We register Paras 1 and 2
		let validation_code = test_validation_code(32);
		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
		assert_ok!(Registrar::reserve(RuntimeOrigin::signed(2)));
		assert_ok!(Registrar::register(
			RuntimeOrigin::signed(1),
			para_1,
			test_genesis_head(32),
			validation_code.clone(),
		));
		assert_ok!(Registrar::register(
			RuntimeOrigin::signed(2),
			para_2,
			test_genesis_head(32),
			validation_code.clone(),
		));
		conclude_pvf_checking::<Test>(&validation_code, VALIDATORS, START_SESSION_INDEX);

		// Cannot swap
		assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
		assert_noop!(
			Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
			Error::<Test>::CannotSwap
		);

		run_to_session(START_SESSION_INDEX + 2);

		// They are now a parathread.
		assert!(Parachains::is_parathread(para_1));
		assert!(Parachains::is_parathread(para_2));

		// Cannot swap
		assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
		assert_noop!(
			Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
			Error::<Test>::CannotSwap
		);

		// Some other external process will elevate one parathread to parachain
		assert_ok!(Registrar::make_parachain(para_1));

		// Cannot swap
		assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
		assert_noop!(
			Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
			Error::<Test>::CannotSwap
		);

		run_to_session(START_SESSION_INDEX + 3);

		// Cannot swap
		assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
		assert_noop!(
			Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
			Error::<Test>::CannotSwap
		);

		run_to_session(START_SESSION_INDEX + 4);

		// It is now a parachain.
		assert!(Parachains::is_parachain(para_1));
		assert!(Parachains::is_parathread(para_2));

		// Swap works here.
		assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
		assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_2, para_1));
		assert!(System::events().iter().any(|r| matches!(
			r.event,
			RuntimeEvent::Registrar(paras_registrar::Event::Swapped { .. })
		)));

		run_to_session(START_SESSION_INDEX + 5);

		// Cannot swap
		assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
		assert_noop!(
			Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
			Error::<Test>::CannotSwap
		);

		run_to_session(START_SESSION_INDEX + 6);

		// Swap worked!
		assert!(Parachains::is_parachain(para_2));
		assert!(Parachains::is_parathread(para_1));
		assert!(System::events().iter().any(|r| matches!(
			r.event,
			RuntimeEvent::Registrar(paras_registrar::Event::Swapped { .. })
		)));

		// Something starts to downgrade a para
		assert_ok!(Registrar::make_parathread(para_2));

		run_to_session(START_SESSION_INDEX + 7);

		// Cannot swap
		assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
		assert_noop!(
			Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
			Error::<Test>::CannotSwap
		);

		run_to_session(START_SESSION_INDEX + 8);

		assert!(Parachains::is_parathread(para_1));
		assert!(Parachains::is_parathread(para_2));
	});
}
