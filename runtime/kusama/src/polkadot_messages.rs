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

//! Over-bridge messaging support for Kusama <> Polkadot bridge.

use crate::Runtime;

use bp_messages::{
	source_chain::{LaneMessageVerifier, TargetHeaderChain},
	target_chain::{ProvedMessages, SourceHeaderChain},
	InboundLaneData, LaneId, Message, MessageNonce, OutboundLaneData, Parameter as MessagesParameter,
};
use bp_runtime::{ChainId, KUSAMA_CHAIN_ID, POLKADOT_CHAIN_ID};
use bridge_runtime_common::messages::{self, MessageBridge, MessageTransaction, ThisChainWithMessages};
use parity_scale_codec::{Decode, Encode};
use frame_support::{
	parameter_types,
	traits::Get,
	weights::{DispatchClass, Weight, WeightToFeePolynomial},
	RuntimeDebug,
};
use frame_system::RawOrigin;
use sp_runtime::{traits::Saturating, FixedPointNumber, FixedU128};
use sp_std::{convert::TryFrom, ops::RangeInclusive};

/// Initial value of `PolkadotToKusamaConversionRate` parameter.
pub const INITIAL_POLKADOT_TO_KUSAMA_CONVERSION_RATE: FixedU128 = FixedU128::from_inner(FixedU128::DIV);
/// Initial value of `PolkadotFeeMultiplier` parameter.
pub const INITIAL_POLKADOT_FEE_MULTIPLIER: FixedU128 = FixedU128::from_inner(FixedU128::DIV);

parameter_types! {
	/// Polkadot (DOT) to Kusama (KSM) conversion rate.
	pub storage PolkadotToKusamaConversionRate: FixedU128 = INITIAL_POLKADOT_TO_KUSAMA_CONVERSION_RATE;
	/// Fee multiplier at Polkadot.
	pub storage PolkadotFeeMultiplier: FixedU128 = INITIAL_POLKADOT_FEE_MULTIPLIER;
	/// The only Kusama account that is allowed to send messages to Polkadot.
	pub storage AllowedMessageSender: bp_kusama::AccountId = Default::default();
}

/// Message payload for Kusama -> Polkadot messages.
pub type ToPolkadotMessagePayload = messages::source::FromThisChainMessagePayload<WithPolkadotMessageBridge>;

/// Message payload for Polkadot -> Kusama messages.
pub type FromPolkadotMessagePayload = messages::target::FromBridgedChainMessagePayload<WithPolkadotMessageBridge>;

/// Encoded Kusama Call as it comes from Polkadot.
pub type FromPolkadotEncodedCall = messages::target::FromBridgedChainEncodedMessageCall<crate::Call>;

/// Call-dispatch based message dispatch for Polkadot -> Kusama messages.
pub type FromPolkadotMessageDispatch = messages::target::FromBridgedChainMessageDispatch<
	WithPolkadotMessageBridge,
	crate::Runtime,
	pallet_balances::Pallet<Runtime>,
	crate::PolkadotMessagesDispatchInstance,
>;

/// Error that happens when message is sent by anyone but `AllowedMessageSender`.
const NOT_ALLOWED_MESSAGE_SENDER: &str = "Cannot accept message from this account";

/// Message verifier for Kusama -> Polkadot messages.
#[derive(RuntimeDebug)]
pub struct ToPolkadotMessageVerifier;

impl LaneMessageVerifier<
	bp_kusama::AccountId,
	ToPolkadotMessagePayload,
	bp_kusama::Balance,
> for ToPolkadotMessageVerifier {
	type Error = &'static str;

	fn verify_message(
		submitter: &RawOrigin<bp_kusama::AccountId>,
		delivery_and_dispatch_fee: &bp_kusama::Balance,
		lane: &LaneId,
		lane_outbound_data: &OutboundLaneData,
		payload: &ToPolkadotMessagePayload,
	) -> Result<(), Self::Error> {
		// we only allow messages to be sent by given account
		let allowed_sender = AllowedMessageSender::get();
		if *submitter != RawOrigin::Signed(allowed_sender) {
			return Err(NOT_ALLOWED_MESSAGE_SENDER);
		}

		// perform other checks
		messages::source::FromThisChainMessageVerifier::<WithPolkadotMessageBridge>::verify_message(
			submitter,
			delivery_and_dispatch_fee,
			lane,
			lane_outbound_data,
			payload,
		)
	}
}

/// Kusama <-> Polkadot message bridge.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct WithPolkadotMessageBridge;

impl MessageBridge for WithPolkadotMessageBridge {
	const RELAYER_FEE_PERCENT: u32 = 10;
	const THIS_CHAIN_ID: ChainId = KUSAMA_CHAIN_ID;
	const BRIDGED_CHAIN_ID: ChainId = POLKADOT_CHAIN_ID;
	const BRIDGED_MESSAGES_PALLET_NAME: &'static str = bp_polkadot::WITH_KUSAMA_MESSAGES_PALLET_NAME;

	type ThisChain = Kusama;
	type BridgedChain = Polkadot;

	fn bridged_balance_to_this_balance(bridged_balance: bp_polkadot::Balance) -> bp_kusama::Balance {
		bp_kusama::Balance::try_from(PolkadotToKusamaConversionRate::get().saturating_mul_int(bridged_balance))
			.unwrap_or(bp_kusama::Balance::MAX)
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

impl ThisChainWithMessages for Kusama {
	type Call = crate::Call;

	fn is_outbound_lane_enabled(lane: &LaneId) -> bool {
		*lane == [0, 0, 0, 0]
	}

	fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
		bp_polkadot::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE
	}

	fn estimate_delivery_confirmation_transaction() -> MessageTransaction<Weight> {
		let inbound_data_size = InboundLaneData::<bp_kusama::AccountId>::encoded_size_hint(
			bp_kusama::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
			1,
			1,
		)
		.unwrap_or(u32::MAX);

		MessageTransaction {
			dispatch_weight: bp_kusama::MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
			size: inbound_data_size
				.saturating_add(bp_polkadot::EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(bp_kusama::TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> bp_kusama::Balance {
		// `transaction` may represent transaction from the future, when multiplier value will
		// be larger, so let's use slightly increased value
		let multiplier = FixedU128::saturating_from_rational(110, 100)
			.saturating_mul(pallet_transaction_payment::Pallet::<Runtime>::next_fee_multiplier());
		let per_byte_fee = crate::TransactionByteFee::get();
		messages::transaction_payment(
			bp_kusama::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			per_byte_fee,
			multiplier,
			|weight| crate::WeightToFee::calc(&weight),
			transaction,
		)
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

impl messages::BridgedChainWithMessages for Polkadot {
	fn maximal_extrinsic_size() -> u32 {
		bp_polkadot::max_extrinsic_size()
	}

	fn message_weight_limits(_message_payload: &[u8]) -> RangeInclusive<Weight> {
		// we don't want to relay too large messages + keep reserve for future upgrades
		let upper_limit = messages::target::maximal_incoming_message_dispatch_weight(
			bp_polkadot::max_extrinsic_weight(),
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
				.saturating_mul(bp_polkadot::ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT)
				.saturating_add(bp_polkadot::DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT)
				.saturating_sub(if include_pay_dispatch_fee_cost {
					0
				} else {
					bp_polkadot::PAY_INBOUND_DISPATCH_FEE_WEIGHT
				})
				.saturating_add(message_dispatch_weight),
			size: message_payload_len
				.saturating_add(bp_kusama::EXTRA_STORAGE_PROOF_SIZE)
				.saturating_add(bp_polkadot::TX_EXTRA_BYTES),
		}
	}

	fn transaction_payment(transaction: MessageTransaction<Weight>) -> bp_polkadot::Balance {
		// we don't have a direct access to the value of multiplier of Polkadot chain
		// => it is a messages module parameter
		let multiplier = PolkadotFeeMultiplier::get();
		let per_byte_fee = bp_polkadot::TRANSACTION_BYTE_FEE;
		messages::transaction_payment(
			bp_polkadot::BlockWeights::get().get(DispatchClass::Normal).base_extrinsic,
			per_byte_fee,
			multiplier,
			|weight| bp_polkadot::WeightToFee::calc(&weight),
			transaction,
		)
	}
}

impl TargetHeaderChain<ToPolkadotMessagePayload, bp_polkadot::AccountId> for Polkadot {
	type Error = &'static str;
	type MessagesDeliveryProof = messages::source::FromBridgedChainMessagesDeliveryProof<bp_polkadot::Hash>;

	fn verify_message(payload: &ToPolkadotMessagePayload) -> Result<(), Self::Error> {
		messages::source::verify_chain_message::<WithPolkadotMessageBridge>(payload)
	}

	fn verify_messages_delivery_proof(
		proof: Self::MessagesDeliveryProof,
	) -> Result<(LaneId, InboundLaneData<bp_kusama::AccountId>), Self::Error> {
		messages::source::verify_messages_delivery_proof::<
			WithPolkadotMessageBridge,
			Runtime,
			crate::PolkadotGrandpaInstance,
		>(proof)
	}
}

impl SourceHeaderChain<bp_polkadot::Balance> for Polkadot {
	type Error = &'static str;
	type MessagesProof = messages::target::FromBridgedChainMessagesProof<bp_polkadot::Hash>;

	fn verify_messages_proof(
		proof: Self::MessagesProof,
		messages_count: u32,
	) -> Result<ProvedMessages<Message<bp_polkadot::Balance>>, Self::Error> {
		messages::target::verify_messages_proof::<WithPolkadotMessageBridge, Runtime, crate::PolkadotGrandpaInstance>(
			proof,
			messages_count,
		)
	}
}

/// Kusama <> Polkadot messages pallet parameters.
#[derive(RuntimeDebug, Clone, Encode, Decode, PartialEq, Eq)]
pub enum WithPolkadotMessageBridgeParameter {
	/// The conversion formula we use is: `KusamaTokens = PolkadotTokens * conversion_rate`.
	PolkadotToKusamaConversionRate(FixedU128),
	/// Fee multiplier at the Polkadot chain.
	PolkadotFeeMultiplier(FixedU128),
	/// The only Kusama account that is allowed to send messages to Polkadot.
	AllowedMessageSender(bp_kusama::AccountId),
}

impl MessagesParameter for WithPolkadotMessageBridgeParameter {
	fn save(&self) {
		match *self {
			WithPolkadotMessageBridgeParameter::PolkadotToKusamaConversionRate(ref conversion_rate) => {
				PolkadotToKusamaConversionRate::set(conversion_rate);
			},
			WithPolkadotMessageBridgeParameter::PolkadotFeeMultiplier(ref fee_multiplier) => {
				PolkadotFeeMultiplier::set(fee_multiplier);
			},
			WithPolkadotMessageBridgeParameter::AllowedMessageSender(ref message_sender) => {
				AllowedMessageSender::set(message_sender);
			}
		}
	}
}

/// The cost of delivery confirmation transaction.
pub struct GetDeliveryConfirmationTransactionFee;

impl Get<bp_kusama::Balance> for GetDeliveryConfirmationTransactionFee {
	fn get() -> crate::Balance {
		<Kusama as ThisChainWithMessages>::transaction_payment(
			Kusama::estimate_delivery_confirmation_transaction(),
		)
	}
}

#[cfg(test)]
mod tests {
	use runtime_common::RocksDbWeight;
	use crate::*;
	use super::*;

	#[test]
	fn ensure_kusama_message_lane_weights_are_correct() {
		type Weights = pallet_bridge_messages::weights::RialtoWeight<Runtime>; // TODO: use Kusama weights

		pallet_bridge_messages::ensure_weights_are_correct::<Weights>(
			bp_kusama::DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT,
			bp_kusama::ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT,
			bp_kusama::MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
			bp_kusama::PAY_INBOUND_DISPATCH_FEE_WEIGHT,
			RocksDbWeight::get(),
		);

		let max_incoming_message_proof_size = bp_polkadot::EXTRA_STORAGE_PROOF_SIZE.saturating_add(
			messages::target::maximal_incoming_message_size(bp_kusama::max_extrinsic_size()),
		);
		pallet_bridge_messages::ensure_able_to_receive_message::<Weights>(
			bp_kusama::max_extrinsic_size(),
			bp_kusama::max_extrinsic_weight(),
			max_incoming_message_proof_size,
			messages::target::maximal_incoming_message_dispatch_weight(bp_kusama::max_extrinsic_weight()),
		);

		let max_incoming_inbound_lane_data_proof_size = bp_messages::InboundLaneData::<()>::encoded_size_hint(
			bp_kusama::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
			bp_polkadot::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE as _,
			bp_polkadot::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE as _,
		)
		.unwrap_or(u32::MAX);
		pallet_bridge_messages::ensure_able_to_receive_confirmation::<Weights>(
			bp_kusama::max_extrinsic_size(),
			bp_kusama::max_extrinsic_weight(),
			max_incoming_inbound_lane_data_proof_size,
			bp_polkadot::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
			bp_polkadot::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE,
			RocksDbWeight::get(),
		);
	}

	#[test]
	fn message_by_invalid_submitter_are_rejected() {
		sp_io::TestExternalities::new(Default::default()).execute_with(|| {
			fn message_payload(sender: bp_kusama::AccountId) -> ToPolkadotMessagePayload {
				bp_message_dispatch::MessagePayload {
					spec_version: 1,
					weight: 100,
					origin: bp_message_dispatch::CallOrigin::SourceAccount(sender),
					dispatch_fee_payment: bp_runtime::messages::DispatchFeePayment::AtSourceChain,
					call: vec![42],
				}
			}

			let invalid_sender = bp_kusama::AccountId::from([1u8; 32]);
			let valid_sender = AllowedMessageSender::get();
			assert_eq!(
				ToPolkadotMessageVerifier::verify_message(
					&RawOrigin::Signed(invalid_sender.clone()),
					&bp_kusama::Balance::MAX,
					&Default::default(),
					&Default::default(),
					&message_payload(invalid_sender),
				),
				Err(NOT_ALLOWED_MESSAGE_SENDER),
			);
			assert_eq!(
				ToPolkadotMessageVerifier::verify_message(
					&RawOrigin::Signed(valid_sender.clone()),
					&bp_kusama::Balance::MAX,
					&Default::default(),
					&Default::default(),
					&message_payload(valid_sender),
				),
				Ok(()),
			);
		});
	}
}
