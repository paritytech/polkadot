// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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
// along with Polkadot. If not, see <http://www.gnu.org/licenses/>.

//! Over-bridge messaging support for Polkadot <> Kusama bridge.

use crate::Runtime;

use bp_messages::{
	source_chain::TargetHeaderChain,
	target_chain::{ProvedMessages, SourceHeaderChain},
	InboundLaneData, LaneId, Message, MessageNonce, Parameter as MessagesParameter,
};
use bp_runtime::{ChainId, POLKADOT_CHAIN_ID, KUSAMA_CHAIN_ID};
use bridge_runtime_common::messages::{self, MessageBridge, MessageTransaction, ThisChainWithMessages};
use parity_scale_codec::{Decode, Encode};
use frame_support::{
	parameter_types,
	traits::Get,
	weights::{DispatchClass, Weight, WeightToFeePolynomial},
	RuntimeDebug,
};
use sp_runtime::{traits::Saturating, FixedPointNumber, FixedU128};
use sp_std::{convert::TryFrom, ops::RangeInclusive};

/// Initial value of `KusamaToPolkadotConversionRate` parameter.
pub const INITIAL_KUSAMA_TO_POLKADOT_CONVERSION_RATE: FixedU128 = FixedU128::from_inner(FixedU128::DIV);
/// Initial value of `KusamaFeeMultiplier` parameter.
pub const INITIAL_KUSAMA_FEE_MULTIPLIER: FixedU128 = FixedU128::from_inner(FixedU128::DIV);

parameter_types! {
	/// Kusama (DOT) to Polkadot (KSM) conversion rate.
	pub storage KusamaToPolkadotConversionRate: FixedU128 = INITIAL_KUSAMA_TO_POLKADOT_CONVERSION_RATE;
	/// Fee multiplier at Kusama.
	pub storage KusamaFeeMultiplier: FixedU128 = INITIAL_KUSAMA_FEE_MULTIPLIER;
}

/// Message payload for Polkadot -> Kusama messages.
pub type ToKusamaMessagePayload = messages::source::FromThisChainMessagePayload<WithKusamaMessageBridge>;

/// Message verifier for Polkadot -> Kusama messages.
pub type ToKusamaMessageVerifier = messages::source::FromThisChainMessageVerifier<WithKusamaMessageBridge>;

/// Message payload for Kusama -> Polkadot messages.
pub type FromKusamaMessagePayload = messages::target::FromBridgedChainMessagePayload<WithKusamaMessageBridge>;

/// Encoded Polkadot Call as it comes from Kusama.
pub type FromKusamaEncodedCall = messages::target::FromBridgedChainEncodedMessageCall<crate::Call>;

/// Call-dispatch based message dispatch for Kusama -> Polkadot messages.
pub type FromKusamaMessageDispatch = messages::target::FromBridgedChainMessageDispatch<
	WithKusamaMessageBridge,
	crate::Runtime,
	pallet_balances::Pallet<Runtime>,
	crate::KusamaMessagesDispatchInstance,
>;

/// Polkadot <-> Kusama message bridge.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct WithKusamaMessageBridge;

impl MessageBridge for WithKusamaMessageBridge {
	const RELAYER_FEE_PERCENT: u32 = 10;
	const THIS_CHAIN_ID: ChainId = POLKADOT_CHAIN_ID;
	const BRIDGED_CHAIN_ID: ChainId = KUSAMA_CHAIN_ID;
	const BRIDGED_MESSAGES_PALLET_NAME: &'static str = bp_kusama::WITH_POLKADOT_MESSAGES_PALLET_NAME;

	type ThisChain = Polkadot;
	type BridgedChain = Kusama;

	fn bridged_balance_to_this_balance(bridged_balance: bp_kusama::Balance) -> bp_polkadot::Balance {
		bp_polkadot::Balance::try_from(KusamaToPolkadotConversionRate::get().saturating_mul_int(bridged_balance))
			.unwrap_or(bp_polkadot::Balance::MAX)
	}
}

/// Polkadot from messages point of view.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct Polkadot;

impl messages::ChainWithMessages for Polkadot {
	type Hash = bp_polkadot::Hash;
	type AccountId = bp_polkadot::AccountId;
	type Signer = bp_polkadot::AccountPublic;
	type Signature = bp_polkadot::Signature;
	type Weight = Weight;
	type Balance = bp_polkadot::Balance;
}

impl ThisChainWithMessages for Polkadot {
	type Call = crate::Call;

	fn is_outbound_lane_enabled(lane: &LaneId) -> bool {
		*lane == [0, 0, 0, 0]
	}

	fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
		bp_kusama::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE
	}

	fn estimate_delivery_confirmation_transaction() -> MessageTransaction<Weight> {
		let inbound_data_size = InboundLaneData::<bp_polkadot::AccountId>::encoded_size_hint(
			bp_polkadot::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
			1,
			1,
		)
		.unwrap_or(u32::MAX);

		MessageTransaction {
			dispatch_weight: bp_polkadot::MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
			size: inbound_data_size
				.saturating_add(bp_kusama::EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(bp_polkadot::TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> bp_polkadot::Balance {
		// `transaction` may represent transaction from the future, when multiplier value will
		// be larger, so let's use slightly increased value
		let multiplier = FixedU128::saturating_from_rational(110, 100)
			.saturating_mul(pallet_transaction_payment::Pallet::<Runtime>::next_fee_multiplier());
		let per_byte_fee = crate::TransactionByteFee::get();
		messages::transaction_payment(
			bp_polkadot::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			per_byte_fee,
			multiplier,
			|weight| crate::WeightToFee::calc(&weight),
			transaction,
		)
	}
}

/// Kusama from messages point of view.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct Kusama;

impl messages::ChainWithMessages for Kusama {
	type Hash = bp_kusama::Hash;
	type AccountId = bp_kusama::AccountId;
	type Signer = bp_kusama::AccountPublic;
	type Signature = bp_kusama::Signature;
	type Weight = Weight;
	type Balance = bp_kusama::Balance;
}

impl messages::BridgedChainWithMessages for Kusama {
	fn maximal_extrinsic_size() -> u32 {
		bp_kusama::max_extrinsic_size()
	}

	fn message_weight_limits(_message_payload: &[u8]) -> RangeInclusive<Weight> {
		// we don't want to relay too large messages + keep reserve for future upgrades
		let upper_limit = messages::target::maximal_incoming_message_dispatch_weight(
			bp_kusama::max_extrinsic_weight(),
		);

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
				.saturating_mul(bp_kusama::ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT)
				.saturating_add(bp_kusama::DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT)
				.saturating_sub(if include_pay_dispatch_fee_cost {
					0
				} else {
					bp_kusama::PAY_INBOUND_DISPATCH_FEE_WEIGHT
				})
				.saturating_add(message_dispatch_weight),
			size: message_payload_len
				.saturating_add(bp_polkadot::EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(bp_kusama::TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> bp_kusama::Balance {
		// we don't have a direct access to the value of multiplier of Kusama chain
		// => it is a messages module parameter
		let multiplier = KusamaFeeMultiplier::get();
		let per_byte_fee = bp_kusama::TRANSACTION_BYTE_FEE;
		messages::transaction_payment(
			bp_kusama::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			per_byte_fee,
			multiplier,
			|weight| bp_kusama::WeightToFee::calc(&weight),
			transaction,
		)
	}
}

impl TargetHeaderChain<ToKusamaMessagePayload, bp_kusama::AccountId> for Kusama {
	type Error = &'static str;
	type MessagesDeliveryProof = messages::source::FromBridgedChainMessagesDeliveryProof<bp_kusama::Hash>;

	fn verify_message(payload: &ToKusamaMessagePayload) -> Result<(), Self::Error> {
		messages::source::verify_chain_message::<WithKusamaMessageBridge>(payload)
	}

	fn verify_messages_delivery_proof(
		proof: Self::MessagesDeliveryProof,
	) -> Result<(LaneId, InboundLaneData<bp_polkadot::AccountId>), Self::Error> {
		messages::source::verify_messages_delivery_proof::<
			WithKusamaMessageBridge,
			Runtime,
			crate::KusamaGrandpaInstance,
		>(proof)
	}
}

impl SourceHeaderChain<bp_kusama::Balance> for Kusama {
	type Error = &'static str;
	type MessagesProof = messages::target::FromBridgedChainMessagesProof<bp_kusama::Hash>;

	fn verify_messages_proof(
		proof: Self::MessagesProof,
		messages_count: u32,
	) -> Result<ProvedMessages<Message<bp_kusama::Balance>>, Self::Error> {
		messages::target::verify_messages_proof::<WithKusamaMessageBridge, Runtime, crate::KusamaGrandpaInstance>(
			proof,
			messages_count,
		)
	}
}

/// Polkadot <> Kusama messages pallet parameters.
#[derive(RuntimeDebug, Clone, Encode, Decode, PartialEq, Eq)]
pub enum WithKusamaMessageBridgeParameter {
	/// The conversion formula we use is: `PolkadotTokens = KusamaTokens * conversion_rate`.
	KusamaToPolkadotConversionRate(FixedU128),
	/// Fee multiplier at the Kusama chain.
	KusamaFeeMultiplier(FixedU128),
}

impl MessagesParameter for WithKusamaMessageBridgeParameter {
	fn save(&self) {
		match *self {
			WithKusamaMessageBridgeParameter::KusamaToPolkadotConversionRate(ref conversion_rate) => {
				KusamaToPolkadotConversionRate::set(conversion_rate);
			},
			WithKusamaMessageBridgeParameter::KusamaFeeMultiplier(ref fee_multiplier) => {
				KusamaFeeMultiplier::set(fee_multiplier);
			},
		}
	}
}

/// The cost of delivery confirmation transaction.
pub struct GetDeliveryConfirmationTransactionFee;

impl Get<bp_polkadot::Balance> for GetDeliveryConfirmationTransactionFee {
	fn get() -> crate::Balance {
		<Polkadot as ThisChainWithMessages>::transaction_payment(
			Polkadot::estimate_delivery_confirmation_transaction(),
		)
	}
}

#[cfg(test)]
mod tests {
	use crate::*;
	use super::*;

	#[test]
	fn ensure_polkadot_message_lane_weights_are_correct() {
		type Weights = pallet_bridge_messages::weights::RialtoWeight<Runtime>; // TODO: use Polkadot weights

		pallet_bridge_messages::ensure_weights_are_correct::<Weights>(
			bp_polkadot::DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT,
			bp_polkadot::ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT,
			bp_polkadot::MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
			bp_polkadot::PAY_INBOUND_DISPATCH_FEE_WEIGHT,
			DbWeight::get(),
		);

		let max_incoming_message_proof_size = bp_kusama::EXTRA_STORAGE_PROOF_SIZE.saturating_add(
			messages::target::maximal_incoming_message_size(bp_polkadot::max_extrinsic_size()),
		);
		pallet_bridge_messages::ensure_able_to_receive_message::<Weights>(
			bp_polkadot::max_extrinsic_size(),
			bp_polkadot::max_extrinsic_weight(),
			max_incoming_message_proof_size,
			messages::target::maximal_incoming_message_dispatch_weight(bp_polkadot::max_extrinsic_weight()),
		);

		let max_incoming_inbound_lane_data_proof_size = bp_messages::InboundLaneData::<()>::encoded_size_hint(
			bp_polkadot::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
			bp_kusama::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE as _,
			bp_kusama::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE as _,
		)
		.unwrap_or(u32::MAX);
		pallet_bridge_messages::ensure_able_to_receive_confirmation::<Weights>(
			bp_polkadot::max_extrinsic_size(),
			bp_polkadot::max_extrinsic_weight(),
			max_incoming_inbound_lane_data_proof_size,
			bp_kusama::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
			bp_kusama::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE,
			DbWeight::get(),
		);
	}
}
