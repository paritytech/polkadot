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
use primitives::v1::{AuthorityDiscoveryId, BlockNumber, Header, ValidatorIndex};
use frame_support::{
	impl_outer_origin, impl_outer_dispatch, impl_outer_event, parameter_types,
	traits::Randomness as RandomnessT,
};
use std::cell::RefCell;
use std::collections::HashMap;
use crate::inclusion;
use crate as parachains;

/// A test runtime struct.
#[derive(Clone, Eq, PartialEq)]
pub struct Test;

impl_outer_origin! {
	pub enum Origin for Test {
		parachains
	}
}

impl_outer_dispatch! {
	pub enum Call for Test where origin: Origin {
		initializer::Initializer,
	}
}

impl_outer_event! {
	pub enum TestEvent for Test {
		frame_system<T>,
		inclusion<T>,
	}
}

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
	type Event = TestEvent;
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = ();
	type AccountData = pallet_balances::AccountData<u128>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
	type SS58Prefix = ();
}

impl crate::initializer::Config for Test {
	type Randomness = TestRandomness;
}

impl crate::configuration::Config for Test { }

impl crate::paras::Config for Test {
	type Origin = Origin;
}

impl crate::dmp::Config for Test { }

impl crate::ump::Config for Test {
	type UmpSink = crate::ump::mock_sink::MockUmpSink;
}

impl crate::hrmp::Config for Test {
	type Origin = Origin;
}

impl crate::scheduler::Config for Test { }

impl crate::inclusion::Config for Test {
	type Event = TestEvent;
	type RewardValidators = TestRewardValidators;
}

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

pub type System = frame_system::Module<Test>;

/// Mocked initializer.
pub type Initializer = crate::initializer::Module<Test>;

/// Mocked configuration.
pub type Configuration = crate::configuration::Module<Test>;

/// Mocked paras.
pub type Paras = crate::paras::Module<Test>;

/// Mocked DMP
pub type Dmp = crate::dmp::Module<Test>;

/// Mocked UMP
pub type Ump = crate::ump::Module<Test>;

/// Mocked HRMP
pub type Hrmp = crate::hrmp::Module<Test>;

/// Mocked scheduler.
pub type Scheduler = crate::scheduler::Module<Test>;

/// Mocked inclusion module.
pub type Inclusion = crate::inclusion::Module<Test>;

/// Mocked session info module.
pub type SessionInfo = crate::session_info::Module<Test>;

/// Create a new set of test externalities.
pub fn new_test_ext(state: GenesisConfig) -> TestExternalities {
	BACKING_REWARDS.with(|r| r.borrow_mut().clear());
	AVAILABILITY_REWARDS.with(|r| r.borrow_mut().clear());

	let mut t = state.system.build_storage::<Test>().unwrap();
	state.configuration.assimilate_storage(&mut t).unwrap();
	state.paras.assimilate_storage(&mut t).unwrap();

	t.into()
}

#[derive(Default)]
pub struct GenesisConfig {
	pub system: frame_system::GenesisConfig,
	pub configuration: crate::configuration::GenesisConfig<Test>,
	pub paras: crate::paras::GenesisConfig<Test>,
}
