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
	messages_lane::{
		MessageLaneAdapter, ReceiveMessagesDeliveryProofCallBuilder, SubstrateMessageLane,
	},
	messages_target::SubstrateMessagesDeliveryProof,
	on_demand_headers::OnDemandHeadersRelay,
	TransactionParams,
};

use async_trait::async_trait;
use bp_messages::{
	storage_keys::{operating_mode_key, outbound_lane_data_key},
	LaneId, MessageNonce, OperatingMode, OutboundLaneData, UnrewardedRelayersState,
};
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
	AccountIdOf, AccountKeyPairOf, BalanceOf, BlockNumberOf, Chain, ChainWithMessages, Client,
	Error as SubstrateError, HashOf, HeaderIdOf, IndexOf, SignParam, TransactionEra,
	TransactionSignScheme, UnsignedTransaction,
};
use relay_utils::{relay_loop::Client as RelayClient, HeaderId};
use sp_core::{Bytes, Pair};
use sp_runtime::{traits::Header as HeaderT, DeserializeOwned};
use std::ops::RangeInclusive;

/// Intermediate message proof returned by the source Substrate node. Includes everything
/// required to submit to the target node: cumulative dispatch weight of bundled messages and
/// the proof itself.
pub type SubstrateMessagesProof<C> = (Weight, FromBridgedChainMessagesProof<HashOf<C>>);

/// Substrate client as Substrate messages source.
pub struct SubstrateMessagesSource<P: SubstrateMessageLane> {
	source_client: Client<P::SourceChain>,
	target_client: Client<P::TargetChain>,
	lane_id: LaneId,
	transaction_params: TransactionParams<AccountKeyPairOf<P::SourceTransactionSignScheme>>,
	target_to_source_headers_relay: Option<OnDemandHeadersRelay<P::TargetChain>>,
}

impl<P: SubstrateMessageLane> SubstrateMessagesSource<P> {
	/// Create new Substrate headers source.
	pub fn new(
		source_client: Client<P::SourceChain>,
		target_client: Client<P::TargetChain>,
		lane_id: LaneId,
		transaction_params: TransactionParams<AccountKeyPairOf<P::SourceTransactionSignScheme>>,
		target_to_source_headers_relay: Option<OnDemandHeadersRelay<P::TargetChain>>,
	) -> Self {
		SubstrateMessagesSource {
			source_client,
			target_client,
			lane_id,
			transaction_params,
			target_to_source_headers_relay,
		}
	}

	/// Read outbound lane state from the on-chain storage at given block.
	async fn outbound_lane_data(
		&self,
		id: SourceHeaderIdOf<MessageLaneAdapter<P>>,
	) -> Result<Option<OutboundLaneData>, SubstrateError> {
		self.source_client
			.storage_value(
				outbound_lane_data_key(
					P::TargetChain::WITH_CHAIN_MESSAGES_PALLET_NAME,
					&self.lane_id,
				),
				Some(id.1),
			)
			.await
	}

	/// Ensure that the messages pallet at source chain is active.
	async fn ensure_pallet_active(&self) -> Result<(), SubstrateError> {
		ensure_messages_pallet_active::<P::SourceChain, P::TargetChain>(&self.source_client).await
	}
}

impl<P: SubstrateMessageLane> Clone for SubstrateMessagesSource<P> {
	fn clone(&self) -> Self {
		Self {
			source_client: self.source_client.clone(),
			target_client: self.target_client.clone(),
			lane_id: self.lane_id,
			transaction_params: self.transaction_params.clone(),
			target_to_source_headers_relay: self.target_to_source_headers_relay.clone(),
		}
	}
}

#[async_trait]
impl<P: SubstrateMessageLane> RelayClient for SubstrateMessagesSource<P> {
	type Error = SubstrateError;

	async fn reconnect(&mut self) -> Result<(), SubstrateError> {
		self.source_client.reconnect().await?;
		self.target_client.reconnect().await
	}
}

#[async_trait]
impl<P: SubstrateMessageLane> SourceClient<MessageLaneAdapter<P>> for SubstrateMessagesSource<P>
where
	AccountIdOf<P::SourceChain>:
		From<<AccountKeyPairOf<P::SourceTransactionSignScheme> as Pair>::Public>,
	P::SourceTransactionSignScheme: TransactionSignScheme<Chain = P::SourceChain>,
{
	async fn state(&self) -> Result<SourceClientState<MessageLaneAdapter<P>>, SubstrateError> {
		// we can't continue to deliver confirmations if source node is out of sync, because
		// it may have already received confirmations that we're going to deliver
		self.source_client.ensure_synced().await?;
		// we can't relay confirmations if messages pallet at source chain is halted
		self.ensure_pallet_active().await?;

		read_client_state(
			&self.source_client,
			Some(&self.target_client),
			P::TargetChain::BEST_FINALIZED_HEADER_ID_METHOD,
		)
		.await
	}

	async fn latest_generated_nonce(
		&self,
		id: SourceHeaderIdOf<MessageLaneAdapter<P>>,
	) -> Result<(SourceHeaderIdOf<MessageLaneAdapter<P>>, MessageNonce), SubstrateError> {
		// lane data missing from the storage is fine until first message is sent
		let latest_generated_nonce = self
			.outbound_lane_data(id)
			.await?
			.map(|data| data.latest_generated_nonce)
			.unwrap_or(0);
		Ok((id, latest_generated_nonce))
	}

	async fn latest_confirmed_received_nonce(
		&self,
		id: SourceHeaderIdOf<MessageLaneAdapter<P>>,
	) -> Result<(SourceHeaderIdOf<MessageLaneAdapter<P>>, MessageNonce), SubstrateError> {
		// lane data missing from the storage is fine until first message is sent
		let latest_received_nonce = self
			.outbound_lane_data(id)
			.await?
			.map(|data| data.latest_received_nonce)
			.unwrap_or(0);
		Ok((id, latest_received_nonce))
	}

	async fn generated_message_details(
		&self,
		id: SourceHeaderIdOf<MessageLaneAdapter<P>>,
		nonces: RangeInclusive<MessageNonce>,
	) -> Result<
		MessageDetailsMap<<MessageLaneAdapter<P> as MessageLane>::SourceChainBalance>,
		SubstrateError,
	> {
		let encoded_response = self
			.source_client
			.state_call(
				P::TargetChain::TO_CHAIN_MESSAGE_DETAILS_METHOD.into(),
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
		id: SourceHeaderIdOf<MessageLaneAdapter<P>>,
		nonces: RangeInclusive<MessageNonce>,
		proof_parameters: MessageProofParameters,
	) -> Result<
		(
			SourceHeaderIdOf<MessageLaneAdapter<P>>,
			RangeInclusive<MessageNonce>,
			<MessageLaneAdapter<P> as MessageLane>::MessagesProof,
		),
		SubstrateError,
	> {
		let mut storage_keys =
			Vec::with_capacity(nonces.end().saturating_sub(*nonces.start()) as usize + 1);
		let mut message_nonce = *nonces.start();
		while message_nonce <= *nonces.end() {
			let message_key = bp_messages::storage_keys::message_key(
				P::TargetChain::WITH_CHAIN_MESSAGES_PALLET_NAME,
				&self.lane_id,
				message_nonce,
			);
			storage_keys.push(message_key);
			message_nonce += 1;
		}
		if proof_parameters.outbound_state_proof_required {
			storage_keys.push(bp_messages::storage_keys::outbound_lane_data_key(
				P::TargetChain::WITH_CHAIN_MESSAGES_PALLET_NAME,
				&self.lane_id,
			));
		}

		let proof = self
			.source_client
			.prove_storage(storage_keys, id.1)
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
		_generated_at_block: TargetHeaderIdOf<MessageLaneAdapter<P>>,
		proof: <MessageLaneAdapter<P> as MessageLane>::MessagesReceivingProof,
	) -> Result<(), SubstrateError> {
		let genesis_hash = *self.source_client.genesis_hash();
		let transaction_params = self.transaction_params.clone();
		let (spec_version, transaction_version) =
			self.source_client.simple_runtime_version().await?;
		self.source_client
			.submit_signed_extrinsic(
				self.transaction_params.signer.public().into(),
				move |best_block_id, transaction_nonce| {
					make_messages_delivery_proof_transaction::<P>(
						spec_version,
						transaction_version,
						&genesis_hash,
						&transaction_params,
						best_block_id,
						transaction_nonce,
						proof,
						true,
					)
				},
			)
			.await?;
		Ok(())
	}

	async fn require_target_header_on_source(&self, id: TargetHeaderIdOf<MessageLaneAdapter<P>>) {
		if let Some(ref target_to_source_headers_relay) = self.target_to_source_headers_relay {
			target_to_source_headers_relay.require_finalized_header(id).await;
		}
	}

	async fn estimate_confirmation_transaction(
		&self,
	) -> <MessageLaneAdapter<P> as MessageLane>::SourceChainBalance {
		let runtime_version = match self.source_client.runtime_version().await {
			Ok(v) => v,
			Err(_) => return BalanceOf::<P::SourceChain>::max_value(),
		};
		async {
			let dummy_tx = make_messages_delivery_proof_transaction::<P>(
				runtime_version.spec_version,
				runtime_version.transaction_version,
				self.source_client.genesis_hash(),
				&self.transaction_params,
				HeaderId(Default::default(), Default::default()),
				Zero::zero(),
				prepare_dummy_messages_delivery_proof::<P::SourceChain, P::TargetChain>(),
				false,
			)?;
			self.source_client
				.estimate_extrinsic_fee(dummy_tx)
				.await
				.map(|fee| fee.inclusion_fee())
		}
		.await
		.unwrap_or_else(|_| BalanceOf::<P::SourceChain>::max_value())
	}
}

/// Ensure that the messages pallet at source chain is active.
pub(crate) async fn ensure_messages_pallet_active<AtChain, WithChain>(
	client: &Client<AtChain>,
) -> Result<(), SubstrateError>
where
	AtChain: ChainWithMessages,
	WithChain: ChainWithMessages,
{
	let operating_mode = client
		.storage_value(operating_mode_key(WithChain::WITH_CHAIN_MESSAGES_PALLET_NAME), None)
		.await?;
	let is_halted = operating_mode == Some(OperatingMode::Halted);
	if is_halted {
		Err(SubstrateError::BridgePalletIsHalted)
	} else {
		Ok(())
	}
}

/// Make messages delivery proof transaction from given proof.
#[allow(clippy::too_many_arguments)]
fn make_messages_delivery_proof_transaction<P: SubstrateMessageLane>(
	spec_version: u32,
	transaction_version: u32,
	source_genesis_hash: &HashOf<P::SourceChain>,
	source_transaction_params: &TransactionParams<AccountKeyPairOf<P::SourceTransactionSignScheme>>,
	source_best_block_id: HeaderIdOf<P::SourceChain>,
	transaction_nonce: IndexOf<P::SourceChain>,
	proof: SubstrateMessagesDeliveryProof<P::TargetChain>,
	trace_call: bool,
) -> Result<Bytes, SubstrateError>
where
	P::SourceTransactionSignScheme: TransactionSignScheme<Chain = P::SourceChain>,
{
	let call =
		P::ReceiveMessagesDeliveryProofCallBuilder::build_receive_messages_delivery_proof_call(
			proof, trace_call,
		);
	Ok(Bytes(
		P::SourceTransactionSignScheme::sign_transaction(SignParam {
			spec_version,
			transaction_version,
			genesis_hash: *source_genesis_hash,
			signer: source_transaction_params.signer.clone(),
			era: TransactionEra::new(source_best_block_id, source_transaction_params.mortality),
			unsigned: UnsignedTransaction::new(call.into(), transaction_nonce),
		})?
		.encode(),
	))
}

/// Prepare 'dummy' messages delivery proof that will compose the delivery confirmation transaction.
///
/// We don't care about proof actually being the valid proof, because its validity doesn't
/// affect the call weight - we only care about its size.
fn prepare_dummy_messages_delivery_proof<SC: Chain, TC: Chain>(
) -> SubstrateMessagesDeliveryProof<TC> {
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
///
/// If `peer_client` is `None`, the value of `actual_best_finalized_peer_at_best_self` will
/// always match the `best_finalized_peer_at_best_self`.
pub async fn read_client_state<SelfChain, PeerChain>(
	self_client: &Client<SelfChain>,
	peer_client: Option<&Client<PeerChain>>,
	best_finalized_header_id_method_name: &str,
) -> Result<ClientState<HeaderIdOf<SelfChain>, HeaderIdOf<PeerChain>>, SubstrateError>
where
	SelfChain: Chain,
	SelfChain::Header: DeserializeOwned,
	SelfChain::Index: DeserializeOwned,
	PeerChain: Chain,
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
	let decoded_best_finalized_peer_on_self: (BlockNumberOf<PeerChain>, HashOf<PeerChain>) =
		Decode::decode(&mut &encoded_best_finalized_peer_on_self.0[..])
			.map_err(SubstrateError::ResponseParseFailed)?;
	let peer_on_self_best_finalized_id =
		HeaderId(decoded_best_finalized_peer_on_self.0, decoded_best_finalized_peer_on_self.1);

	// read actual header, matching the `peer_on_self_best_finalized_id` from the peer chain
	let actual_peer_on_self_best_finalized_id = match peer_client {
		Some(peer_client) => {
			let actual_peer_on_self_best_finalized =
				peer_client.header_by_number(peer_on_self_best_finalized_id.0).await?;
			HeaderId(peer_on_self_best_finalized_id.0, actual_peer_on_self_best_finalized.hash())
		},
		None => peer_on_self_best_finalized_id,
	};

	Ok(ClientState {
		best_self: self_best_id,
		best_finalized_self: self_best_finalized_id,
		best_finalized_peer_at_best_self: peer_on_self_best_finalized_id,
		actual_best_finalized_peer_at_best_self: actual_peer_on_self_best_finalized_id,
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
