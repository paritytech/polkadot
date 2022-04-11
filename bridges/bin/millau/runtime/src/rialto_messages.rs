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
	source_chain::{SenderOrigin, TargetHeaderChain},
	target_chain::{ProvedMessages, SourceHeaderChain},
	InboundLaneData, LaneId, Message, MessageNonce, Parameter as MessagesParameter,
};
use bp_runtime::{Chain, ChainId, MILLAU_CHAIN_ID, RIALTO_CHAIN_ID};
use bridge_runtime_common::messages::{self, MessageBridge, MessageTransaction};
use codec::{Decode, Encode};
use frame_support::{
	parameter_types,
	weights::{DispatchClass, Weight},
	RuntimeDebug,
};
use scale_info::TypeInfo;
use sp_runtime::{traits::Saturating, FixedPointNumber, FixedU128};
use sp_std::{convert::TryFrom, ops::RangeInclusive};

/// Initial value of `RialtoToMillauConversionRate` parameter.
pub const INITIAL_RIALTO_TO_MILLAU_CONVERSION_RATE: FixedU128 =
	FixedU128::from_inner(FixedU128::DIV);
/// Initial value of `RialtoFeeMultiplier` parameter.
pub const INITIAL_RIALTO_FEE_MULTIPLIER: FixedU128 = FixedU128::from_inner(FixedU128::DIV);

parameter_types! {
	/// Rialto to Millau conversion rate. Initially we treat both tokens as equal.
	pub storage RialtoToMillauConversionRate: FixedU128 = INITIAL_RIALTO_TO_MILLAU_CONVERSION_RATE;
	/// Fee multiplier value at Rialto chain.
	pub storage RialtoFeeMultiplier: FixedU128 = INITIAL_RIALTO_FEE_MULTIPLIER;
}

/// Message payload for Millau -> Rialto messages.
pub type ToRialtoMessagePayload =
	messages::source::FromThisChainMessagePayload<WithRialtoMessageBridge>;

/// Message verifier for Millau -> Rialto messages.
pub type ToRialtoMessageVerifier =
	messages::source::FromThisChainMessageVerifier<WithRialtoMessageBridge>;

/// Message payload for Rialto -> Millau messages.
pub type FromRialtoMessagePayload =
	messages::target::FromBridgedChainMessagePayload<WithRialtoMessageBridge>;

/// Encoded Millau Call as it comes from Rialto.
pub type FromRialtoEncodedCall = messages::target::FromBridgedChainEncodedMessageCall<crate::Call>;

/// Messages proof for Rialto -> Millau messages.
pub type FromRialtoMessagesProof = messages::target::FromBridgedChainMessagesProof<bp_rialto::Hash>;

/// Messages delivery proof for Millau -> Rialto messages.
pub type ToRialtoMessagesDeliveryProof =
	messages::source::FromBridgedChainMessagesDeliveryProof<bp_rialto::Hash>;

/// Call-dispatch based message dispatch for Rialto -> Millau messages.
pub type FromRialtoMessageDispatch = messages::target::FromBridgedChainMessageDispatch<
	WithRialtoMessageBridge,
	crate::Runtime,
	pallet_balances::Pallet<Runtime>,
	(),
>;

/// Millau <-> Rialto message bridge.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct WithRialtoMessageBridge;

impl MessageBridge for WithRialtoMessageBridge {
	const RELAYER_FEE_PERCENT: u32 = 10;
	const THIS_CHAIN_ID: ChainId = MILLAU_CHAIN_ID;
	const BRIDGED_CHAIN_ID: ChainId = RIALTO_CHAIN_ID;
	const BRIDGED_MESSAGES_PALLET_NAME: &'static str = bp_millau::WITH_MILLAU_MESSAGES_PALLET_NAME;

	type ThisChain = Millau;
	type BridgedChain = Rialto;

	fn bridged_balance_to_this_balance(
		bridged_balance: bp_rialto::Balance,
		bridged_to_this_conversion_rate_override: Option<FixedU128>,
	) -> bp_millau::Balance {
		let conversion_rate = bridged_to_this_conversion_rate_override
			.unwrap_or_else(|| RialtoToMillauConversionRate::get());
		bp_millau::Balance::try_from(conversion_rate.saturating_mul_int(bridged_balance))
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
}

impl messages::ThisChainWithMessages for Millau {
	type Origin = crate::Origin;
	type Call = crate::Call;

	fn is_message_accepted(send_origin: &Self::Origin, lane: &LaneId) -> bool {
		// lanes 0x00000000 && 0x00000001 are accepting any paid messages, while
		// `TokenSwapMessageLane` only accepts messages from token swap pallet
		let token_swap_dedicated_lane = crate::TokenSwapMessagesLane::get();
		match *lane {
			[0, 0, 0, 0] | [0, 0, 0, 1] => send_origin.linked_account().is_some(),
			_ if *lane == token_swap_dedicated_lane => matches!(
				send_origin.caller,
				crate::OriginCaller::BridgeRialtoTokenSwap(
					pallet_bridge_token_swap::RawOrigin::TokenSwap { .. }
				)
			),
			_ => false,
		}
	}

	fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
		MessageNonce::MAX
	}

	fn estimate_delivery_confirmation_transaction() -> MessageTransaction<Weight> {
		let inbound_data_size = InboundLaneData::<bp_millau::AccountId>::encoded_size_hint(
			bp_millau::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
			1,
			1,
		)
		.unwrap_or(u32::MAX);

		MessageTransaction {
			dispatch_weight: bp_millau::MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
			size: inbound_data_size
				.saturating_add(bp_rialto::EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(bp_millau::TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> bp_millau::Balance {
		// `transaction` may represent transaction from the future, when multiplier value will
		// be larger, so let's use slightly increased value
		let multiplier = FixedU128::saturating_from_rational(110, 100)
			.saturating_mul(pallet_transaction_payment::Pallet::<Runtime>::next_fee_multiplier());
		// in our testnets, both per-byte fee and weight-to-fee are 1:1
		messages::transaction_payment(
			bp_millau::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			1,
			multiplier,
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
}

impl messages::BridgedChainWithMessages for Rialto {
	fn maximal_extrinsic_size() -> u32 {
		bp_rialto::Rialto::max_extrinsic_size()
	}

	fn message_weight_limits(_message_payload: &[u8]) -> RangeInclusive<Weight> {
		// we don't want to relay too large messages + keep reserve for future upgrades
		let upper_limit = messages::target::maximal_incoming_message_dispatch_weight(
			bp_rialto::Rialto::max_extrinsic_weight(),
		);

		// we're charging for payload bytes in `WithRialtoMessageBridge::transaction_payment`
		// function
		//
		// this bridge may be used to deliver all kind of messages, so we're not making any
		// assumptions about minimal dispatch weight here

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
				.saturating_mul(bp_rialto::ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT)
				.saturating_add(bp_rialto::DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT)
				.saturating_sub(if include_pay_dispatch_fee_cost {
					0
				} else {
					bp_rialto::PAY_INBOUND_DISPATCH_FEE_WEIGHT
				})
				.saturating_add(message_dispatch_weight),
			size: message_payload_len
				.saturating_add(bp_millau::EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(bp_rialto::TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> bp_rialto::Balance {
		// we don't have a direct access to the value of multiplier at Rialto chain
		// => it is a messages module parameter
		let multiplier = RialtoFeeMultiplier::get();
		// in our testnets, both per-byte fee and weight-to-fee are 1:1
		messages::transaction_payment(
			bp_rialto::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			1,
			multiplier,
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
		messages::source::verify_messages_delivery_proof::<
			WithRialtoMessageBridge,
			Runtime,
			crate::RialtoGrandpaInstance,
		>(proof)
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
		messages::target::verify_messages_proof::<
			WithRialtoMessageBridge,
			Runtime,
			crate::RialtoGrandpaInstance,
		>(proof, messages_count)
	}
}

impl SenderOrigin<crate::AccountId> for crate::Origin {
	fn linked_account(&self) -> Option<crate::AccountId> {
		match self.caller {
			crate::OriginCaller::system(frame_system::RawOrigin::Signed(ref submitter)) =>
				Some(submitter.clone()),
			crate::OriginCaller::system(frame_system::RawOrigin::Root) |
			crate::OriginCaller::system(frame_system::RawOrigin::None) =>
				crate::RootAccountForPayments::get(),
			crate::OriginCaller::BridgeRialtoTokenSwap(
				pallet_bridge_token_swap::RawOrigin::TokenSwap {
					ref swap_account_at_this_chain,
					..
				},
			) => Some(swap_account_at_this_chain.clone()),
			_ => None,
		}
	}
}

/// Millau -> Rialto message lane pallet parameters.
#[derive(RuntimeDebug, Clone, Encode, Decode, PartialEq, Eq, TypeInfo)]
pub enum MillauToRialtoMessagesParameter {
	/// The conversion formula we use is: `MillauTokens = RialtoTokens * conversion_rate`.
	RialtoToMillauConversionRate(FixedU128),
}

impl MessagesParameter for MillauToRialtoMessagesParameter {
	fn save(&self) {
		match *self {
			MillauToRialtoMessagesParameter::RialtoToMillauConversionRate(ref conversion_rate) =>
				RialtoToMillauConversionRate::set(conversion_rate),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{DbWeight, RialtoGrandpaInstance, Runtime, WithRialtoMessagesInstance};

	use bp_runtime::Chain;
	use bridge_runtime_common::{
		assert_complete_bridge_types,
		integrity::{
			assert_complete_bridge_constants, AssertBridgeMessagesPalletConstants,
			AssertBridgePalletNames, AssertChainConstants, AssertCompleteBridgeConstants,
		},
		messages,
	};

	#[test]
	fn ensure_millau_message_lane_weights_are_correct() {
		type Weights = pallet_bridge_messages::weights::MillauWeight<Runtime>;

		pallet_bridge_messages::ensure_weights_are_correct::<Weights>(
			bp_millau::DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT,
			bp_millau::ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT,
			bp_millau::MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
			bp_millau::PAY_INBOUND_DISPATCH_FEE_WEIGHT,
			DbWeight::get(),
		);

		let max_incoming_message_proof_size = bp_rialto::EXTRA_STORAGE_PROOF_SIZE.saturating_add(
			messages::target::maximal_incoming_message_size(bp_millau::Millau::max_extrinsic_size()),
		);
		pallet_bridge_messages::ensure_able_to_receive_message::<Weights>(
			bp_millau::Millau::max_extrinsic_size(),
			bp_millau::Millau::max_extrinsic_weight(),
			max_incoming_message_proof_size,
			messages::target::maximal_incoming_message_dispatch_weight(
				bp_millau::Millau::max_extrinsic_weight(),
			),
		);

		let max_incoming_inbound_lane_data_proof_size =
			bp_messages::InboundLaneData::<()>::encoded_size_hint(
				bp_millau::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
				bp_millau::MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX as _,
				bp_millau::MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX as _,
			)
			.unwrap_or(u32::MAX);
		pallet_bridge_messages::ensure_able_to_receive_confirmation::<Weights>(
			bp_millau::Millau::max_extrinsic_size(),
			bp_millau::Millau::max_extrinsic_weight(),
			max_incoming_inbound_lane_data_proof_size,
			bp_millau::MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX,
			bp_millau::MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX,
			DbWeight::get(),
		);
	}

	#[test]
	fn ensure_bridge_integrity() {
		assert_complete_bridge_types!(
			runtime: Runtime,
			with_bridged_chain_grandpa_instance: RialtoGrandpaInstance,
			with_bridged_chain_messages_instance: WithRialtoMessagesInstance,
			bridge: WithRialtoMessageBridge,
			this_chain: bp_millau::Millau,
			bridged_chain: bp_rialto::Rialto,
			this_chain_account_id_converter: bp_millau::AccountIdConverter
		);

		assert_complete_bridge_constants::<
			Runtime,
			RialtoGrandpaInstance,
			WithRialtoMessagesInstance,
			WithRialtoMessageBridge,
			bp_millau::Millau,
		>(AssertCompleteBridgeConstants {
			this_chain_constants: AssertChainConstants {
				block_length: bp_millau::BlockLength::get(),
				block_weights: bp_millau::BlockWeights::get(),
			},
			messages_pallet_constants: AssertBridgeMessagesPalletConstants {
				max_unrewarded_relayers_in_bridged_confirmation_tx:
					bp_rialto::MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX,
				max_unconfirmed_messages_in_bridged_confirmation_tx:
					bp_rialto::MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX,
				bridged_chain_id: bp_runtime::RIALTO_CHAIN_ID,
			},
			pallet_names: AssertBridgePalletNames {
				with_this_chain_messages_pallet_name: bp_millau::WITH_MILLAU_MESSAGES_PALLET_NAME,
				with_bridged_chain_grandpa_pallet_name: bp_rialto::WITH_RIALTO_GRANDPA_PALLET_NAME,
				with_bridged_chain_messages_pallet_name:
					bp_rialto::WITH_RIALTO_MESSAGES_PALLET_NAME,
			},
		});

		assert_eq!(
			RialtoToMillauConversionRate::key().to_vec(),
			bp_runtime::storage_parameter_key(
				bp_millau::RIALTO_TO_MILLAU_CONVERSION_RATE_PARAMETER_NAME
			)
			.0,
		);
	}
}
