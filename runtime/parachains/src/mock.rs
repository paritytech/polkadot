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
use primitives::{
	BlockNumber,
	Header,
};
use frame_support::{
	impl_outer_origin, impl_outer_dispatch, parameter_types,
	weights::Weight,
};

/// A test runtime struct.
#[derive(Clone, Eq, PartialEq)]
pub struct Test;

impl_outer_origin! {
	pub enum Origin for Test { }
}

impl_outer_dispatch! {
	pub enum Call for Test where origin: Origin {
		initializer::Initializer,
	}
}

parameter_types! {
	pub const BlockHashCount: u32 = 250;
	pub const MaximumBlockWeight: Weight = 4 * 1024 * 1024;
	pub const MaximumBlockLength: u32 = 4 * 1024 * 1024;
	pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
}

impl system::Trait for Test {
	type Origin = Origin;
	type Call = Call;
	type Index = u64;
	type BlockNumber = BlockNumber;
	type Hash = H256;
	type Hashing = BlakeTwo256;
	type AccountId = u64;
	type Lookup = IdentityLookup<u64>;
	type Header = Header;
	type Event = ();
	type BlockHashCount = BlockHashCount;
	type MaximumBlockWeight = MaximumBlockWeight;
	type DbWeight = ();
	type BlockExecutionWeight = ();
	type ExtrinsicBaseWeight = ();
	type MaximumExtrinsicWeight = MaximumBlockWeight;
	type MaximumBlockLength = MaximumBlockLength;
	type AvailableBlockRatio = AvailableBlockRatio;
	type Version = ();
	type ModuleToIndex = ();
	type AccountData = balances::AccountData<u128>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
}

impl crate::initializer::Trait for Test { }

impl crate::configuration::Trait for Test { }

impl crate::paras::Trait for Test { }

pub type System = system::Module<Test>;

/// Mocked initializer.
pub type Initializer = crate::initializer::Module<Test>;

/// Mocked configuration.
pub type Configuration = crate::configuration::Module<Test>;

/// Mocked paras.
pub type Paras = crate::paras::Module<Test>;

/// Create a new set of test externalities.
pub fn new_test_ext(state: GenesisConfig) -> TestExternalities {
	let mut t = state.system.build_storage::<Test>().unwrap();
	state.configuration.assimilate_storage(&mut t).unwrap();
	state.paras.assimilate_storage(&mut t).unwrap();

	t.into()
}

#[derive(Default)]
pub struct GenesisConfig {
	pub system: system::GenesisConfig,
	pub configuration: crate::configuration::GenesisConfig<Test>,
	pub paras: crate::paras::GenesisConfig<Test>,
}
