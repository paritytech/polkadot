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

//! Mocks for all the traits.

use sp_io::TestExternalities;
use sp_core::H256;
use sp_runtime::traits::{
	BlakeTwo256, IdentityLookup,
};
use primitives::v1::{AuthorityDiscoveryId, Balance, BlockNumber, Header, ValidatorIndex};
use frame_support::{parameter_types, traits::Randomness as RandomnessT};
use std::cell::RefCell;
use std::collections::HashMap;
use crate::{
	inclusion, scheduler, dmp, ump, hrmp, session_info, paras, configuration,
	initializer, shared,
};

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

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
		Shared: shared::{Module, Call, Storage},
		Inclusion: inclusion::{Module, Call, Storage, Event<T>},
		Scheduler: scheduler::{Module, Call, Storage},
		Initializer: initializer::{Module, Call, Storage},
		Dmp: dmp::{Module, Call, Storage},
		Ump: ump::{Module, Call, Storage},
		Hrmp: hrmp::{Module, Call, Storage},
		SessionInfo: session_info::{Module, Call, Storage},
	}
);

pub struct TestRandomness;

impl RandomnessT<H256> for TestRandomness {
	fn random(_subject: &[u8]) -> H256 {
		Default::default()
	}
}

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
	type AccountId = u64;
	type Lookup = IdentityLookup<u64>;
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

impl crate::initializer::Config for Test {
	type Randomness = TestRandomness;
}

impl crate::configuration::Config for Test { }

impl crate::shared::Config for Test { }

impl crate::paras::Config for Test {
	type Origin = Origin;
}

impl crate::dmp::Config for Test { }

impl crate::ump::Config for Test {
	type UmpSink = crate::ump::mock_sink::MockUmpSink;
}

impl crate::hrmp::Config for Test {
	type Origin = Origin;
	type Currency = pallet_balances::Module<Test>;
}

impl crate::scheduler::Config for Test { }

impl crate::inclusion::Config for Test {
	type Event = Event;
	type RewardValidators = TestRewardValidators;
}

impl crate::inclusion_inherent::Config for Test { }

impl crate::session_info::Config for Test { }

impl crate::session_info::AuthorityDiscoveryConfig for Test {
	fn authorities() -> Vec<AuthorityDiscoveryId> {
		Vec::new()
	}
}

thread_local! {
	pub static BACKING_REWARDS: RefCell<HashMap<ValidatorIndex, usize>>
		= RefCell::new(HashMap::new());

	pub static AVAILABILITY_REWARDS: RefCell<HashMap<ValidatorIndex, usize>>
		= RefCell::new(HashMap::new());
}

pub fn backing_rewards() -> HashMap<ValidatorIndex, usize> {
	BACKING_REWARDS.with(|r| r.borrow().clone())
}

pub fn availability_rewards() -> HashMap<ValidatorIndex, usize> {
	AVAILABILITY_REWARDS.with(|r| r.borrow().clone())
}

pub struct TestRewardValidators;

impl inclusion::RewardValidators for TestRewardValidators {
	fn reward_backing(v: impl IntoIterator<Item = ValidatorIndex>) {
		BACKING_REWARDS.with(|r| {
			let mut r = r.borrow_mut();
			for i in v {
				*r.entry(i).or_insert(0) += 1;
			}
		})
	}
	fn reward_bitfields(v: impl IntoIterator<Item = ValidatorIndex>) {
		AVAILABILITY_REWARDS.with(|r| {
			let mut r = r.borrow_mut();
			for i in v {
				*r.entry(i).or_insert(0) += 1;
			}
		})
	}
}

/// Create a new set of test externalities.
pub fn new_test_ext(state: MockGenesisConfig) -> TestExternalities {
	BACKING_REWARDS.with(|r| r.borrow_mut().clear());
	AVAILABILITY_REWARDS.with(|r| r.borrow_mut().clear());

	let mut t = state.system.build_storage::<Test>().unwrap();
	state.configuration.assimilate_storage(&mut t).unwrap();
	state.paras.assimilate_storage(&mut t).unwrap();

	t.into()
}

#[derive(Default)]
pub struct MockGenesisConfig {
	pub system: frame_system::GenesisConfig,
	pub configuration: crate::configuration::GenesisConfig<Test>,
	pub paras: crate::paras::GenesisConfig<Test>,
}
