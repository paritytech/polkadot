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

//! Tools for supporting message lanes between two Substrate-based chains.

use crate::{
	conversion_rate_update::UpdateConversionRateCallBuilder,
	messages_metrics::StandaloneMessagesMetrics,
	messages_source::{SubstrateMessagesProof, SubstrateMessagesSource},
	messages_target::{SubstrateMessagesDeliveryProof, SubstrateMessagesTarget},
	on_demand_headers::OnDemandHeadersRelay,
	TransactionParams, STALL_TIMEOUT,
};

use bp_messages::{LaneId, MessageNonce};
use bp_runtime::{AccountIdOf, Chain as _};
use bridge_runtime_common::messages::{
	source::FromBridgedChainMessagesDeliveryProof, target::FromBridgedChainMessagesProof,
};
use codec::Encode;
use frame_support::weights::{GetDispatchInfo, Weight};
use messages_relay::{message_lane::MessageLane, relay_strategy::RelayStrategy};
use pallet_bridge_messages::{Call as BridgeMessagesCall, Config as BridgeMessagesConfig};
use relay_substrate_client::{
	transaction_stall_timeout, AccountKeyPairOf, BalanceOf, BlockNumberOf, CallOf, Chain,
	ChainWithMessages, Client, HashOf, TransactionSignScheme,
};
use relay_utils::metrics::MetricsParams;
use sp_core::Pair;
use std::{fmt::Debug, marker::PhantomData};

/// Substrate -> Substrate messages synchronization pipeline.
pub trait SubstrateMessageLane: 'static + Clone + Debug + Send + Sync {
	/// Name of the source -> target tokens conversion rate parameter.
	///
	/// The parameter is stored at the target chain and the storage key is computed using
	/// `bp_runtime::storage_parameter_key` function. If value is unknown, it is assumed
	/// to be 1.
	const SOURCE_TO_TARGET_CONVERSION_RATE_PARAMETER_NAME: Option<&'static str>;
	/// Name of the target -> source tokens conversion rate parameter.
	///
	/// The parameter is stored at the source chain and the storage key is computed using
	/// `bp_runtime::storage_parameter_key` function. If value is unknown, it is assumed
	/// to be 1.
	const TARGET_TO_SOURCE_CONVERSION_RATE_PARAMETER_NAME: Option<&'static str>;

	/// Name of the source chain fee multiplier parameter.
	///
	/// The parameter is stored at the target chain and the storage key is computed using
	/// `bp_runtime::storage_parameter_key` function. If value is unknown, it is assumed
	/// to be 1.
	const SOURCE_FEE_MULTIPLIER_PARAMETER_NAME: Option<&'static str>;
	/// Name of the target chain fee multiplier parameter.
	///
	/// The parameter is stored at the source chain and the storage key is computed using
	/// `bp_runtime::storage_parameter_key` function. If value is unknown, it is assumed
	/// to be 1.
	const TARGET_FEE_MULTIPLIER_PARAMETER_NAME: Option<&'static str>;

	/// Name of the transaction payment pallet, deployed at the source chain.
	const AT_SOURCE_TRANSACTION_PAYMENT_PALLET_NAME: Option<&'static str>;
	/// Name of the transaction payment pallet, deployed at the target chain.
	const AT_TARGET_TRANSACTION_PAYMENT_PALLET_NAME: Option<&'static str>;

	/// Messages of this chain are relayed to the `TargetChain`.
	type SourceChain: ChainWithMessages;
	/// Messages from the `SourceChain` are dispatched on this chain.
	type TargetChain: ChainWithMessages;

	/// Scheme used to sign source chain transactions.
	type SourceTransactionSignScheme: TransactionSignScheme;
	/// Scheme used to sign target chain transactions.
	type TargetTransactionSignScheme: TransactionSignScheme;

	/// How receive messages proof call is built?
	type ReceiveMessagesProofCallBuilder: ReceiveMessagesProofCallBuilder<Self>;
	/// How receive messages delivery proof call is built?
	type ReceiveMessagesDeliveryProofCallBuilder: ReceiveMessagesDeliveryProofCallBuilder<Self>;

	/// `TargetChain` tokens to `SourceChain` tokens conversion rate update builder.
	///
	/// If not applicable to this bridge, you may use `()` here.
	type TargetToSourceChainConversionRateUpdateBuilder: UpdateConversionRateCallBuilder<
		Self::SourceChain,
	>;

	/// Message relay strategy.
	type RelayStrategy: RelayStrategy;
}

/// Adapter that allows all `SubstrateMessageLane` to act as `MessageLane`.
#[derive(Clone, Debug)]
pub(crate) struct MessageLaneAdapter<P: SubstrateMessageLane> {
	_phantom: PhantomData<P>,
}

impl<P: SubstrateMessageLane> MessageLane for MessageLaneAdapter<P> {
	const SOURCE_NAME: &'static str = P::SourceChain::NAME;
	const TARGET_NAME: &'static str = P::TargetChain::NAME;

	type MessagesProof = SubstrateMessagesProof<P::SourceChain>;
	type MessagesReceivingProof = SubstrateMessagesDeliveryProof<P::TargetChain>;

	type SourceChainBalance = BalanceOf<P::SourceChain>;
	type SourceHeaderNumber = BlockNumberOf<P::SourceChain>;
	type SourceHeaderHash = HashOf<P::SourceChain>;

	type TargetHeaderNumber = BlockNumberOf<P::TargetChain>;
	type TargetHeaderHash = HashOf<P::TargetChain>;
}

/// Substrate <-> Substrate messages relay parameters.
pub struct MessagesRelayParams<P: SubstrateMessageLane> {
	/// Messages source client.
	pub source_client: Client<P::SourceChain>,
	/// Source transaction params.
	pub source_transaction_params:
		TransactionParams<AccountKeyPairOf<P::SourceTransactionSignScheme>>,
	/// Messages target client.
	pub target_client: Client<P::TargetChain>,
	/// Target transaction params.
	pub target_transaction_params:
		TransactionParams<AccountKeyPairOf<P::TargetTransactionSignScheme>>,
	/// Optional on-demand source to target headers relay.
	pub source_to_target_headers_relay: Option<OnDemandHeadersRelay<P::SourceChain>>,
	/// Optional on-demand target to source headers relay.
	pub target_to_source_headers_relay: Option<OnDemandHeadersRelay<P::TargetChain>>,
	/// Identifier of lane that needs to be served.
	pub lane_id: LaneId,
	/// Metrics parameters.
	pub metrics_params: MetricsParams,
	/// Pre-registered standalone metrics.
	pub standalone_metrics: Option<StandaloneMessagesMetrics<P::SourceChain, P::TargetChain>>,
	/// Relay strategy.
	pub relay_strategy: P::RelayStrategy,
}

/// Run Substrate-to-Substrate messages sync loop.
pub async fn run<P: SubstrateMessageLane>(params: MessagesRelayParams<P>) -> anyhow::Result<()>
where
	AccountIdOf<P::SourceChain>:
		From<<AccountKeyPairOf<P::SourceTransactionSignScheme> as Pair>::Public>,
	AccountIdOf<P::TargetChain>:
		From<<AccountKeyPairOf<P::TargetTransactionSignScheme> as Pair>::Public>,
	BalanceOf<P::SourceChain>: TryFrom<BalanceOf<P::TargetChain>>,
	P::SourceTransactionSignScheme: TransactionSignScheme<Chain = P::SourceChain>,
	P::TargetTransactionSignScheme: TransactionSignScheme<Chain = P::TargetChain>,
{
	let source_client = params.source_client;
	let target_client = params.target_client;
	let stall_timeout = relay_substrate_client::bidirectional_transaction_stall_timeout(
		params.source_transaction_params.mortality,
		params.target_transaction_params.mortality,
		P::SourceChain::AVERAGE_BLOCK_INTERVAL,
		P::TargetChain::AVERAGE_BLOCK_INTERVAL,
		STALL_TIMEOUT,
	);
	let relayer_id_at_source: AccountIdOf<P::SourceChain> =
		params.source_transaction_params.signer.public().into();

	// 2/3 is reserved for proofs and tx overhead
	let max_messages_size_in_single_batch = P::TargetChain::max_extrinsic_size() / 3;
	// we don't know exact weights of the Polkadot runtime. So to guess weights we'll be using
	// weights from Rialto and then simply dividing it by x2.
	let (max_messages_in_single_batch, max_messages_weight_in_single_batch) =
		crate::messages_lane::select_delivery_transaction_limits::<
			<P::TargetChain as ChainWithMessages>::WeightInfo,
		>(
			P::TargetChain::max_extrinsic_weight(),
			P::SourceChain::MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX,
		);
	let (max_messages_in_single_batch, max_messages_weight_in_single_batch) =
		(max_messages_in_single_batch / 2, max_messages_weight_in_single_batch / 2);

	let standalone_metrics = params.standalone_metrics.map(Ok).unwrap_or_else(|| {
		crate::messages_metrics::standalone_metrics::<P>(
			source_client.clone(),
			target_client.clone(),
		)
	})?;

	log::info!(
		target: "bridge",
		"Starting {} -> {} messages relay.\n\t\
			{} relayer account id: {:?}\n\t\
			Max messages in single transaction: {}\n\t\
			Max messages size in single transaction: {}\n\t\
			Max messages weight in single transaction: {}\n\t\
			Tx mortality: {:?} (~{}m)/{:?} (~{}m)\n\t\
			Stall timeout: {:?}",
		P::SourceChain::NAME,
		P::TargetChain::NAME,
		P::SourceChain::NAME,
		relayer_id_at_source,
		max_messages_in_single_batch,
		max_messages_size_in_single_batch,
		max_messages_weight_in_single_batch,
		params.source_transaction_params.mortality,
		transaction_stall_timeout(
			params.source_transaction_params.mortality,
			P::SourceChain::AVERAGE_BLOCK_INTERVAL,
			STALL_TIMEOUT,
		).as_secs_f64() / 60.0f64,
		params.target_transaction_params.mortality,
		transaction_stall_timeout(
			params.target_transaction_params.mortality,
			P::TargetChain::AVERAGE_BLOCK_INTERVAL,
			STALL_TIMEOUT,
		).as_secs_f64() / 60.0f64,
		stall_timeout,
	);

	messages_relay::message_lane_loop::run(
		messages_relay::message_lane_loop::Params {
			lane: params.lane_id,
			source_tick: P::SourceChain::AVERAGE_BLOCK_INTERVAL,
			target_tick: P::TargetChain::AVERAGE_BLOCK_INTERVAL,
			reconnect_delay: relay_utils::relay_loop::RECONNECT_DELAY,
			stall_timeout,
			delivery_params: messages_relay::message_lane_loop::MessageDeliveryParams {
				max_unrewarded_relayer_entries_at_target:
					P::SourceChain::MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX,
				max_unconfirmed_nonces_at_target:
					P::SourceChain::MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX,
				max_messages_in_single_batch,
				max_messages_weight_in_single_batch,
				max_messages_size_in_single_batch,
				relay_strategy: params.relay_strategy,
			},
		},
		SubstrateMessagesSource::<P>::new(
			source_client.clone(),
			target_client.clone(),
			params.lane_id,
			params.source_transaction_params,
			params.target_to_source_headers_relay,
		),
		SubstrateMessagesTarget::<P>::new(
			target_client,
			source_client,
			params.lane_id,
			relayer_id_at_source,
			params.target_transaction_params,
			standalone_metrics.clone(),
			params.source_to_target_headers_relay,
		),
		standalone_metrics.register_and_spawn(params.metrics_params)?,
		futures::future::pending(),
	)
	.await
	.map_err(Into::into)
}

/// Different ways of building `receive_messages_proof` calls.
pub trait ReceiveMessagesProofCallBuilder<P: SubstrateMessageLane> {
	/// Given messages proof, build call of `receive_messages_proof` function of bridge
	/// messages module at the target chain.
	fn build_receive_messages_proof_call(
		relayer_id_at_source: AccountIdOf<P::SourceChain>,
		proof: SubstrateMessagesProof<P::SourceChain>,
		messages_count: u32,
		dispatch_weight: Weight,
		trace_call: bool,
	) -> CallOf<P::TargetChain>;
}

/// Building `receive_messages_proof` call when you have direct access to the target
/// chain runtime.
pub struct DirectReceiveMessagesProofCallBuilder<P, R, I> {
	_phantom: PhantomData<(P, R, I)>,
}

impl<P, R, I> ReceiveMessagesProofCallBuilder<P> for DirectReceiveMessagesProofCallBuilder<P, R, I>
where
	P: SubstrateMessageLane,
	R: BridgeMessagesConfig<I, InboundRelayer = AccountIdOf<P::SourceChain>>,
	I: 'static,
	R::SourceHeaderChain: bp_messages::target_chain::SourceHeaderChain<
		R::InboundMessageFee,
		MessagesProof = FromBridgedChainMessagesProof<HashOf<P::SourceChain>>,
	>,
	CallOf<P::TargetChain>: From<BridgeMessagesCall<R, I>> + GetDispatchInfo,
{
	fn build_receive_messages_proof_call(
		relayer_id_at_source: AccountIdOf<P::SourceChain>,
		proof: SubstrateMessagesProof<P::SourceChain>,
		messages_count: u32,
		dispatch_weight: Weight,
		trace_call: bool,
	) -> CallOf<P::TargetChain> {
		let call: CallOf<P::TargetChain> = BridgeMessagesCall::<R, I>::receive_messages_proof {
			relayer_id_at_bridged_chain: relayer_id_at_source,
			proof: proof.1,
			messages_count,
			dispatch_weight,
		}
		.into();
		if trace_call {
			// this trace isn't super-accurate, because limits are for transactions and we
			// have a call here, but it provides required information
			log::trace!(
				target: "bridge",
				"Prepared {} -> {} messages delivery call. Weight: {}/{}, size: {}/{}",
				P::SourceChain::NAME,
				P::TargetChain::NAME,
				call.get_dispatch_info().weight,
				P::TargetChain::max_extrinsic_weight(),
				call.encode().len(),
				P::TargetChain::max_extrinsic_size(),
			);
		}
		call
	}
}

/// Macro that generates `ReceiveMessagesProofCallBuilder` implementation for the case when
/// you only have an access to the mocked version of target chain runtime. In this case you
/// should provide "name" of the call variant for the bridge messages calls and the "name" of
/// the variant for the `receive_messages_proof` call within that first option.
#[rustfmt::skip]
#[macro_export]
macro_rules! generate_mocked_receive_message_proof_call_builder {
	($pipeline:ident, $mocked_builder:ident, $bridge_messages:path, $receive_messages_proof:path) => {
		pub struct $mocked_builder;

		impl $crate::messages_lane::ReceiveMessagesProofCallBuilder<$pipeline>
			for $mocked_builder
		{
			fn build_receive_messages_proof_call(
				relayer_id_at_source: relay_substrate_client::AccountIdOf<
					<$pipeline as $crate::messages_lane::SubstrateMessageLane>::SourceChain
				>,
				proof: $crate::messages_source::SubstrateMessagesProof<
					<$pipeline as $crate::messages_lane::SubstrateMessageLane>::SourceChain
				>,
				messages_count: u32,
				dispatch_weight: Weight,
				_trace_call: bool,
			) -> relay_substrate_client::CallOf<
				<$pipeline as $crate::messages_lane::SubstrateMessageLane>::TargetChain
			> {
				$bridge_messages($receive_messages_proof(
					relayer_id_at_source,
					proof.1,
					messages_count,
					dispatch_weight,
				))
			}
		}
	};
}

/// Different ways of building `receive_messages_delivery_proof` calls.
pub trait ReceiveMessagesDeliveryProofCallBuilder<P: SubstrateMessageLane> {
	/// Given messages delivery proof, build call of `receive_messages_delivery_proof` function of
	/// bridge messages module at the source chain.
	fn build_receive_messages_delivery_proof_call(
		proof: SubstrateMessagesDeliveryProof<P::TargetChain>,
		trace_call: bool,
	) -> CallOf<P::SourceChain>;
}

/// Building `receive_messages_delivery_proof` call when you have direct access to the source
/// chain runtime.
pub struct DirectReceiveMessagesDeliveryProofCallBuilder<P, R, I> {
	_phantom: PhantomData<(P, R, I)>,
}

impl<P, R, I> ReceiveMessagesDeliveryProofCallBuilder<P>
	for DirectReceiveMessagesDeliveryProofCallBuilder<P, R, I>
where
	P: SubstrateMessageLane,
	R: BridgeMessagesConfig<I>,
	I: 'static,
	R::TargetHeaderChain: bp_messages::source_chain::TargetHeaderChain<
		R::OutboundPayload,
		R::AccountId,
		MessagesDeliveryProof = FromBridgedChainMessagesDeliveryProof<HashOf<P::TargetChain>>,
	>,
	CallOf<P::SourceChain>: From<BridgeMessagesCall<R, I>> + GetDispatchInfo,
{
	fn build_receive_messages_delivery_proof_call(
		proof: SubstrateMessagesDeliveryProof<P::TargetChain>,
		trace_call: bool,
	) -> CallOf<P::SourceChain> {
		let call: CallOf<P::SourceChain> =
			BridgeMessagesCall::<R, I>::receive_messages_delivery_proof {
				proof: proof.1,
				relayers_state: proof.0,
			}
			.into();
		if trace_call {
			// this trace isn't super-accurate, because limits are for transactions and we
			// have a call here, but it provides required information
			log::trace!(
				target: "bridge",
				"Prepared {} -> {} delivery confirmation transaction. Weight: {}/{}, size: {}/{}",
				P::TargetChain::NAME,
				P::SourceChain::NAME,
				call.get_dispatch_info().weight,
				P::SourceChain::max_extrinsic_weight(),
				call.encode().len(),
				P::SourceChain::max_extrinsic_size(),
			);
		}
		call
	}
}

/// Macro that generates `ReceiveMessagesDeliveryProofCallBuilder` implementation for the case when
/// you only have an access to the mocked version of source chain runtime. In this case you
/// should provide "name" of the call variant for the bridge messages calls and the "name" of
/// the variant for the `receive_messages_delivery_proof` call within that first option.
#[rustfmt::skip]
#[macro_export]
macro_rules! generate_mocked_receive_message_delivery_proof_call_builder {
	($pipeline:ident, $mocked_builder:ident, $bridge_messages:path, $receive_messages_delivery_proof:path) => {
		pub struct $mocked_builder;

		impl $crate::messages_lane::ReceiveMessagesDeliveryProofCallBuilder<$pipeline>
			for $mocked_builder
		{
			fn build_receive_messages_delivery_proof_call(
				proof: $crate::messages_target::SubstrateMessagesDeliveryProof<
					<$pipeline as $crate::messages_lane::SubstrateMessageLane>::TargetChain
				>,
				_trace_call: bool,
			) -> relay_substrate_client::CallOf<
				<$pipeline as $crate::messages_lane::SubstrateMessageLane>::SourceChain
			> {
				$bridge_messages($receive_messages_delivery_proof(proof.1, proof.0))
			}
		}
	};
}

/// Returns maximal number of messages and their maximal cumulative dispatch weight, based
/// on given chain parameters.
pub fn select_delivery_transaction_limits<W: pallet_bridge_messages::WeightInfoExt>(
	max_extrinsic_weight: Weight,
	max_unconfirmed_messages_at_inbound_lane: MessageNonce,
) -> (MessageNonce, Weight) {
	// We may try to guess accurate value, based on maximal number of messages and per-message
	// weight overhead, but the relay loop isn't using this info in a super-accurate way anyway.
	// So just a rough guess: let's say 1/3 of max tx weight is for tx itself and the rest is
	// for messages dispatch.

	// Another thing to keep in mind is that our runtimes (when this code was written) accept
	// messages with dispatch weight <= max_extrinsic_weight/2. So we can't reserve less than
	// that for dispatch.

	let weight_for_delivery_tx = max_extrinsic_weight / 3;
	let weight_for_messages_dispatch = max_extrinsic_weight - weight_for_delivery_tx;

	let delivery_tx_base_weight = W::receive_messages_proof_overhead() +
		W::receive_messages_proof_outbound_lane_state_overhead();
	let delivery_tx_weight_rest = weight_for_delivery_tx - delivery_tx_base_weight;
	let max_number_of_messages = std::cmp::min(
		delivery_tx_weight_rest / W::receive_messages_proof_messages_overhead(1),
		max_unconfirmed_messages_at_inbound_lane,
	);

	assert!(
		max_number_of_messages > 0,
		"Relay should fit at least one message in every delivery transaction",
	);
	assert!(
		weight_for_messages_dispatch >= max_extrinsic_weight / 2,
		"Relay shall be able to deliver messages with dispatch weight = max_extrinsic_weight / 2",
	);

	(max_number_of_messages, weight_for_messages_dispatch)
}

#[cfg(test)]
mod tests {
	use super::*;
	use bp_runtime::Chain;

	type RialtoToMillauMessagesWeights =
		pallet_bridge_messages::weights::MillauWeight<rialto_runtime::Runtime>;

	#[test]
	fn select_delivery_transaction_limits_works() {
		let (max_count, max_weight) =
			select_delivery_transaction_limits::<RialtoToMillauMessagesWeights>(
				bp_millau::Millau::max_extrinsic_weight(),
				bp_rialto::MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX,
			);
		assert_eq!(
			(max_count, max_weight),
			// We don't actually care about these values, so feel free to update them whenever test
			// fails. The only thing to do before that is to ensure that new values looks sane:
			// i.e. weight reserved for messages dispatch allows dispatch of non-trivial messages.
			//
			// Any significant change in this values should attract additional attention.
			(958, 216_583_333_334),
		);
	}
}
