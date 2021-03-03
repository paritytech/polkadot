// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Mock Runtime for Substrate Pallet Testing.
//!
//! Includes some useful testing types and functions.

#![cfg(test)]
// From construct_runtime macro
#![allow(clippy::from_over_into)]

use crate::{BridgedBlockHash, BridgedBlockNumber, BridgedHeader, Config};
use bp_runtime::Chain;
use frame_support::{parameter_types, weights::Weight};
use sp_runtime::{
	testing::{Header, H256},
	traits::{BlakeTwo256, IdentityLookup},
	Perbill,
};

pub type AccountId = u64;
pub type TestHeader = BridgedHeader<TestRuntime>;
pub type TestNumber = BridgedBlockNumber<TestRuntime>;
pub type TestHash = BridgedBlockHash<TestRuntime>;

type Block = frame_system::mocking::MockBlock<TestRuntime>;
type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<TestRuntime>;

use crate as pallet_substrate;

frame_support::construct_runtime! {
	pub enum TestRuntime where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Module, Call, Config, Storage, Event<T>},
		Substrate: pallet_substrate::{Module, Call},
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
	type Header = Header;
	type Event = ();
	type BlockHashCount = BlockHashCount;
	type Version = ();
	type PalletInfo = PalletInfo;
	type AccountData = ();
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type BaseCallFilter = ();
	type SystemWeightInfo = ();
	type DbWeight = ();
	type BlockWeights = ();
	type BlockLength = ();
	type SS58Prefix = ();
}

impl Config for TestRuntime {
	type BridgedChain = TestBridgedChain;
}

#[derive(Debug)]
pub struct TestBridgedChain;

impl Chain for TestBridgedChain {
	type BlockNumber = <TestRuntime as frame_system::Config>::BlockNumber;
	type Hash = <TestRuntime as frame_system::Config>::Hash;
	type Hasher = <TestRuntime as frame_system::Config>::Hashing;
	type Header = <TestRuntime as frame_system::Config>::Header;
}

pub fn run_test<T>(test: impl FnOnce() -> T) -> T {
	sp_io::TestExternalities::new(Default::default()).execute_with(test)
}

pub fn test_header(num: TestNumber) -> TestHeader {
	// We wrap the call to avoid explicit type annotations in our tests
	bp_test_utils::test_header(num)
}

pub fn unfinalized_header(num: u64) -> crate::storage::ImportedHeader<TestHeader> {
	crate::storage::ImportedHeader {
		header: test_header(num),
		requires_justification: false,
		is_finalized: false,
		signal_hash: None,
	}
}
