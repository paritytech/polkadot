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

//! Substrate client as Substrate messages source. The chain we connect to should have
//! runtime that implements `<BridgedChainName>HeaderApi` to allow bridging with
//! <BridgedName> chain.

use crate::{
	messages_lane::SubstrateMessageLane, messages_target::SubstrateMessagesReceivingProof,
	on_demand_headers::OnDemandHeadersRelay,
};

use async_trait::async_trait;
use bp_messages::{LaneId, MessageNonce, UnrewardedRelayersState};
use bridge_runtime_common::messages::{
	source::FromBridgedChainMessagesDeliveryProof, target::FromBridgedChainMessagesProof,
};
use codec::{Decode, Encode};
use frame_support::weights::Weight;
use messages_relay::{
	message_lane::{MessageLane, SourceHeaderIdOf, TargetHeaderIdOf},
	message_lane_loop::{
		ClientState, MessageDetails, MessageDetailsMap, MessageProofParameters, SourceClient,
		SourceClientState,
	},
};
use num_traits::{Bounded, Zero};
use relay_substrate_client::{
	BalanceOf, BlockNumberOf, Chain, Client, Error as SubstrateError, HashOf, HeaderIdOf, HeaderOf,
	IndexOf,
};
use relay_utils::{relay_loop::Client as RelayClient, BlockNumberBase, HeaderId};
use sp_core::Bytes;
use sp_runtime::{
	traits::{AtLeast32BitUnsigned, Header as HeaderT},
	DeserializeOwned,
};
use std::ops::RangeInclusive;

/// Intermediate message proof returned by the source Substrate node. Includes everything
/// required to submit to the target node: cumulative dispatch weight of bundled messages and
/// the proof itself.
pub type SubstrateMessagesProof<C> = (Weight, FromBridgedChainMessagesProof<HashOf<C>>);

/// Substrate client as Substrate messages source.
pub struct SubstrateMessagesSource<P: SubstrateMessageLane> {
	client: Client<P::SourceChain>,
	lane: P,
	lane_id: LaneId,
	target_to_source_headers_relay: Option<OnDemandHeadersRelay<P::TargetChain>>,
}

impl<P: SubstrateMessageLane> SubstrateMessagesSource<P> {
	/// Create new Substrate headers source.
	pub fn new(
		client: Client<P::SourceChain>,
		lane: P,
		lane_id: LaneId,
		target_to_source_headers_relay: Option<OnDemandHeadersRelay<P::TargetChain>>,
	) -> Self {
		SubstrateMessagesSource { client, lane, lane_id, target_to_source_headers_relay }
	}
}

impl<P: SubstrateMessageLane> Clone for SubstrateMessagesSource<P> {
	fn clone(&self) -> Self {
		Self {
			client: self.client.clone(),
			lane: self.lane.clone(),
			lane_id: self.lane_id,
			target_to_source_headers_relay: self.target_to_source_headers_relay.clone(),
		}
	}
}

#[async_trait]
impl<P: SubstrateMessageLane> RelayClient for SubstrateMessagesSource<P> {
	type Error = SubstrateError;

	async fn reconnect(&mut self) -> Result<(), SubstrateError> {
		self.client.reconnect().await
	}
}

#[async_trait]
impl<P> SourceClient<P::MessageLane> for SubstrateMessagesSource<P>
where
	P: SubstrateMessageLane,
	P::SourceChain: Chain<
		Hash = <P::MessageLane as MessageLane>::SourceHeaderHash,
		BlockNumber = <P::MessageLane as MessageLane>::SourceHeaderNumber,
		Balance = <P::MessageLane as MessageLane>::SourceChainBalance,
	>,
	BalanceOf<P::SourceChain>: Decode + Bounded,
	IndexOf<P::SourceChain>: DeserializeOwned,
	HashOf<P::SourceChain>: Copy,
	BlockNumberOf<P::SourceChain>: BlockNumberBase + Copy,
	HeaderOf<P::SourceChain>: DeserializeOwned,
	P::TargetChain: Chain<
		Hash = <P::MessageLane as MessageLane>::TargetHeaderHash,
		BlockNumber = <P::MessageLane as MessageLane>::TargetHeaderNumber,
	>,

	P::MessageLane: MessageLane<
		MessagesProof = SubstrateMessagesProof<P::SourceChain>,
		MessagesReceivingProof = SubstrateMessagesReceivingProof<P::TargetChain>,
	>,
	<P::MessageLane as MessageLane>::TargetHeaderNumber: Decode,
	<P::MessageLane as MessageLane>::TargetHeaderHash: Decode,
	<P::MessageLane as MessageLane>::SourceChainBalance: AtLeast32BitUnsigned,
{
	async fn state(&self) -> Result<SourceClientState<P::MessageLane>, SubstrateError> {
		// we can't continue to deliver confirmations if source node is out of sync, because
		// it may have already received confirmations that we're going to deliver
		self.client.ensure_synced().await?;

		read_client_state::<
			_,
			<P::MessageLane as MessageLane>::TargetHeaderHash,
			<P::MessageLane as MessageLane>::TargetHeaderNumber,
		>(&self.client, P::BEST_FINALIZED_TARGET_HEADER_ID_AT_SOURCE)
		.await
	}

	async fn latest_generated_nonce(
		&self,
		id: SourceHeaderIdOf<P::MessageLane>,
	) -> Result<(SourceHeaderIdOf<P::MessageLane>, MessageNonce), SubstrateError> {
		let encoded_response = self
			.client
			.state_call(
				P::OUTBOUND_LANE_LATEST_GENERATED_NONCE_METHOD.into(),
				Bytes(self.lane_id.encode()),
				Some(id.1),
			)
			.await?;
		let latest_generated_nonce: MessageNonce = Decode::decode(&mut &encoded_response.0[..])
			.map_err(SubstrateError::ResponseParseFailed)?;
		Ok((id, latest_generated_nonce))
	}

	async fn latest_confirmed_received_nonce(
		&self,
		id: SourceHeaderIdOf<P::MessageLane>,
	) -> Result<(SourceHeaderIdOf<P::MessageLane>, MessageNonce), SubstrateError> {
		let encoded_response = self
			.client
			.state_call(
				P::OUTBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD.into(),
				Bytes(self.lane_id.encode()),
				Some(id.1),
			)
			.await?;
		let latest_received_nonce: MessageNonce = Decode::decode(&mut &encoded_response.0[..])
			.map_err(SubstrateError::ResponseParseFailed)?;
		Ok((id, latest_received_nonce))
	}

	async fn generated_message_details(
		&self,
		id: SourceHeaderIdOf<P::MessageLane>,
		nonces: RangeInclusive<MessageNonce>,
	) -> Result<
		MessageDetailsMap<<P::MessageLane as MessageLane>::SourceChainBalance>,
		SubstrateError,
	> {
		let encoded_response = self
			.client
			.state_call(
				P::OUTBOUND_LANE_MESSAGE_DETAILS_METHOD.into(),
				Bytes((self.lane_id, nonces.start(), nonces.end()).encode()),
				Some(id.1),
			)
			.await?;

		make_message_details_map::<P::SourceChain>(
			Decode::decode(&mut &encoded_response.0[..])
				.map_err(SubstrateError::ResponseParseFailed)?,
			nonces,
		)
	}

	async fn prove_messages(
		&self,
		id: SourceHeaderIdOf<P::MessageLane>,
		nonces: RangeInclusive<MessageNonce>,
		proof_parameters: MessageProofParameters,
	) -> Result<
		(
			SourceHeaderIdOf<P::MessageLane>,
			RangeInclusive<MessageNonce>,
			<P::MessageLane as MessageLane>::MessagesProof,
		),
		SubstrateError,
	> {
		let mut storage_keys =
			Vec::with_capacity(nonces.end().saturating_sub(*nonces.start()) as usize + 1);
		let mut message_nonce = *nonces.start();
		while message_nonce <= *nonces.end() {
			let message_key = pallet_bridge_messages::storage_keys::message_key(
				P::MESSAGE_PALLET_NAME_AT_SOURCE,
				&self.lane_id,
				message_nonce,
			);
			storage_keys.push(message_key);
			message_nonce += 1;
		}
		if proof_parameters.outbound_state_proof_required {
			storage_keys.push(pallet_bridge_messages::storage_keys::outbound_lane_data_key(
				P::MESSAGE_PALLET_NAME_AT_SOURCE,
				&self.lane_id,
			));
		}

		let proof = self.client.prove_storage(storage_keys, id.1).await?.iter_nodes().collect();
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
		generated_at_block: TargetHeaderIdOf<P::MessageLane>,
		proof: <P::MessageLane as MessageLane>::MessagesReceivingProof,
	) -> Result<(), SubstrateError> {
		let lane = self.lane.clone();
		self.client
			.submit_signed_extrinsic(
				self.lane.source_transactions_author(),
				move |best_block_id, transaction_nonce| {
					lane.make_messages_receiving_proof_transaction(
						best_block_id,
						transaction_nonce,
						generated_at_block,
						proof,
					)
				},
			)
			.await?;
		Ok(())
	}

	async fn require_target_header_on_source(&self, id: TargetHeaderIdOf<P::MessageLane>) {
		if let Some(ref target_to_source_headers_relay) = self.target_to_source_headers_relay {
			target_to_source_headers_relay.require_finalized_header(id).await;
		}
	}

	async fn estimate_confirmation_transaction(
		&self,
	) -> <P::MessageLane as MessageLane>::SourceChainBalance {
		self.client
			.estimate_extrinsic_fee(self.lane.make_messages_receiving_proof_transaction(
				HeaderId(Default::default(), Default::default()),
				Zero::zero(),
				HeaderId(Default::default(), Default::default()),
				prepare_dummy_messages_delivery_proof::<P::SourceChain, P::TargetChain>(),
			))
			.await
			.map(|fee| fee.inclusion_fee())
			.unwrap_or_else(|_| BalanceOf::<P::SourceChain>::max_value())
	}
}

/// Prepare 'dummy' messages delivery proof that will compose the delivery confirmation transaction.
///
/// We don't care about proof actually being the valid proof, because its validity doesn't
/// affect the call weight - we only care about its size.
fn prepare_dummy_messages_delivery_proof<SC: Chain, TC: Chain>(
) -> SubstrateMessagesReceivingProof<TC> {
	let single_message_confirmation_size = bp_messages::InboundLaneData::<()>::encoded_size_hint(
		SC::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE,
		1,
		1,
	)
	.unwrap_or(u32::MAX);
	let proof_size = TC::STORAGE_PROOF_OVERHEAD.saturating_add(single_message_confirmation_size);
	(
		UnrewardedRelayersState {
			unrewarded_relayer_entries: 1,
			messages_in_oldest_entry: 1,
			total_messages: 1,
		},
		FromBridgedChainMessagesDeliveryProof {
			bridged_header_hash: Default::default(),
			storage_proof: vec![vec![0; proof_size as usize]],
			lane: Default::default(),
		},
	)
}

/// Read best blocks from given client.
///
/// This function assumes that the chain that is followed by the `self_client` has
/// bridge GRANDPA pallet deployed and it provides `best_finalized_header_id_method_name`
/// runtime API to read the best finalized Bridged chain header.
pub async fn read_client_state<SelfChain, BridgedHeaderHash, BridgedHeaderNumber>(
	self_client: &Client<SelfChain>,
	best_finalized_header_id_method_name: &str,
) -> Result<
	ClientState<HeaderIdOf<SelfChain>, HeaderId<BridgedHeaderHash, BridgedHeaderNumber>>,
	SubstrateError,
>
where
	SelfChain: Chain,
	SelfChain::Header: DeserializeOwned,
	SelfChain::Index: DeserializeOwned,
	BridgedHeaderHash: Decode,
	BridgedHeaderNumber: Decode,
{
	// let's read our state first: we need best finalized header hash on **this** chain
	let self_best_finalized_header_hash = self_client.best_finalized_header_hash().await?;
	let self_best_finalized_header =
		self_client.header_by_hash(self_best_finalized_header_hash).await?;
	let self_best_finalized_id =
		HeaderId(*self_best_finalized_header.number(), self_best_finalized_header_hash);

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
		Decode::decode(&mut &encoded_best_finalized_peer_on_self.0[..])
			.map_err(SubstrateError::ResponseParseFailed)?;
	let peer_on_self_best_finalized_id =
		HeaderId(decoded_best_finalized_peer_on_self.0, decoded_best_finalized_peer_on_self.1);

	Ok(ClientState {
		best_self: self_best_id,
		best_finalized_self: self_best_finalized_id,
		best_finalized_peer_at_best_self: peer_on_self_best_finalized_id,
	})
}

fn make_message_details_map<C: Chain>(
	weights: Vec<bp_messages::MessageDetails<C::Balance>>,
	nonces: RangeInclusive<MessageNonce>,
) -> Result<MessageDetailsMap<C::Balance>, SubstrateError> {
	let make_missing_nonce_error = |expected_nonce| {
		Err(SubstrateError::Custom(format!(
			"Missing nonce {} in message_details call result. Expected all nonces from {:?}",
			expected_nonce, nonces,
		)))
	};

	let mut weights_map = MessageDetailsMap::new();

	// this is actually prevented by external logic
	if nonces.is_empty() {
		return Ok(weights_map)
	}

	// check if last nonce is missing - loop below is not checking this
	let last_nonce_is_missing =
		weights.last().map(|details| details.nonce != *nonces.end()).unwrap_or(true);
	if last_nonce_is_missing {
		return make_missing_nonce_error(*nonces.end())
	}

	let mut expected_nonce = *nonces.start();
	let mut is_at_head = true;

	for details in weights {
		match (details.nonce == expected_nonce, is_at_head) {
			(true, _) => (),
			(false, true) => {
				// this may happen if some messages were already pruned from the source node
				//
				// this is not critical error and will be auto-resolved by messages lane (and target
				// node)
				log::info!(
					target: "bridge",
					"Some messages are missing from the {} node: {:?}. Target node may be out of sync?",
					C::NAME,
					expected_nonce..details.nonce,
				);
			},
			(false, false) => {
				// some nonces are missing from the middle/tail of the range
				//
				// this is critical error, because we can't miss any nonces
				return make_missing_nonce_error(expected_nonce)
			},
		}

		weights_map.insert(
			details.nonce,
			MessageDetails {
				dispatch_weight: details.dispatch_weight,
				size: details.size as _,
				reward: details.delivery_and_dispatch_fee,
				dispatch_fee_payment: details.dispatch_fee_payment,
			},
		);
		expected_nonce = details.nonce + 1;
		is_at_head = false;
	}

	Ok(weights_map)
}

#[cfg(test)]
mod tests {
	use super::*;
	use bp_runtime::messages::DispatchFeePayment;
	use relay_rococo_client::Rococo;
	use relay_wococo_client::Wococo;

	fn message_details_from_rpc(
		nonces: RangeInclusive<MessageNonce>,
	) -> Vec<bp_messages::MessageDetails<bp_wococo::Balance>> {
		nonces
			.into_iter()
			.map(|nonce| bp_messages::MessageDetails {
				nonce,
				dispatch_weight: 0,
				size: 0,
				delivery_and_dispatch_fee: 0,
				dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
			})
			.collect()
	}

	#[test]
	fn make_message_details_map_succeeds_if_no_messages_are_missing() {
		assert_eq!(
			make_message_details_map::<Wococo>(message_details_from_rpc(1..=3), 1..=3,).unwrap(),
			vec![
				(
					1,
					MessageDetails {
						dispatch_weight: 0,
						size: 0,
						reward: 0,
						dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
					}
				),
				(
					2,
					MessageDetails {
						dispatch_weight: 0,
						size: 0,
						reward: 0,
						dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
					}
				),
				(
					3,
					MessageDetails {
						dispatch_weight: 0,
						size: 0,
						reward: 0,
						dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
					}
				),
			]
			.into_iter()
			.collect(),
		);
	}

	#[test]
	fn make_message_details_map_succeeds_if_head_messages_are_missing() {
		assert_eq!(
			make_message_details_map::<Wococo>(message_details_from_rpc(2..=3), 1..=3,).unwrap(),
			vec![
				(
					2,
					MessageDetails {
						dispatch_weight: 0,
						size: 0,
						reward: 0,
						dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
					}
				),
				(
					3,
					MessageDetails {
						dispatch_weight: 0,
						size: 0,
						reward: 0,
						dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
					}
				),
			]
			.into_iter()
			.collect(),
		);
	}

	#[test]
	fn make_message_details_map_fails_if_mid_messages_are_missing() {
		let mut message_details_from_rpc = message_details_from_rpc(1..=3);
		message_details_from_rpc.remove(1);
		assert!(matches!(
			make_message_details_map::<Wococo>(message_details_from_rpc, 1..=3,),
			Err(SubstrateError::Custom(_))
		));
	}

	#[test]
	fn make_message_details_map_fails_if_tail_messages_are_missing() {
		assert!(matches!(
			make_message_details_map::<Wococo>(message_details_from_rpc(1..=2), 1..=3,),
			Err(SubstrateError::Custom(_))
		));
	}

	#[test]
	fn make_message_details_map_fails_if_all_messages_are_missing() {
		assert!(matches!(
			make_message_details_map::<Wococo>(vec![], 1..=3),
			Err(SubstrateError::Custom(_))
		));
	}

	#[test]
	fn prepare_dummy_messages_delivery_proof_works() {
		let expected_minimal_size =
			Wococo::MAXIMAL_ENCODED_ACCOUNT_ID_SIZE + Rococo::STORAGE_PROOF_OVERHEAD;
		let dummy_proof = prepare_dummy_messages_delivery_proof::<Wococo, Rococo>();
		assert!(
			dummy_proof.1.encode().len() as u32 > expected_minimal_size,
			"Expected proof size at least {}. Got: {}",
			expected_minimal_size,
			dummy_proof.1.encode().len(),
		);
	}
}
