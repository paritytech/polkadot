// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

use crate as pallet_bridge_token_swap;
use crate::MessagePayloadOf;

use bp_messages::{
	source_chain::{MessagesBridge, SendMessageArtifacts},
	LaneId, MessageNonce,
};
use bp_runtime::ChainId;
use frame_support::weights::Weight;
use sp_core::H256;
use sp_runtime::{
	testing::Header as SubstrateHeader,
	traits::{BlakeTwo256, IdentityLookup},
	Perbill,
};

pub type AccountId = u64;
pub type Balance = u64;
pub type Block = frame_system::mocking::MockBlock<TestRuntime>;
pub type BridgedAccountId = u64;
pub type BridgedAccountPublic = sp_runtime::testing::UintAuthorityId;
pub type BridgedAccountSignature = sp_runtime::testing::TestSignature;
pub type BridgedBalance = u64;
pub type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<TestRuntime>;

pub const OK_TRANSFER_CALL: u8 = 1;
pub const BAD_TRANSFER_CALL: u8 = 2;
pub const MESSAGE_NONCE: MessageNonce = 3;

pub const THIS_CHAIN_ACCOUNT: AccountId = 1;
pub const THIS_CHAIN_ACCOUNT_BALANCE: Balance = 100_000;

pub const SWAP_DELIVERY_AND_DISPATCH_FEE: Balance = 1;

frame_support::construct_runtime! {
	pub enum TestRuntime where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Event<T>},
		TokenSwap: pallet_bridge_token_swap::{Pallet, Call, Event<T>, Origin<T>},
	}
}

frame_support::parameter_types! {
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
	type AccountData = pallet_balances::AccountData<Balance>;
	type OnNewAccount = ();
	type OnKilledAccount = ();
	type BaseCallFilter = frame_support::traits::Everything;
	type SystemWeightInfo = ();
	type BlockWeights = ();
	type BlockLength = ();
	type DbWeight = ();
	type SS58Prefix = ();
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

frame_support::parameter_types! {
	pub const ExistentialDeposit: u64 = 10;
	pub const MaxReserves: u32 = 50;
}

impl pallet_balances::Config for TestRuntime {
	type MaxLocks = ();
	type Balance = Balance;
	type DustRemoval = ();
	type Event = Event;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = frame_system::Pallet<TestRuntime>;
	type WeightInfo = ();
	type MaxReserves = MaxReserves;
	type ReserveIdentifier = [u8; 8];
}

frame_support::parameter_types! {
	pub const BridgedChainId: ChainId = *b"inst";
	pub const OutboundMessageLaneId: LaneId = *b"lane";
}

impl pallet_bridge_token_swap::Config for TestRuntime {
	type Event = Event;
	type WeightInfo = ();

	type BridgedChainId = BridgedChainId;
	type OutboundMessageLaneId = OutboundMessageLaneId;
	type MessagesBridge = TestMessagesBridge;

	type ThisCurrency = pallet_balances::Pallet<TestRuntime>;
	type FromSwapToThisAccountIdConverter = TestAccountConverter;

	type BridgedChain = BridgedChain;
	type FromBridgedToThisAccountIdConverter = TestAccountConverter;
}

pub struct BridgedChain;

impl bp_runtime::Chain for BridgedChain {
	type BlockNumber = u64;
	type Hash = H256;
	type Hasher = BlakeTwo256;
	type Header = sp_runtime::generic::Header<u64, BlakeTwo256>;

	type AccountId = BridgedAccountId;
	type Balance = BridgedBalance;
	type Index = u64;
	type Signature = BridgedAccountSignature;

	fn max_extrinsic_size() -> u32 {
		unreachable!()
	}
	fn max_extrinsic_weight() -> Weight {
		unreachable!()
	}
}

pub struct TestMessagesBridge;

impl MessagesBridge<Origin, AccountId, Balance, MessagePayloadOf<TestRuntime, ()>>
	for TestMessagesBridge
{
	type Error = ();

	fn send_message(
		sender: Origin,
		lane: LaneId,
		message: MessagePayloadOf<TestRuntime, ()>,
		delivery_and_dispatch_fee: Balance,
	) -> Result<SendMessageArtifacts, Self::Error> {
		assert_eq!(lane, OutboundMessageLaneId::get());
		assert_eq!(delivery_and_dispatch_fee, SWAP_DELIVERY_AND_DISPATCH_FEE);
		match sender.caller {
			OriginCaller::TokenSwap(_) => (),
			_ => panic!("unexpected origin"),
		}
		match message.call[0] {
			OK_TRANSFER_CALL => Ok(SendMessageArtifacts { nonce: MESSAGE_NONCE, weight: 0 }),
			BAD_TRANSFER_CALL => Err(()),
			_ => unreachable!(),
		}
	}
}

pub struct TestAccountConverter;

impl sp_runtime::traits::Convert<H256, AccountId> for TestAccountConverter {
	fn convert(hash: H256) -> AccountId {
		hash.to_low_u64_ne()
	}
}

/// Run pallet test.
pub fn run_test<T>(test: impl FnOnce() -> T) -> T {
	let mut t = frame_system::GenesisConfig::default().build_storage::<TestRuntime>().unwrap();
	pallet_balances::GenesisConfig::<TestRuntime> {
		balances: vec![(THIS_CHAIN_ACCOUNT, THIS_CHAIN_ACCOUNT_BALANCE)],
	}
	.assimilate_storage(&mut t)
	.unwrap();
	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(test)
}
