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

//! Everything required to serve Millau <-> Rialto messages.

use crate::Runtime;

use bp_messages::{
	source_chain::TargetHeaderChain,
	target_chain::{ProvedMessages, SourceHeaderChain},
	InboundLaneData, LaneId, Message, MessageNonce, Parameter as MessagesParameter,
};
use bp_runtime::{InstanceId, RIALTO_BRIDGE_INSTANCE};
use bridge_runtime_common::messages::{self, MessageBridge, MessageTransaction};
use codec::{Decode, Encode};
use frame_support::{
	parameter_types,
	weights::{DispatchClass, Weight},
	RuntimeDebug,
};
use sp_runtime::{traits::Zero, FixedPointNumber, FixedU128};
use sp_std::{convert::TryFrom, ops::RangeInclusive};

/// Initial value of `RialtoToMillauConversionRate` parameter.
pub const INITIAL_RIALTO_TO_MILLAU_CONVERSION_RATE: FixedU128 = FixedU128::from_inner(FixedU128::DIV);

parameter_types! {
	/// Rialto to Millau conversion rate. Initially we treat both tokens as equal.
	pub storage RialtoToMillauConversionRate: FixedU128 = INITIAL_RIALTO_TO_MILLAU_CONVERSION_RATE;
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
	pallet_bridge_dispatch::DefaultInstance,
>;

/// Millau <-> Rialto message bridge.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct WithRialtoMessageBridge;

impl MessageBridge for WithRialtoMessageBridge {
	const INSTANCE: InstanceId = RIALTO_BRIDGE_INSTANCE;

	const RELAYER_FEE_PERCENT: u32 = 10;

	type ThisChain = Millau;
	type BridgedChain = Rialto;

	fn bridged_balance_to_this_balance(bridged_balance: bp_rialto::Balance) -> bp_millau::Balance {
		bp_millau::Balance::try_from(RialtoToMillauConversionRate::get().saturating_mul_int(bridged_balance))
			.unwrap_or(bp_millau::Balance::MAX)
	}
}

/// Millau chain from message lane point of view.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct Millau;

impl messages::ChainWithMessages for Millau {
	type Hash = bp_millau::Hash;
	type AccountId = bp_millau::AccountId;
	type Signer = bp_millau::AccountSigner;
	type Signature = bp_millau::Signature;
	type Weight = Weight;
	type Balance = bp_millau::Balance;

	type MessagesInstance = crate::WithRialtoMessagesInstance;
}

impl messages::ThisChainWithMessages for Millau {
	type Call = crate::Call;

	fn is_outbound_lane_enabled(lane: &LaneId) -> bool {
		*lane == [0, 0, 0, 0] || *lane == [0, 0, 0, 1]
	}

	fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
		MessageNonce::MAX
	}

	fn estimate_delivery_confirmation_transaction() -> MessageTransaction<Weight> {
		let inbound_data_size =
			InboundLaneData::<bp_millau::AccountId>::encoded_size_hint(bp_millau::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE, 1)
				.unwrap_or(u32::MAX);

		MessageTransaction {
			dispatch_weight: bp_millau::MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
			size: inbound_data_size
				.saturating_add(bp_rialto::EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(bp_millau::TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> bp_millau::Balance {
		// in our testnets, both per-byte fee and weight-to-fee are 1:1
		messages::transaction_payment(
			bp_millau::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			1,
			FixedU128::zero(),
			|weight| weight as _,
			transaction,
		)
	}
}

/// Rialto chain from message lane point of view.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct Rialto;

impl messages::ChainWithMessages for Rialto {
	type Hash = bp_rialto::Hash;
	type AccountId = bp_rialto::AccountId;
	type Signer = bp_rialto::AccountSigner;
	type Signature = bp_rialto::Signature;
	type Weight = Weight;
	type Balance = bp_rialto::Balance;

	type MessagesInstance = pallet_bridge_messages::DefaultInstance;
}

impl messages::BridgedChainWithMessages for Rialto {
	fn maximal_extrinsic_size() -> u32 {
		bp_rialto::max_extrinsic_size()
	}

	fn message_weight_limits(_message_payload: &[u8]) -> RangeInclusive<Weight> {
		// we don't want to relay too large messages + keep reserve for future upgrades
		let upper_limit = messages::target::maximal_incoming_message_dispatch_weight(bp_rialto::max_extrinsic_weight());

		// we're charging for payload bytes in `WithRialtoMessageBridge::transaction_payment` function
		//
		// this bridge may be used to deliver all kind of messages, so we're not making any assumptions about
		// minimal dispatch weight here

		0..=upper_limit
	}

	fn estimate_delivery_transaction(
		message_payload: &[u8],
		message_dispatch_weight: Weight,
	) -> MessageTransaction<Weight> {
		let message_payload_len = u32::try_from(message_payload.len()).unwrap_or(u32::MAX);
		let extra_bytes_in_payload = Weight::from(message_payload_len)
			.saturating_sub(pallet_bridge_messages::EXPECTED_DEFAULT_MESSAGE_LENGTH.into());

		MessageTransaction {
			dispatch_weight: extra_bytes_in_payload
				.saturating_mul(bp_rialto::ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT)
				.saturating_add(bp_rialto::DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT)
				.saturating_add(message_dispatch_weight),
			size: message_payload_len
				.saturating_add(bp_millau::EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(bp_rialto::TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> bp_rialto::Balance {
		// in our testnets, both per-byte fee and weight-to-fee are 1:1
		messages::transaction_payment(
			bp_rialto::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			1,
			FixedU128::zero(),
			|weight| weight as _,
			transaction,
		)
	}
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
		messages::source::verify_messages_delivery_proof::<WithRialtoMessageBridge, Runtime, crate::RialtoGrandpaInstance>(
			proof,
		)
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
		messages::target::verify_messages_proof::<WithRialtoMessageBridge, Runtime, crate::RialtoGrandpaInstance>(
			proof,
			messages_count,
		)
	}
}

/// Millau -> Rialto message lane pallet parameters.
#[derive(RuntimeDebug, Clone, Encode, Decode, PartialEq, Eq)]
pub enum MillauToRialtoMessagesParameter {
	/// The conversion formula we use is: `MillauTokens = RialtoTokens * conversion_rate`.
	RialtoToMillauConversionRate(FixedU128),
}

impl MessagesParameter for MillauToRialtoMessagesParameter {
	fn save(&self) {
		match *self {
			MillauToRialtoMessagesParameter::RialtoToMillauConversionRate(ref conversion_rate) => {
				RialtoToMillauConversionRate::set(conversion_rate)
			}
		}
	}
}
