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

//! Substrate client as Substrate messages target. The chain we connect to should have
//! runtime that implements `<BridgedChainName>HeaderApi` to allow bridging with
//! <BridgedName> chain.

use crate::messages_lane::SubstrateMessageLane;
use crate::messages_source::read_client_state;

use async_trait::async_trait;
use bp_message_lane::{LaneId, MessageNonce, UnrewardedRelayersState};
use bp_runtime::InstanceId;
use bridge_runtime_common::messages::source::FromBridgedChainMessagesDeliveryProof;
use codec::{Decode, Encode};
use messages_relay::{
	message_lane::{SourceHeaderIdOf, TargetHeaderIdOf},
	message_lane_loop::{TargetClient, TargetClientState},
};
use relay_substrate_client::{Chain, Client, Error as SubstrateError, HashOf};
use relay_utils::{relay_loop::Client as RelayClient, BlockNumberBase};
use sp_core::Bytes;
use sp_runtime::{traits::Header as HeaderT, DeserializeOwned};
use std::ops::RangeInclusive;

/// Message receiving proof returned by the target Substrate node.
pub type SubstrateMessagesReceivingProof<C> = (
	UnrewardedRelayersState,
	FromBridgedChainMessagesDeliveryProof<HashOf<C>>,
);

/// Substrate client as Substrate messages target.
pub struct SubstrateMessagesTarget<C: Chain, P> {
	client: Client<C>,
	lane: P,
	lane_id: LaneId,
	instance: InstanceId,
}

impl<C: Chain, P> SubstrateMessagesTarget<C, P> {
	/// Create new Substrate headers target.
	pub fn new(client: Client<C>, lane: P, lane_id: LaneId, instance: InstanceId) -> Self {
		SubstrateMessagesTarget {
			client,
			lane,
			lane_id,
			instance,
		}
	}
}

impl<C: Chain, P: SubstrateMessageLane> Clone for SubstrateMessagesTarget<C, P> {
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
impl<C: Chain, P: SubstrateMessageLane> RelayClient for SubstrateMessagesTarget<C, P> {
	type Error = SubstrateError;

	async fn reconnect(&mut self) -> Result<(), SubstrateError> {
		self.client.reconnect().await
	}
}

#[async_trait]
impl<C, P> TargetClient<P> for SubstrateMessagesTarget<C, P>
where
	C: Chain,
	C::Header: DeserializeOwned,
	C::Index: DeserializeOwned,
	<C::Header as HeaderT>::Number: BlockNumberBase,
	P: SubstrateMessageLane<
		MessagesReceivingProof = SubstrateMessagesReceivingProof<C>,
		TargetHeaderNumber = <C::Header as HeaderT>::Number,
		TargetHeaderHash = <C::Header as HeaderT>::Hash,
	>,
	P::SourceHeaderNumber: Decode,
	P::SourceHeaderHash: Decode,
{
	async fn state(&self) -> Result<TargetClientState<P>, SubstrateError> {
		// we can't continue to deliver messages if target node is out of sync, because
		// it may have already received (some of) messages that we're going to deliver
		self.client.ensure_synced().await?;

		read_client_state::<_, P::SourceHeaderHash, P::SourceHeaderNumber>(
			&self.client,
			P::BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET,
		)
		.await
	}

	async fn latest_received_nonce(
		&self,
		id: TargetHeaderIdOf<P>,
	) -> Result<(TargetHeaderIdOf<P>, MessageNonce), SubstrateError> {
		let encoded_response = self
			.client
			.state_call(
				P::INBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD.into(),
				Bytes(self.lane_id.encode()),
				Some(id.1),
			)
			.await?;
		let latest_received_nonce: MessageNonce =
			Decode::decode(&mut &encoded_response.0[..]).map_err(SubstrateError::ResponseParseFailed)?;
		Ok((id, latest_received_nonce))
	}

	async fn latest_confirmed_received_nonce(
		&self,
		id: TargetHeaderIdOf<P>,
	) -> Result<(TargetHeaderIdOf<P>, MessageNonce), SubstrateError> {
		let encoded_response = self
			.client
			.state_call(
				P::INBOUND_LANE_LATEST_CONFIRMED_NONCE_METHOD.into(),
				Bytes(self.lane_id.encode()),
				Some(id.1),
			)
			.await?;
		let latest_received_nonce: MessageNonce =
			Decode::decode(&mut &encoded_response.0[..]).map_err(SubstrateError::ResponseParseFailed)?;
		Ok((id, latest_received_nonce))
	}

	async fn unrewarded_relayers_state(
		&self,
		id: TargetHeaderIdOf<P>,
	) -> Result<(TargetHeaderIdOf<P>, UnrewardedRelayersState), SubstrateError> {
		let encoded_response = self
			.client
			.state_call(
				P::INBOUND_LANE_UNREWARDED_RELAYERS_STATE.into(),
				Bytes(self.lane_id.encode()),
				Some(id.1),
			)
			.await?;
		let unrewarded_relayers_state: UnrewardedRelayersState =
			Decode::decode(&mut &encoded_response.0[..]).map_err(SubstrateError::ResponseParseFailed)?;
		Ok((id, unrewarded_relayers_state))
	}

	async fn prove_messages_receiving(
		&self,
		id: TargetHeaderIdOf<P>,
	) -> Result<(TargetHeaderIdOf<P>, P::MessagesReceivingProof), SubstrateError> {
		let (id, relayers_state) = self.unrewarded_relayers_state(id).await?;
		let proof = self
			.client
			.prove_messages_delivery(self.instance, self.lane_id, id.1)
			.await?;
		let proof = FromBridgedChainMessagesDeliveryProof {
			bridged_header_hash: id.1,
			storage_proof: proof,
			lane: self.lane_id,
		};
		Ok((id, (relayers_state, proof)))
	}

	async fn submit_messages_proof(
		&self,
		generated_at_header: SourceHeaderIdOf<P>,
		nonces: RangeInclusive<MessageNonce>,
		proof: P::MessagesProof,
	) -> Result<RangeInclusive<MessageNonce>, SubstrateError> {
		let tx = self
			.lane
			.make_messages_delivery_transaction(generated_at_header, nonces.clone(), proof)
			.await?;
		self.client.submit_extrinsic(Bytes(tx.encode())).await?;
		Ok(nonces)
	}
}
