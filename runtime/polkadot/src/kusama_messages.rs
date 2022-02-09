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

use crate::{AccountId, Balance, Balances, Call, Origin, OriginCaller, RootAccountForPayments, Runtime};

use bp_messages::{
	source_chain::{LaneMessageVerifier, SenderOrigin, TargetHeaderChain},
	target_chain::{ProvedMessages, SourceHeaderChain},
	InboundLaneData, LaneId, Message, MessageNonce, OutboundLaneData, Parameter as MessagesParameter,
};
use bp_runtime::{Chain, ChainId, POLKADOT_CHAIN_ID, KUSAMA_CHAIN_ID};
use bridge_runtime_common::messages::{
	source as messages_source, target as messages_target,
	BridgedChainWithMessages, ChainWithMessages, MessageBridge,
	MessageTransaction, ThisChainWithMessages,
	transaction_payment,
};
use parity_scale_codec::{Decode, Encode};
use frame_support::{
	parameter_types,
	traits::{Contains, Get},
	weights::{DispatchClass, Weight, WeightToFeePolynomial},
	RuntimeDebug,
};
use scale_info::TypeInfo;
use sp_runtime::{traits::Saturating, FixedPointNumber, FixedU128};
use sp_std::{convert::TryFrom, ops::RangeInclusive};

#[cfg(feature = "runtime-benchmarks")]
use crate::Event;
#[cfg(feature = "runtime-benchmarks")]
use bp_polkadot::{Hasher, Header};
#[cfg(feature = "runtime-benchmarks")]
use bridge_runtime_common::messages_benchmarking::{
	dispatch_account,
	prepare_message_delivery_proof,
	prepare_message_proof,
	prepare_outbound_message,
};
#[cfg(feature = "runtime-benchmarks")]
use frame_support::traits::Currency;
#[cfg(feature = "runtime-benchmarks")]
use pallet_bridge_messages::benchmarking::{
	Config as MessagesConfig,
	MessageDeliveryProofParams,
	MessageParams,
	MessageProofParams,
};

/// Initial value of `KusamaToPolkadotConversionRate` parameter.
pub const INITIAL_KUSAMA_TO_POLKADOT_CONVERSION_RATE: FixedU128 = FixedU128::from_inner(FixedU128::DIV);
/// Initial value of `KusamaFeeMultiplier` parameter.
pub const INITIAL_KUSAMA_FEE_MULTIPLIER: FixedU128 = FixedU128::from_inner(FixedU128::DIV);

parameter_types! {
	/// Kusama (DOT) to Polkadot (KSM) conversion rate.
	pub storage KusamaToPolkadotConversionRate: FixedU128 = INITIAL_KUSAMA_TO_POLKADOT_CONVERSION_RATE;
	/// Fee multiplier at Kusama.
	pub storage KusamaFeeMultiplier: FixedU128 = INITIAL_KUSAMA_FEE_MULTIPLIER;
	/// The only Polkadot account that is allowed to send messages to Kusama.
	pub storage AllowedMessageSender: Option<bp_polkadot::AccountId> = None;
}

/// Message payload for Polkadot -> Kusama messages.
pub type ToKusamaMessagePayload = messages_source::FromThisChainMessagePayload<WithKusamaMessageBridge>;

/// Message payload for Kusama -> Polkadot messages.
pub type FromKusamaMessagePayload = messages_target::FromBridgedChainMessagePayload<WithKusamaMessageBridge>;

/// Encoded Polkadot Call as it comes from Kusama.
pub type FromKusamaEncodedCall = messages_target::FromBridgedChainEncodedMessageCall<crate::Call>;

/// Call-dispatch based message dispatch for Kusama -> Polkadot messages.
pub type FromKusamaMessageDispatch = messages_target::FromBridgedChainMessageDispatch<
	WithKusamaMessageBridge,
	crate::Runtime,
	pallet_balances::Pallet<Runtime>,
	crate::KusamaMessagesDispatchInstance,
>;

/// Error that happens when message is sent by anyone but `AllowedMessageSender`.
#[cfg(not(feature = "runtime-benchmarks"))]
const NOT_ALLOWED_MESSAGE_SENDER: &str = "Cannot accept message from this account";
/// Error that happens when we are receiving incoming message via unexpected lane.
const INBOUND_LANE_DISABLED: &str = "The inbound message lane is disaled.";

/// Message verifier for Polkadot -> Kusama messages.
#[derive(RuntimeDebug)]
pub struct ToKusamaMessageVerifier;

impl LaneMessageVerifier<
	Origin,
	bp_polkadot::AccountId,
	ToKusamaMessagePayload,
	bp_polkadot::Balance,
> for ToKusamaMessageVerifier {
	type Error = &'static str;

	fn verify_message(
		submitter: &Origin,
		delivery_and_dispatch_fee: &bp_polkadot::Balance,
		lane: &LaneId,
		lane_outbound_data: &OutboundLaneData,
		payload: &ToKusamaMessagePayload,
	) -> Result<(), Self::Error> {
		// we only allow messages to be sent by given account
		let allowed_sender = AllowedMessageSender::get();
		// for benchmarks we're still interested in this additional storage, read, but we don't
		// want actual checks
		#[cfg(feature = "runtime-benchmarks")]
		drop(allowed_sender);
		// outside of benchmarks, we only allow messages to be sent by given account
		#[cfg(not(feature = "runtime-benchmarks"))]
		{
			match allowed_sender {
				Some(ref allowed_sender) if submitter.linked_account().as_ref() == Some(allowed_sender) => (),
				_ => return Err(NOT_ALLOWED_MESSAGE_SENDER),
			}
		}

		// perform other checks
		messages_source::FromThisChainMessageVerifier::<WithKusamaMessageBridge>::verify_message(
			submitter,
			delivery_and_dispatch_fee,
			lane,
			lane_outbound_data,
			payload,
		)
	}
}

/// Polkadot <-> Kusama message bridge.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct WithKusamaMessageBridge;

impl MessageBridge for WithKusamaMessageBridge {
	const RELAYER_FEE_PERCENT: u32 = 10;
	const THIS_CHAIN_ID: ChainId = POLKADOT_CHAIN_ID;
	const BRIDGED_CHAIN_ID: ChainId = KUSAMA_CHAIN_ID;
	const BRIDGED_MESSAGES_PALLET_NAME: &'static str = bp_polkadot::WITH_POLKADOT_MESSAGES_PALLET_NAME;

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

impl ChainWithMessages for Polkadot {
	type Hash = bp_polkadot::Hash;
	type AccountId = bp_polkadot::AccountId;
	type Signer = bp_polkadot::AccountPublic;
	type Signature = bp_polkadot::Signature;
	type Weight = Weight;
	type Balance = bp_polkadot::Balance;
}

impl ThisChainWithMessages for Polkadot {
	type Call = crate::Call;
	type Origin = crate::Origin;

	fn is_message_accepted(submitter: &crate::Origin, lane: &LaneId) -> bool {
		*lane == [0, 0, 0, 0] && submitter.linked_account().is_some()
	}

	fn maximal_pending_messages_at_outbound_lane() -> MessageNonce {
		bp_kusama::MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX
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
		transaction_payment(
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

impl ChainWithMessages for Kusama {
	type Hash = bp_kusama::Hash;
	type AccountId = bp_kusama::AccountId;
	type Signer = bp_kusama::AccountPublic;
	type Signature = bp_kusama::Signature;
	type Weight = Weight;
	type Balance = bp_kusama::Balance;
}

impl BridgedChainWithMessages for Kusama {
	fn maximal_extrinsic_size() -> u32 {
		bp_kusama::Kusama::max_extrinsic_size()
	}

	fn message_weight_limits(_message_payload: &[u8]) -> RangeInclusive<Weight> {
		// we don't want to relay too large messages + keep reserve for future upgrades
		let upper_limit = messages_target::maximal_incoming_message_dispatch_weight(
			bp_kusama::Kusama::max_extrinsic_weight(),
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
		transaction_payment(
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
	type MessagesDeliveryProof = messages_source::FromBridgedChainMessagesDeliveryProof<bp_kusama::Hash>;

	fn verify_message(payload: &ToKusamaMessagePayload) -> Result<(), Self::Error> {
		messages_source::verify_chain_message::<WithKusamaMessageBridge>(payload)
	}

	fn verify_messages_delivery_proof(
		proof: Self::MessagesDeliveryProof,
	) -> Result<(LaneId, InboundLaneData<bp_polkadot::AccountId>), Self::Error> {
		messages_source::verify_messages_delivery_proof::<
			WithKusamaMessageBridge,
			Runtime,
			crate::KusamaGrandpaInstance,
		>(proof)
	}
}

impl SourceHeaderChain<bp_kusama::Balance> for Kusama {
	type Error = &'static str;
	type MessagesProof = messages_target::FromBridgedChainMessagesProof<bp_kusama::Hash>;

	fn verify_messages_proof(
		proof: Self::MessagesProof,
		messages_count: u32,
	) -> Result<ProvedMessages<Message<bp_kusama::Balance>>, Self::Error> {
		messages_target::verify_messages_proof::<
			WithKusamaMessageBridge,
			Runtime,
			crate::KusamaGrandpaInstance,
		>(proof, messages_count).and_then(verify_inbound_messages_lane)
	}
}

/// Verify that lanes of inbound messages are enabled.
fn verify_inbound_messages_lane(
	messages: ProvedMessages<Message<bp_polkadot::Balance>>,
) -> Result<ProvedMessages<Message<bp_polkadot::Balance>>, &'static str> {
	let allowed_incoming_lanes = [[0, 0, 0, 0]];
	if messages.keys().any(|lane_id| !allowed_incoming_lanes.contains(lane_id)) {
		return Err(INBOUND_LANE_DISABLED);
	}
	Ok(messages)
}

impl SenderOrigin<AccountId> for Origin {
	fn linked_account(&self) -> Option<AccountId> {
		match self.caller {
			OriginCaller::system(frame_system::RawOrigin::Signed(ref submitter)) =>
				Some(submitter.clone()),
			OriginCaller::system(frame_system::RawOrigin::Root) |
			OriginCaller::system(frame_system::RawOrigin::None) =>
				RootAccountForPayments::get(),
			OriginCaller::Council(_) => AllowedMessageSender::get(), // TODO: rename to council account,
				_ => None,
		}
	}
}

/// Polkadot <> Kusama messages pallet parameters.
#[derive(RuntimeDebug, Clone, Encode, Decode, PartialEq, Eq, TypeInfo)]
pub enum WithKusamaMessageBridgeParameter {
	/// The conversion formula we use is: `PolkadotTokens = KusamaTokens * conversion_rate`.
	KusamaToPolkadotConversionRate(FixedU128),
	/// Fee multiplier at the Kusama chain.
	KusamaFeeMultiplier(FixedU128),
	/// The only Polkadot account that is allowed to send messages to Kusama.
	AllowedMessageSender(Option<bp_polkadot::AccountId>),
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
			WithKusamaMessageBridgeParameter::AllowedMessageSender(ref message_sender) => {
				AllowedMessageSender::set(message_sender);
			}
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

/// Call filter for messages that are coming from Kusama.
pub struct FromKusamaCallFilter;

impl Contains<Call> for FromKusamaCallFilter {
	fn contains(call: &Call) -> bool {
		#[cfg(feature = "runtime-benchmarks")]
		{
			drop(call);
			true
		}
		#[cfg(not(feature = "runtime-benchmarks"))]
		matches!(call, Call::Balances(pallet_balances::Call::transfer { .. }))
	}
}

#[cfg(feature = "runtime-benchmarks")]
impl MessagesConfig<crate::WithKusamaMessagesInstance> for Runtime {
	fn maximal_message_size() -> u32 {
		messages_source::maximal_message_size::<WithKusamaMessageBridge>()
	}

	fn bridged_relayer_id() -> Self::InboundRelayer {
		[0u8; 32].into()
	}

	fn account_balance(account: &Self::AccountId) -> Self::OutboundMessageFee {
		Balances::free_balance(account)
	}

	fn endow_account(account: &Self::AccountId) {
		Balances::make_free_balance_be(account, Balance::MAX / 100);
	}

	fn prepare_outbound_message(
		params: MessageParams<Self::AccountId>,
	) -> (ToKusamaMessagePayload, Balance) {
		(prepare_outbound_message::<WithKusamaMessageBridge>(params), Self::message_fee())
	}

	fn prepare_message_proof(
		params: MessageProofParams,
	) -> (messages_target::FromBridgedChainMessagesProof<crate::Hash>, bp_messages::Weight) {
		Self::endow_account(&dispatch_account::<WithKusamaMessageBridge>());
		prepare_message_proof::<Runtime, (), (), WithKusamaMessageBridge, Header, Hasher>(
			params,
			&crate::VERSION,
			Balance::MAX / 100,
		)
	}

	fn prepare_message_delivery_proof(
		params: MessageDeliveryProofParams<Self::AccountId>,
	) -> messages_source::FromBridgedChainMessagesDeliveryProof<crate::Hash> {
		prepare_message_delivery_proof::<Runtime, (), WithKusamaMessageBridge, Header, Hasher>(
			params,
		)
	}

	fn is_message_dispatched(nonce: bp_messages::MessageNonce) -> bool {
		frame_system::Pallet::<Runtime>::events()
			.into_iter()
			.map(|event_record| event_record.event)
			.any(|event| matches!(
				event,
				Event::BridgeKusamaMessagesDispatch(pallet_bridge_dispatch::Event::<Runtime, _>::MessageDispatched(
					_, ([0, 0, 0, 0], nonce_from_event), _,
				)) if nonce_from_event == nonce
			))
	}
}

#[cfg(test)]
mod tests {
	use bp_messages::{target_chain::ProvedLaneMessages, MessageData, MessageKey};
	use bridge_runtime_common::messages::source::estimate_message_dispatch_and_delivery_fee;
	use frame_support::weights::GetDispatchInfo;
	use runtime_common::RocksDbWeight;
	use crate::*;
	use super::*;

	fn message_payload(sender: bp_polkadot::AccountId) -> ToKusamaMessagePayload {
		let call = Call::Balances(pallet_balances::Call::<Runtime>::transfer {
			dest: bp_polkadot::AccountId::from([0u8; 32]).into(),
			value: 10_000_000_000,
		});
		let weight = call.get_dispatch_info().weight;
		bp_message_dispatch::MessagePayload {
			spec_version: 4242,
			weight,
			origin: bp_message_dispatch::CallOrigin::SourceAccount(sender),
			dispatch_fee_payment: bp_runtime::messages::DispatchFeePayment::AtSourceChain,
			call: call.encode(),
		}
	}

	#[test]
	fn ensure_polkadot_message_lane_weights_are_correct() {
		type Weights = pallet_bridge_messages::weights::MillauWeight<Runtime>; // TODO: use Polkadot weights

		pallet_bridge_messages::ensure_weights_are_correct::<Weights>(
			bp_polkadot::DEFAULT_MESSAGE_DELIVERY_TX_WEIGHT,
			bp_polkadot::ADDITIONAL_MESSAGE_BYTE_DELIVERY_WEIGHT,
			bp_polkadot::MAX_SINGLE_MESSAGE_DELIVERY_CONFIRMATION_TX_WEIGHT,
			bp_polkadot::PAY_INBOUND_DISPATCH_FEE_WEIGHT,
			RocksDbWeight::get(),
		);

		let max_incoming_message_proof_size = bp_kusama::EXTRA_STORAGE_PROOF_SIZE.saturating_add(
			messages_target::maximal_incoming_message_size(bp_polkadot::Polkadot::max_extrinsic_size()),
		);
		pallet_bridge_messages::ensure_able_to_receive_message::<Weights>(
			bp_polkadot::Polkadot::max_extrinsic_size(),
			bp_polkadot::Polkadot::max_extrinsic_weight(),
			max_incoming_message_proof_size,
			messages_target::maximal_incoming_message_dispatch_weight(bp_polkadot::Polkadot::max_extrinsic_weight()),
		);

		let max_incoming_inbound_lane_data_proof_size = bp_messages::InboundLaneData::<()>::encoded_size_hint(
			bp_polkadot::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
			bp_polkadot::MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX as _,
			bp_polkadot::MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX as _,
		)
		.unwrap_or(u32::MAX);
		pallet_bridge_messages::ensure_able_to_receive_confirmation::<Weights>(
			bp_polkadot::Polkadot::max_extrinsic_size(),
			bp_polkadot::Polkadot::max_extrinsic_weight(),
			max_incoming_inbound_lane_data_proof_size,
			bp_polkadot::MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX,
			bp_polkadot::MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX,
			RocksDbWeight::get(),
		);
	}

	#[test]
	fn message_by_invalid_submitter_are_rejected() {
		sp_io::TestExternalities::new(Default::default()).execute_with(|| {
			let invalid_sender = bp_polkadot::AccountId::from([1u8; 32]);
			let valid_sender = bp_polkadot::AccountId::from([2u8; 32]);
			AllowedMessageSender::set(&Some(valid_sender.clone()));

			assert_eq!(
				ToKusamaMessageVerifier::verify_message(
					&frame_system::RawOrigin::Signed(invalid_sender.clone()).into(),
					&bp_polkadot::Balance::MAX,
					&Default::default(),
					&Default::default(),
					&message_payload(invalid_sender),
				),
				Err(NOT_ALLOWED_MESSAGE_SENDER),
			);
			assert_eq!(
				ToKusamaMessageVerifier::verify_message(
					&frame_system::RawOrigin::Signed(valid_sender.clone()).into(),
					&bp_polkadot::Balance::MAX,
					&Default::default(),
					&Default::default(),
					&message_payload(valid_sender),
				),
				Ok(()),
			);
		});
	}

	fn proved_messages(lane_id: LaneId) -> ProvedMessages<Message<bp_kusama::Balance>> {
		vec![
			(
				lane_id,
				ProvedLaneMessages {
					lane_state: None,
					messages: vec![Message {
						key: MessageKey { lane_id, nonce: 0 },
						data: MessageData { payload: vec![], fee: 0 },
					}],
				},
			)
		].into_iter().collect()
	}

	#[test]
	fn verify_inbound_messages_lane_succeeds() {
		assert_eq!(
			verify_inbound_messages_lane(proved_messages([0, 0, 0, 0])),
			Ok(proved_messages([0, 0, 0, 0])),
		);
	}

	#[test]
	fn verify_inbound_messages_lane_fails() {
		assert_eq!(
			verify_inbound_messages_lane(proved_messages([0, 0, 0, 1])),
			Err(INBOUND_LANE_DISABLED),
		);

		let proved_messages = proved_messages([0, 0, 0, 0])
			.into_iter()
			.chain(proved_messages([0, 0, 0, 1]))
			.collect();
		assert_eq!(
			verify_inbound_messages_lane(proved_messages),
			Err(INBOUND_LANE_DISABLED),
		);
	}

	#[test]
	#[ignore]
	fn estimate_polkadot_to_kusama_message_fee() {
		sp_io::TestExternalities::new(Default::default()).execute_with(|| {
			KusamaToPolkadotConversionRate::set(&FixedU128::from_float(0.11188793806675539));
			let fee = estimate_message_dispatch_and_delivery_fee::<WithKusamaMessageBridge>(
				&message_payload(bp_kusama::AccountId::from([1u8; 32])),
				WithKusamaMessageBridge::RELAYER_FEE_PERCENT,
			).unwrap();
			println!(
				"Message fee for balances::transfer call is: {} ({} DOT)",
				fee,
				FixedU128::saturating_from_rational(fee, polkadot_runtime_constants::currency::UNITS).to_float(),
			);
		});
	}
}
