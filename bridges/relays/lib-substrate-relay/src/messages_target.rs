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
	messages_lane::{StandaloneMessagesMetrics, SubstrateMessageLane},
	messages_source::{read_client_state, SubstrateMessagesProof},
	on_demand_headers::OnDemandHeadersRelay,
};

use async_trait::async_trait;
use bp_messages::{LaneId, MessageNonce, UnrewardedRelayersState};

use bridge_runtime_common::messages::{
	source::FromBridgedChainMessagesDeliveryProof, target::FromBridgedChainMessagesProof,
};
use codec::{Decode, Encode};
use frame_support::weights::{Weight, WeightToFeePolynomial};
use messages_relay::{
	message_lane::{MessageLane, SourceHeaderIdOf, TargetHeaderIdOf},
	message_lane_loop::{TargetClient, TargetClientState},
};
use num_traits::{Bounded, Zero};
use relay_substrate_client::{
	BalanceOf, BlockNumberOf, Chain, Client, Error as SubstrateError, HashOf, HeaderOf, IndexOf,
	WeightToFeeOf,
};
use relay_utils::{relay_loop::Client as RelayClient, BlockNumberBase, HeaderId};
use sp_core::Bytes;
use sp_runtime::{traits::Saturating, DeserializeOwned, FixedPointNumber, FixedU128};
use std::{convert::TryFrom, ops::RangeInclusive};

/// Message receiving proof returned by the target Substrate node.
pub type SubstrateMessagesReceivingProof<C> =
	(UnrewardedRelayersState, FromBridgedChainMessagesDeliveryProof<HashOf<C>>);

/// Substrate client as Substrate messages target.
pub struct SubstrateMessagesTarget<P: SubstrateMessageLane> {
	client: Client<P::TargetChain>,
	lane: P,
	lane_id: LaneId,
	metric_values: StandaloneMessagesMetrics<P::SourceChain, P::TargetChain>,
	source_to_target_headers_relay: Option<OnDemandHeadersRelay<P::SourceChain>>,
}

impl<P: SubstrateMessageLane> SubstrateMessagesTarget<P> {
	/// Create new Substrate headers target.
	pub fn new(
		client: Client<P::TargetChain>,
		lane: P,
		lane_id: LaneId,
		metric_values: StandaloneMessagesMetrics<P::SourceChain, P::TargetChain>,
		source_to_target_headers_relay: Option<OnDemandHeadersRelay<P::SourceChain>>,
	) -> Self {
		SubstrateMessagesTarget {
			client,
			lane,
			lane_id,
			metric_values,
			source_to_target_headers_relay,
		}
	}
}

impl<P: SubstrateMessageLane> Clone for SubstrateMessagesTarget<P> {
	fn clone(&self) -> Self {
		Self {
			client: self.client.clone(),
			lane: self.lane.clone(),
			lane_id: self.lane_id,
			metric_values: self.metric_values.clone(),
			source_to_target_headers_relay: self.source_to_target_headers_relay.clone(),
		}
	}
}

#[async_trait]
impl<P: SubstrateMessageLane> RelayClient for SubstrateMessagesTarget<P> {
	type Error = SubstrateError;

	async fn reconnect(&mut self) -> Result<(), SubstrateError> {
		self.client.reconnect().await
	}
}

#[async_trait]
impl<P> TargetClient<P::MessageLane> for SubstrateMessagesTarget<P>
where
	P: SubstrateMessageLane,
	P::SourceChain: Chain<
		Hash = <P::MessageLane as MessageLane>::SourceHeaderHash,
		BlockNumber = <P::MessageLane as MessageLane>::SourceHeaderNumber,
		Balance = <P::MessageLane as MessageLane>::SourceChainBalance,
	>,
	BalanceOf<P::SourceChain>: TryFrom<BalanceOf<P::TargetChain>> + Bounded,
	P::TargetChain: Chain<
		Hash = <P::MessageLane as MessageLane>::TargetHeaderHash,
		BlockNumber = <P::MessageLane as MessageLane>::TargetHeaderNumber,
	>,
	IndexOf<P::TargetChain>: DeserializeOwned,
	HashOf<P::TargetChain>: Copy,
	BlockNumberOf<P::TargetChain>: Copy,
	HeaderOf<P::TargetChain>: DeserializeOwned,
	BlockNumberOf<P::TargetChain>: BlockNumberBase,
	P::MessageLane: MessageLane<
		MessagesProof = SubstrateMessagesProof<P::SourceChain>,
		MessagesReceivingProof = SubstrateMessagesReceivingProof<P::TargetChain>,
	>,
	<P::MessageLane as MessageLane>::SourceHeaderNumber: Decode,
	<P::MessageLane as MessageLane>::SourceHeaderHash: Decode,
{
	async fn state(&self) -> Result<TargetClientState<P::MessageLane>, SubstrateError> {
		// we can't continue to deliver messages if target node is out of sync, because
		// it may have already received (some of) messages that we're going to deliver
		self.client.ensure_synced().await?;

		read_client_state::<
			_,
			<P::MessageLane as MessageLane>::SourceHeaderHash,
			<P::MessageLane as MessageLane>::SourceHeaderNumber,
		>(&self.client, P::BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET)
		.await
	}

	async fn latest_received_nonce(
		&self,
		id: TargetHeaderIdOf<P::MessageLane>,
	) -> Result<(TargetHeaderIdOf<P::MessageLane>, MessageNonce), SubstrateError> {
		let encoded_response = self
			.client
			.state_call(
				P::INBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD.into(),
				Bytes(self.lane_id.encode()),
				Some(id.1),
			)
			.await?;
		let latest_received_nonce: MessageNonce = Decode::decode(&mut &encoded_response.0[..])
			.map_err(SubstrateError::ResponseParseFailed)?;
		Ok((id, latest_received_nonce))
	}

	async fn latest_confirmed_received_nonce(
		&self,
		id: TargetHeaderIdOf<P::MessageLane>,
	) -> Result<(TargetHeaderIdOf<P::MessageLane>, MessageNonce), SubstrateError> {
		let encoded_response = self
			.client
			.state_call(
				P::INBOUND_LANE_LATEST_CONFIRMED_NONCE_METHOD.into(),
				Bytes(self.lane_id.encode()),
				Some(id.1),
			)
			.await?;
		let latest_received_nonce: MessageNonce = Decode::decode(&mut &encoded_response.0[..])
			.map_err(SubstrateError::ResponseParseFailed)?;
		Ok((id, latest_received_nonce))
	}

	async fn unrewarded_relayers_state(
		&self,
		id: TargetHeaderIdOf<P::MessageLane>,
	) -> Result<(TargetHeaderIdOf<P::MessageLane>, UnrewardedRelayersState), SubstrateError> {
		let encoded_response = self
			.client
			.state_call(
				P::INBOUND_LANE_UNREWARDED_RELAYERS_STATE.into(),
				Bytes(self.lane_id.encode()),
				Some(id.1),
			)
			.await?;
		let unrewarded_relayers_state: UnrewardedRelayersState =
			Decode::decode(&mut &encoded_response.0[..])
				.map_err(SubstrateError::ResponseParseFailed)?;
		Ok((id, unrewarded_relayers_state))
	}

	async fn prove_messages_receiving(
		&self,
		id: TargetHeaderIdOf<P::MessageLane>,
	) -> Result<
		(TargetHeaderIdOf<P::MessageLane>, <P::MessageLane as MessageLane>::MessagesReceivingProof),
		SubstrateError,
	> {
		let (id, relayers_state) = self.unrewarded_relayers_state(id).await?;
		let inbound_data_key = pallet_bridge_messages::storage_keys::inbound_lane_data_key(
			P::MESSAGE_PALLET_NAME_AT_TARGET,
			&self.lane_id,
		);
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
		generated_at_header: SourceHeaderIdOf<P::MessageLane>,
		nonces: RangeInclusive<MessageNonce>,
		proof: <P::MessageLane as MessageLane>::MessagesProof,
	) -> Result<RangeInclusive<MessageNonce>, SubstrateError> {
		let lane = self.lane.clone();
		let nonces_clone = nonces.clone();
		self.client
			.submit_signed_extrinsic(
				self.lane.target_transactions_author(),
				move |best_block_id, transaction_nonce| {
					lane.make_messages_delivery_transaction(
						best_block_id,
						transaction_nonce,
						generated_at_header,
						nonces_clone,
						proof,
					)
				},
			)
			.await?;
		Ok(nonces)
	}

	async fn require_source_header_on_target(&self, id: SourceHeaderIdOf<P::MessageLane>) {
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
	) -> Result<<P::MessageLane as MessageLane>::SourceChainBalance, SubstrateError> {
		let conversion_rate =
			self.metric_values.target_to_source_conversion_rate().await.ok_or_else(|| {
				SubstrateError::Custom(format!(
					"Failed to compute conversion rate from {} to {}",
					P::TargetChain::NAME,
					P::SourceChain::NAME,
				))
			})?;

		// Prepare 'dummy' delivery transaction - we only care about its length and dispatch weight.
		let delivery_tx = self.lane.make_messages_delivery_transaction(
			HeaderId(Default::default(), Default::default()),
			Zero::zero(),
			HeaderId(Default::default(), Default::default()),
			nonces.clone(),
			prepare_dummy_messages_proof::<P::SourceChain>(
				nonces.clone(),
				total_dispatch_weight,
				total_size,
			),
		);
		let delivery_tx_fee = self.client.estimate_extrinsic_fee(delivery_tx).await?;
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

			let larger_dispatch_weight = total_dispatch_weight.saturating_add(WEIGHT_DIFFERENCE);
			let larger_delivery_tx_fee = self
				.client
				.estimate_extrinsic_fee(self.lane.make_messages_delivery_transaction(
					HeaderId(Default::default(), Default::default()),
					Zero::zero(),
					HeaderId(Default::default(), Default::default()),
					nonces.clone(),
					prepare_dummy_messages_proof::<P::SourceChain>(
						nonces.clone(),
						larger_dispatch_weight,
						total_size,
					),
				))
				.await?;

			compute_prepaid_messages_refund::<P>(
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
	let smaller_tx_unadjusted_weight_fee = WeightToFeeOf::<C>::calc(&smaller_tx_weight);
	let larger_tx_unadjusted_weight_fee = WeightToFeeOf::<C>::calc(&larger_tx_weight);
	FixedU128::saturating_from_rational(
		adjusted_weight_fee_difference,
		larger_tx_unadjusted_weight_fee.saturating_sub(smaller_tx_unadjusted_weight_fee),
	)
}

/// Compute fee that will be refunded to the relayer because dispatch of `total_prepaid_nonces`
/// messages has been paid at the source chain.
fn compute_prepaid_messages_refund<P: SubstrateMessageLane>(
	total_prepaid_nonces: MessageNonce,
	fee_multiplier: FixedU128,
) -> BalanceOf<P::TargetChain> {
	fee_multiplier.saturating_mul_int(WeightToFeeOf::<P::TargetChain>::calc(
		&P::PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_TARGET_CHAIN.saturating_mul(total_prepaid_nonces),
	))
}

#[cfg(test)]
mod tests {
	use super::*;
	use relay_rococo_client::{Rococo, SigningParams as RococoSigningParams};
	use relay_wococo_client::{SigningParams as WococoSigningParams, Wococo};

	#[derive(Clone)]
	struct TestSubstrateMessageLane;

	impl SubstrateMessageLane for TestSubstrateMessageLane {
		type MessageLane = crate::messages_lane::SubstrateMessageLaneToSubstrate<
			Rococo,
			RococoSigningParams,
			Wococo,
			WococoSigningParams,
		>;

		const OUTBOUND_LANE_MESSAGE_DETAILS_METHOD: &'static str = "";
		const OUTBOUND_LANE_LATEST_GENERATED_NONCE_METHOD: &'static str = "";
		const OUTBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str = "";

		const INBOUND_LANE_LATEST_RECEIVED_NONCE_METHOD: &'static str = "";
		const INBOUND_LANE_LATEST_CONFIRMED_NONCE_METHOD: &'static str = "";
		const INBOUND_LANE_UNREWARDED_RELAYERS_STATE: &'static str = "";

		const BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET: &'static str = "";
		const BEST_FINALIZED_TARGET_HEADER_ID_AT_SOURCE: &'static str = "";

		const MESSAGE_PALLET_NAME_AT_SOURCE: &'static str = "";
		const MESSAGE_PALLET_NAME_AT_TARGET: &'static str = "";

		const PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_TARGET_CHAIN: Weight = 100_000;

		type SourceChain = Rococo;
		type TargetChain = Wococo;

		fn source_transactions_author(&self) -> bp_rococo::AccountId {
			unreachable!()
		}

		fn make_messages_receiving_proof_transaction(
			&self,
			_best_block_id: SourceHeaderIdOf<Self::MessageLane>,
			_transaction_nonce: IndexOf<Rococo>,
			_generated_at_block: TargetHeaderIdOf<Self::MessageLane>,
			_proof: <Self::MessageLane as MessageLane>::MessagesReceivingProof,
		) -> Bytes {
			unreachable!()
		}

		fn target_transactions_author(&self) -> bp_wococo::AccountId {
			unreachable!()
		}

		fn make_messages_delivery_transaction(
			&self,
			_best_block_id: TargetHeaderIdOf<Self::MessageLane>,
			_transaction_nonce: IndexOf<Wococo>,
			_generated_at_header: SourceHeaderIdOf<Self::MessageLane>,
			_nonces: RangeInclusive<MessageNonce>,
			_proof: <Self::MessageLane as MessageLane>::MessagesProof,
		) -> Bytes {
			unreachable!()
		}
	}

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
			multiplier.saturating_mul_int(WeightToFeeOf::<Rococo>::calc(&smaller_weight));

		let larger_weight = smaller_weight + 200_000;
		let larger_adjusted_weight_fee =
			multiplier.saturating_mul_int(WeightToFeeOf::<Rococo>::calc(&larger_weight));

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
			compute_prepaid_messages_refund::<TestSubstrateMessageLane>(
				10,
				FixedU128::saturating_from_rational(110, 100),
			) > (10 * TestSubstrateMessageLane::PAY_INBOUND_DISPATCH_FEE_WEIGHT_AT_TARGET_CHAIN)
				.into()
		);
	}
}
