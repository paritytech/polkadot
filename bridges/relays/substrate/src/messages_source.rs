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

//! Substrate client as Substrate messages source. The chain we connect to should have
//! runtime that implements `<BridgedChainName>HeaderApi` to allow bridging with
//! <BridgedName> chain.

use crate::messages_lane::SubstrateMessageLane;

use async_trait::async_trait;
use bp_message_lane::{LaneId, MessageNonce};
use bp_runtime::InstanceId;
use bridge_runtime_common::messages::target::FromBridgedChainMessagesProof;
use codec::{Decode, Encode};
use frame_support::weights::Weight;
use messages_relay::{
	message_lane::{SourceHeaderIdOf, TargetHeaderIdOf},
	message_lane_loop::{
		ClientState, MessageProofParameters, MessageWeights, MessageWeightsMap, SourceClient, SourceClientState,
	},
};
use relay_substrate_client::{Chain, Client, Error as SubstrateError, HashOf, HeaderIdOf};
use relay_utils::{relay_loop::Client as RelayClient, BlockNumberBase, HeaderId};
use sp_core::Bytes;
use sp_runtime::{traits::Header as HeaderT, DeserializeOwned};
use std::ops::RangeInclusive;

/// Intermediate message proof returned by the source Substrate node. Includes everything
/// required to submit to the target node: cumulative dispatch weight of bundled messages and
/// the proof itself.
pub type SubstrateMessagesProof<C> = (Weight, FromBridgedChainMessagesProof<HashOf<C>>);

/// Substrate client as Substrate messages source.
pub struct SubstrateMessagesSource<C: Chain, P> {
	client: Client<C>,
	lane: P,
	lane_id: LaneId,
	instance: InstanceId,
}

impl<C: Chain, P> SubstrateMessagesSource<C, P> {
	/// Create new Substrate headers source.
	pub fn new(client: Client<C>, lane: P, lane_id: LaneId, instance: InstanceId) -> Self {
		SubstrateMessagesSource {
			client,
			lane,
			lane_id,
			instance,
		}
	}
}

impl<C: Chain, P: SubstrateMessageLane> Clone for SubstrateMessagesSource<C, P> {
	fn clone(&self) -> Self {
		Self {
			client: self.client.clone(),
			lane: self.lane.clone(),
			lane_id: self.lane_id,
			instance: self.instance,
		}
	}
}

#[async_trait]
impl<C: Chain, P: SubstrateMessageLane> RelayClient for SubstrateMessagesSource<C, P> {
	type Error = SubstrateError;

	async fn reconnect(&mut self) -> Result<(), SubstrateError> {
		self.client.reconnect().await
	}
}

#[async_trait]
impl<C, P> SourceClient<P> for SubstrateMessagesSource<C, P>
where
	C: Chain,
	C::Header: DeserializeOwned,
	C::Index: DeserializeOwned,
	C::BlockNumber: BlockNumberBase,
	P: SubstrateMessageLane<
		MessagesProof = SubstrateMessagesProof<C>,
		SourceHeaderNumber = <C::Header as HeaderT>::Number,
		SourceHeaderHash = <C::Header as HeaderT>::Hash,
	>,
	P::TargetHeaderNumber: Decode,
	P::TargetHeaderHash: Decode,
{
	async fn state(&self) -> Result<SourceClientState<P>, SubstrateError> {
		// we can't continue to deliver confirmations if source node is out of sync, because
		// it may have already received confirmations that we're going to deliver
		self.client.ensure_synced().await?;

		read_client_state::<_, P::TargetHeaderHash, P::TargetHeaderNumber>(
			&self.client,
			P::BEST_FINALIZED_TARGET_HEADER_ID_AT_SOURCE,
		)
		.await
	}

	async fn latest_generated_nonce(
		&self,
		id: SourceHeaderIdOf<P>,
	) -> Result<(SourceHeaderIdOf<P>, MessageNonce), SubstrateError> {
		let encoded_response = self
			.client
			.state_call(
				P::OUTBOUND_LANE_LATEST_GENERATED_NONCE_METHOD.into(),
				Bytes(self.lane_id.encode()),
				Some(id.1),
			)
			.await?;
		let latest_generated_nonce: MessageNonce =
			Decode::decode(&mut &encoded_response.0[..]).map_err(SubstrateError::ResponseParseFailed)?;
		Ok((id, latest_generated_nonce))
	}

	async fn latest_confirmed_received_nonce(
		&self,
		id: SourceHeaderIdOf<P>,
	) -> Result<(SourceHeaderIdOf<P>, MessageNonce), SubstrateError> {
		let encoded_response = self
			.client
			.state_call(
				P::OUTBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD.into(),
				Bytes(self.lane_id.encode()),
				Some(id.1),
			)
			.await?;
		let latest_received_nonce: MessageNonce =
			Decode::decode(&mut &encoded_response.0[..]).map_err(SubstrateError::ResponseParseFailed)?;
		Ok((id, latest_received_nonce))
	}

	async fn generated_messages_weights(
		&self,
		id: SourceHeaderIdOf<P>,
		nonces: RangeInclusive<MessageNonce>,
	) -> Result<MessageWeightsMap, SubstrateError> {
		let encoded_response = self
			.client
			.state_call(
				P::OUTBOUND_LANE_MESSAGES_DISPATCH_WEIGHT_METHOD.into(),
				Bytes((self.lane_id, nonces.start(), nonces.end()).encode()),
				Some(id.1),
			)
			.await?;

		make_message_weights_map::<C>(
			Decode::decode(&mut &encoded_response.0[..]).map_err(SubstrateError::ResponseParseFailed)?,
			nonces,
		)
	}

	async fn prove_messages(
		&self,
		id: SourceHeaderIdOf<P>,
		nonces: RangeInclusive<MessageNonce>,
		proof_parameters: MessageProofParameters,
	) -> Result<(SourceHeaderIdOf<P>, RangeInclusive<MessageNonce>, P::MessagesProof), SubstrateError> {
		let proof = self
			.client
			.prove_messages(
				self.instance,
				self.lane_id,
				nonces.clone(),
				proof_parameters.outbound_state_proof_required,
				id.1,
			)
			.await?
			.iter_nodes()
			.collect();
		let proof = FromBridgedChainMessagesProof {
			bridged_header_hash: id.1,
			storage_proof: proof,
			lane: self.lane_id,
			nonces_start: *nonces.start(),
			nonces_end: *nonces.end(),
		};
		Ok((id, nonces, (proof_parameters.dispatch_weight, proof)))
	}

	async fn submit_messages_receiving_proof(
		&self,
		generated_at_block: TargetHeaderIdOf<P>,
		proof: P::MessagesReceivingProof,
	) -> Result<(), SubstrateError> {
		let tx = self
			.lane
			.make_messages_receiving_proof_transaction(generated_at_block, proof)
			.await?;
		self.client.submit_extrinsic(Bytes(tx.encode())).await?;
		Ok(())
	}
}

pub async fn read_client_state<SelfChain, BridgedHeaderHash, BridgedHeaderNumber>(
	self_client: &Client<SelfChain>,
	best_finalized_header_id_method_name: &str,
) -> Result<ClientState<HeaderIdOf<SelfChain>, HeaderId<BridgedHeaderHash, BridgedHeaderNumber>>, SubstrateError>
where
	SelfChain: Chain,
	SelfChain::Header: DeserializeOwned,
	SelfChain::Index: DeserializeOwned,
	BridgedHeaderHash: Decode,
	BridgedHeaderNumber: Decode,
{
	// let's read our state first: we need best finalized header hash on **this** chain
	let self_best_finalized_header_hash = self_client.best_finalized_header_hash().await?;
	let self_best_finalized_header = self_client.header_by_hash(self_best_finalized_header_hash).await?;
	let self_best_finalized_id = HeaderId(*self_best_finalized_header.number(), self_best_finalized_header_hash);

	// now let's read our best header on **this** chain
	let self_best_header = self_client.best_header().await?;
	let self_best_hash = self_best_header.hash();
	let self_best_id = HeaderId(*self_best_header.number(), self_best_hash);

	// now let's read id of best finalized peer header at our best finalized block
	let encoded_best_finalized_peer_on_self = self_client
		.state_call(
			best_finalized_header_id_method_name.into(),
			Bytes(Vec::new()),
			Some(self_best_hash),
		)
		.await?;
	let decoded_best_finalized_peer_on_self: (BridgedHeaderNumber, BridgedHeaderHash) =
		Decode::decode(&mut &encoded_best_finalized_peer_on_self.0[..]).map_err(SubstrateError::ResponseParseFailed)?;
	let peer_on_self_best_finalized_id = HeaderId(
		decoded_best_finalized_peer_on_self.0,
		decoded_best_finalized_peer_on_self.1,
	);

	Ok(ClientState {
		best_self: self_best_id,
		best_finalized_self: self_best_finalized_id,
		best_finalized_peer_at_best_self: peer_on_self_best_finalized_id,
	})
}

fn make_message_weights_map<C: Chain>(
	weights: Vec<(MessageNonce, Weight, u32)>,
	nonces: RangeInclusive<MessageNonce>,
) -> Result<MessageWeightsMap, SubstrateError> {
	let make_missing_nonce_error = |expected_nonce| {
		Err(SubstrateError::Custom(format!(
			"Missing nonce {} in messages_dispatch_weight call result. Expected all nonces from {:?}",
			expected_nonce, nonces,
		)))
	};

	let mut weights_map = MessageWeightsMap::new();

	// this is actually prevented by external logic
	if nonces.is_empty() {
		return Ok(weights_map);
	}

	// check if last nonce is missing - loop below is not checking this
	let last_nonce_is_missing = weights
		.last()
		.map(|(last_nonce, _, _)| last_nonce != nonces.end())
		.unwrap_or(true);
	if last_nonce_is_missing {
		return make_missing_nonce_error(*nonces.end());
	}

	let mut expected_nonce = *nonces.start();
	let mut is_at_head = true;

	for (nonce, weight, size) in weights {
		match (nonce == expected_nonce, is_at_head) {
			(true, _) => (),
			(false, true) => {
				// this may happen if some messages were already pruned from the source node
				//
				// this is not critical error and will be auto-resolved by messages lane (and target node)
				log::info!(
					target: "bridge",
					"Some messages are missing from the {} node: {:?}. Target node may be out of sync?",
					C::NAME,
					expected_nonce..nonce,
				);
			}
			(false, false) => {
				// some nonces are missing from the middle/tail of the range
				//
				// this is critical error, because we can't miss any nonces
				return make_missing_nonce_error(expected_nonce);
			}
		}

		weights_map.insert(
			nonce,
			MessageWeights {
				weight,
				size: size as _,
			},
		);
		expected_nonce = nonce + 1;
		is_at_head = false;
	}

	Ok(weights_map)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn make_message_weights_map_succeeds_if_no_messages_are_missing() {
		assert_eq!(
			make_message_weights_map::<relay_rialto_client::Rialto>(vec![(1, 0, 0), (2, 0, 0), (3, 0, 0)], 1..=3,)
				.unwrap(),
			vec![
				(1, MessageWeights { weight: 0, size: 0 }),
				(2, MessageWeights { weight: 0, size: 0 }),
				(3, MessageWeights { weight: 0, size: 0 }),
			]
			.into_iter()
			.collect(),
		);
	}

	#[test]
	fn make_message_weights_map_succeeds_if_head_messages_are_missing() {
		assert_eq!(
			make_message_weights_map::<relay_rialto_client::Rialto>(vec![(2, 0, 0), (3, 0, 0)], 1..=3,).unwrap(),
			vec![
				(2, MessageWeights { weight: 0, size: 0 }),
				(3, MessageWeights { weight: 0, size: 0 }),
			]
			.into_iter()
			.collect(),
		);
	}

	#[test]
	fn make_message_weights_map_fails_if_mid_messages_are_missing() {
		assert!(matches!(
			make_message_weights_map::<relay_rialto_client::Rialto>(vec![(1, 0, 0), (3, 0, 0)], 1..=3,),
			Err(SubstrateError::Custom(_))
		));
	}

	#[test]
	fn make_message_weights_map_fails_if_tail_messages_are_missing() {
		assert!(matches!(
			make_message_weights_map::<relay_rialto_client::Rialto>(vec![(1, 0, 0), (2, 0, 0)], 1..=3,),
			Err(SubstrateError::Custom(_))
		));
	}

	#[test]
	fn make_message_weights_map_fails_if_all_messages_are_missing() {
		assert!(matches!(
			make_message_weights_map::<relay_rialto_client::Rialto>(vec![], 1..=3),
			Err(SubstrateError::Custom(_))
		));
	}
}
