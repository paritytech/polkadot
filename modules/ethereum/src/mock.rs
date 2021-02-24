// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

// From construct_runtime macro
#![allow(clippy::from_over_into)]

pub use crate::test_utils::{insert_header, validator_utils::*, validators_change_receipt, HeaderBuilder, GAS_LIMIT};
pub use bp_eth_poa::signatures::secret_to_address;

use crate::validators::{ValidatorsConfiguration, ValidatorsSource};
use crate::{AuraConfiguration, ChainTime, Config, GenesisConfig as CrateGenesisConfig, PruningStrategy};
use bp_eth_poa::{Address, AuraHeader, H256, U256};
use frame_support::{parameter_types, weights::Weight};
use secp256k1::SecretKey;
use sp_runtime::{
	testing::Header as SubstrateHeader,
	traits::{BlakeTwo256, IdentityLookup},
	Perbill,
};

pub type AccountId = u64;

type Block = frame_system::mocking::MockBlock<TestRuntime>;
type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<TestRuntime>;

use crate as pallet_ethereum;

frame_support::construct_runtime! {
	pub enum TestRuntime where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Module, Call, Config, Storage, Event<T>},
		Ethereum: pallet_ethereum::{Module, Call},
	}
}

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub const MaximumBlockWeight: Weight = 1024;
	pub const MaximumBlockLength: u32 = 2 * 1024;
	pub const AvailableBlockRatio: Perbill = Perbill::one();
}

impl frame_system::Config for TestRuntime {
	type Origin = Origin;
	type Index = u64;
	type Call = Call;
	type BlockNumber = u64;
	type Hash = H256;
	type Hashing = BlakeTwo256;
	type AccountId = AccountId;
	type Lookup = IdentityLookup<Self::AccountId>;
	type Header = SubstrateHeader;
	type Event = Event;
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = ();
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type BaseCallFilter = ();
	type SystemWeightInfo = ();
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type SS58Prefix = ();
}

parameter_types! {
	pub const TestFinalityVotesCachingInterval: Option<u64> = Some(16);
	pub TestAuraConfiguration: AuraConfiguration = test_aura_config();
	pub TestValidatorsConfiguration: ValidatorsConfiguration = test_validators_config();
}

impl Config for TestRuntime {
	type AuraConfiguration = TestAuraConfiguration;
	type ValidatorsConfiguration = TestValidatorsConfiguration;
	type FinalityVotesCachingInterval = TestFinalityVotesCachingInterval;
	type PruningStrategy = KeepSomeHeadersBehindBest;
	type ChainTime = ConstChainTime;
	type OnHeadersSubmitted = ();
}

/// Test context.
pub struct TestContext {
	/// Initial (genesis) header.
	pub genesis: AuraHeader,
	/// Number of initial validators.
	pub total_validators: usize,
	/// Secret keys of validators, ordered by validator index.
	pub validators: Vec<SecretKey>,
	/// Addresses of validators, ordered by validator index.
	pub addresses: Vec<Address>,
}

/// Aura configuration that is used in tests by default.
pub fn test_aura_config() -> AuraConfiguration {
	AuraConfiguration {
		empty_steps_transition: u64::max_value(),
		strict_empty_steps_transition: 0,
		validate_step_transition: 0x16e360,
		validate_score_transition: 0x41a3c4,
		two_thirds_majority_transition: u64::max_value(),
		min_gas_limit: 0x1388.into(),
		max_gas_limit: U256::max_value(),
		maximum_extra_data_size: 0x20,
	}
}

/// Validators configuration that is used in tests by default.
pub fn test_validators_config() -> ValidatorsConfiguration {
	ValidatorsConfiguration::Single(ValidatorsSource::List(validators_addresses(3)))
}

/// Genesis header that is used in tests by default.
pub fn genesis() -> AuraHeader {
	HeaderBuilder::genesis().sign_by(&validator(0))
}

/// Run test with default genesis header.
pub fn run_test<T>(total_validators: usize, test: impl FnOnce(TestContext) -> T) -> T {
	run_test_with_genesis(genesis(), total_validators, test)
}

/// Run test with default genesis header.
pub fn run_test_with_genesis<T>(
	genesis: AuraHeader,
	total_validators: usize,
	test: impl FnOnce(TestContext) -> T,
) -> T {
	let validators = validators(total_validators);
	let addresses = validators_addresses(total_validators);
	sp_io::TestExternalities::new(
		CrateGenesisConfig {
			initial_header: genesis.clone(),
			initial_difficulty: 0.into(),
			initial_validators: addresses.clone(),
		}
		.build_storage::<TestRuntime, crate::DefaultInstance>()
		.unwrap(),
	)
	.execute_with(|| {
		test(TestContext {
			genesis,
			total_validators,
			validators,
			addresses,
		})
	})
}

/// Pruning strategy that keeps 10 headers behind best block.
pub struct KeepSomeHeadersBehindBest(pub u64);

impl Default for KeepSomeHeadersBehindBest {
	fn default() -> KeepSomeHeadersBehindBest {
		KeepSomeHeadersBehindBest(10)
	}
}

impl PruningStrategy for KeepSomeHeadersBehindBest {
	fn pruning_upper_bound(&mut self, best_number: u64, _: u64) -> u64 {
		best_number.saturating_sub(self.0)
	}
}

/// Constant chain time
#[derive(Default)]
pub struct ConstChainTime;

impl ChainTime for ConstChainTime {
	fn is_timestamp_ahead(&self, timestamp: u64) -> bool {
		let now = i32::max_value() as u64 / 2;
		timestamp > now
	}
}
