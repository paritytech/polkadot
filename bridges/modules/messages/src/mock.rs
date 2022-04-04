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

// From construct_runtime macro
#![allow(clippy::from_over_into)]

use crate::{instant_payments::cal_relayers_rewards, Config};

use bitvec::prelude::*;
use bp_messages::{
	source_chain::{
		LaneMessageVerifier, MessageDeliveryAndDispatchPayment, OnDeliveryConfirmed,
		OnMessageAccepted, SenderOrigin, TargetHeaderChain,
	},
	target_chain::{
		DispatchMessage, MessageDispatch, ProvedLaneMessages, ProvedMessages, SourceHeaderChain,
	},
	DeliveredMessages, InboundLaneData, LaneId, Message, MessageData, MessageKey, MessageNonce,
	OutboundLaneData, Parameter as MessagesParameter, UnrewardedRelayer,
};
use bp_runtime::{messages::MessageDispatchResult, Size};
use codec::{Decode, Encode};
use frame_support::{
	parameter_types,
	weights::{RuntimeDbWeight, Weight},
};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::{
	testing::Header as SubstrateHeader,
	traits::{BlakeTwo256, IdentityLookup},
	FixedU128, Perbill,
};
use std::{
	collections::{BTreeMap, VecDeque},
	ops::RangeInclusive,
};

pub type AccountId = u64;
pub type Balance = u64;
#[derive(Decode, Encode, Clone, Debug, PartialEq, Eq, TypeInfo)]
pub struct TestPayload {
	/// Field that may be used to identify messages.
	pub id: u64,
	/// Dispatch weight that is declared by the message sender.
	pub declared_weight: Weight,
	/// Message dispatch result.
	///
	/// Note: in correct code `dispatch_result.unspent_weight` will always be <= `declared_weight`,
	/// but for test purposes we'll be making it larger than `declared_weight` sometimes.
	pub dispatch_result: MessageDispatchResult,
	/// Extra bytes that affect payload size.
	pub extra: Vec<u8>,
}
pub type TestMessageFee = u64;
pub type TestRelayer = u64;

pub struct AccountIdConverter;

impl sp_runtime::traits::Convert<H256, AccountId> for AccountIdConverter {
	fn convert(hash: H256) -> AccountId {
		hash.to_low_u64_ne()
	}
}

type Block = frame_system::mocking::MockBlock<TestRuntime>;
type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<TestRuntime>;

use crate as pallet_bridge_messages;

frame_support::construct_runtime! {
	pub enum TestRuntime where
		Block = Block,
		NodeBlock = Block,
		UncheckedExtrinsic = UncheckedExtrinsic,
	{
		System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
		Balances: pallet_balances::{Pallet, Call, Event<T>},
		Messages: pallet_bridge_messages::{Pallet, Call, Event<T>},
	}
}

parameter_types! {
	pub const BlockHashCount: u64 = 250;
	pub const MaximumBlockWeight: Weight = 1024;
	pub const MaximumBlockLength: u32 = 2 * 1024;
	pub const AvailableBlockRatio: Perbill = Perbill::one();
	pub const DbWeight: RuntimeDbWeight = RuntimeDbWeight { read: 1, write: 2 };
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
	type DbWeight = DbWeight;
	type SS58Prefix = ();
	type OnSetCode = ();
	type MaxConsumers = frame_support::traits::ConstU32<16>;
}

parameter_types! {
	pub const ExistentialDeposit: u64 = 1;
}

impl pallet_balances::Config for TestRuntime {
	type MaxLocks = ();
	type Balance = Balance;
	type DustRemoval = ();
	type Event = Event;
	type ExistentialDeposit = ExistentialDeposit;
	type AccountStore = frame_system::Pallet<TestRuntime>;
	type WeightInfo = ();
	type MaxReserves = ();
	type ReserveIdentifier = ();
}

parameter_types! {
	pub const MaxMessagesToPruneAtOnce: u64 = 10;
	pub const MaxUnrewardedRelayerEntriesAtInboundLane: u64 = 16;
	pub const MaxUnconfirmedMessagesAtInboundLane: u64 = 32;
	pub storage TokenConversionRate: FixedU128 = 1.into();
  pub const TestBridgedChainId: bp_runtime::ChainId = *b"test";
}

#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq, TypeInfo)]
pub enum TestMessagesParameter {
	TokenConversionRate(FixedU128),
}

impl MessagesParameter for TestMessagesParameter {
	fn save(&self) {
		match *self {
			TestMessagesParameter::TokenConversionRate(conversion_rate) =>
				TokenConversionRate::set(&conversion_rate),
		}
	}
}

impl Config for TestRuntime {
	type Event = Event;
	type WeightInfo = ();
	type Parameter = TestMessagesParameter;
	type MaxMessagesToPruneAtOnce = MaxMessagesToPruneAtOnce;
	type MaxUnrewardedRelayerEntriesAtInboundLane = MaxUnrewardedRelayerEntriesAtInboundLane;
	type MaxUnconfirmedMessagesAtInboundLane = MaxUnconfirmedMessagesAtInboundLane;

	type OutboundPayload = TestPayload;
	type OutboundMessageFee = TestMessageFee;

	type InboundPayload = TestPayload;
	type InboundMessageFee = TestMessageFee;
	type InboundRelayer = TestRelayer;

	type AccountIdConverter = AccountIdConverter;

	type TargetHeaderChain = TestTargetHeaderChain;
	type LaneMessageVerifier = TestLaneMessageVerifier;
	type MessageDeliveryAndDispatchPayment = TestMessageDeliveryAndDispatchPayment;
	type OnMessageAccepted = TestOnMessageAccepted;
	type OnDeliveryConfirmed = (TestOnDeliveryConfirmed1, TestOnDeliveryConfirmed2);

	type SourceHeaderChain = TestSourceHeaderChain;
	type MessageDispatch = TestMessageDispatch;
	type BridgedChainId = TestBridgedChainId;
}

impl SenderOrigin<AccountId> for Origin {
	fn linked_account(&self) -> Option<AccountId> {
		match self.caller {
			OriginCaller::system(frame_system::RawOrigin::Signed(ref submitter)) =>
				Some(submitter.clone()),
			_ => None,
		}
	}
}

impl Size for TestPayload {
	fn size_hint(&self) -> u32 {
		16 + self.extra.len() as u32
	}
}

/// Account that has balance to use in tests.
pub const ENDOWED_ACCOUNT: AccountId = 0xDEAD;

/// Account id of test relayer.
pub const TEST_RELAYER_A: AccountId = 100;

/// Account id of additional test relayer - B.
pub const TEST_RELAYER_B: AccountId = 101;

/// Account id of additional test relayer - C.
pub const TEST_RELAYER_C: AccountId = 102;

/// Error that is returned by all test implementations.
pub const TEST_ERROR: &str = "Test error";

/// Lane that we're using in tests.
pub const TEST_LANE_ID: LaneId = [0, 0, 0, 1];

/// Regular message payload.
pub const REGULAR_PAYLOAD: TestPayload = message_payload(0, 50);

/// Payload that is rejected by `TestTargetHeaderChain`.
pub const PAYLOAD_REJECTED_BY_TARGET_CHAIN: TestPayload = message_payload(1, 50);

/// Vec of proved messages, grouped by lane.
pub type MessagesByLaneVec = Vec<(LaneId, ProvedLaneMessages<Message<TestMessageFee>>)>;

/// Test messages proof.
#[derive(Debug, Encode, Decode, Clone, PartialEq, Eq, TypeInfo)]
pub struct TestMessagesProof {
	pub result: Result<MessagesByLaneVec, ()>,
}

impl Size for TestMessagesProof {
	fn size_hint(&self) -> u32 {
		0
	}
}

impl From<Result<Vec<Message<TestMessageFee>>, ()>> for TestMessagesProof {
	fn from(result: Result<Vec<Message<TestMessageFee>>, ()>) -> Self {
		Self {
			result: result.map(|messages| {
				let mut messages_by_lane: BTreeMap<
					LaneId,
					ProvedLaneMessages<Message<TestMessageFee>>,
				> = BTreeMap::new();
				for message in messages {
					messages_by_lane.entry(message.key.lane_id).or_default().messages.push(message);
				}
				messages_by_lane.into_iter().collect()
			}),
		}
	}
}

/// Messages delivery proof used in tests.
#[derive(Debug, Encode, Decode, Eq, Clone, PartialEq, TypeInfo)]
pub struct TestMessagesDeliveryProof(pub Result<(LaneId, InboundLaneData<TestRelayer>), ()>);

impl Size for TestMessagesDeliveryProof {
	fn size_hint(&self) -> u32 {
		0
	}
}

/// Target header chain that is used in tests.
#[derive(Debug, Default)]
pub struct TestTargetHeaderChain;

impl TargetHeaderChain<TestPayload, TestRelayer> for TestTargetHeaderChain {
	type Error = &'static str;

	type MessagesDeliveryProof = TestMessagesDeliveryProof;

	fn verify_message(payload: &TestPayload) -> Result<(), Self::Error> {
		if *payload == PAYLOAD_REJECTED_BY_TARGET_CHAIN {
			Err(TEST_ERROR)
		} else {
			Ok(())
		}
	}

	fn verify_messages_delivery_proof(
		proof: Self::MessagesDeliveryProof,
	) -> Result<(LaneId, InboundLaneData<TestRelayer>), Self::Error> {
		proof.0.map_err(|_| TEST_ERROR)
	}
}

/// Lane message verifier that is used in tests.
#[derive(Debug, Default)]
pub struct TestLaneMessageVerifier;

impl LaneMessageVerifier<Origin, AccountId, TestPayload, TestMessageFee>
	for TestLaneMessageVerifier
{
	type Error = &'static str;

	fn verify_message(
		_submitter: &Origin,
		delivery_and_dispatch_fee: &TestMessageFee,
		_lane: &LaneId,
		_lane_outbound_data: &OutboundLaneData,
		_payload: &TestPayload,
	) -> Result<(), Self::Error> {
		if *delivery_and_dispatch_fee != 0 {
			Ok(())
		} else {
			Err(TEST_ERROR)
		}
	}
}

/// Message fee payment system that is used in tests.
#[derive(Debug, Default)]
pub struct TestMessageDeliveryAndDispatchPayment;

impl TestMessageDeliveryAndDispatchPayment {
	/// Reject all payments.
	pub fn reject_payments() {
		frame_support::storage::unhashed::put(b":reject-message-fee:", &true);
	}

	/// Returns true if given fee has been paid by given submitter.
	pub fn is_fee_paid(submitter: AccountId, fee: TestMessageFee) -> bool {
		let raw_origin: Result<frame_system::RawOrigin<_>, _> = Origin::signed(submitter).into();
		frame_support::storage::unhashed::get(b":message-fee:") == Some((raw_origin.unwrap(), fee))
	}

	/// Returns true if given relayer has been rewarded with given balance. The reward-paid flag is
	/// cleared after the call.
	pub fn is_reward_paid(relayer: AccountId, fee: TestMessageFee) -> bool {
		let key = (b":relayer-reward:", relayer, fee).encode();
		frame_support::storage::unhashed::take::<bool>(&key).is_some()
	}
}

impl MessageDeliveryAndDispatchPayment<Origin, AccountId, TestMessageFee>
	for TestMessageDeliveryAndDispatchPayment
{
	type Error = &'static str;

	fn pay_delivery_and_dispatch_fee(
		submitter: &Origin,
		fee: &TestMessageFee,
		_relayer_fund_account: &AccountId,
	) -> Result<(), Self::Error> {
		if frame_support::storage::unhashed::get(b":reject-message-fee:") == Some(true) {
			return Err(TEST_ERROR)
		}

		let raw_origin: Result<frame_system::RawOrigin<_>, _> = submitter.clone().into();
		frame_support::storage::unhashed::put(b":message-fee:", &(raw_origin.unwrap(), fee));
		Ok(())
	}

	fn pay_relayers_rewards(
		lane_id: LaneId,
		message_relayers: VecDeque<UnrewardedRelayer<AccountId>>,
		_confirmation_relayer: &AccountId,
		received_range: &RangeInclusive<MessageNonce>,
		_relayer_fund_account: &AccountId,
	) {
		let relayers_rewards =
			cal_relayers_rewards::<TestRuntime, ()>(lane_id, message_relayers, received_range);
		for (relayer, reward) in &relayers_rewards {
			let key = (b":relayer-reward:", relayer, reward.reward).encode();
			frame_support::storage::unhashed::put(&key, &true);
		}
	}
}

#[derive(Debug)]
pub struct TestOnMessageAccepted;

impl TestOnMessageAccepted {
	/// Verify that the callback has been called when the message is accepted.
	pub fn ensure_called(lane: &LaneId, message: &MessageNonce) {
		let key = (b"TestOnMessageAccepted", lane, message).encode();
		assert_eq!(frame_support::storage::unhashed::get(&key), Some(true));
	}

	/// Set consumed weight returned by the callback.
	pub fn set_consumed_weight_per_message(weight: Weight) {
		frame_support::storage::unhashed::put(b"TestOnMessageAccepted_Weight", &weight);
	}

	/// Get consumed weight returned by the callback.
	pub fn get_consumed_weight_per_message() -> Option<Weight> {
		frame_support::storage::unhashed::get(b"TestOnMessageAccepted_Weight")
	}
}

impl OnMessageAccepted for TestOnMessageAccepted {
	fn on_messages_accepted(lane: &LaneId, message: &MessageNonce) -> Weight {
		let key = (b"TestOnMessageAccepted", lane, message).encode();
		frame_support::storage::unhashed::put(&key, &true);
		Self::get_consumed_weight_per_message()
			.unwrap_or_else(|| DbWeight::get().reads_writes(1, 1))
	}
}

/// First on-messages-delivered callback.
#[derive(Debug)]
pub struct TestOnDeliveryConfirmed1;

impl TestOnDeliveryConfirmed1 {
	/// Verify that the callback has been called with given delivered messages.
	pub fn ensure_called(lane: &LaneId, messages: &DeliveredMessages) {
		let key = (b"TestOnDeliveryConfirmed1", lane, messages).encode();
		assert_eq!(frame_support::storage::unhashed::get(&key), Some(true));
	}

	/// Set consumed weight returned by the callback.
	pub fn set_consumed_weight_per_message(weight: Weight) {
		frame_support::storage::unhashed::put(b"TestOnDeliveryConfirmed1_Weight", &weight);
	}

	/// Get consumed weight returned by the callback.
	pub fn get_consumed_weight_per_message() -> Option<Weight> {
		frame_support::storage::unhashed::get(b"TestOnDeliveryConfirmed1_Weight")
	}
}

impl OnDeliveryConfirmed for TestOnDeliveryConfirmed1 {
	fn on_messages_delivered(lane: &LaneId, messages: &DeliveredMessages) -> Weight {
		let key = (b"TestOnDeliveryConfirmed1", lane, messages).encode();
		frame_support::storage::unhashed::put(&key, &true);
		Self::get_consumed_weight_per_message()
			.unwrap_or_else(|| DbWeight::get().reads_writes(1, 1))
			.saturating_mul(messages.total_messages())
	}
}

/// Second on-messages-delivered callback.
#[derive(Debug)]
pub struct TestOnDeliveryConfirmed2;

impl TestOnDeliveryConfirmed2 {
	/// Verify that the callback has been called with given delivered messages.
	pub fn ensure_called(lane: &LaneId, messages: &DeliveredMessages) {
		let key = (b"TestOnDeliveryConfirmed2", lane, messages).encode();
		assert_eq!(frame_support::storage::unhashed::get(&key), Some(true));
	}
}

impl OnDeliveryConfirmed for TestOnDeliveryConfirmed2 {
	fn on_messages_delivered(lane: &LaneId, messages: &DeliveredMessages) -> Weight {
		let key = (b"TestOnDeliveryConfirmed2", lane, messages).encode();
		frame_support::storage::unhashed::put(&key, &true);
		0
	}
}

/// Source header chain that is used in tests.
#[derive(Debug)]
pub struct TestSourceHeaderChain;

impl SourceHeaderChain<TestMessageFee> for TestSourceHeaderChain {
	type Error = &'static str;

	type MessagesProof = TestMessagesProof;

	fn verify_messages_proof(
		proof: Self::MessagesProof,
		_messages_count: u32,
	) -> Result<ProvedMessages<Message<TestMessageFee>>, Self::Error> {
		proof.result.map(|proof| proof.into_iter().collect()).map_err(|_| TEST_ERROR)
	}
}

/// Source header chain that is used in tests.
#[derive(Debug)]
pub struct TestMessageDispatch;

impl MessageDispatch<AccountId, TestMessageFee> for TestMessageDispatch {
	type DispatchPayload = TestPayload;

	fn dispatch_weight(message: &DispatchMessage<TestPayload, TestMessageFee>) -> Weight {
		match message.data.payload.as_ref() {
			Ok(payload) => payload.declared_weight,
			Err(_) => 0,
		}
	}

	fn dispatch(
		_relayer_account: &AccountId,
		message: DispatchMessage<TestPayload, TestMessageFee>,
	) -> MessageDispatchResult {
		match message.data.payload.as_ref() {
			Ok(payload) => payload.dispatch_result.clone(),
			Err(_) => dispatch_result(0),
		}
	}
}

/// Return test lane message with given nonce and payload.
pub fn message(nonce: MessageNonce, payload: TestPayload) -> Message<TestMessageFee> {
	Message { key: MessageKey { lane_id: TEST_LANE_ID, nonce }, data: message_data(payload) }
}

/// Constructs message payload using given arguments and zero unspent weight.
pub const fn message_payload(id: u64, declared_weight: Weight) -> TestPayload {
	TestPayload { id, declared_weight, dispatch_result: dispatch_result(0), extra: Vec::new() }
}

/// Return message data with valid fee for given payload.
pub fn message_data(payload: TestPayload) -> MessageData<TestMessageFee> {
	MessageData { payload: payload.encode(), fee: 1 }
}

/// Returns message dispatch result with given unspent weight.
pub const fn dispatch_result(unspent_weight: Weight) -> MessageDispatchResult {
	MessageDispatchResult {
		dispatch_result: true,
		unspent_weight,
		dispatch_fee_paid_during_dispatch: true,
	}
}

/// Constructs unrewarded relayer entry from nonces range and relayer id.
pub fn unrewarded_relayer(
	begin: MessageNonce,
	end: MessageNonce,
	relayer: TestRelayer,
) -> UnrewardedRelayer<TestRelayer> {
	UnrewardedRelayer {
		relayer,
		messages: DeliveredMessages {
			begin,
			end,
			dispatch_results: if end >= begin {
				bitvec![u8, Msb0; 1; (end - begin + 1) as _]
			} else {
				Default::default()
			},
		},
	}
}

/// Run pallet test.
pub fn run_test<T>(test: impl FnOnce() -> T) -> T {
	let mut t = frame_system::GenesisConfig::default().build_storage::<TestRuntime>().unwrap();
	pallet_balances::GenesisConfig::<TestRuntime> { balances: vec![(ENDOWED_ACCOUNT, 1_000_000)] }
		.assimilate_storage(&mut t)
		.unwrap();
	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(test)
}
