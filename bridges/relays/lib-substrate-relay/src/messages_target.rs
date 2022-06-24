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

use crate::{
	messages_lane::{MessageLaneAdapter, ReceiveMessagesProofCallBuilder, SubstrateMessageLane},
	messages_metrics::StandaloneMessagesMetrics,
	messages_source::{ensure_messages_pallet_active, read_client_state, SubstrateMessagesProof},
	on_demand_headers::OnDemandHeadersRelay,
	TransactionParams,
};

use async_trait::async_trait;
use bp_messages::{
	storage_keys::inbound_lane_data_key, total_unrewarded_messages, InboundLaneData, LaneId,
	MessageNonce, UnrewardedRelayersState,
};
use bridge_runtime_common::messages::{
	source::FromBridgedChainMessagesDeliveryProof, target::FromBridgedChainMessagesProof,
};
use codec::Encode;
use frame_support::weights::{Weight, WeightToFeePolynomial};
use messages_relay::{
	message_lane::{MessageLane, SourceHeaderIdOf, TargetHeaderIdOf},
	message_lane_loop::{TargetClient, TargetClientState},
};
use num_traits::{Bounded, Zero};
use relay_substrate_client::{
	AccountIdOf, AccountKeyPairOf, BalanceOf, Chain, ChainWithMessages, Client,
	Error as SubstrateError, HashOf, HeaderIdOf, IndexOf, SignParam, TransactionEra,
	TransactionSignScheme, UnsignedTransaction, WeightToFeeOf,
};
use relay_utils::{relay_loop::Client as RelayClient, HeaderId};
use sp_core::{Bytes, Pair};
use sp_runtime::{traits::Saturating, FixedPointNumber, FixedU128};
use std::{collections::VecDeque, ops::RangeInclusive};

/// Message receiving proof returned by the target Substrate node.
pub type SubstrateMessagesDeliveryProof<C> =
	(UnrewardedRelayersState, FromBridgedChainMessagesDeliveryProof<HashOf<C>>);

/// Substrate client as Substrate messages target.
pub struct SubstrateMessagesTarget<P: SubstrateMessageLane> {
	target_client: Client<P::TargetChain>,
	source_client: Client<P::SourceChain>,
	lane_id: LaneId,
	relayer_id_at_source: AccountIdOf<P::SourceChain>,
	transaction_params: TransactionParams<AccountKeyPairOf<P::TargetTransactionSignScheme>>,
	metric_values: StandaloneMessagesMetrics<P::SourceChain, P::TargetChain>,
	source_to_target_headers_relay: Option<OnDemandHeadersRelay<P::SourceChain>>,
}

impl<P: SubstrateMessageLane> SubstrateMessagesTarget<P> {
	/// Create new Substrate headers target.
	pub fn new(
		target_client: Client<P::TargetChain>,
		source_client: Client<P::SourceChain>,
		lane_id: LaneId,
		relayer_id_at_source: AccountIdOf<P::SourceChain>,
		transaction_params: TransactionParams<AccountKeyPairOf<P::TargetTransactionSignScheme>>,
		metric_values: StandaloneMessagesMetrics<P::SourceChain, P::TargetChain>,
		source_to_target_headers_relay: Option<OnDemandHeadersRelay<P::SourceChain>>,
	) -> Self {
		SubstrateMessagesTarget {
			target_client,
			source_client,
			lane_id,
			relayer_id_at_source,
			transaction_params,
			metric_values,
			source_to_target_headers_relay,
		}
	}

	/// Read inbound lane state from the on-chain storage at given block.
	async fn inbound_lane_data(
		&self,
		id: TargetHeaderIdOf<MessageLaneAdapter<P>>,
	) -> Result<Option<InboundLaneData<AccountIdOf<P::SourceChain>>>, SubstrateError> {
		self.target_client
			.storage_value(
				inbound_lane_data_key(
					P::SourceChain::WITH_CHAIN_MESSAGES_PALLET_NAME,
					&self.lane_id,
				),
				Some(id.1),
			)
			.await
	}

	/// Ensure that the messages pallet at target chain is active.
	async fn ensure_pallet_active(&self) -> Result<(), SubstrateError> {
		ensure_messages_pallet_active::<P::TargetChain, P::SourceChain>(&self.target_client).await
	}
}

impl<P: SubstrateMessageLane> Clone for SubstrateMessagesTarget<P> {
	fn clone(&self) -> Self {
		Self {
			target_client: self.target_client.clone(),
			source_client: self.source_client.clone(),
			lane_id: self.lane_id,
			relayer_id_at_source: self.relayer_id_at_source.clone(),
			transaction_params: self.transaction_params.clone(),
			metric_values: self.metric_values.clone(),
			source_to_target_headers_relay: self.source_to_target_headers_relay.clone(),
		}
	}
}

#[async_trait]
impl<P: SubstrateMessageLane> RelayClient for SubstrateMessagesTarget<P> {
	type Error = SubstrateError;

	async fn reconnect(&mut self) -> Result<(), SubstrateError> {
		self.target_client.reconnect().await?;
		self.source_client.reconnect().await
	}
}

#[async_trait]
impl<P: SubstrateMessageLane> TargetClient<MessageLaneAdapter<P>> for SubstrateMessagesTarget<P>
where
	AccountIdOf<P::TargetChain>:
		From<<AccountKeyPairOf<P::TargetTransactionSignScheme> as Pair>::Public>,
	P::TargetTransactionSignScheme: TransactionSignScheme<Chain = P::TargetChain>,
	BalanceOf<P::SourceChain>: TryFrom<BalanceOf<P::TargetChain>>,
{
	async fn state(&self) -> Result<TargetClientState<MessageLaneAdapter<P>>, SubstrateError> {
		// we can't continue to deliver messages if target node is out of sync, because
		// it may have already received (some of) messages that we're going to deliver
		self.target_client.ensure_synced().await?;
		// we can't relay messages if messages pallet at target chain is halted
		self.ensure_pallet_active().await?;

		read_client_state(
			&self.target_client,
			Some(&self.source_client),
			P::SourceChain::BEST_FINALIZED_HEADER_ID_METHOD,
		)
		.await
	}

	async fn latest_received_nonce(
		&self,
		id: TargetHeaderIdOf<MessageLaneAdapter<P>>,
	) -> Result<(TargetHeaderIdOf<MessageLaneAdapter<P>>, MessageNonce), SubstrateError> {
		// lane data missing from the storage is fine until first message is received
		let latest_received_nonce = self
			.inbound_lane_data(id)
			.await?
			.map(|data| data.last_delivered_nonce())
			.unwrap_or(0);
		Ok((id, latest_received_nonce))
	}

	async fn latest_confirmed_received_nonce(
		&self,
		id: TargetHeaderIdOf<MessageLaneAdapter<P>>,
	) -> Result<(TargetHeaderIdOf<MessageLaneAdapter<P>>, MessageNonce), SubstrateError> {
		// lane data missing from the storage is fine until first message is received
		let last_confirmed_nonce = self
			.inbound_lane_data(id)
			.await?
			.map(|data| data.last_confirmed_nonce)
			.unwrap_or(0);
		Ok((id, last_confirmed_nonce))
	}

	async fn unrewarded_relayers_state(
		&self,
		id: TargetHeaderIdOf<MessageLaneAdapter<P>>,
	) -> Result<(TargetHeaderIdOf<MessageLaneAdapter<P>>, UnrewardedRelayersState), SubstrateError>
	{
		let relayers = self
			.inbound_lane_data(id)
			.await?
			.map(|data| data.relayers)
			.unwrap_or_else(|| VecDeque::new());
		let unrewarded_relayers_state = bp_messages::UnrewardedRelayersState {
			unrewarded_relayer_entries: relayers.len() as _,
			messages_in_oldest_entry: relayers
				.front()
				.map(|entry| 1 + entry.messages.end - entry.messages.begin)
				.unwrap_or(0),
			total_messages: total_unrewarded_messages(&relayers).unwrap_or(MessageNonce::MAX),
		};
		Ok((id, unrewarded_relayers_state))
	}

	async fn prove_messages_receiving(
		&self,
		id: TargetHeaderIdOf<MessageLaneAdapter<P>>,
	) -> Result<
		(
			TargetHeaderIdOf<MessageLaneAdapter<P>>,
			<MessageLaneAdapter<P> as MessageLane>::MessagesReceivingProof,
		),
		SubstrateError,
	> {
		let (id, relayers_state) = self.unrewarded_relayers_state(id).await?;
		let inbound_data_key = bp_messages::storage_keys::inbound_lane_data_key(
			P::SourceChain::WITH_CHAIN_MESSAGES_PALLET_NAME,
			&self.lane_id,
		);
		let proof = self
			.target_client
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
		_generated_at_header: SourceHeaderIdOf<MessageLaneAdapter<P>>,
		nonces: RangeInclusive<MessageNonce>,
		proof: <MessageLaneAdapter<P> as MessageLane>::MessagesProof,
	) -> Result<RangeInclusive<MessageNonce>, SubstrateError> {
		let genesis_hash = *self.target_client.genesis_hash();
		let transaction_params = self.transaction_params.clone();
		let relayer_id_at_source = self.relayer_id_at_source.clone();
		let nonces_clone = nonces.clone();
		let (spec_version, transaction_version) =
			self.target_client.simple_runtime_version().await?;
		self.target_client
			.submit_signed_extrinsic(
				self.transaction_params.signer.public().into(),
				move |best_block_id, transaction_nonce| {
					make_messages_delivery_transaction::<P>(
						spec_version,
						transaction_version,
						&genesis_hash,
						&transaction_params,
						best_block_id,
						transaction_nonce,
						relayer_id_at_source,
						nonces_clone,
						proof,
						true,
					)
				},
			)
			.await?;
		Ok(nonces)
	}

	async fn require_source_header_on_target(&self, id: SourceHeaderIdOf<MessageLaneAdapter<P>>) {
		if let Some(ref source_to_target_headers_relay) = self.source_to_target_headers_relay {
			source_to_target_headers_relay.require_finalized_header(id).await;
		}
	}

	async fn estimate_delivery_transaction_in_source_tokens(
		&self,
		nonces: RangeInclusive<MessageNonce>,
		total_prepaid_nonces: MessageNonce,
		total_dispatch_weight: Weight,
		total_size: u32,
	) -> Result<<MessageLaneAdapter<P> as MessageLane>::SourceChainBalance, SubstrateError> {
		let conversion_rate =
			self.metric_values.target_to_source_conversion_rate().await.ok_or_else(|| {
				SubstrateError::Custom(format!(
					"Failed to compute conversion rate from {} to {}",
					P::TargetChain::NAME,
					P::SourceChain::NAME,
				))
			})?;

		let (spec_version, transaction_version) =
			self.target_client.simple_runtime_version().await?;
		// Prepare 'dummy' delivery transaction - we only care about its length and dispatch weight.
		let delivery_tx = make_messages_delivery_transaction::<P>(
			spec_version,
			transaction_version,
			self.target_client.genesis_hash(),
			&self.transaction_params,
			HeaderId(Default::default(), Default::default()),
			Zero::zero(),
			self.relayer_id_at_source.clone(),
			nonces.clone(),
			prepare_dummy_messages_proof::<P::SourceChain>(
				nonces.clone(),
				total_dispatch_weight,
				total_size,
			),
			false,
		)?;
		let delivery_tx_fee = self.target_client.estimate_extrinsic_fee(delivery_tx).await?;
		let inclusion_fee_in_target_tokens = delivery_tx_fee.inclusion_fee();

		// The pre-dispatch cost of delivery transaction includes additional fee to cover dispatch
		// fee payment (Currency::transfer in regular deployment). But if message dispatch has
		// already been paid at the Source chain, the delivery transaction will refund relayer with
		// this additional cost. But `estimate_extrinsic_fee` obviously just returns pre-dispatch
		// cost of the transaction. So if transaction delivers prepaid message, then it may happen
		// that pre-dispatch cost is larger than reward and `Rational` relayer will refuse to
		// deliver this message.
		//
		// The most obvious solution would be to deduct total weight of dispatch fee payments from
		// the `total_dispatch_weight` and use regular `estimate_extrinsic_fee` call. But what if
		// `total_dispatch_weight` is less than total dispatch fee payments weight? Weight is
		// strictly positive, so we can't use this option.
		//
		// Instead we'll be directly using `WeightToFee` and `NextFeeMultiplier` of the Target
		// chain. This requires more knowledge of the Target chain, but seems there's no better way
		// to solve this now.
		let expected_refund_in_target_tokens = if total_prepaid_nonces != 0 {
			const WEIGHT_DIFFERENCE: Weight = 100;

			let (spec_version, transaction_version) =
				self.target_client.simple_runtime_version().await?;
			let larger_dispatch_weight = total_dispatch_weight.saturating_add(WEIGHT_DIFFERENCE);
			let dummy_tx = make_messages_delivery_transaction::<P>(
				spec_version,
				transaction_version,
				self.target_client.genesis_hash(),
				&self.transaction_params,
				HeaderId(Default::default(), Default::default()),
				Zero::zero(),
				self.relayer_id_at_source.clone(),
				nonces.clone(),
				prepare_dummy_messages_proof::<P::SourceChain>(
					nonces.clone(),
					larger_dispatch_weight,
					total_size,
				),
				false,
			)?;
			let larger_delivery_tx_fee =
				self.target_client.estimate_extrinsic_fee(dummy_tx).await?;

			compute_prepaid_messages_refund::<P::TargetChain>(
				total_prepaid_nonces,
				compute_fee_multiplier::<P::TargetChain>(
					delivery_tx_fee.adjusted_weight_fee,
					total_dispatch_weight,
					larger_delivery_tx_fee.adjusted_weight_fee,
					larger_dispatch_weight,
				),
			)
		} else {
			Zero::zero()
		};

		let delivery_fee_in_source_tokens =
			convert_target_tokens_to_source_tokens::<P::SourceChain, P::TargetChain>(
				FixedU128::from_float(conversion_rate),
				inclusion_fee_in_target_tokens.saturating_sub(expected_refund_in_target_tokens),
			);

		log::trace!(
			target: "bridge",
			"Estimated {} -> {} messages delivery transaction.\n\t\
				Total nonces: {:?}\n\t\
				Prepaid messages: {}\n\t\
				Total messages size: {}\n\t\
				Total messages dispatch weight: {}\n\t\
				Inclusion fee (in {1} tokens): {:?}\n\t\
				Expected refund (in {1} tokens): {:?}\n\t\
				{1} -> {0} conversion rate: {:?}\n\t\
				Expected delivery tx fee (in {0} tokens): {:?}",
				P::SourceChain::NAME,
				P::TargetChain::NAME,
				nonces,
				total_prepaid_nonces,
				total_size,
				total_dispatch_weight,
				inclusion_fee_in_target_tokens,
				expected_refund_in_target_tokens,
				conversion_rate,
				delivery_fee_in_source_tokens,
		);

		Ok(delivery_fee_in_source_tokens)
	}
}

/// Make messages delivery transaction from given proof.
#[allow(clippy::too_many_arguments)]
fn make_messages_delivery_transaction<P: SubstrateMessageLane>(
	spec_version: u32,
	transaction_version: u32,
	target_genesis_hash: &HashOf<P::TargetChain>,
	target_transaction_params: &TransactionParams<AccountKeyPairOf<P::TargetTransactionSignScheme>>,
	target_best_block_id: HeaderIdOf<P::TargetChain>,
	transaction_nonce: IndexOf<P::TargetChain>,
	relayer_id_at_source: AccountIdOf<P::SourceChain>,
	nonces: RangeInclusive<MessageNonce>,
	proof: SubstrateMessagesProof<P::SourceChain>,
	trace_call: bool,
) -> Result<Bytes, SubstrateError>
where
	P::TargetTransactionSignScheme: TransactionSignScheme<Chain = P::TargetChain>,
{
	let messages_count = nonces.end() - nonces.start() + 1;
	let dispatch_weight = proof.0;
	let call = P::ReceiveMessagesProofCallBuilder::build_receive_messages_proof_call(
		relayer_id_at_source,
		proof,
		messages_count as _,
		dispatch_weight,
		trace_call,
	);
	Ok(Bytes(
		P::TargetTransactionSignScheme::sign_transaction(SignParam {
			spec_version,
			transaction_version,
			genesis_hash: *target_genesis_hash,
			signer: target_transaction_params.signer.clone(),
			era: TransactionEra::new(target_best_block_id, target_transaction_params.mortality),
			unsigned: UnsignedTransaction::new(call.into(), transaction_nonce),
		})?
		.encode(),
	))
}

/// Prepare 'dummy' messages proof that will compose the delivery transaction.
///
/// We don't care about proof actually being the valid proof, because its validity doesn't
/// affect the call weight - we only care about its size.
fn prepare_dummy_messages_proof<SC: Chain>(
	nonces: RangeInclusive<MessageNonce>,
	total_dispatch_weight: Weight,
	total_size: u32,
) -> SubstrateMessagesProof<SC> {
	(
		total_dispatch_weight,
		FromBridgedChainMessagesProof {
			bridged_header_hash: Default::default(),
			storage_proof: vec![vec![
				0;
				SC::STORAGE_PROOF_OVERHEAD.saturating_add(total_size) as usize
			]],
			lane: Default::default(),
			nonces_start: *nonces.start(),
			nonces_end: *nonces.end(),
		},
	)
}

/// Given delivery transaction fee in target chain tokens and conversion rate to the source
/// chain tokens, compute transaction cost in source chain tokens.
fn convert_target_tokens_to_source_tokens<SC: Chain, TC: Chain>(
	target_to_source_conversion_rate: FixedU128,
	target_transaction_fee: TC::Balance,
) -> SC::Balance
where
	SC::Balance: TryFrom<TC::Balance>,
{
	SC::Balance::try_from(
		target_to_source_conversion_rate.saturating_mul_int(target_transaction_fee),
	)
	.unwrap_or_else(|_| SC::Balance::max_value())
}

/// Compute fee multiplier that is used by the chain, given a couple of fees for transactions
/// that are only differ in dispatch weights.
///
/// This function assumes that standard transaction payment pallet is used by the chain.
/// The only fee component that depends on dispatch weight is the `adjusted_weight_fee`.
///
/// **WARNING**: this functions will only be accurate if weight-to-fee conversion function
/// is linear. For non-linear polynomials the error will grow with `weight_difference` growth.
/// So better to use smaller differences.
fn compute_fee_multiplier<C: Chain>(
	smaller_adjusted_weight_fee: BalanceOf<C>,
	smaller_tx_weight: Weight,
	larger_adjusted_weight_fee: BalanceOf<C>,
	larger_tx_weight: Weight,
) -> FixedU128 {
	let adjusted_weight_fee_difference =
		larger_adjusted_weight_fee.saturating_sub(smaller_adjusted_weight_fee);
	let smaller_tx_unadjusted_weight_fee = WeightToFeeOf::<C>::weight_to_fee(&smaller_tx_weight);
	let larger_tx_unadjusted_weight_fee = WeightToFeeOf::<C>::weight_to_fee(&larger_tx_weight);
	FixedU128::saturating_from_rational(
		adjusted_weight_fee_difference,
		larger_tx_unadjusted_weight_fee.saturating_sub(smaller_tx_unadjusted_weight_fee),
	)
}

/// Compute fee that will be refunded to the relayer because dispatch of `total_prepaid_nonces`
/// messages has been paid at the source chain.
fn compute_prepaid_messages_refund<C: ChainWithMessages>(
	total_prepaid_nonces: MessageNonce,
	fee_multiplier: FixedU128,
) -> BalanceOf<C> {
	fee_multiplier.saturating_mul_int(WeightToFeeOf::<C>::weight_to_fee(
		&C::PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_CHAIN.saturating_mul(total_prepaid_nonces),
	))
}

#[cfg(test)]
mod tests {
	use super::*;
	use relay_rococo_client::Rococo;
	use relay_wococo_client::Wococo;

	#[test]
	fn prepare_dummy_messages_proof_works() {
		const DISPATCH_WEIGHT: Weight = 1_000_000;
		const SIZE: u32 = 1_000;
		let dummy_proof = prepare_dummy_messages_proof::<Rococo>(1..=10, DISPATCH_WEIGHT, SIZE);
		assert_eq!(dummy_proof.0, DISPATCH_WEIGHT);
		assert!(
			dummy_proof.1.encode().len() as u32 > SIZE,
			"Expected proof size at least {}. Got: {}",
			SIZE,
			dummy_proof.1.encode().len(),
		);
	}

	#[test]
	fn convert_target_tokens_to_source_tokens_works() {
		assert_eq!(
			convert_target_tokens_to_source_tokens::<Rococo, Wococo>((150, 100).into(), 1_000),
			1_500
		);
		assert_eq!(
			convert_target_tokens_to_source_tokens::<Rococo, Wococo>((50, 100).into(), 1_000),
			500
		);
		assert_eq!(
			convert_target_tokens_to_source_tokens::<Rococo, Wococo>((100, 100).into(), 1_000),
			1_000
		);
	}

	#[test]
	fn compute_fee_multiplier_returns_sane_results() {
		let multiplier = FixedU128::saturating_from_rational(1, 1000);

		let smaller_weight = 1_000_000;
		let smaller_adjusted_weight_fee =
			multiplier.saturating_mul_int(WeightToFeeOf::<Rococo>::weight_to_fee(&smaller_weight));

		let larger_weight = smaller_weight + 200_000;
		let larger_adjusted_weight_fee =
			multiplier.saturating_mul_int(WeightToFeeOf::<Rococo>::weight_to_fee(&larger_weight));

		assert_eq!(
			compute_fee_multiplier::<Rococo>(
				smaller_adjusted_weight_fee,
				smaller_weight,
				larger_adjusted_weight_fee,
				larger_weight,
			),
			multiplier,
		);
	}

	#[test]
	fn compute_prepaid_messages_refund_returns_sane_results() {
		assert!(
			compute_prepaid_messages_refund::<Wococo>(
				10,
				FixedU128::saturating_from_rational(110, 100),
			) > (10 * Wococo::PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_CHAIN).into()
		);
	}
}
