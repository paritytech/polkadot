// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

use crate::messages_source::SubstrateMessagesProof;
use crate::messages_target::SubstrateMessagesReceivingProof;

use async_trait::async_trait;
use bp_message_lane::MessageNonce;
use codec::Encode;
use frame_support::weights::Weight;
use messages_relay::message_lane::{MessageLane, SourceHeaderIdOf, TargetHeaderIdOf};
use relay_substrate_client::{BlockNumberOf, Chain, Client, Error as SubstrateError, HashOf};
use relay_utils::BlockNumberBase;
use std::ops::RangeInclusive;

/// Message sync pipeline for Substrate <-> Substrate relays.
#[async_trait]
pub trait SubstrateMessageLane: MessageLane {
	/// Name of the runtime method that returns dispatch weight of outbound messages at the source chain.
	const OUTBOUND_LANE_MESSAGES_DISPATCH_WEIGHT_METHOD: &'static str;
	/// Name of the runtime method that returns latest generated nonce at the source chain.
	const OUTBOUND_LANE_LATEST_GENERATED_NONCE_METHOD: &'static str;
	/// Name of the runtime method that returns latest received (confirmed) nonce at the the source chain.
	const OUTBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str;

	/// Name of the runtime method that returns latest received nonce at the target chain.
	const INBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str;
	/// Name of the runtime method that returns latest confirmed (reward-paid) nonce at the target chain.
	const INBOUND_LANE_LATEST_CONFIRMED_NONCE_METHOD: &'static str;
	/// Numebr of the runtime method that returns state of "unrewarded relayers" set at the target chain.
	const INBOUND_LANE_UNREWARDED_RELAYERS_STATE: &'static str;

	/// Name of the runtime method that returns id of best finalized source header at target chain.
	const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str;
	/// Name of the runtime method that returns id of best finalized target header at source chain.
	const BEST_FINALIZED_TARGET_HEADER_ID_AT_SOURCE: &'static str;

	/// Signed transaction type of the source chain.
	type SourceSignedTransaction: Send + Sync + Encode;
	/// Signed transaction type of the target chain.
	type TargetSignedTransaction: Send + Sync + Encode;

	/// Make messages delivery transaction.
	async fn make_messages_delivery_transaction(
		&self,
		generated_at_header: SourceHeaderIdOf<Self>,
		nonces: RangeInclusive<MessageNonce>,
		proof: Self::MessagesProof,
	) -> Result<Self::TargetSignedTransaction, SubstrateError>;

	/// Make messages receiving proof transaction.
	async fn make_messages_receiving_proof_transaction(
		&self,
		generated_at_header: TargetHeaderIdOf<Self>,
		proof: Self::MessagesReceivingProof,
	) -> Result<Self::SourceSignedTransaction, SubstrateError>;
}

/// Substrate-to-Substrate message lane.
#[derive(Debug)]
pub struct SubstrateMessageLaneToSubstrate<Source: Chain, SourceSignParams, Target: Chain, TargetSignParams> {
	/// Client for the source Substrate chain.
	pub(crate) source_client: Client<Source>,
	/// Parameters required to sign transactions for source chain.
	pub(crate) source_sign: SourceSignParams,
	/// Client for the target Substrate chain.
	pub(crate) target_client: Client<Target>,
	/// Parameters required to sign transactions for target chain.
	pub(crate) target_sign: TargetSignParams,
	/// Account id of relayer at the source chain.
	pub(crate) relayer_id_at_source: Source::AccountId,
}

impl<Source: Chain, SourceSignParams: Clone, Target: Chain, TargetSignParams: Clone> Clone
	for SubstrateMessageLaneToSubstrate<Source, SourceSignParams, Target, TargetSignParams>
{
	fn clone(&self) -> Self {
		Self {
			source_client: self.source_client.clone(),
			source_sign: self.source_sign.clone(),
			target_client: self.target_client.clone(),
			target_sign: self.target_sign.clone(),
			relayer_id_at_source: self.relayer_id_at_source.clone(),
		}
	}
}

impl<Source: Chain, SourceSignParams, Target: Chain, TargetSignParams> MessageLane
	for SubstrateMessageLaneToSubstrate<Source, SourceSignParams, Target, TargetSignParams>
where
	SourceSignParams: Clone + Send + Sync + 'static,
	TargetSignParams: Clone + Send + Sync + 'static,
	BlockNumberOf<Source>: BlockNumberBase,
	BlockNumberOf<Target>: BlockNumberBase,
{
	const SOURCE_NAME: &'static str = Source::NAME;
	const TARGET_NAME: &'static str = Target::NAME;

	type MessagesProof = SubstrateMessagesProof<Source>;
	type MessagesReceivingProof = SubstrateMessagesReceivingProof<Target>;

	type SourceHeaderNumber = BlockNumberOf<Source>;
	type SourceHeaderHash = HashOf<Source>;

	type TargetHeaderNumber = BlockNumberOf<Target>;
	type TargetHeaderHash = HashOf<Target>;
}

/// Returns maximal number of messages and their maximal cumulative dispatch weight, based
/// on given chain parameters.
pub fn select_delivery_transaction_limits<W: pallet_message_lane::WeightInfoExt>(
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

	let delivery_tx_base_weight =
		W::receive_messages_proof_overhead() + W::receive_messages_proof_outbound_lane_state_overhead();
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

	type RialtoToMillauMessageLaneWeights = pallet_message_lane::weights::RialtoWeight<rialto_runtime::Runtime>;

	#[test]
	fn select_delivery_transaction_limits_works() {
		let (max_count, max_weight) = select_delivery_transaction_limits::<RialtoToMillauMessageLaneWeights>(
			bp_millau::max_extrinsic_weight(),
			bp_millau::MAX_UNREWARDED_RELAYER_ENTRIES_AT_INBOUND_LANE,
		);
		assert_eq!(
			(max_count, max_weight),
			// We don't actually care about these values, so feel free to update them whenever test
			// fails. The only thing to do before that is to ensure that new values looks sane: i.e. weight
			// reserved for messages dispatch allows dispatch of non-trivial messages.
			//
			// Any significant change in this values should attract additional attention.
			(955, 216_583_333_334),
		);
	}
}
