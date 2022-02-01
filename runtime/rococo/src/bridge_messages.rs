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

//! Over-bridge messaging support for Rococo <> Wococo bridge.

pub use self::{at_rococo::*, at_wococo::*};

use bp_message_dispatch::MessageDispatch as _;
use bp_messages::{
	source_chain::{LaneMessageVerifier, SendMessageArtifacts, Sender, TargetHeaderChain},
	target_chain::{DispatchMessage, MessageDispatch, ProvedMessages, SourceHeaderChain},
	InboundLaneData, LaneId, Message, MessageNonce, OutboundLaneData, Parameter as MessagesParameter,
};
use bp_rococo::{
	max_extrinsic_size, max_extrinsic_weight, EXTRA_STORAGE_PROOF_SIZE,
	MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
};
use bp_runtime::{messages::MessageDispatchResult, ChainId, ROCOCO_CHAIN_ID, WOCOCO_CHAIN_ID};
use bridge_runtime_common::messages::{
	source as messages_source, target as messages_target, BridgedChainWithMessages,
	ChainWithMessages, MessageBridge, MessageTransaction, ThisChainWithMessages,
};
use frame_support::{
	traits::{Currency, Get},
	weights::{PostDispatchInfo, Weight, WeightToFeePolynomial},
	RuntimeDebug,
};
use scale_info::TypeInfo;
use sp_runtime::{codec::{Decode, Encode}, traits::Zero, FixedPointNumber, DispatchErrorWithPostInfo};
use sp_std::{convert::TryFrom, marker::PhantomData, ops::RangeInclusive, vec::Vec};

use rococo_runtime_constants::fee::WeightToFee;

/// Lane for regular encoded calls.
const REGULAR_LANE: LaneId = [0, 0, 0, 0];
/// Lane for XCM calls.
const XCM_LANE: LaneId = [0, 0, 0, 1];

/// Send XCM message from Rococo to Wococo.
pub fn send_xcm_from_rococo_to_wococo(opaque_xcm: Vec<u8>) -> Result<SendMessageArtifacts, DispatchErrorWithPostInfo<PostDispatchInfo>> {
	send_xcm::<AtRococoWithWococoMessageBridge, crate::AtRococoWithWococoMessagesInstance>(opaque_xcm)
}

/// Send XCM message from Wococo to Rococo.
pub fn send_xcm_from_wococo_to_rococo(opaque_xcm: Vec<u8>) -> Result<SendMessageArtifacts, DispatchErrorWithPostInfo<PostDispatchInfo>> {
	send_xcm::<AtWococoWithRococoMessageBridge, crate::AtWococoWithRococoMessagesInstance>(opaque_xcm)
}

/// Process message from Rococo to Wococo (at Wococo).
pub fn process_xcm_from_rococo_to_wococo(opaque_xcm: Vec<u8>) {
	// @gavofyork: you may call whatever required here with `opaque_xcm`, but the weight shall be
	// no more than weight returned by `RococoLikeMessageDispatch::dispatch_weight` method.
	log::trace!(target: "runtime::bridge-xcm", "Delivered XCM from Rococo to Wococo: {:?}", opaque_xcm);
}

/// Process message from Wococo to Rococo (at Rococo).
pub fn process_xcm_from_wococo_to_rococo(opaque_xcm: Vec<u8>) {
	// @gavofyork: you may call whatever required here with `opaque_xcm`, but the weight shall be
	// no more than weight returned by `RococoLikeMessageDispatch::dispatch_weight` method.
	log::trace!(target: "runtime::bridge-xcm", "Delivered XCM from Wococo to Rococo: {:?}", opaque_xcm);
}

/// XCM processor at the target chain.
pub trait ProcessXcm {
	/// Process XCM received from the bridge chain.
	fn process_xcm(opaque_xcm: Vec<u8>);
}

/// Send XCM message between chains.
fn send_xcm<B: MessageBridge, MessagesInstance: 'static>(
	opaque_xcm: Vec<u8>,
) -> Result<SendMessageArtifacts, DispatchErrorWithPostInfo<PostDispatchInfo>> where
	pallet_bridge_messages::Pallet<crate::Runtime, MessagesInstance>: bp_messages::source_chain::MessagesBridge<
		crate::AccountId,
		crate::Balance,
		messages_source::FromThisChainMessagePayload<B>,
		Error = DispatchErrorWithPostInfo<PostDispatchInfo>,
	>,
{
	// @gavofyork: this shall be set to the weight of the message dispatch. Normally we dispatch encoded calls
	// as they're delivered. But if you're just queueing your XCMs somewhere, set it to the weight of db write.
	let delivery_weight = 0;

	// @gavofyork: all other fields below are irrelevant for your case
	<pallet_bridge_messages::Pallet::<crate::Runtime, MessagesInstance> as bp_messages::source_chain::MessagesBridge<_, _, _>>::send_message(
		frame_system::RawOrigin::Root.into(),
		XCM_LANE,
		bp_message_dispatch::MessagePayload {
			spec_version: 0,
			weight: delivery_weight,
			origin: bp_message_dispatch::CallOrigin::SourceRoot,
			dispatch_fee_payment: bp_runtime::messages::DispatchFeePayment::AtSourceChain,
			call: opaque_xcm,
		},
		0,
	)
}

/// Maximal number of pending outbound messages.
const MAXIMAL_PENDING_MESSAGES_AT_OUTBOUND_LANE: MessageNonce =
	bp_rococo::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE;
/// Maximal weight of single message delivery confirmation transaction on Rococo/Wococo chain.
///
/// This value is a result of `pallet_bridge_messages::Pallet::receive_messages_delivery_proof` weight formula
/// computation for the case when single message is confirmed. The result then must be rounded up to account
/// possible future runtime upgrades.
const MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT: Weight = 2_000_000_000;
/// Increase of delivery transaction weight on Rococo/Wococo chain with every additional message byte.
///
/// This value is a result of `pallet_bridge_messages::WeightInfoExt::storage_proof_size_overhead(1)` call. The
/// result then must be rounded up to account possible future runtime upgrades.
const ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT: Weight = 25_000;
/// Weight of single regular message delivery transaction on Rococo/Wococo chain.
///
/// This value is a result of `pallet_bridge_messages::Pallet::receive_messages_proof_weight()` call
/// for the case when single message of `pallet_bridge_messages::EXPECTED_DEFAULT_MESSAGE_LENGTH` bytes is delivered.
/// The message must have dispatch weight set to zero. The result then must be rounded up to account
/// possible future runtime upgrades.
const DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT: Weight = 1_500_000_000;
/// Weight of pay-dispatch-fee operation for inbound messages at Rococo/Wococo chain.
///
/// This value corresponds to the result of `pallet_bridge_messages::WeightInfoExt::pay_inbound_dispatch_fee_overhead()`
/// call for your chain. Don't put too much reserve there, because it is used to **decrease**
/// `DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT` cost. So putting large reserve would make delivery transactions cheaper.
const PAY_INBOUND_DISPATCH_FEE_WEIGHT: Weight = 600_000_000;
/// Number of bytes, included in the signed Rococo/Wococo transaction apart from the encoded call itself.
///
/// Can be computed by subtracting encoded call size from raw transaction size.
const TX_EXTRA_BYTES: u32 = 130;

/// Rococo chain as it is seen at Rococo.
pub type RococoAtRococo =
	RococoLikeChain<AtRococoWithWococoMessageBridge, crate::RococoGrandpaInstance>;

/// Rococo chain as it is seen at Wococo.
pub type RococoAtWococo =
	RococoLikeChain<AtWococoWithRococoMessageBridge, crate::RococoGrandpaInstance>;

/// Wococo chain as it is seen at Wococo.
pub type WococoAtWococo =
	RococoLikeChain<AtWococoWithRococoMessageBridge, crate::WococoGrandpaInstance>;

/// Wococo chain as it is seen at Rococo.
pub type WococoAtRococo =
	RococoLikeChain<AtRococoWithWococoMessageBridge, crate::WococoGrandpaInstance>;

/// Rococo/Wococo chain from message lane point of view.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct RococoLikeChain<B, GI> {
	_bridge_definition: PhantomData<B>,
	_at_this_chain_grandpa_pallet_instance: PhantomData<GI>,
}

impl<B, GI> ChainWithMessages for RococoLikeChain<B, GI> {
	type Hash = crate::Hash;
	type AccountId = crate::AccountId;
	type Signer = primitives::v1::AccountPublic;
	type Signature = crate::Signature;
	type Weight = Weight;
	type Balance = crate::Balance;
}

impl<B, GI> ThisChainWithMessages for RococoLikeChain<B, GI> {
	type Call = crate::Call;

	fn is_outbound_lane_enabled(lane: &LaneId) -> bool {
		*lane == REGULAR_LANE || *lane == XCM_LANE
	}

	fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
		MAXIMAL_PENDING_MESSAGES_AT_OUTBOUND_LANE
	}

	fn estimate_delivery_confirmation_transaction() -> MessageTransaction<Weight> {
		let inbound_data_size = InboundLaneData::<crate::AccountId>::encoded_size_hint(
			MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
			1,
			1,
		)
		.unwrap_or(u32::MAX);

		MessageTransaction {
			dispatch_weight: MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
			size: inbound_data_size
				.saturating_add(EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> crate::Balance {
		// current fee multiplier is used here
		bridge_runtime_common::messages::transaction_payment(
			crate::BlockWeights::get()
				.get(frame_support::weights::DispatchClass::Normal)
				.base_extrinsic,
			crate::TransactionByteFee::get(),
			pallet_transaction_payment::Pallet::<crate::Runtime>::next_fee_multiplier(),
			|weight| WeightToFee::calc(&weight),
			transaction,
		)
	}
}

impl<B, GI> BridgedChainWithMessages for RococoLikeChain<B, GI> {
	fn maximal_extrinsic_size() -> u32 {
		max_extrinsic_size()
	}

	fn message_weight_limits(_message_payload: &[u8]) -> RangeInclusive<Weight> {
		// we don't want to relay too large messages + keep reserve for future upgrades
		let upper_limit =
			messages_target::maximal_incoming_message_dispatch_weight(max_extrinsic_weight());

		// we're charging for payload bytes in `With(Wococo | Rococo)MessageBridge::transaction_payment` function
		//
		// this bridge may be used to deliver all kind of messages, so we're not making any assumptions about
		// minimal dispatch weight here

		0..=upper_limit
	}

	fn estimate_delivery_transaction(
		message_payload: &[u8],
		include_pay_dispatch_fee_cost: bool,
		message_dispatch_weight: Weight,
	) -> MessageTransaction<Weight> {
		let message_payload_len = u32::try_from(message_payload.len()).unwrap_or(u32::MAX);
		let extra_bytes_in_payload = Weight::from(message_payload_len)
			.saturating_sub(pallet_bridge_messages::EXPECTED_DEFAULT_MESSAGE_LENGTH.into());

		MessageTransaction {
			dispatch_weight: extra_bytes_in_payload
				.saturating_mul(ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT)
				.saturating_add(DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT)
				.saturating_sub(if include_pay_dispatch_fee_cost {
					0
				} else {
					PAY_INBOUND_DISPATCH_FEE_WEIGHT
				})
				.saturating_add(message_dispatch_weight),
			size: message_payload_len
				.saturating_add(EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> crate::Balance {
		// current fee multiplier is used here
		bridge_runtime_common::messages::transaction_payment(
			crate::BlockWeights::get()
				.get(frame_support::weights::DispatchClass::Normal)
				.base_extrinsic,
			crate::TransactionByteFee::get(),
			pallet_transaction_payment::Pallet::<crate::Runtime>::next_fee_multiplier(),
			|weight| WeightToFee::calc(&weight),
			transaction,
		)
	}
}

impl<B, GI> TargetHeaderChain<messages_source::FromThisChainMessagePayload<B>, crate::AccountId>
	for RococoLikeChain<B, GI>
where
	B: MessageBridge,
	B::ThisChain: ChainWithMessages<AccountId = crate::AccountId>,
	B::BridgedChain: ChainWithMessages<Hash = crate::Hash>,
	GI: 'static,
	crate::Runtime: pallet_bridge_grandpa::Config<GI>,
	<<crate::Runtime as pallet_bridge_grandpa::Config<GI>>::BridgedChain as bp_runtime::Chain>::Hash: From<crate::Hash>,
{
	type Error = &'static str;
	type MessagesDeliveryProof = messages_source::FromBridgedChainMessagesDeliveryProof<crate::Hash>;

	fn verify_message(payload: &messages_source::FromThisChainMessagePayload<B>) -> Result<(), Self::Error> {
		messages_source::verify_chain_message::<B>(payload)
	}

	fn verify_messages_delivery_proof(
		proof: Self::MessagesDeliveryProof,
	) -> Result<(LaneId, InboundLaneData<crate::AccountId>), Self::Error> {
		messages_source::verify_messages_delivery_proof::<B, crate::Runtime, GI>(proof)
	}
}

impl<B, GI> SourceHeaderChain<crate::Balance> for RococoLikeChain<B, GI>
where
	B: MessageBridge,
	B::BridgedChain: ChainWithMessages<Balance = crate::Balance, Hash = crate::Hash>,
	GI: 'static,
	crate::Runtime: pallet_bridge_grandpa::Config<GI>,
	<<crate::Runtime as pallet_bridge_grandpa::Config<GI>>::BridgedChain as bp_runtime::Chain>::Hash: From<crate::Hash>,
{
	type Error = &'static str;
	type MessagesProof = messages_target::FromBridgedChainMessagesProof<crate::Hash>;

	fn verify_messages_proof(
		proof: Self::MessagesProof,
		messages_count: u32,
	) -> Result<ProvedMessages<Message<crate::Balance>>, Self::Error> {
		messages_target::verify_messages_proof::<B, crate::Runtime, GI>(proof, messages_count).and_then(verify_inbound_messages_lane)
	}
}

/// Verification of Rococo/Wococo messages at the source chain.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct RococoLikeMessageVerifier<B> {
	_marker: PhantomData<B>,
}

impl<B>
	LaneMessageVerifier<
		crate::AccountId,
		messages_source::FromThisChainMessagePayload<B>,
		crate::Balance,
	> for RococoLikeMessageVerifier<B>
where
	B: MessageBridge,
	bridge_runtime_common::messages::ThisChain<B>: bridge_runtime_common::messages::ChainWithMessages<
		AccountId = crate::AccountId,
		Balance = crate::Balance,
	>,
{
	type Error = &'static str;

	fn verify_message(
		submitter: &Sender<crate::AccountId>,
		delivery_and_dispatch_fee: &crate::Balance,
		lane: &LaneId,
		lane_outbound_data: &OutboundLaneData,
		payload: &messages_source::FromThisChainMessagePayload<B>,
	) -> Result<(), Self::Error> {
		match *lane {
			REGULAR_LANE => messages_source::FromThisChainMessageVerifier::<B>::verify_message(
				submitter,
				delivery_and_dispatch_fee,
				lane,
				lane_outbound_data,
				payload,
			),
			XCM_LANE => {
				// @gavofyork: apart from fee checks (that we now ignore for XCM messages), this function
				// checks that: there aren't many queued (undelivered) messages at the lane. Otherwise the
				// storage may grow too large. I you need it, please add check here e.g. with:
				//
				// let max_pending_messages = ThisChain::<B>::maximal_pending_messages_at_outbound_lane();
				// let pending_messages = lane_outbound_data
				//     .latest_generated_nonce
				//     .saturating_sub(lane_outbound_data.latest_received_nonce);
				// if pending_messages > max_pending_messages {
				//     return Err(TOO_MANY_PENDING_MESSAGES)
				// }
				Ok(())
			}
			_ => Err(messages_source::OUTBOUND_LANE_DISABLED),
		}
	}
}

/// Dispatch of Rococo/Wococo messages at the target chain.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct RococoLikeMessageDispatch<B, ThisDispatchInstance> {
	_marker: PhantomData<(B, ThisDispatchInstance)>,
}

impl<B: MessageBridge + ProcessXcm, ThisDispatchInstance>
	MessageDispatch<crate::AccountId, crate::Balance>
for RococoLikeMessageDispatch<B, ThisDispatchInstance> where
	ThisDispatchInstance: 'static,
	crate::Runtime: pallet_bridge_dispatch::Config<
		ThisDispatchInstance,
		BridgeMessageId = (LaneId, MessageNonce),
		SourceChainAccountId = crate::AccountId,
	>,
	pallet_bridge_dispatch::Pallet<crate::Runtime, ThisDispatchInstance>:
		bp_message_dispatch::MessageDispatch<
			crate::AccountId,
			(LaneId, MessageNonce),
			Message = messages_target::FromBridgedChainMessagePayload<B>,
		>,
{
	type DispatchPayload = bridge_runtime_common::messages::target::FromBridgedChainMessagePayload<B>;

	fn dispatch_weight(
		message: &DispatchMessage<Self::DispatchPayload, crate::Balance>,
	) -> frame_support::weights::Weight {
		message.data.payload.as_ref().map(|payload| payload.weight).unwrap_or(0)
	}

	fn dispatch(
		relayer_account: &crate::AccountId,
		message: DispatchMessage<Self::DispatchPayload, crate::Balance>,
	) -> MessageDispatchResult {
		match message.key.lane_id {
			REGULAR_LANE => {
				let message_id = (message.key.lane_id, message.key.nonce);
				pallet_bridge_dispatch::Pallet::<crate::Runtime, ThisDispatchInstance>::dispatch(
					B::BRIDGED_CHAIN_ID,
					B::THIS_CHAIN_ID,
					message_id,
					message.data.payload.map_err(drop),
					|dispatch_origin, dispatch_weight| {
						let unadjusted_weight_fee = <crate::Runtime as pallet_transaction_payment::Config>::WeightToFee::calc(&dispatch_weight);
						let fee_multiplier =
							pallet_transaction_payment::Pallet::<crate::Runtime>::next_fee_multiplier();
						let adjusted_weight_fee =
							fee_multiplier.saturating_mul_int(unadjusted_weight_fee);
						if !adjusted_weight_fee.is_zero() {
							<pallet_balances::Pallet::<crate::Runtime> as Currency<crate::AccountId>>::transfer(
								dispatch_origin,
								relayer_account,
								adjusted_weight_fee,
								frame_support::traits::ExistenceRequirement::AllowDeath,
							)
							.map_err(drop)
						} else {
							Ok(())
						}
					},
				)
			},
			XCM_LANE => {
				if let Ok(opaque_xcm) = message.data.payload {
					B::process_xcm(opaque_xcm.call.into_encoded_call());
				} else {
					// this means that something is misconfigured in the bridge or chain runtimes are incompatible
					// don't bother now about that.
					log::trace!(target: "runtime::bridge-xcm", "Failed to decode payload with XCM :/");
				}

				// @gavofyork: don't bother abouth these fields now. The only field that may be important to you
				// is `dispatch_result`, which is then delivered back to the source chain.
				MessageDispatchResult {
					dispatch_result: true,
					unspent_weight: 0,
					dispatch_fee_paid_during_dispatch: false,
				}
			},
			_ => {
				// we shall never reach this line, because messages from forbidden lanes are
				// rejected earlier (see `verify_inbound_messages_lane`)
				MessageDispatchResult {
					dispatch_result: false,
					unspent_weight: 0,
					dispatch_fee_paid_during_dispatch: false,
				}
			},
		}
	}
}

/// Error that happens when we are receiving incoming message via unexpected lane.
const INBOUND_LANE_DISABLED: &str = "The inbound message lane is disabled.";

/// Verify that lanes of inbound messages are enabled.
fn verify_inbound_messages_lane(
	messages: ProvedMessages<Message<crate::Balance>>,
) -> Result<ProvedMessages<Message<crate::Balance>>, &'static str> {
	let allowed_incoming_lanes = [REGULAR_LANE, XCM_LANE];
	if messages.keys().any(|lane_id| !allowed_incoming_lanes.contains(lane_id)) {
		return Err(INBOUND_LANE_DISABLED)
	}
	Ok(messages)
}

/// The cost of delivery confirmation transaction.
pub struct GetDeliveryConfirmationTransactionFee;

impl Get<crate::Balance> for GetDeliveryConfirmationTransactionFee {
	fn get() -> crate::Balance {
		<RococoAtRococo as ThisChainWithMessages>::transaction_payment(
			RococoAtRococo::estimate_delivery_confirmation_transaction(),
		)
	}
}

/// This module contains definitions that are used by the messages pallet instance, "deployed" at Rococo.
mod at_rococo {
	use super::*;

	/// Message bridge that is "deployed" at Rococo chain and connecting it to Wococo chain.
	#[derive(RuntimeDebug, Clone, Copy)]
	pub struct AtRococoWithWococoMessageBridge;

	impl MessageBridge for AtRococoWithWococoMessageBridge {
		const THIS_CHAIN_ID: ChainId = ROCOCO_CHAIN_ID;
		const BRIDGED_CHAIN_ID: ChainId = WOCOCO_CHAIN_ID;
		const RELAYER_FEE_PERCENT: u32 = 10;
		const BRIDGED_MESSAGES_PALLET_NAME: &'static str =
			bp_wococo::WITH_ROCOCO_MESSAGES_PALLET_NAME;

		type ThisChain = RococoAtRococo;
		type BridgedChain = WococoAtRococo;

		fn bridged_balance_to_this_balance(
			bridged_balance: bp_wococo::Balance,
		) -> bp_rococo::Balance {
			bridged_balance
		}
	}

	impl ProcessXcm for AtRococoWithWococoMessageBridge {
		fn process_xcm(opaque_xcm: Vec<u8>) {
			process_xcm_from_wococo_to_rococo(opaque_xcm)
		}
	}

	/// Message payload for Rococo -> Wococo messages as it is seen at the Rococo.
	pub type ToWococoMessagePayload =
		messages_source::FromThisChainMessagePayload<AtRococoWithWococoMessageBridge>;

	/// Message verifier for Rococo -> Wococo messages at Rococo.
	pub type ToWococoMessageVerifier = RococoLikeMessageVerifier<AtRococoWithWococoMessageBridge>;

	/// Message payload for Wococo -> Rococo messages as it is seen at Rococo.
	pub type FromWococoMessagePayload =
		messages_target::FromBridgedChainMessagePayload<AtRococoWithWococoMessageBridge>;

	/// Encoded Rococo Call as it comes from Wococo.
	pub type FromWococoEncodedCall =
		messages_target::FromBridgedChainEncodedMessageCall<crate::Call>;

	/// Call-dispatch based message dispatch for Wococo -> Rococo messages.
	pub type FromWococoMessageDispatch = RococoLikeMessageDispatch<
		AtRococoWithWococoMessageBridge,
		crate::AtRococoFromWococoMessagesDispatch,
	>;
}

/// This module contains definitions that are used by the messages pallet instance, "deployed" at Wococo.
mod at_wococo {
	use super::*;

	/// Message bridge that is "deployed" at Wococo chain and connecting it to Rococo chain.
	#[derive(RuntimeDebug, Clone, Copy)]
	pub struct AtWococoWithRococoMessageBridge;

	impl MessageBridge for AtWococoWithRococoMessageBridge {
		const THIS_CHAIN_ID: ChainId = WOCOCO_CHAIN_ID;
		const BRIDGED_CHAIN_ID: ChainId = ROCOCO_CHAIN_ID;
		const RELAYER_FEE_PERCENT: u32 = 10;
		const BRIDGED_MESSAGES_PALLET_NAME: &'static str =
			bp_rococo::WITH_WOCOCO_MESSAGES_PALLET_NAME;

		type ThisChain = WococoAtWococo;
		type BridgedChain = RococoAtWococo;

		fn bridged_balance_to_this_balance(
			bridged_balance: bp_rococo::Balance,
		) -> bp_wococo::Balance {
			bridged_balance
		}
	}

	impl ProcessXcm for AtWococoWithRococoMessageBridge {
		fn process_xcm(opaque_xcm: Vec<u8>) {
			process_xcm_from_rococo_to_wococo(opaque_xcm)
		}
	}

	/// Message payload for Wococo -> Rococo messages as it is seen at the Wococo.
	pub type ToRococoMessagePayload =
		messages_source::FromThisChainMessagePayload<AtWococoWithRococoMessageBridge>;

	/// Message verifier for Wococo -> Rococo messages at Wococo.
	pub type ToRococoMessageVerifier = RococoLikeMessageVerifier<AtRococoWithWococoMessageBridge>;

	/// Message payload for Rococo -> Wococo messages as it is seen at Wococo.
	pub type FromRococoMessagePayload =
		messages_target::FromBridgedChainMessagePayload<AtWococoWithRococoMessageBridge>;

	/// Encoded Wococo Call as it comes from Rococo.
	pub type FromRococoEncodedCall =
		messages_target::FromBridgedChainEncodedMessageCall<crate::Call>;

	/// Call-dispatch based message dispatch for Rococo -> Wococo messages.
	pub type FromRococoMessageDispatch = RococoLikeMessageDispatch<
		AtWococoWithRococoMessageBridge,
		crate::AtWococoFromRococoMessagesDispatch,
	>;
}

/// Just a trigger for sending XCM from Rococo to Wococo - remove it.
#[derive(RuntimeDebug, Clone, Encode, Decode, PartialEq, Eq, TypeInfo)]
pub enum FromRococoToWococoXcmTrigger {
	/// Send XCM from Rococo to Wococo.
	SendXcm(Vec<u8>),
}

impl MessagesParameter for FromRococoToWococoXcmTrigger {
	fn save(&self) {
		match *self {
			FromRococoToWococoXcmTrigger::SendXcm(ref xcm) => {
				let result = send_xcm_from_rococo_to_wococo(xcm.clone());
				log::trace!(target: "runtime::bridge-xcm", "Send XCM result: {:?}", result);
			},
		}
	}
}

/// Just a trigger for sending XCM from Wococo to Rococo - remove it.
#[derive(RuntimeDebug, Clone, Encode, Decode, PartialEq, Eq, TypeInfo)]
pub enum FromWococoToRococoXcmTrigger {
	/// Send XCM from Wococo to Rococo.
	SendXcm(Vec<u8>),
}

impl MessagesParameter for FromWococoToRococoXcmTrigger {
	fn save(&self) {
		match *self {
			FromWococoToRococoXcmTrigger::SendXcm(ref xcm) => {
				let result = send_xcm_from_wococo_to_rococo(xcm.clone());
				log::trace!(target: "runtime::bridge-xcm", "Send XCM result: {:?}", result);
			},
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use bp_messages::{target_chain::ProvedLaneMessages, MessageData, MessageKey};
	use bridge_runtime_common::messages;
	use parity_scale_codec::{Decode, Encode};
	use sp_runtime::traits::TrailingZeroInput;

	#[test]
	fn ensure_rococo_messages_weights_are_correct() {
		// **NOTE**: the main purpose of this test is to be sure that any message that is sumbitted
		// to (any) inbound lane in Rococo<>Wococo bridge can be delivered to the bridged chain.
		// Since we deal with testnets here, in case of failure + urgency:
		//
		// 1) ping bridges team about this failure (see the CODEOWNERS file if you're unsure who to ping);
		// 2) comment/#[ignore] the test.

		// we don't have any knowledge of messages-at-Rococo weights, so we'll be using
		// weights of one of our testnets, which should be accurate enough
		type Weights = pallet_bridge_messages::weights::RialtoWeight<crate::Runtime>;

		pallet_bridge_messages::ensure_weights_are_correct::<Weights>(
			DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT,
			ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT,
			MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
			PAY_INBOUND_DISPATCH_FEE_WEIGHT,
			crate::RocksDbWeight::get(),
		);

		let max_incoming_message_proof_size = bp_rococo::EXTRA_STORAGE_PROOF_SIZE.saturating_add(
			messages::target::maximal_incoming_message_size(bp_rococo::max_extrinsic_size()),
		);
		pallet_bridge_messages::ensure_able_to_receive_message::<Weights>(
			bp_rococo::max_extrinsic_size(),
			bp_rococo::max_extrinsic_weight(),
			max_incoming_message_proof_size,
			messages::target::maximal_incoming_message_dispatch_weight(
				bp_rococo::max_extrinsic_weight(),
			),
		);

		let max_incoming_inbound_lane_data_proof_size =
			bp_messages::InboundLaneData::<()>::encoded_size_hint(
				bp_rococo::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
				bp_rococo::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE as _,
				bp_rococo::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE as _,
			)
			.unwrap_or(u32::MAX);
		pallet_bridge_messages::ensure_able_to_receive_confirmation::<Weights>(
			bp_rococo::max_extrinsic_size(),
			bp_rococo::max_extrinsic_weight(),
			max_incoming_inbound_lane_data_proof_size,
			bp_rococo::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
			bp_rococo::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE,
			crate::RocksDbWeight::get(),
		);
	}

	#[test]
	fn ensure_rococo_tx_extra_bytes_constant_is_correct() {
		// **NOTE**: this test checks that we're computing transaction fee (for bridged chain, which, in
		// case of Rococo<>Wococo, means any chain) on-chain properly. If this assert fails:
		//
		// 1) just fix the `TX_EXTRA_BYTES` constant to actual (or sightly rounded up) value;
		// 2) (only if it has changed significantly (> x2 times)) ping the bridges team (see the CODEOWNERS
		//    file if you're unsure who to ping)

		let signed_extra: crate::SignedExtra = (
			frame_system::CheckNonZeroSender::new(),
			frame_system::CheckSpecVersion::new(),
			frame_system::CheckTxVersion::new(),
			frame_system::CheckGenesis::new(),
			frame_system::CheckMortality::from(sp_runtime::generic::Era::mortal(
				u64::MAX,
				u64::MAX,
			)),
			frame_system::CheckNonce::from(primitives::v1::Nonce::MAX),
			frame_system::CheckWeight::new(),
			pallet_transaction_payment::ChargeTransactionPayment::from(
				primitives::v1::Balance::MAX,
			),
		);
		let mut zeroes = TrailingZeroInput::zeroes();
		let extra_bytes_in_transaction = signed_extra.encoded_size() +
			crate::Address::decode(&mut zeroes).unwrap().encoded_size() +
			crate::Signature::decode(&mut zeroes).unwrap().encoded_size();
		assert!(
			TX_EXTRA_BYTES as usize >= extra_bytes_in_transaction,
			"Hardcoded number of extra bytes in Rococo transaction {} is lower than actual value: {}",
			TX_EXTRA_BYTES,
			extra_bytes_in_transaction,
		);
	}

	fn proved_messages(lane_id: LaneId) -> ProvedMessages<Message<crate::Balance>> {
		vec![(
			lane_id,
			ProvedLaneMessages {
				lane_state: None,
				messages: vec![Message {
					key: MessageKey { lane_id, nonce: 0 },
					data: MessageData { payload: vec![], fee: 0 },
				}],
			},
		)]
		.into_iter()
		.collect()
	}

	#[test]
	fn verify_inbound_messages_lane_succeeds() {
		assert_eq!(
			verify_inbound_messages_lane(proved_messages(REGULAR_LANE)),
			Ok(proved_messages(REGULAR_LANE)),
		);
	}

	#[test]
	fn verify_inbound_messages_lane_fails() {
		assert_eq!(
			verify_inbound_messages_lane(proved_messages([0, 0, 0, 1])),
			Err(INBOUND_LANE_DISABLED),
		);

		let proved_messages = proved_messages(REGULAR_LANE)
			.into_iter()
			.chain(proved_messages([0, 0, 0, 1]))
			.collect();
		assert_eq!(verify_inbound_messages_lane(proved_messages), Err(INBOUND_LANE_DISABLED),);
	}
}
