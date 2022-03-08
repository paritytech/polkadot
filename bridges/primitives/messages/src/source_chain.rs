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

//! Primitives of messages module, that are used on the source chain.

use crate::{DeliveredMessages, InboundLaneData, LaneId, MessageNonce, OutboundLaneData};

use crate::UnrewardedRelayer;
use bp_runtime::Size;
use frame_support::{weights::Weight, Parameter, RuntimeDebug};
use sp_std::{
	collections::{btree_map::BTreeMap, vec_deque::VecDeque},
	fmt::Debug,
	ops::RangeInclusive,
};

/// The sender of the message on the source chain.
pub type Sender<AccountId> = frame_system::RawOrigin<AccountId>;

/// Relayers rewards, grouped by relayer account id.
pub type RelayersRewards<AccountId, Balance> = BTreeMap<AccountId, RelayerRewards<Balance>>;

/// Single relayer rewards.
#[derive(RuntimeDebug, Default)]
pub struct RelayerRewards<Balance> {
	/// Total rewards that are to be paid to the relayer.
	pub reward: Balance,
	/// Total number of messages relayed by this relayer.
	pub messages: MessageNonce,
}

/// Target chain API. Used by source chain to verify target chain proofs.
///
/// All implementations of this trait should only work with finalized data that
/// can't change. Wrong implementation may lead to invalid lane states (i.e. lane
/// that's stuck) and/or processing messages without paying fees.
pub trait TargetHeaderChain<Payload, AccountId> {
	/// Error type.
	type Error: Debug + Into<&'static str>;

	/// Proof that messages have been received by target chain.
	type MessagesDeliveryProof: Parameter + Size;

	/// Verify message payload before we accept it.
	///
	/// **CAUTION**: this is very important function. Incorrect implementation may lead
	/// to stuck lanes and/or relayers loses.
	///
	/// The proper implementation must ensure that the delivery-transaction with this
	/// payload would (at least) be accepted into target chain transaction pool AND
	/// eventually will be successfully mined. The most obvious incorrect implementation
	/// example would be implementation for BTC chain that accepts payloads larger than
	/// 1MB. BTC nodes aren't accepting transactions that are larger than 1MB, so relayer
	/// will be unable to craft valid transaction => this (and all subsequent) messages will
	/// never be delivered.
	fn verify_message(payload: &Payload) -> Result<(), Self::Error>;

	/// Verify messages delivery proof and return lane && nonce of the latest received message.
	fn verify_messages_delivery_proof(
		proof: Self::MessagesDeliveryProof,
	) -> Result<(LaneId, InboundLaneData<AccountId>), Self::Error>;
}

/// Lane message verifier.
///
/// Runtime developer may implement any additional validation logic over message-lane mechanism.
/// E.g. if lanes should have some security (e.g. you can only accept Lane1 messages from
/// Submitter1, Lane2 messages for those who has submitted first message to this lane, disable
/// Lane3 until some block, ...), then it may be built using this verifier.
///
/// Any fee requirements should also be enforced here.
pub trait LaneMessageVerifier<Submitter, Payload, Fee> {
	/// Error type.
	type Error: Debug + Into<&'static str>;

	/// Verify message payload and return Ok(()) if message is valid and allowed to be sent over the
	/// lane.
	fn verify_message(
		submitter: &Sender<Submitter>,
		delivery_and_dispatch_fee: &Fee,
		lane: &LaneId,
		outbound_data: &OutboundLaneData,
		payload: &Payload,
	) -> Result<(), Self::Error>;
}

/// Message delivery payment. It is called as a part of submit-message transaction. Transaction
/// submitter is paying (in source chain tokens/assets) for:
///
/// 1) submit-message-transaction-fee itself. This fee is not included in the
/// `delivery_and_dispatch_fee` and is withheld by the regular transaction payment mechanism;
/// 2) message-delivery-transaction-fee. It is submitted to the target node by relayer;
/// 3) message-dispatch fee. It is paid by relayer for processing message by target chain;
/// 4) message-receiving-delivery-transaction-fee. It is submitted to the source node
/// by relayer.
///
/// So to be sure that any non-altruist relayer would agree to deliver message, submitter
/// should set `delivery_and_dispatch_fee` to at least (equivalent of): sum of fees from (2)
/// to (4) above, plus some interest for the relayer.
pub trait MessageDeliveryAndDispatchPayment<AccountId, Balance> {
	/// Error type.
	type Error: Debug + Into<&'static str>;

	/// Withhold/write-off delivery_and_dispatch_fee from submitter account to
	/// some relayers-fund account.
	fn pay_delivery_and_dispatch_fee(
		submitter: &Sender<AccountId>,
		fee: &Balance,
		relayer_fund_account: &AccountId,
	) -> Result<(), Self::Error>;

	/// Pay rewards for delivering messages to the given relayers.
	///
	/// The implementation may also choose to pay reward to the `confirmation_relayer`, which is
	/// a relayer that has submitted delivery confirmation transaction.
	fn pay_relayers_rewards(
		lane_id: LaneId,
		messages_relayers: VecDeque<UnrewardedRelayer<AccountId>>,
		confirmation_relayer: &AccountId,
		received_range: &RangeInclusive<MessageNonce>,
		relayer_fund_account: &AccountId,
	);
}

/// Send message artifacts.
#[derive(RuntimeDebug, PartialEq)]
pub struct SendMessageArtifacts {
	/// Nonce of the message.
	pub nonce: MessageNonce,
	/// Actual weight of send message call.
	pub weight: Weight,
}

/// Messages bridge API to be used from other pallets.
pub trait MessagesBridge<AccountId, Balance, Payload> {
	/// Error type.
	type Error: Debug;

	/// Send message over the bridge.
	///
	/// Returns unique message nonce or error if send has failed.
	fn send_message(
		sender: Sender<AccountId>,
		lane: LaneId,
		message: Payload,
		delivery_and_dispatch_fee: Balance,
	) -> Result<SendMessageArtifacts, Self::Error>;
}

/// Bridge that does nothing when message is being sent.
#[derive(RuntimeDebug, PartialEq)]
pub struct NoopMessagesBridge;

impl<AccountId, Balance, Payload> MessagesBridge<AccountId, Balance, Payload>
	for NoopMessagesBridge
{
	type Error = &'static str;

	fn send_message(
		_sender: Sender<AccountId>,
		_lane: LaneId,
		_message: Payload,
		_delivery_and_dispatch_fee: Balance,
	) -> Result<SendMessageArtifacts, Self::Error> {
		Ok(SendMessageArtifacts { nonce: 0, weight: 0 })
	}
}

/// Handler for messages delivery confirmation.
pub trait OnDeliveryConfirmed {
	/// Called when we receive confirmation that our messages have been delivered to the
	/// target chain. The confirmation also has single bit dispatch result for every
	/// confirmed message (see `DeliveredMessages` for details). Guaranteed to be called
	/// only when at least one message is delivered.
	///
	/// Should return total weight consumed by the call.
	///
	/// NOTE: messages pallet assumes that maximal weight that may be spent on processing
	/// single message is single DB read + single DB write. So this function shall never
	/// return weight that is larger than total number of messages * (db read + db write).
	/// If your pallet needs more time for processing single message, please do it
	/// from `on_initialize` call(s) of the next block(s).
	fn on_messages_delivered(_lane: &LaneId, _messages: &DeliveredMessages) -> Weight;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl OnDeliveryConfirmed for Tuple {
	fn on_messages_delivered(lane: &LaneId, messages: &DeliveredMessages) -> Weight {
		let mut total_weight: Weight = 0;
		for_tuples!(
			#(
				total_weight = total_weight.saturating_add(Tuple::on_messages_delivered(lane, messages));
			)*
		);
		total_weight
	}
}

/// Handler for messages have been accepted
pub trait OnMessageAccepted {
	/// Called when a message has been accepted by message pallet.
	fn on_messages_accepted(lane: &LaneId, message: &MessageNonce) -> Weight;
}

impl OnMessageAccepted for () {
	fn on_messages_accepted(_lane: &LaneId, _message: &MessageNonce) -> Weight {
		0
	}
}

/// Structure that may be used in place of `TargetHeaderChain`, `LaneMessageVerifier` and
/// `MessageDeliveryAndDispatchPayment` on chains, where outbound messages are forbidden.
pub struct ForbidOutboundMessages;

/// Error message that is used in `ForbidOutboundMessages` implementation.
const ALL_OUTBOUND_MESSAGES_REJECTED: &str =
	"This chain is configured to reject all outbound messages";

impl<Payload, AccountId> TargetHeaderChain<Payload, AccountId> for ForbidOutboundMessages {
	type Error = &'static str;

	type MessagesDeliveryProof = ();

	fn verify_message(_payload: &Payload) -> Result<(), Self::Error> {
		Err(ALL_OUTBOUND_MESSAGES_REJECTED)
	}

	fn verify_messages_delivery_proof(
		_proof: Self::MessagesDeliveryProof,
	) -> Result<(LaneId, InboundLaneData<AccountId>), Self::Error> {
		Err(ALL_OUTBOUND_MESSAGES_REJECTED)
	}
}

impl<Submitter, Payload, Fee> LaneMessageVerifier<Submitter, Payload, Fee>
	for ForbidOutboundMessages
{
	type Error = &'static str;

	fn verify_message(
		_submitter: &Sender<Submitter>,
		_delivery_and_dispatch_fee: &Fee,
		_lane: &LaneId,
		_outbound_data: &OutboundLaneData,
		_payload: &Payload,
	) -> Result<(), Self::Error> {
		Err(ALL_OUTBOUND_MESSAGES_REJECTED)
	}
}

impl<AccountId, Balance> MessageDeliveryAndDispatchPayment<AccountId, Balance>
	for ForbidOutboundMessages
{
	type Error = &'static str;

	fn pay_delivery_and_dispatch_fee(
		_submitter: &Sender<AccountId>,
		_fee: &Balance,
		_relayer_fund_account: &AccountId,
	) -> Result<(), Self::Error> {
		Err(ALL_OUTBOUND_MESSAGES_REJECTED)
	}

	fn pay_relayers_rewards(
		_lane_id: LaneId,
		_messages_relayers: VecDeque<UnrewardedRelayer<AccountId>>,
		_confirmation_relayer: &AccountId,
		_received_range: &RangeInclusive<MessageNonce>,
		_relayer_fund_account: &AccountId,
	) {
	}
}
