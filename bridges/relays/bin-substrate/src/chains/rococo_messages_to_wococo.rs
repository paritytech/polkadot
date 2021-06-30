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

//! Rococo-to-Wococo messages sync entrypoint.

use crate::messages_lane::{
	select_delivery_transaction_limits, MessagesRelayParams, SubstrateMessageLane, SubstrateMessageLaneToSubstrate,
};
use crate::messages_source::SubstrateMessagesSource;
use crate::messages_target::SubstrateMessagesTarget;

use bp_messages::MessageNonce;
use bp_runtime::{ROCOCO_CHAIN_ID, WOCOCO_CHAIN_ID};
use bridge_runtime_common::messages::target::FromBridgedChainMessagesProof;
use codec::Encode;
use messages_relay::message_lane::MessageLane;
use relay_rococo_client::{HeaderId as RococoHeaderId, Rococo, SigningParams as RococoSigningParams};
use relay_substrate_client::{metrics::StorageProofOverheadMetric, Chain, TransactionSignScheme};
use relay_wococo_client::{HeaderId as WococoHeaderId, SigningParams as WococoSigningParams, Wococo};
use sp_core::{Bytes, Pair};
use std::{ops::RangeInclusive, time::Duration};

/// Rococo-to-Wococo message lane.
pub type RococoMessagesToWococo =
	SubstrateMessageLaneToSubstrate<Rococo, RococoSigningParams, Wococo, WococoSigningParams>;

impl SubstrateMessageLane for RococoMessagesToWococo {
	const OUTBOUND_LANE_MESSAGE_DETAILS_METHOD: &'static str = bp_wococo::TO_WOCOCO_MESSAGE_DETAILS_METHOD;
	const OUTBOUND_LANE_LATEST_GENERATED_NONCE_METHOD: &'static str =
		bp_wococo::TO_WOCOCO_LATEST_GENERATED_NONCE_METHOD;
	const OUTBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str = bp_wococo::TO_WOCOCO_LATEST_RECEIVED_NONCE_METHOD;

	const INBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str = bp_rococo::FROM_ROCOCO_LATEST_RECEIVED_NONCE_METHOD;
	const INBOUND_LANE_LATEST_CONFIRMED_NONCE_METHOD: &'static str =
		bp_rococo::FROM_ROCOCO_LATEST_CONFIRMED_NONCE_METHOD;
	const INBOUND_LANE_UNREWARDED_RELAYERS_STATE: &'static str = bp_rococo::FROM_ROCOCO_UNREWARDED_RELAYERS_STATE;

	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str = bp_rococo::BEST_FINALIZED_ROCOCO_HEADER_METHOD;
	const BEST_FINALIZED_TARGET_HEADER_ID_AT_SOURCE: &'static str = bp_wococo::BEST_FINALIZED_WOCOCO_HEADER_METHOD;

	type SourceChain = Rococo;
	type TargetChain = Wococo;

	fn source_transactions_author(&self) -> bp_rococo::AccountId {
		(*self.source_sign.public().as_array_ref()).into()
	}

	fn make_messages_receiving_proof_transaction(
		&self,
		transaction_nonce: <Rococo as Chain>::Index,
		_generated_at_block: WococoHeaderId,
		proof: <Self as MessageLane>::MessagesReceivingProof,
	) -> Bytes {
		let (relayers_state, proof) = proof;
		let call = relay_rococo_client::runtime::Call::BridgeMessagesWococo(
			relay_rococo_client::runtime::BridgeMessagesWococoCall::receive_messages_delivery_proof(
				proof,
				relayers_state,
			),
		);
		let genesis_hash = *self.source_client.genesis_hash();
		let transaction = Rococo::sign_transaction(genesis_hash, &self.source_sign, transaction_nonce, call);
		log::trace!(
			target: "bridge",
			"Prepared Wococo -> Rococo confirmation transaction. Weight: <unknown>/{}, size: {}/{}",
			bp_rococo::max_extrinsic_weight(),
			transaction.encode().len(),
			bp_rococo::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}

	fn target_transactions_author(&self) -> bp_wococo::AccountId {
		(*self.target_sign.public().as_array_ref()).into()
	}

	fn make_messages_delivery_transaction(
		&self,
		transaction_nonce: <Wococo as Chain>::Index,
		_generated_at_header: RococoHeaderId,
		_nonces: RangeInclusive<MessageNonce>,
		proof: <Self as MessageLane>::MessagesProof,
	) -> Bytes {
		let (dispatch_weight, proof) = proof;
		let FromBridgedChainMessagesProof {
			ref nonces_start,
			ref nonces_end,
			..
		} = proof;
		let messages_count = nonces_end - nonces_start + 1;

		let call = relay_wococo_client::runtime::Call::BridgeMessagesRococo(
			relay_wococo_client::runtime::BridgeMessagesRococoCall::receive_messages_proof(
				self.relayer_id_at_source.clone(),
				proof,
				messages_count as _,
				dispatch_weight,
			),
		);
		let genesis_hash = *self.target_client.genesis_hash();
		let transaction = Wococo::sign_transaction(genesis_hash, &self.target_sign, transaction_nonce, call);
		log::trace!(
			target: "bridge",
			"Prepared Rococo -> Wococo delivery transaction. Weight: <unknown>/{}, size: {}/{}",
			bp_wococo::max_extrinsic_weight(),
			transaction.encode().len(),
			bp_wococo::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}
}

/// Rococo node as messages source.
type RococoSourceClient =
	SubstrateMessagesSource<Rococo, RococoMessagesToWococo, relay_rococo_client::runtime::WithWococoMessagesInstance>;

/// Wococo node as messages target.
type WococoTargetClient =
	SubstrateMessagesTarget<Wococo, RococoMessagesToWococo, relay_wococo_client::runtime::WithRococoMessagesInstance>;

/// Run Rococo-to-Wococo messages sync.
pub async fn run(
	params: MessagesRelayParams<Rococo, RococoSigningParams, Wococo, WococoSigningParams>,
) -> Result<(), String> {
	let stall_timeout = Duration::from_secs(5 * 60);
	let relayer_id_at_rococo = (*params.source_sign.public().as_array_ref()).into();

	let lane_id = params.lane_id;
	let source_client = params.source_client;
	let lane = RococoMessagesToWococo {
		source_client: source_client.clone(),
		source_sign: params.source_sign,
		target_client: params.target_client.clone(),
		target_sign: params.target_sign,
		relayer_id_at_source: relayer_id_at_rococo,
	};

	// 2/3 is reserved for proofs and tx overhead
	let max_messages_size_in_single_batch = bp_wococo::max_extrinsic_size() / 3;
	// we don't know exact weights of the Wococo runtime. So to guess weights we'll be using
	// weights from Rialto and then simply dividing it by x2.
	let (max_messages_in_single_batch, max_messages_weight_in_single_batch) =
		select_delivery_transaction_limits::<pallet_bridge_messages::weights::RialtoWeight<rialto_runtime::Runtime>>(
			bp_wococo::max_extrinsic_weight(),
			bp_wococo::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
		);
	let (max_messages_in_single_batch, max_messages_weight_in_single_batch) = (
		max_messages_in_single_batch / 2,
		max_messages_weight_in_single_batch / 2,
	);

	log::info!(
		target: "bridge",
		"Starting Rococo -> Wococo messages relay.\n\t\
			Rococo relayer account id: {:?}\n\t\
			Max messages in single transaction: {}\n\t\
			Max messages size in single transaction: {}\n\t\
			Max messages weight in single transaction: {}",
		lane.relayer_id_at_source,
		max_messages_in_single_batch,
		max_messages_size_in_single_batch,
		max_messages_weight_in_single_batch,
	);

	messages_relay::message_lane_loop::run(
		messages_relay::message_lane_loop::Params {
			lane: lane_id,
			source_tick: Rococo::AVERAGE_BLOCK_INTERVAL,
			target_tick: Wococo::AVERAGE_BLOCK_INTERVAL,
			reconnect_delay: relay_utils::relay_loop::RECONNECT_DELAY,
			stall_timeout,
			delivery_params: messages_relay::message_lane_loop::MessageDeliveryParams {
				max_unrewarded_relayer_entries_at_target: bp_wococo::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
				max_unconfirmed_nonces_at_target: bp_wococo::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE,
				max_messages_in_single_batch,
				max_messages_weight_in_single_batch,
				max_messages_size_in_single_batch,
				relayer_mode: messages_relay::message_lane_loop::RelayerMode::Altruistic,
			},
		},
		RococoSourceClient::new(
			source_client.clone(),
			lane.clone(),
			lane_id,
			WOCOCO_CHAIN_ID,
			params.target_to_source_headers_relay,
		),
		WococoTargetClient::new(
			params.target_client,
			lane,
			lane_id,
			ROCOCO_CHAIN_ID,
			params.source_to_target_headers_relay,
		),
		relay_utils::relay_metrics(
			Some(messages_relay::message_lane_loop::metrics_prefix::<
				RococoMessagesToWococo,
			>(&lane_id)),
			params.metrics_params,
		)
		.standalone_metric(|registry, prefix| {
			StorageProofOverheadMetric::new(
				registry,
				prefix,
				source_client.clone(),
				"rococo_storage_proof_overhead".into(),
				"Rococo storage proof overhead".into(),
			)
		})?
		.into_params(),
		futures::future::pending(),
	)
	.await
}
