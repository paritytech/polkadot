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

//! Substrate client as Substrate messages target. The chain we connect to should have
//! runtime that implements `<BridgedChainName>HeaderApi` to allow bridging with
//! <BridgedName> chain.

use crate::messages_lane::SubstrateMessageLane;
use crate::messages_source::read_client_state;
use crate::on_demand_headers::OnDemandHeadersRelay;

use async_trait::async_trait;
use bp_messages::{LaneId, MessageNonce, UnrewardedRelayersState};
use bp_runtime::InstanceId;
use bridge_runtime_common::messages::source::FromBridgedChainMessagesDeliveryProof;
use codec::{Decode, Encode};
use frame_support::traits::Instance;
use messages_relay::{
	message_lane::{SourceHeaderIdOf, TargetHeaderIdOf},
	message_lane_loop::{TargetClient, TargetClientState},
};
use pallet_bridge_messages::Config as MessagesConfig;
use relay_substrate_client::{Chain, Client, Error as SubstrateError, HashOf};
use relay_utils::{relay_loop::Client as RelayClient, BlockNumberBase};
use sp_core::Bytes;
use sp_runtime::{traits::Header as HeaderT, DeserializeOwned};
use std::{marker::PhantomData, ops::RangeInclusive};

/// Message receiving proof returned by the target Substrate node.
pub type SubstrateMessagesReceivingProof<C> = (
	UnrewardedRelayersState,
	FromBridgedChainMessagesDeliveryProof<HashOf<C>>,
);

/// Substrate client as Substrate messages target.
pub struct SubstrateMessagesTarget<C: Chain, P: SubstrateMessageLane, R, I> {
	client: Client<C>,
	lane: P,
	lane_id: LaneId,
	instance: InstanceId,
	source_to_target_headers_relay: Option<OnDemandHeadersRelay<P::SourceChain>>,
	_phantom: PhantomData<(R, I)>,
}

impl<C: Chain, P: SubstrateMessageLane, R, I> SubstrateMessagesTarget<C, P, R, I> {
	/// Create new Substrate headers target.
	pub fn new(
		client: Client<C>,
		lane: P,
		lane_id: LaneId,
		instance: InstanceId,
		source_to_target_headers_relay: Option<OnDemandHeadersRelay<P::SourceChain>>,
	) -> Self {
		SubstrateMessagesTarget {
			client,
			lane,
			lane_id,
			instance,
			source_to_target_headers_relay,
			_phantom: Default::default(),
		}
	}
}

impl<C: Chain, P: SubstrateMessageLane, R, I> Clone for SubstrateMessagesTarget<C, P, R, I> {
	fn clone(&self) -> Self {
		Self {
			client: self.client.clone(),
			lane: self.lane.clone(),
			lane_id: self.lane_id,
			instance: self.instance,
			source_to_target_headers_relay: self.source_to_target_headers_relay.clone(),
			_phantom: Default::default(),
		}
	}
}

#[async_trait]
impl<C, P, R, I> RelayClient for SubstrateMessagesTarget<C, P, R, I>
where
	C: Chain,
	P: SubstrateMessageLane,
	R: 'static + Send + Sync,
	I: Send + Sync + Instance,
{
	type Error = SubstrateError;

	async fn reconnect(&mut self) -> Result<(), SubstrateError> {
		self.client.reconnect().await
	}
}

#[async_trait]
impl<C, P, R, I> TargetClient<P> for SubstrateMessagesTarget<C, P, R, I>
where
	C: Chain,
	C::Header: DeserializeOwned,
	C::Index: DeserializeOwned,
	<C::Header as HeaderT>::Number: BlockNumberBase,
	P: SubstrateMessageLane<
		TargetChain = C,
		MessagesReceivingProof = SubstrateMessagesReceivingProof<C>,
		TargetHeaderNumber = <C::Header as HeaderT>::Number,
		TargetHeaderHash = <C::Header as HeaderT>::Hash,
	>,
	P::SourceChain: Chain<Hash = P::SourceHeaderHash, BlockNumber = P::SourceHeaderNumber>,
	P::SourceHeaderNumber: Decode,
	P::SourceHeaderHash: Decode,
	R: Send + Sync + MessagesConfig<I>,
	I: Send + Sync + Instance,
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
		let inbound_data_key = pallet_bridge_messages::storage_keys::inbound_lane_data_key::<R, I>(&self.lane_id);
		let proof = self
			.client
			.prove_storage(vec![inbound_data_key], id.1)
			.await?
			.iter_nodes()
			.collect();
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
		self.client
			.submit_signed_extrinsic(self.lane.target_transactions_author(), |transaction_nonce| {
				self.lane.make_messages_delivery_transaction(
					transaction_nonce,
					generated_at_header,
					nonces.clone(),
					proof,
				)
			})
			.await?;
		Ok(nonces)
	}

	async fn require_source_header_on_target(&self, id: SourceHeaderIdOf<P>) {
		if let Some(ref source_to_target_headers_relay) = self.source_to_target_headers_relay {
			source_to_target_headers_relay.require_finalized_header(id);
		}
	}
}
