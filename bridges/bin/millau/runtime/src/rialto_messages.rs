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

//! Everything required to serve Millau <-> Rialto message lanes.

use crate::Runtime;

use bp_message_lane::{
	source_chain::TargetHeaderChain,
	target_chain::{ProvedMessages, SourceHeaderChain},
	InboundLaneData, LaneId, Message, MessageNonce, Parameter as MessageLaneParameter,
};
use bp_runtime::{InstanceId, RIALTO_BRIDGE_INSTANCE};
use bridge_runtime_common::messages::{self, ChainWithMessageLanes, MessageBridge};
use codec::{Decode, Encode};
use frame_support::{
	parameter_types,
	weights::{DispatchClass, Weight, WeightToFeePolynomial},
	RuntimeDebug,
};
use sp_core::storage::StorageKey;
use sp_runtime::{FixedPointNumber, FixedU128};
use sp_std::{convert::TryFrom, ops::RangeInclusive};

parameter_types! {
	/// Rialto to Millau conversion rate. Initially we treat both tokens as equal.
	storage RialtoToMillauConversionRate: FixedU128 = 1.into();
}

/// Storage key of the Millau -> Rialto message in the runtime storage.
pub fn message_key(lane: &LaneId, nonce: MessageNonce) -> StorageKey {
	pallet_message_lane::storage_keys::message_key::<Runtime, <Millau as ChainWithMessageLanes>::MessageLaneInstance>(
		lane, nonce,
	)
}

/// Storage key of the Millau -> Rialto message lane state in the runtime storage.
pub fn outbound_lane_data_key(lane: &LaneId) -> StorageKey {
	pallet_message_lane::storage_keys::outbound_lane_data_key::<<Millau as ChainWithMessageLanes>::MessageLaneInstance>(
		lane,
	)
}

/// Storage key of the Rialto -> Millau message lane state in the runtime storage.
pub fn inbound_lane_data_key(lane: &LaneId) -> StorageKey {
	pallet_message_lane::storage_keys::inbound_lane_data_key::<
		Runtime,
		<Millau as ChainWithMessageLanes>::MessageLaneInstance,
	>(lane)
}

/// Message payload for Millau -> Rialto messages.
pub type ToRialtoMessagePayload = messages::source::FromThisChainMessagePayload<WithRialtoMessageBridge>;

/// Message verifier for Millau -> Rialto messages.
pub type ToRialtoMessageVerifier = messages::source::FromThisChainMessageVerifier<WithRialtoMessageBridge>;

/// Message payload for Rialto -> Millau messages.
pub type FromRialtoMessagePayload = messages::target::FromBridgedChainMessagePayload<WithRialtoMessageBridge>;

/// Encoded Millau Call as it comes from Rialto.
pub type FromRialtoEncodedCall = messages::target::FromBridgedChainEncodedMessageCall<WithRialtoMessageBridge>;

/// Messages proof for Rialto -> Millau messages.
type FromRialtoMessagesProof = messages::target::FromBridgedChainMessagesProof<bp_rialto::Hash>;

/// Messages delivery proof for Millau -> Rialto messages.
type ToRialtoMessagesDeliveryProof = messages::source::FromBridgedChainMessagesDeliveryProof<bp_rialto::Hash>;

/// Call-dispatch based message dispatch for Rialto -> Millau messages.
pub type FromRialtoMessageDispatch = messages::target::FromBridgedChainMessageDispatch<
	WithRialtoMessageBridge,
	crate::Runtime,
	pallet_bridge_call_dispatch::DefaultInstance,
>;

/// Millau <-> Rialto message bridge.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct WithRialtoMessageBridge;

impl MessageBridge for WithRialtoMessageBridge {
	const INSTANCE: InstanceId = RIALTO_BRIDGE_INSTANCE;

	const RELAYER_FEE_PERCENT: u32 = 10;

	type ThisChain = Millau;
	type BridgedChain = Rialto;

	fn maximal_extrinsic_size_on_target_chain() -> u32 {
		bp_rialto::max_extrinsic_size()
	}

	fn weight_limits_of_message_on_bridged_chain(_message_payload: &[u8]) -> RangeInclusive<Weight> {
		// we don't want to relay too large messages + keep reserve for future upgrades
		let upper_limit = messages::target::maximal_incoming_message_dispatch_weight(bp_rialto::max_extrinsic_weight());

		// we're charging for payload bytes in `WithRialtoMessageBridge::weight_of_delivery_transaction` function
		//
		// this bridge may be used to deliver all kind of messages, so we're not making any assumptions about
		// minimal dispatch weight here

		0..=upper_limit
	}

	fn weight_of_delivery_transaction(message_payload: &[u8]) -> Weight {
		let message_payload_len = u32::try_from(message_payload.len())
			.map(Into::into)
			.unwrap_or(Weight::MAX);
		let extra_bytes_in_payload =
			message_payload_len.saturating_sub(pallet_message_lane::EXPECTED_DEFAULT_MESSAGE_LENGTH.into());
		messages::transaction_weight_without_multiplier(
			bp_rialto::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			message_payload_len.saturating_add(bp_millau::EXTRA_STORAGE_PROOF_SIZE as _),
			extra_bytes_in_payload
				.saturating_mul(bp_rialto::ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT)
				.saturating_add(bp_rialto::DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT),
		)
	}

	fn weight_of_delivery_confirmation_transaction_on_this_chain() -> Weight {
		let inbounded_data_size: Weight =
			InboundLaneData::<bp_rialto::AccountId>::encoded_size_hint(bp_rialto::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE, 1)
				.map(Into::into)
				.unwrap_or(Weight::MAX);

		messages::transaction_weight_without_multiplier(
			bp_millau::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			inbounded_data_size.saturating_add(bp_rialto::EXTRA_STORAGE_PROOF_SIZE as _),
			bp_millau::MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
		)
	}

	fn this_weight_to_this_balance(weight: Weight) -> bp_millau::Balance {
		<crate::Runtime as pallet_transaction_payment::Config>::WeightToFee::calc(&weight)
	}

	fn bridged_weight_to_bridged_balance(weight: Weight) -> bp_rialto::Balance {
		// we're using the same weights in both chains now
		<crate::Runtime as pallet_transaction_payment::Config>::WeightToFee::calc(&weight) as _
	}

	fn bridged_balance_to_this_balance(bridged_balance: bp_rialto::Balance) -> bp_millau::Balance {
		bp_millau::Balance::try_from(RialtoToMillauConversionRate::get().saturating_mul_int(bridged_balance))
			.unwrap_or(bp_millau::Balance::MAX)
	}
}

/// Millau chain from message lane point of view.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct Millau;

impl messages::ChainWithMessageLanes for Millau {
	type Hash = bp_millau::Hash;
	type AccountId = bp_millau::AccountId;
	type Signer = bp_millau::AccountSigner;
	type Signature = bp_millau::Signature;
	type Call = crate::Call;
	type Weight = Weight;
	type Balance = bp_millau::Balance;

	type MessageLaneInstance = pallet_message_lane::DefaultInstance;
}

impl messages::ThisChainWithMessageLanes for Millau {
	fn is_outbound_lane_enabled(lane: &LaneId) -> bool {
		*lane == LaneId::default()
	}

	fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
		MessageNonce::MAX
	}
}

/// Rialto chain from message lane point of view.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct Rialto;

impl messages::ChainWithMessageLanes for Rialto {
	type Hash = bp_rialto::Hash;
	type AccountId = bp_rialto::AccountId;
	type Signer = bp_rialto::AccountSigner;
	type Signature = bp_rialto::Signature;
	type Call = (); // unknown to us
	type Weight = Weight;
	type Balance = bp_rialto::Balance;

	type MessageLaneInstance = pallet_message_lane::DefaultInstance;
}

impl TargetHeaderChain<ToRialtoMessagePayload, bp_rialto::AccountId> for Rialto {
	type Error = &'static str;
	// The proof is:
	// - hash of the header this proof has been created with;
	// - the storage proof or one or several keys;
	// - id of the lane we prove state of.
	type MessagesDeliveryProof = ToRialtoMessagesDeliveryProof;

	fn verify_message(payload: &ToRialtoMessagePayload) -> Result<(), Self::Error> {
		messages::source::verify_chain_message::<WithRialtoMessageBridge>(payload)
	}

	fn verify_messages_delivery_proof(
		proof: Self::MessagesDeliveryProof,
	) -> Result<(LaneId, InboundLaneData<bp_millau::AccountId>), Self::Error> {
		messages::source::verify_messages_delivery_proof::<WithRialtoMessageBridge, Runtime>(proof)
	}
}

impl SourceHeaderChain<bp_rialto::Balance> for Rialto {
	type Error = &'static str;
	// The proof is:
	// - hash of the header this proof has been created with;
	// - the storage proof or one or several keys;
	// - id of the lane we prove messages for;
	// - inclusive range of messages nonces that are proved.
	type MessagesProof = FromRialtoMessagesProof;

	fn verify_messages_proof(
		proof: Self::MessagesProof,
		messages_count: u32,
	) -> Result<ProvedMessages<Message<bp_rialto::Balance>>, Self::Error> {
		messages::target::verify_messages_proof::<WithRialtoMessageBridge, Runtime>(proof, messages_count)
	}
}

/// Millau -> Rialto message lane pallet parameters.
#[derive(RuntimeDebug, Clone, Encode, Decode, PartialEq, Eq)]
pub enum MillauToRialtoMessageLaneParameter {
	/// The conversion formula we use is: `MillauTokens = RialtoTokens * conversion_rate`.
	RialtoToMillauConversionRate(FixedU128),
}

impl MessageLaneParameter for MillauToRialtoMessageLaneParameter {
	fn save(&self) {
		match *self {
			MillauToRialtoMessageLaneParameter::RialtoToMillauConversionRate(ref conversion_rate) => {
				RialtoToMillauConversionRate::set(conversion_rate)
			}
		}
	}
}
