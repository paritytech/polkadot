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

//! Millau-to-Rialto messages sync entrypoint.

use std::ops::RangeInclusive;

use codec::Encode;
use frame_support::dispatch::GetDispatchInfo;
use sp_core::{Bytes, Pair};

use bp_messages::MessageNonce;
use bridge_runtime_common::messages::target::FromBridgedChainMessagesProof;
use frame_support::weights::Weight;
use messages_relay::{message_lane::MessageLane, relay_strategy::MixStrategy};
use relay_millau_client::{
	HeaderId as MillauHeaderId, Millau, SigningParams as MillauSigningParams,
};
use relay_rialto_client::{
	HeaderId as RialtoHeaderId, Rialto, SigningParams as RialtoSigningParams,
};
use relay_substrate_client::{Chain, Client, IndexOf, TransactionSignScheme, UnsignedTransaction};
use substrate_relay_helper::{
	messages_lane::{
		select_delivery_transaction_limits, MessagesRelayParams, StandaloneMessagesMetrics,
		SubstrateMessageLane, SubstrateMessageLaneToSubstrate,
	},
	messages_source::SubstrateMessagesSource,
	messages_target::SubstrateMessagesTarget,
	STALL_TIMEOUT,
};

/// Millau-to-Rialto message lane.
pub type MessageLaneMillauMessagesToRialto =
	SubstrateMessageLaneToSubstrate<Millau, MillauSigningParams, Rialto, RialtoSigningParams>;

#[derive(Clone)]
pub struct MillauMessagesToRialto {
	message_lane: MessageLaneMillauMessagesToRialto,
}

impl SubstrateMessageLane for MillauMessagesToRialto {
	type MessageLane = MessageLaneMillauMessagesToRialto;

	const OUTBOUND_LANE_MESSAGE_DETAILS_METHOD: &'static str =
		bp_rialto::TO_RIALTO_MESSAGE_DETAILS_METHOD;
	const OUTBOUND_LANE_LATEST_GENERATED_NONCE_METHOD: &'static str =
		bp_rialto::TO_RIALTO_LATEST_GENERATED_NONCE_METHOD;
	const OUTBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str =
		bp_rialto::TO_RIALTO_LATEST_RECEIVED_NONCE_METHOD;

	const INBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str =
		bp_millau::FROM_MILLAU_LATEST_RECEIVED_NONCE_METHOD;
	const INBOUND_LANE_LATEST_CONFIRMED_NONCE_METHOD: &'static str =
		bp_millau::FROM_MILLAU_LATEST_CONFIRMED_NONCE_METHOD;
	const INBOUND_LANE_UNREWARDED_RELAYERS_STATE: &'static str =
		bp_millau::FROM_MILLAU_UNREWARDED_RELAYERS_STATE;

	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str =
		bp_millau::BEST_FINALIZED_MILLAU_HEADER_METHOD;
	const BEST_FINALIZED_TARGET_HEADER_ID_AT_SOURCE: &'static str =
		bp_rialto::BEST_FINALIZED_RIALTO_HEADER_METHOD;

	const MESSAGE_PALLET_NAME_AT_SOURCE: &'static str = bp_millau::WITH_RIALTO_MESSAGES_PALLET_NAME;
	const MESSAGE_PALLET_NAME_AT_TARGET: &'static str = bp_rialto::WITH_MILLAU_MESSAGES_PALLET_NAME;

	const PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_TARGET_CHAIN: Weight =
		bp_rialto::PAY_INBOUND_DISPATCH_FEE_WEIGHT;

	type SourceChain = Millau;
	type TargetChain = Rialto;

	fn source_transactions_author(&self) -> bp_millau::AccountId {
		(*self.message_lane.source_sign.public().as_array_ref()).into()
	}

	fn make_messages_receiving_proof_transaction(
		&self,
		best_block_id: MillauHeaderId,
		transaction_nonce: IndexOf<Millau>,
		_generated_at_block: RialtoHeaderId,
		proof: <Self::MessageLane as MessageLane>::MessagesReceivingProof,
	) -> Bytes {
		let (relayers_state, proof) = proof;
		let call: millau_runtime::Call =
			millau_runtime::MessagesCall::receive_messages_delivery_proof { proof, relayers_state }
				.into();
		let call_weight = call.get_dispatch_info().weight;
		let genesis_hash = *self.message_lane.source_client.genesis_hash();
		let transaction = Millau::sign_transaction(
			genesis_hash,
			&self.message_lane.source_sign,
			relay_substrate_client::TransactionEra::new(
				best_block_id,
				self.message_lane.source_transactions_mortality,
			),
			UnsignedTransaction::new(call, transaction_nonce),
		);
		log::trace!(
			target: "bridge",
			"Prepared Rialto -> Millau confirmation transaction. Weight: {}/{}, size: {}/{}",
			call_weight,
			bp_millau::max_extrinsic_weight(),
			transaction.encode().len(),
			bp_millau::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}

	fn target_transactions_author(&self) -> bp_rialto::AccountId {
		(*self.message_lane.target_sign.public().as_array_ref()).into()
	}

	fn make_messages_delivery_transaction(
		&self,
		best_block_id: RialtoHeaderId,
		transaction_nonce: IndexOf<Rialto>,
		_generated_at_header: MillauHeaderId,
		_nonces: RangeInclusive<MessageNonce>,
		proof: <Self::MessageLane as MessageLane>::MessagesProof,
	) -> Bytes {
		let (dispatch_weight, proof) = proof;
		let FromBridgedChainMessagesProof { ref nonces_start, ref nonces_end, .. } = proof;
		let messages_count = nonces_end - nonces_start + 1;
		let call: rialto_runtime::Call = rialto_runtime::MessagesCall::receive_messages_proof {
			relayer_id_at_bridged_chain: self.message_lane.relayer_id_at_source.clone(),
			proof,
			messages_count: messages_count as _,
			dispatch_weight,
		}
		.into();
		let call_weight = call.get_dispatch_info().weight;
		let genesis_hash = *self.message_lane.target_client.genesis_hash();
		let transaction = Rialto::sign_transaction(
			genesis_hash,
			&self.message_lane.target_sign,
			relay_substrate_client::TransactionEra::new(
				best_block_id,
				self.message_lane.target_transactions_mortality,
			),
			UnsignedTransaction::new(call, transaction_nonce),
		);
		log::trace!(
			target: "bridge",
			"Prepared Millau -> Rialto delivery transaction. Weight: {}/{}, size: {}/{}",
			call_weight,
			bp_rialto::max_extrinsic_weight(),
			transaction.encode().len(),
			bp_rialto::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}
}

/// Millau node as messages source.
type MillauSourceClient = SubstrateMessagesSource<MillauMessagesToRialto>;

/// Rialto node as messages target.
type RialtoTargetClient = SubstrateMessagesTarget<MillauMessagesToRialto>;

/// Run Millau-to-Rialto messages sync.
pub async fn run(
	params: MessagesRelayParams<
		Millau,
		MillauSigningParams,
		Rialto,
		RialtoSigningParams,
		MixStrategy,
	>,
) -> anyhow::Result<()> {
	let stall_timeout = relay_substrate_client::bidirectional_transaction_stall_timeout(
		params.source_transactions_mortality,
		params.target_transactions_mortality,
		Millau::AVERAGE_BLOCK_INTERVAL,
		Rialto::AVERAGE_BLOCK_INTERVAL,
		STALL_TIMEOUT,
	);
	let relayer_id_at_millau = (*params.source_sign.public().as_array_ref()).into();

	let lane_id = params.lane_id;
	let source_client = params.source_client;
	let target_client = params.target_client;
	let lane = MillauMessagesToRialto {
		message_lane: SubstrateMessageLaneToSubstrate {
			source_client: source_client.clone(),
			source_sign: params.source_sign,
			source_transactions_mortality: params.source_transactions_mortality,
			target_client: target_client.clone(),
			target_sign: params.target_sign,
			target_transactions_mortality: params.target_transactions_mortality,
			relayer_id_at_source: relayer_id_at_millau,
		},
	};

	// 2/3 is reserved for proofs and tx overhead
	let max_messages_size_in_single_batch = bp_rialto::max_extrinsic_size() / 3;
	// TODO: use Millau weights after https://github.com/paritytech/parity-bridges-common/issues/390
	let (max_messages_in_single_batch, max_messages_weight_in_single_batch) =
		select_delivery_transaction_limits::<
			pallet_bridge_messages::weights::RialtoWeight<millau_runtime::Runtime>,
		>(
			bp_rialto::max_extrinsic_weight(),
			bp_rialto::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
		);

	log::info!(
		target: "bridge",
		"Starting Millau -> Rialto messages relay.\n\t\
			Millau relayer account id: {:?}\n\t\
			Max messages in single transaction: {}\n\t\
			Max messages size in single transaction: {}\n\t\
			Max messages weight in single transaction: {}\n\t\
			Tx mortality: {:?}/{:?}\n\t\
			Stall timeout: {:?}",
		lane.message_lane.relayer_id_at_source,
		max_messages_in_single_batch,
		max_messages_size_in_single_batch,
		max_messages_weight_in_single_batch,
		params.source_transactions_mortality,
		params.target_transactions_mortality,
		stall_timeout,
	);

	let standalone_metrics = params
		.standalone_metrics
		.map(Ok)
		.unwrap_or_else(|| standalone_metrics(source_client.clone(), target_client.clone()))?;
	messages_relay::message_lane_loop::run(
		messages_relay::message_lane_loop::Params {
			lane: lane_id,
			source_tick: Millau::AVERAGE_BLOCK_INTERVAL,
			target_tick: Rialto::AVERAGE_BLOCK_INTERVAL,
			reconnect_delay: relay_utils::relay_loop::RECONNECT_DELAY,
			stall_timeout,
			delivery_params: messages_relay::message_lane_loop::MessageDeliveryParams {
				max_unrewarded_relayer_entries_at_target:
					bp_rialto::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
				max_unconfirmed_nonces_at_target:
					bp_rialto::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE,
				max_messages_in_single_batch,
				max_messages_weight_in_single_batch,
				max_messages_size_in_single_batch,
				relay_strategy: params.relay_strategy,
			},
		},
		MillauSourceClient::new(
			source_client.clone(),
			lane.clone(),
			lane_id,
			params.target_to_source_headers_relay,
		),
		RialtoTargetClient::new(
			target_client,
			lane,
			lane_id,
			standalone_metrics.clone(),
			params.source_to_target_headers_relay,
		),
		standalone_metrics.register_and_spawn(params.metrics_params)?,
		futures::future::pending(),
	)
	.await
	.map_err(Into::into)
}

/// Create standalone metrics for the Millau -> Rialto messages loop.
pub(crate) fn standalone_metrics(
	source_client: Client<Millau>,
	target_client: Client<Rialto>,
) -> anyhow::Result<StandaloneMessagesMetrics<Millau, Rialto>> {
	substrate_relay_helper::messages_lane::standalone_metrics(
		source_client,
		target_client,
		Some(crate::chains::millau::ASSOCIATED_TOKEN_ID),
		Some(crate::chains::rialto::ASSOCIATED_TOKEN_ID),
		Some(crate::chains::rialto::millau_to_rialto_conversion_rate_params()),
		Some(crate::chains::millau::rialto_to_millau_conversion_rate_params()),
	)
}

/// Update Rialto -> Millau conversion rate, stored in Millau runtime storage.
pub(crate) async fn update_rialto_to_millau_conversion_rate(
	client: Client<Millau>,
	signer: <Millau as TransactionSignScheme>::AccountKeyPair,
	updated_rate: f64,
) -> anyhow::Result<()> {
	let genesis_hash = *client.genesis_hash();
	let signer_id = (*signer.public().as_array_ref()).into();
	client
		.submit_signed_extrinsic(signer_id, move |_, transaction_nonce| {
			Bytes(
				Millau::sign_transaction(
					genesis_hash,
					&signer,
					relay_substrate_client::TransactionEra::immortal(),
					UnsignedTransaction::new(
						millau_runtime::MessagesCall::update_pallet_parameter {
							parameter: millau_runtime::rialto_messages::MillauToRialtoMessagesParameter::RialtoToMillauConversionRate(
								sp_runtime::FixedU128::from_float(updated_rate),
							),
						}
						.into(),
						transaction_nonce,
					),
				)
				.encode(),
			)
		})
		.await
		.map(drop)
		.map_err(|err| anyhow::format_err!("{:?}", err))
}
