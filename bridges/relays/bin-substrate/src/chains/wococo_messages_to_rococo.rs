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

//! Wococo-to-Rococo messages sync entrypoint.

use std::ops::RangeInclusive;

use codec::Encode;
use sp_core::{Bytes, Pair};

use bp_messages::MessageNonce;
use bridge_runtime_common::messages::target::FromBridgedChainMessagesProof;
use frame_support::weights::Weight;
use messages_relay::{message_lane::MessageLane, relay_strategy::MixStrategy};
use relay_rococo_client::{
	HeaderId as RococoHeaderId, Rococo, SigningParams as RococoSigningParams,
};
use relay_substrate_client::{Chain, Client, IndexOf, TransactionSignScheme, UnsignedTransaction};
use relay_wococo_client::{
	HeaderId as WococoHeaderId, SigningParams as WococoSigningParams, Wococo,
};
use substrate_relay_helper::{
	messages_lane::{
		select_delivery_transaction_limits, MessagesRelayParams, StandaloneMessagesMetrics,
		SubstrateMessageLane, SubstrateMessageLaneToSubstrate,
	},
	messages_source::SubstrateMessagesSource,
	messages_target::SubstrateMessagesTarget,
	STALL_TIMEOUT,
};

/// Wococo-to-Rococo message lane.
pub type MessageLaneWococoMessagesToRococo =
	SubstrateMessageLaneToSubstrate<Wococo, WococoSigningParams, Rococo, RococoSigningParams>;

#[derive(Clone)]
pub struct WococoMessagesToRococo {
	message_lane: MessageLaneWococoMessagesToRococo,
}

impl SubstrateMessageLane for WococoMessagesToRococo {
	type MessageLane = MessageLaneWococoMessagesToRococo;
	const OUTBOUND_LANE_MESSAGE_DETAILS_METHOD: &'static str =
		bp_rococo::TO_ROCOCO_MESSAGE_DETAILS_METHOD;
	const OUTBOUND_LANE_LATEST_GENERATED_NONCE_METHOD: &'static str =
		bp_rococo::TO_ROCOCO_LATEST_GENERATED_NONCE_METHOD;
	const OUTBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str =
		bp_rococo::TO_ROCOCO_LATEST_RECEIVED_NONCE_METHOD;

	const INBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str =
		bp_wococo::FROM_WOCOCO_LATEST_RECEIVED_NONCE_METHOD;
	const INBOUND_LANE_LATEST_CONFIRMED_NONCE_METHOD: &'static str =
		bp_wococo::FROM_WOCOCO_LATEST_CONFIRMED_NONCE_METHOD;
	const INBOUND_LANE_UNREWARDED_RELAYERS_STATE: &'static str =
		bp_wococo::FROM_WOCOCO_UNREWARDED_RELAYERS_STATE;

	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str =
		bp_wococo::BEST_FINALIZED_WOCOCO_HEADER_METHOD;
	const BEST_FINALIZED_TARGET_HEADER_ID_AT_SOURCE: &'static str =
		bp_rococo::BEST_FINALIZED_ROCOCO_HEADER_METHOD;

	const MESSAGE_PALLET_NAME_AT_SOURCE: &'static str = bp_wococo::WITH_ROCOCO_MESSAGES_PALLET_NAME;
	const MESSAGE_PALLET_NAME_AT_TARGET: &'static str = bp_rococo::WITH_WOCOCO_MESSAGES_PALLET_NAME;

	const PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_TARGET_CHAIN: Weight =
		bp_rococo::PAY_INBOUND_DISPATCH_FEE_WEIGHT;

	type SourceChain = Wococo;
	type TargetChain = Rococo;

	fn source_transactions_author(&self) -> bp_wococo::AccountId {
		(*self.message_lane.source_sign.public().as_array_ref()).into()
	}

	fn make_messages_receiving_proof_transaction(
		&self,
		best_block_id: WococoHeaderId,
		transaction_nonce: IndexOf<Wococo>,
		_generated_at_block: RococoHeaderId,
		proof: <Self::MessageLane as MessageLane>::MessagesReceivingProof,
	) -> Bytes {
		let (relayers_state, proof) = proof;
		let call = relay_wococo_client::runtime::Call::BridgeMessagesRococo(
			relay_wococo_client::runtime::BridgeMessagesRococoCall::receive_messages_delivery_proof(
				proof,
				relayers_state,
			),
		);
		let genesis_hash = *self.message_lane.source_client.genesis_hash();
		let transaction = Wococo::sign_transaction(
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
			"Prepared Rococo -> Wococo confirmation transaction. Weight: <unknown>/{}, size: {}/{}",
			bp_wococo::max_extrinsic_weight(),
			transaction.encode().len(),
			bp_wococo::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}

	fn target_transactions_author(&self) -> bp_rococo::AccountId {
		(*self.message_lane.target_sign.public().as_array_ref()).into()
	}

	fn make_messages_delivery_transaction(
		&self,
		best_block_id: WococoHeaderId,
		transaction_nonce: IndexOf<Rococo>,
		_generated_at_header: WococoHeaderId,
		_nonces: RangeInclusive<MessageNonce>,
		proof: <Self::MessageLane as MessageLane>::MessagesProof,
	) -> Bytes {
		let (dispatch_weight, proof) = proof;
		let FromBridgedChainMessagesProof { ref nonces_start, ref nonces_end, .. } = proof;
		let messages_count = nonces_end - nonces_start + 1;

		let call = relay_rococo_client::runtime::Call::BridgeMessagesWococo(
			relay_rococo_client::runtime::BridgeMessagesWococoCall::receive_messages_proof(
				self.message_lane.relayer_id_at_source.clone(),
				proof,
				messages_count as _,
				dispatch_weight,
			),
		);
		let genesis_hash = *self.message_lane.target_client.genesis_hash();
		let transaction = Rococo::sign_transaction(
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
			"Prepared Wococo -> Rococo delivery transaction. Weight: <unknown>/{}, size: {}/{}",
			bp_rococo::max_extrinsic_weight(),
			transaction.encode().len(),
			bp_rococo::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}
}

/// Wococo node as messages source.
type WococoSourceClient = SubstrateMessagesSource<WococoMessagesToRococo>;

/// Rococo node as messages target.
type RococoTargetClient = SubstrateMessagesTarget<WococoMessagesToRococo>;

/// Run Wococo-to-Rococo messages sync.
pub async fn run(
	params: MessagesRelayParams<
		Wococo,
		WococoSigningParams,
		Rococo,
		RococoSigningParams,
		MixStrategy,
	>,
) -> anyhow::Result<()> {
	let stall_timeout = relay_substrate_client::bidirectional_transaction_stall_timeout(
		params.source_transactions_mortality,
		params.target_transactions_mortality,
		Wococo::AVERAGE_BLOCK_INTERVAL,
		Rococo::AVERAGE_BLOCK_INTERVAL,
		STALL_TIMEOUT,
	);
	let relayer_id_at_wococo = (*params.source_sign.public().as_array_ref()).into();

	let lane_id = params.lane_id;
	let source_client = params.source_client;
	let target_client = params.target_client;
	let lane = WococoMessagesToRococo {
		message_lane: SubstrateMessageLaneToSubstrate {
			source_client: source_client.clone(),
			source_sign: params.source_sign,
			source_transactions_mortality: params.source_transactions_mortality,
			target_client: target_client.clone(),
			target_sign: params.target_sign,
			target_transactions_mortality: params.target_transactions_mortality,
			relayer_id_at_source: relayer_id_at_wococo,
		},
	};

	// 2/3 is reserved for proofs and tx overhead
	let max_messages_size_in_single_batch = bp_rococo::max_extrinsic_size() / 3;
	// we don't know exact weights of the Rococo runtime. So to guess weights we'll be using
	// weights from Rialto and then simply dividing it by x2.
	let (max_messages_in_single_batch, max_messages_weight_in_single_batch) =
		select_delivery_transaction_limits::<
			pallet_bridge_messages::weights::RialtoWeight<rialto_runtime::Runtime>,
		>(
			bp_rococo::max_extrinsic_weight(),
			bp_rococo::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
		);
	let (max_messages_in_single_batch, max_messages_weight_in_single_batch) =
		(max_messages_in_single_batch / 2, max_messages_weight_in_single_batch / 2);

	log::info!(
		target: "bridge",
		"Starting Wococo -> Rococo messages relay.\n\t\
			Wococo relayer account id: {:?}\n\t\
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
			source_tick: Wococo::AVERAGE_BLOCK_INTERVAL,
			target_tick: Rococo::AVERAGE_BLOCK_INTERVAL,
			reconnect_delay: relay_utils::relay_loop::RECONNECT_DELAY,
			stall_timeout,
			delivery_params: messages_relay::message_lane_loop::MessageDeliveryParams {
				max_unrewarded_relayer_entries_at_target:
					bp_rococo::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
				max_unconfirmed_nonces_at_target:
					bp_rococo::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE,
				max_messages_in_single_batch,
				max_messages_weight_in_single_batch,
				max_messages_size_in_single_batch,
				relay_strategy: params.relay_strategy,
			},
		},
		WococoSourceClient::new(
			source_client.clone(),
			lane.clone(),
			lane_id,
			params.target_to_source_headers_relay,
		),
		RococoTargetClient::new(
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

/// Create standalone metrics for the Wococo -> Rococo messages loop.
pub(crate) fn standalone_metrics(
	source_client: Client<Wococo>,
	target_client: Client<Rococo>,
) -> anyhow::Result<StandaloneMessagesMetrics<Wococo, Rococo>> {
	substrate_relay_helper::messages_lane::standalone_metrics(
		source_client,
		target_client,
		None,
		None,
		None,
		None,
	)
}
