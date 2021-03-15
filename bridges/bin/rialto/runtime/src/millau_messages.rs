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
use bp_runtime::{InstanceId, MILLAU_BRIDGE_INSTANCE};
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
	/// Millau to Rialto conversion rate. Initially we treat both tokens as equal.
	storage MillauToRialtoConversionRate: FixedU128 = 1.into();
}

/// Storage key of the Rialto -> Millau message in the runtime storage.
pub fn message_key(lane: &LaneId, nonce: MessageNonce) -> StorageKey {
	pallet_message_lane::storage_keys::message_key::<Runtime, <Rialto as ChainWithMessageLanes>::MessageLaneInstance>(
		lane, nonce,
	)
}

/// Storage key of the Rialto -> Millau message lane state in the runtime storage.
pub fn outbound_lane_data_key(lane: &LaneId) -> StorageKey {
	pallet_message_lane::storage_keys::outbound_lane_data_key::<<Rialto as ChainWithMessageLanes>::MessageLaneInstance>(
		lane,
	)
}

/// Storage key of the Millau -> Rialto message lane state in the runtime storage.
pub fn inbound_lane_data_key(lane: &LaneId) -> StorageKey {
	pallet_message_lane::storage_keys::inbound_lane_data_key::<
		Runtime,
		<Rialto as ChainWithMessageLanes>::MessageLaneInstance,
	>(lane)
}

/// Message payload for Rialto -> Millau messages.
pub type ToMillauMessagePayload = messages::source::FromThisChainMessagePayload<WithMillauMessageBridge>;

/// Message verifier for Rialto -> Millau messages.
pub type ToMillauMessageVerifier = messages::source::FromThisChainMessageVerifier<WithMillauMessageBridge>;

/// Message payload for Millau -> Rialto messages.
pub type FromMillauMessagePayload = messages::target::FromBridgedChainMessagePayload<WithMillauMessageBridge>;

/// Encoded Rialto Call as it comes from Millau.
pub type FromMillauEncodedCall = messages::target::FromBridgedChainEncodedMessageCall<WithMillauMessageBridge>;

/// Call-dispatch based message dispatch for Millau -> Rialto messages.
pub type FromMillauMessageDispatch = messages::target::FromBridgedChainMessageDispatch<
	WithMillauMessageBridge,
	crate::Runtime,
	pallet_bridge_call_dispatch::DefaultInstance,
>;

/// Messages proof for Millau -> Rialto messages.
pub type FromMillauMessagesProof = messages::target::FromBridgedChainMessagesProof<bp_millau::Hash>;

/// Messages delivery proof for Rialto -> Millau messages.
pub type ToMillauMessagesDeliveryProof = messages::source::FromBridgedChainMessagesDeliveryProof<bp_millau::Hash>;

/// Millau <-> Rialto message bridge.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct WithMillauMessageBridge;

impl MessageBridge for WithMillauMessageBridge {
	const INSTANCE: InstanceId = MILLAU_BRIDGE_INSTANCE;

	const RELAYER_FEE_PERCENT: u32 = 10;

	type ThisChain = Rialto;
	type BridgedChain = Millau;

	fn maximal_extrinsic_size_on_target_chain() -> u32 {
		bp_millau::max_extrinsic_size()
	}

	fn weight_limits_of_message_on_bridged_chain(_message_payload: &[u8]) -> RangeInclusive<Weight> {
		// we don't want to relay too large messages + keep reserve for future upgrades
		let upper_limit = messages::target::maximal_incoming_message_dispatch_weight(bp_millau::max_extrinsic_weight());

		// we're charging for payload bytes in `WithMillauMessageBridge::weight_of_delivery_transaction` function
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
			bp_millau::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			message_payload_len.saturating_add(bp_rialto::EXTRA_STORAGE_PROOF_SIZE as _),
			extra_bytes_in_payload
				.saturating_mul(bp_millau::ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT)
				.saturating_add(bp_millau::DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT),
		)
	}

	fn weight_of_delivery_confirmation_transaction_on_this_chain() -> Weight {
		let inbounded_data_size: Weight =
			InboundLaneData::<bp_millau::AccountId>::encoded_size_hint(bp_millau::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE, 1)
				.map(Into::into)
				.unwrap_or(Weight::MAX);

		messages::transaction_weight_without_multiplier(
			bp_millau::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			inbounded_data_size.saturating_add(bp_millau::EXTRA_STORAGE_PROOF_SIZE as _),
			bp_rialto::MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
		)
	}

	fn this_weight_to_this_balance(weight: Weight) -> bp_rialto::Balance {
		<crate::Runtime as pallet_transaction_payment::Config>::WeightToFee::calc(&weight)
	}

	fn bridged_weight_to_bridged_balance(weight: Weight) -> bp_millau::Balance {
		// we're using the same weights in both chains now
		<crate::Runtime as pallet_transaction_payment::Config>::WeightToFee::calc(&weight) as _
	}

	fn bridged_balance_to_this_balance(bridged_balance: bp_millau::Balance) -> bp_rialto::Balance {
		bp_rialto::Balance::try_from(MillauToRialtoConversionRate::get().saturating_mul_int(bridged_balance))
			.unwrap_or(bp_rialto::Balance::MAX)
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
	type Call = crate::Call;
	type Weight = Weight;
	type Balance = bp_rialto::Balance;

	type MessageLaneInstance = crate::WithMillauMessageLaneInstance;
}

impl messages::ThisChainWithMessageLanes for Rialto {
	fn is_outbound_lane_enabled(lane: &LaneId) -> bool {
		*lane == LaneId::default()
	}

	fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
		MessageNonce::MAX
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
	type Call = (); // unknown to us
	type Weight = Weight;
	type Balance = bp_millau::Balance;

	type MessageLaneInstance = pallet_message_lane::DefaultInstance;
}

impl TargetHeaderChain<ToMillauMessagePayload, bp_millau::AccountId> for Millau {
	type Error = &'static str;
	// The proof is:
	// - hash of the header this proof has been created with;
	// - the storage proof of one or several keys;
	// - id of the lane we prove state of.
	type MessagesDeliveryProof = ToMillauMessagesDeliveryProof;

	fn verify_message(payload: &ToMillauMessagePayload) -> Result<(), Self::Error> {
		messages::source::verify_chain_message::<WithMillauMessageBridge>(payload)
	}

	fn verify_messages_delivery_proof(
		proof: Self::MessagesDeliveryProof,
	) -> Result<(LaneId, InboundLaneData<bp_rialto::AccountId>), Self::Error> {
		messages::source::verify_messages_delivery_proof::<WithMillauMessageBridge, Runtime>(proof)
	}
}

impl SourceHeaderChain<bp_millau::Balance> for Millau {
	type Error = &'static str;
	// The proof is:
	// - hash of the header this proof has been created with;
	// - the storage proof of one or several keys;
	// - id of the lane we prove messages for;
	// - inclusive range of messages nonces that are proved.
	type MessagesProof = FromMillauMessagesProof;

	fn verify_messages_proof(
		proof: Self::MessagesProof,
		messages_count: u32,
	) -> Result<ProvedMessages<Message<bp_millau::Balance>>, Self::Error> {
		messages::target::verify_messages_proof::<WithMillauMessageBridge, Runtime>(proof, messages_count)
	}
}

/// Rialto -> Millau message lane pallet parameters.
#[derive(RuntimeDebug, Clone, Encode, Decode, PartialEq, Eq)]
pub enum RialtoToMillauMessageLaneParameter {
	/// The conversion formula we use is: `RialtoTokens = MillauTokens * conversion_rate`.
	MillauToRialtoConversionRate(FixedU128),
}

impl MessageLaneParameter for RialtoToMillauMessageLaneParameter {
	fn save(&self) {
		match *self {
			RialtoToMillauMessageLaneParameter::MillauToRialtoConversionRate(ref conversion_rate) => {
				MillauToRialtoConversionRate::set(conversion_rate)
			}
		}
	}
}
