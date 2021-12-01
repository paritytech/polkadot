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

//! Kusama-to-Polkadot messages sync entrypoint.

use std::ops::RangeInclusive;

use codec::Encode;
use frame_support::weights::Weight;
use sp_core::{Bytes, Pair};

use bp_messages::MessageNonce;
use bridge_runtime_common::messages::target::FromBridgedChainMessagesProof;
use messages_relay::{message_lane::MessageLane, relay_strategy::MixStrategy};
use relay_kusama_client::{
	HeaderId as KusamaHeaderId, Kusama, SigningParams as KusamaSigningParams,
};
use relay_polkadot_client::{
	HeaderId as PolkadotHeaderId, Polkadot, SigningParams as PolkadotSigningParams,
};
use relay_substrate_client::{Chain, Client, TransactionSignScheme, UnsignedTransaction};
use substrate_relay_helper::{
	messages_lane::{
		select_delivery_transaction_limits, MessagesRelayParams, StandaloneMessagesMetrics,
		SubstrateMessageLane, SubstrateMessageLaneToSubstrate,
	},
	messages_source::SubstrateMessagesSource,
	messages_target::SubstrateMessagesTarget,
	STALL_TIMEOUT,
};

/// Kusama-to-Polkadot message lane.
pub type MessageLaneKusamaMessagesToPolkadot =
	SubstrateMessageLaneToSubstrate<Kusama, KusamaSigningParams, Polkadot, PolkadotSigningParams>;

#[derive(Clone)]
pub struct KusamaMessagesToPolkadot {
	message_lane: MessageLaneKusamaMessagesToPolkadot,
}

impl SubstrateMessageLane for KusamaMessagesToPolkadot {
	type MessageLane = MessageLaneKusamaMessagesToPolkadot;

	const OUTBOUND_LANE_MESSAGE_DETAILS_METHOD: &'static str =
		bp_polkadot::TO_POLKADOT_MESSAGE_DETAILS_METHOD;
	const OUTBOUND_LANE_LATEST_GENERATED_NONCE_METHOD: &'static str =
		bp_polkadot::TO_POLKADOT_LATEST_GENERATED_NONCE_METHOD;
	const OUTBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str =
		bp_polkadot::TO_POLKADOT_LATEST_RECEIVED_NONCE_METHOD;

	const INBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str =
		bp_kusama::FROM_KUSAMA_LATEST_RECEIVED_NONCE_METHOD;
	const INBOUND_LANE_LATEST_CONFIRMED_NONCE_METHOD: &'static str =
		bp_kusama::FROM_KUSAMA_LATEST_CONFIRMED_NONCE_METHOD;
	const INBOUND_LANE_UNREWARDED_RELAYERS_STATE: &'static str =
		bp_kusama::FROM_KUSAMA_UNREWARDED_RELAYERS_STATE;

	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str =
		bp_kusama::BEST_FINALIZED_KUSAMA_HEADER_METHOD;
	const BEST_FINALIZED_TARGET_HEADER_ID_AT_SOURCE: &'static str =
		bp_polkadot::BEST_FINALIZED_POLKADOT_HEADER_METHOD;

	const MESSAGE_PALLET_NAME_AT_SOURCE: &'static str =
		bp_kusama::WITH_POLKADOT_MESSAGES_PALLET_NAME;
	const MESSAGE_PALLET_NAME_AT_TARGET: &'static str =
		bp_polkadot::WITH_KUSAMA_MESSAGES_PALLET_NAME;

	const PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_TARGET_CHAIN: Weight =
		bp_polkadot::PAY_INBOUND_DISPATCH_FEE_WEIGHT;

	type SourceChain = Kusama;
	type TargetChain = Polkadot;

	fn source_transactions_author(&self) -> bp_kusama::AccountId {
		(*self.message_lane.source_sign.public().as_array_ref()).into()
	}

	fn make_messages_receiving_proof_transaction(
		&self,
		best_block_id: KusamaHeaderId,
		transaction_nonce: bp_runtime::IndexOf<Kusama>,
		_generated_at_block: PolkadotHeaderId,
		proof: <Self::MessageLane as MessageLane>::MessagesReceivingProof,
	) -> Bytes {
		let (relayers_state, proof) = proof;
		let call = relay_kusama_client::runtime::Call::BridgePolkadotMessages(
			relay_kusama_client::runtime::BridgePolkadotMessagesCall::receive_messages_delivery_proof(
				proof,
				relayers_state,
			),
		);
		let genesis_hash = *self.message_lane.source_client.genesis_hash();
		let transaction = Kusama::sign_transaction(
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
			"Prepared Polkadot -> Kusama confirmation transaction. Weight: <unknown>/{}, size: {}/{}",
			bp_kusama::max_extrinsic_weight(),
			transaction.encode().len(),
			bp_kusama::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}

	fn target_transactions_author(&self) -> bp_polkadot::AccountId {
		(*self.message_lane.target_sign.public().as_array_ref()).into()
	}

	fn make_messages_delivery_transaction(
		&self,
		best_block_id: PolkadotHeaderId,
		transaction_nonce: bp_runtime::IndexOf<Polkadot>,
		_generated_at_header: KusamaHeaderId,
		_nonces: RangeInclusive<MessageNonce>,
		proof: <Self::MessageLane as MessageLane>::MessagesProof,
	) -> Bytes {
		let (dispatch_weight, proof) = proof;
		let FromBridgedChainMessagesProof { ref nonces_start, ref nonces_end, .. } = proof;
		let messages_count = nonces_end - nonces_start + 1;

		let call = relay_polkadot_client::runtime::Call::BridgeKusamaMessages(
			relay_polkadot_client::runtime::BridgeKusamaMessagesCall::receive_messages_proof(
				self.message_lane.relayer_id_at_source.clone(),
				proof,
				messages_count as _,
				dispatch_weight,
			),
		);
		let genesis_hash = *self.message_lane.target_client.genesis_hash();
		let transaction = Polkadot::sign_transaction(
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
			"Prepared Kusama -> Polkadot delivery transaction. Weight: <unknown>/{}, size: {}/{}",
			bp_polkadot::max_extrinsic_weight(),
			transaction.encode().len(),
			bp_polkadot::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}
}

/// Kusama node as messages source.
type KusamaSourceClient = SubstrateMessagesSource<KusamaMessagesToPolkadot>;

/// Polkadot node as messages target.
type PolkadotTargetClient = SubstrateMessagesTarget<KusamaMessagesToPolkadot>;

/// Run Kusama-to-Polkadot messages sync.
pub async fn run(
	params: MessagesRelayParams<
		Kusama,
		KusamaSigningParams,
		Polkadot,
		PolkadotSigningParams,
		MixStrategy,
	>,
) -> anyhow::Result<()> {
	let stall_timeout = relay_substrate_client::bidirectional_transaction_stall_timeout(
		params.source_transactions_mortality,
		params.target_transactions_mortality,
		Kusama::AVERAGE_BLOCK_INTERVAL,
		Polkadot::AVERAGE_BLOCK_INTERVAL,
		STALL_TIMEOUT,
	);
	let relayer_id_at_kusama = (*params.source_sign.public().as_array_ref()).into();

	let lane_id = params.lane_id;
	let source_client = params.source_client;
	let target_client = params.target_client;
	let lane = KusamaMessagesToPolkadot {
		message_lane: SubstrateMessageLaneToSubstrate {
			source_client: source_client.clone(),
			source_sign: params.source_sign,
			source_transactions_mortality: params.source_transactions_mortality,
			target_client: target_client.clone(),
			target_sign: params.target_sign,
			target_transactions_mortality: params.target_transactions_mortality,
			relayer_id_at_source: relayer_id_at_kusama,
		},
	};

	// 2/3 is reserved for proofs and tx overhead
	let max_messages_size_in_single_batch = bp_polkadot::max_extrinsic_size() / 3;
	// we don't know exact weights of the Polkadot runtime. So to guess weights we'll be using
	// weights from Rialto and then simply dividing it by x2.
	let (max_messages_in_single_batch, max_messages_weight_in_single_batch) =
		select_delivery_transaction_limits::<
			pallet_bridge_messages::weights::RialtoWeight<rialto_runtime::Runtime>,
		>(
			bp_polkadot::max_extrinsic_weight(),
			bp_polkadot::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
		);
	let (max_messages_in_single_batch, max_messages_weight_in_single_batch) =
		(max_messages_in_single_batch / 2, max_messages_weight_in_single_batch / 2);

	log::info!(
		target: "bridge",
		"Starting Kusama -> Polkadot messages relay.\n\t\
			Kusama relayer account id: {:?}\n\t\
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
			source_tick: Kusama::AVERAGE_BLOCK_INTERVAL,
			target_tick: Polkadot::AVERAGE_BLOCK_INTERVAL,
			reconnect_delay: relay_utils::relay_loop::RECONNECT_DELAY,
			stall_timeout,
			delivery_params: messages_relay::message_lane_loop::MessageDeliveryParams {
				max_unrewarded_relayer_entries_at_target:
					bp_polkadot::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
				max_unconfirmed_nonces_at_target:
					bp_polkadot::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE,
				max_messages_in_single_batch,
				max_messages_weight_in_single_batch,
				max_messages_size_in_single_batch,
				relay_strategy: params.relay_strategy,
			},
		},
		KusamaSourceClient::new(
			source_client.clone(),
			lane.clone(),
			lane_id,
			params.target_to_source_headers_relay,
		),
		PolkadotTargetClient::new(
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

/// Create standalone metrics for the Kusama -> Polkadot messages loop.
pub(crate) fn standalone_metrics(
	source_client: Client<Kusama>,
	target_client: Client<Polkadot>,
) -> anyhow::Result<StandaloneMessagesMetrics<Kusama, Polkadot>> {
	substrate_relay_helper::messages_lane::standalone_metrics(
		source_client,
		target_client,
		Some(crate::chains::kusama::TOKEN_ID),
		Some(crate::chains::polkadot::TOKEN_ID),
		Some(crate::chains::polkadot::kusama_to_polkadot_conversion_rate_params()),
		Some(crate::chains::kusama::polkadot_to_kusama_conversion_rate_params()),
	)
}

/// Update Polkadot -> Kusama conversion rate, stored in Kusama runtime storage.
pub(crate) async fn update_polkadot_to_kusama_conversion_rate(
	client: Client<Kusama>,
	signer: <Kusama as TransactionSignScheme>::AccountKeyPair,
	updated_rate: f64,
) -> anyhow::Result<()> {
	let genesis_hash = *client.genesis_hash();
	let signer_id = (*signer.public().as_array_ref()).into();
	client
		.submit_signed_extrinsic(signer_id, move |_, transaction_nonce| {
			Bytes(
				Kusama::sign_transaction(
					genesis_hash,
					&signer,
					relay_substrate_client::TransactionEra::immortal(),
					UnsignedTransaction::new(
						relay_kusama_client::runtime::Call::BridgePolkadotMessages(
							relay_kusama_client::runtime::BridgePolkadotMessagesCall::update_pallet_parameter(
								relay_kusama_client::runtime::BridgePolkadotMessagesParameter::PolkadotToKusamaConversionRate(
									sp_runtime::FixedU128::from_float(updated_rate),
								)
							)
						),
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
