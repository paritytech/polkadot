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

//! Rialto-to-Millau messages sync entrypoint.

use crate::messages_lane::{
	select_delivery_transaction_limits, MessagesRelayParams, SubstrateMessageLane, SubstrateMessageLaneToSubstrate,
};
use crate::messages_source::SubstrateMessagesSource;
use crate::messages_target::SubstrateMessagesTarget;

use bp_messages::MessageNonce;
use bp_runtime::{MILLAU_CHAIN_ID, RIALTO_CHAIN_ID};
use bridge_runtime_common::messages::target::FromBridgedChainMessagesProof;
use codec::Encode;
use frame_support::dispatch::GetDispatchInfo;
use messages_relay::message_lane::MessageLane;
use relay_millau_client::{HeaderId as MillauHeaderId, Millau, SigningParams as MillauSigningParams};
use relay_rialto_client::{HeaderId as RialtoHeaderId, Rialto, SigningParams as RialtoSigningParams};
use relay_substrate_client::{
	metrics::{FloatStorageValueMetric, StorageProofOverheadMetric},
	Chain, TransactionSignScheme,
};
use sp_core::{Bytes, Pair};
use std::{ops::RangeInclusive, time::Duration};

/// Rialto-to-Millau message lane.
pub type RialtoMessagesToMillau =
	SubstrateMessageLaneToSubstrate<Rialto, RialtoSigningParams, Millau, MillauSigningParams>;

impl SubstrateMessageLane for RialtoMessagesToMillau {
	const OUTBOUND_LANE_MESSAGE_DETAILS_METHOD: &'static str = bp_millau::TO_MILLAU_MESSAGE_DETAILS_METHOD;
	const OUTBOUND_LANE_LATEST_GENERATED_NONCE_METHOD: &'static str =
		bp_millau::TO_MILLAU_LATEST_GENERATED_NONCE_METHOD;
	const OUTBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str = bp_millau::TO_MILLAU_LATEST_RECEIVED_NONCE_METHOD;

	const INBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str = bp_rialto::FROM_RIALTO_LATEST_RECEIVED_NONCE_METHOD;
	const INBOUND_LANE_LATEST_CONFIRMED_NONCE_METHOD: &'static str =
		bp_rialto::FROM_RIALTO_LATEST_CONFIRMED_NONCE_METHOD;
	const INBOUND_LANE_UNREWARDED_RELAYERS_STATE: &'static str = bp_rialto::FROM_RIALTO_UNREWARDED_RELAYERS_STATE;

	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str = bp_rialto::BEST_FINALIZED_RIALTO_HEADER_METHOD;
	const BEST_FINALIZED_TARGET_HEADER_ID_AT_SOURCE: &'static str = bp_millau::BEST_FINALIZED_MILLAU_HEADER_METHOD;

	type SourceChain = Rialto;
	type TargetChain = Millau;

	fn source_transactions_author(&self) -> bp_rialto::AccountId {
		(*self.source_sign.public().as_array_ref()).into()
	}

	fn make_messages_receiving_proof_transaction(
		&self,
		transaction_nonce: <Rialto as Chain>::Index,
		_generated_at_block: MillauHeaderId,
		proof: <Self as MessageLane>::MessagesReceivingProof,
	) -> Bytes {
		let (relayers_state, proof) = proof;
		let call: rialto_runtime::Call =
			rialto_runtime::MessagesCall::receive_messages_delivery_proof(proof, relayers_state).into();
		let call_weight = call.get_dispatch_info().weight;
		let genesis_hash = *self.source_client.genesis_hash();
		let transaction = Rialto::sign_transaction(genesis_hash, &self.source_sign, transaction_nonce, call);
		log::trace!(
			target: "bridge",
			"Prepared Millau -> Rialto confirmation transaction. Weight: {}/{}, size: {}/{}",
			call_weight,
			bp_rialto::max_extrinsic_weight(),
			transaction.encode().len(),
			bp_rialto::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}

	fn target_transactions_author(&self) -> bp_millau::AccountId {
		(*self.target_sign.public().as_array_ref()).into()
	}

	fn make_messages_delivery_transaction(
		&self,
		transaction_nonce: <Millau as Chain>::Index,
		_generated_at_header: RialtoHeaderId,
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
		let call: millau_runtime::Call = millau_runtime::MessagesCall::receive_messages_proof(
			self.relayer_id_at_source.clone(),
			proof,
			messages_count as _,
			dispatch_weight,
		)
		.into();
		let call_weight = call.get_dispatch_info().weight;
		let genesis_hash = *self.target_client.genesis_hash();
		let transaction = Millau::sign_transaction(genesis_hash, &self.target_sign, transaction_nonce, call);
		log::trace!(
			target: "bridge",
			"Prepared Rialto -> Millau delivery transaction. Weight: {}/{}, size: {}/{}",
			call_weight,
			bp_millau::max_extrinsic_weight(),
			transaction.encode().len(),
			bp_millau::max_extrinsic_size(),
		);
		Bytes(transaction.encode())
	}
}

/// Rialto node as messages source.
type RialtoSourceClient =
	SubstrateMessagesSource<Rialto, RialtoMessagesToMillau, rialto_runtime::WithMillauMessagesInstance>;

/// Millau node as messages target.
type MillauTargetClient =
	SubstrateMessagesTarget<Millau, RialtoMessagesToMillau, millau_runtime::WithRialtoMessagesInstance>;

/// Run Rialto-to-Millau messages sync.
pub async fn run(
	params: MessagesRelayParams<Rialto, RialtoSigningParams, Millau, MillauSigningParams>,
) -> Result<(), String> {
	let stall_timeout = Duration::from_secs(5 * 60);
	let relayer_id_at_rialto = (*params.source_sign.public().as_array_ref()).into();

	let lane_id = params.lane_id;
	let source_client = params.source_client;
	let lane = RialtoMessagesToMillau {
		source_client: source_client.clone(),
		source_sign: params.source_sign,
		target_client: params.target_client.clone(),
		target_sign: params.target_sign,
		relayer_id_at_source: relayer_id_at_rialto,
	};

	// 2/3 is reserved for proofs and tx overhead
	let max_messages_size_in_single_batch = bp_millau::max_extrinsic_size() / 3;
	let (max_messages_in_single_batch, max_messages_weight_in_single_batch) =
		select_delivery_transaction_limits::<pallet_bridge_messages::weights::RialtoWeight<rialto_runtime::Runtime>>(
			bp_millau::max_extrinsic_weight(),
			bp_millau::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
		);

	log::info!(
		target: "bridge",
		"Starting Rialto -> Millau messages relay.\n\t\
			Rialto relayer account id: {:?}\n\t\
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
			source_tick: Rialto::AVERAGE_BLOCK_INTERVAL,
			target_tick: Millau::AVERAGE_BLOCK_INTERVAL,
			reconnect_delay: relay_utils::relay_loop::RECONNECT_DELAY,
			stall_timeout,
			delivery_params: messages_relay::message_lane_loop::MessageDeliveryParams {
				max_unrewarded_relayer_entries_at_target: bp_millau::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
				max_unconfirmed_nonces_at_target: bp_millau::MAX_UNCONFIRMED_MESSAGES_AT_INBOUND_LANE,
				max_messages_in_single_batch,
				max_messages_weight_in_single_batch,
				max_messages_size_in_single_batch,
				relayer_mode: messages_relay::message_lane_loop::RelayerMode::Altruistic,
			},
		},
		RialtoSourceClient::new(
			source_client.clone(),
			lane.clone(),
			lane_id,
			MILLAU_CHAIN_ID,
			params.target_to_source_headers_relay,
		),
		MillauTargetClient::new(
			params.target_client,
			lane,
			lane_id,
			RIALTO_CHAIN_ID,
			params.source_to_target_headers_relay,
		),
		relay_utils::relay_metrics(
			Some(messages_relay::message_lane_loop::metrics_prefix::<
				RialtoMessagesToMillau,
			>(&lane_id)),
			params.metrics_params,
		)
		.standalone_metric(|registry, prefix| {
			StorageProofOverheadMetric::new(
				registry,
				prefix,
				source_client.clone(),
				"rialto_storage_proof_overhead".into(),
				"Rialto storage proof overhead".into(),
			)
		})?
		.standalone_metric(|registry, prefix| {
			FloatStorageValueMetric::<_, sp_runtime::FixedU128>::new(
				registry,
				prefix,
				source_client,
				sp_core::storage::StorageKey(
					rialto_runtime::millau_messages::MillauToRialtoConversionRate::key().to_vec(),
				),
				Some(rialto_runtime::millau_messages::INITIAL_MILLAU_TO_RIALTO_CONVERSION_RATE),
				"rialto_millau_to_rialto_conversion_rate".into(),
				"Millau to Rialto tokens conversion rate (used by Millau)".into(),
			)
		})?
		.into_params(),
		futures::future::pending(),
	)
	.await
}
