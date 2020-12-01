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
use sp_core::{H256};
use sp_runtime::{
	Perbill,
	traits::{
		BlakeTwo256, IdentityLookup,
	},
};
use primitives::v1::{AuthorityDiscoveryId, BlockNumber, Header};
use frame_support::{
	impl_outer_origin, impl_outer_dispatch, impl_outer_event, parameter_types,
	weights::Weight, traits::Randomness as RandomnessT,
};
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
	pub const MaximumBlockWeight: Weight = 4 * 1024 * 1024;
	pub const MaximumBlockLength: u32 = 4 * 1024 * 1024;
	pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
}

impl frame_system::Config for Test {
	type BaseCallFilter = ();
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
	type MaximumBlockWeight = MaximumBlockWeight;
	type DbWeight = ();
	type BlockExecutionWeight = ();
	type ExtrinsicBaseWeight = ();
	type MaximumExtrinsicWeight = MaximumBlockWeight;
	type MaximumBlockLength = MaximumBlockLength;
	type AvailableBlockRatio = AvailableBlockRatio;
	type Version = ();
	type PalletInfo = ();
	type AccountData = pallet_balances::AccountData<u128>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type SystemWeightInfo = ();
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
}

impl crate::session_info::Config for Test { }

impl crate::session_info::AuthorityDiscoveryConfig for Test {
	fn authorities() -> Vec<AuthorityDiscoveryId> {
		Vec::new()
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
