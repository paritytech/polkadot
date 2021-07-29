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

//! Message delivery race delivers proof-of-messages from lane.source to lane.target.

use crate::message_lane::{MessageLane, SourceHeaderIdOf, TargetHeaderIdOf};
use crate::message_lane_loop::{
	MessageDeliveryParams, MessageDetailsMap, MessageProofParameters, RelayerMode,
	SourceClient as MessageLaneSourceClient, SourceClientState, TargetClient as MessageLaneTargetClient,
	TargetClientState,
};
use crate::message_race_loop::{
	MessageRace, NoncesRange, RaceState, RaceStrategy, SourceClient, SourceClientNonces, TargetClient,
	TargetClientNonces,
};
use crate::message_race_strategy::{BasicStrategy, SourceRangesQueue};
use crate::metrics::MessageLaneLoopMetrics;

use async_trait::async_trait;
use bp_messages::{MessageNonce, UnrewardedRelayersState, Weight};
use bp_runtime::messages::DispatchFeePayment;
use futures::stream::FusedStream;
use num_traits::{SaturatingAdd, Zero};
use relay_utils::FailedClient;
use std::{
	collections::VecDeque,
	marker::PhantomData,
	ops::{Range, RangeInclusive},
	time::Duration,
};

/// Run message delivery race.
pub async fn run<P: MessageLane>(
	source_client: impl MessageLaneSourceClient<P>,
	source_state_updates: impl FusedStream<Item = SourceClientState<P>>,
	target_client: impl MessageLaneTargetClient<P>,
	target_state_updates: impl FusedStream<Item = TargetClientState<P>>,
	stall_timeout: Duration,
	metrics_msg: Option<MessageLaneLoopMetrics>,
	params: MessageDeliveryParams,
) -> Result<(), FailedClient> {
	crate::message_race_loop::run(
		MessageDeliveryRaceSource {
			client: source_client.clone(),
			metrics_msg: metrics_msg.clone(),
			_phantom: Default::default(),
		},
		source_state_updates,
		MessageDeliveryRaceTarget {
			client: target_client.clone(),
			metrics_msg,
			_phantom: Default::default(),
		},
		target_state_updates,
		stall_timeout,
		MessageDeliveryStrategy::<P, _, _> {
			lane_source_client: source_client,
			lane_target_client: target_client,
			max_unrewarded_relayer_entries_at_target: params.max_unrewarded_relayer_entries_at_target,
			max_unconfirmed_nonces_at_target: params.max_unconfirmed_nonces_at_target,
			max_messages_in_single_batch: params.max_messages_in_single_batch,
			max_messages_weight_in_single_batch: params.max_messages_weight_in_single_batch,
			max_messages_size_in_single_batch: params.max_messages_size_in_single_batch,
			relayer_mode: params.relayer_mode,
			latest_confirmed_nonces_at_source: VecDeque::new(),
			target_nonces: None,
			strategy: BasicStrategy::new(),
		},
	)
	.await
}

/// Message delivery race.
struct MessageDeliveryRace<P>(std::marker::PhantomData<P>);

impl<P: MessageLane> MessageRace for MessageDeliveryRace<P> {
	type SourceHeaderId = SourceHeaderIdOf<P>;
	type TargetHeaderId = TargetHeaderIdOf<P>;

	type MessageNonce = MessageNonce;
	type Proof = P::MessagesProof;

	fn source_name() -> String {
		format!("{}::MessagesDelivery", P::SOURCE_NAME)
	}

	fn target_name() -> String {
		format!("{}::MessagesDelivery", P::TARGET_NAME)
	}
}

/// Message delivery race source, which is a source of the lane.
struct MessageDeliveryRaceSource<P: MessageLane, C> {
	client: C,
	metrics_msg: Option<MessageLaneLoopMetrics>,
	_phantom: PhantomData<P>,
}

#[async_trait]
impl<P, C> SourceClient<MessageDeliveryRace<P>> for MessageDeliveryRaceSource<P, C>
where
	P: MessageLane,
	C: MessageLaneSourceClient<P>,
{
	type Error = C::Error;
	type NoncesRange = MessageDetailsMap<P::SourceChainBalance>;
	type ProofParameters = MessageProofParameters;

	async fn nonces(
		&self,
		at_block: SourceHeaderIdOf<P>,
		prev_latest_nonce: MessageNonce,
	) -> Result<(SourceHeaderIdOf<P>, SourceClientNonces<Self::NoncesRange>), Self::Error> {
		let (at_block, latest_generated_nonce) = self.client.latest_generated_nonce(at_block).await?;
		let (at_block, latest_confirmed_nonce) = self.client.latest_confirmed_received_nonce(at_block).await?;

		if let Some(metrics_msg) = self.metrics_msg.as_ref() {
			metrics_msg.update_source_latest_generated_nonce::<P>(latest_generated_nonce);
			metrics_msg.update_source_latest_confirmed_nonce::<P>(latest_confirmed_nonce);
		}

		let new_nonces = if latest_generated_nonce > prev_latest_nonce {
			self.client
				.generated_message_details(at_block.clone(), prev_latest_nonce + 1..=latest_generated_nonce)
				.await?
		} else {
			MessageDetailsMap::new()
		};

		Ok((
			at_block,
			SourceClientNonces {
				new_nonces,
				confirmed_nonce: Some(latest_confirmed_nonce),
			},
		))
	}

	async fn generate_proof(
		&self,
		at_block: SourceHeaderIdOf<P>,
		nonces: RangeInclusive<MessageNonce>,
		proof_parameters: Self::ProofParameters,
	) -> Result<(SourceHeaderIdOf<P>, RangeInclusive<MessageNonce>, P::MessagesProof), Self::Error> {
		self.client.prove_messages(at_block, nonces, proof_parameters).await
	}
}

/// Message delivery race target, which is a target of the lane.
struct MessageDeliveryRaceTarget<P: MessageLane, C> {
	client: C,
	metrics_msg: Option<MessageLaneLoopMetrics>,
	_phantom: PhantomData<P>,
}

#[async_trait]
impl<P, C> TargetClient<MessageDeliveryRace<P>> for MessageDeliveryRaceTarget<P, C>
where
	P: MessageLane,
	C: MessageLaneTargetClient<P>,
{
	type Error = C::Error;
	type TargetNoncesData = DeliveryRaceTargetNoncesData;

	async fn require_source_header(&self, id: SourceHeaderIdOf<P>) {
		self.client.require_source_header_on_target(id).await
	}

	async fn nonces(
		&self,
		at_block: TargetHeaderIdOf<P>,
		update_metrics: bool,
	) -> Result<(TargetHeaderIdOf<P>, TargetClientNonces<DeliveryRaceTargetNoncesData>), Self::Error> {
		let (at_block, latest_received_nonce) = self.client.latest_received_nonce(at_block).await?;
		let (at_block, latest_confirmed_nonce) = self.client.latest_confirmed_received_nonce(at_block).await?;
		let (at_block, unrewarded_relayers) = self.client.unrewarded_relayers_state(at_block).await?;

		if update_metrics {
			if let Some(metrics_msg) = self.metrics_msg.as_ref() {
				metrics_msg.update_target_latest_received_nonce::<P>(latest_received_nonce);
				metrics_msg.update_target_latest_confirmed_nonce::<P>(latest_confirmed_nonce);
			}
		}

		Ok((
			at_block,
			TargetClientNonces {
				latest_nonce: latest_received_nonce,
				nonces_data: DeliveryRaceTargetNoncesData {
					confirmed_nonce: latest_confirmed_nonce,
					unrewarded_relayers,
				},
			},
		))
	}

	async fn submit_proof(
		&self,
		generated_at_block: SourceHeaderIdOf<P>,
		nonces: RangeInclusive<MessageNonce>,
		proof: P::MessagesProof,
	) -> Result<RangeInclusive<MessageNonce>, Self::Error> {
		self.client
			.submit_messages_proof(generated_at_block, nonces, proof)
			.await
	}
}

/// Additional nonces data from the target client used by message delivery race.
#[derive(Debug, Clone)]
struct DeliveryRaceTargetNoncesData {
	/// Latest nonce that we know: (1) has been delivered to us (2) has been confirmed
	/// back to the source node (by confirmations race) and (3) relayer has received
	/// reward for (and this has been confirmed by the message delivery race).
	confirmed_nonce: MessageNonce,
	/// State of the unrewarded relayers set at the target node.
	unrewarded_relayers: UnrewardedRelayersState,
}

/// Messages delivery strategy.
struct MessageDeliveryStrategy<P: MessageLane, SC, TC> {
	/// The client that is connected to the message lane source node.
	lane_source_client: SC,
	/// The client that is connected to the message lane target node.
	lane_target_client: TC,
	/// Maximal unrewarded relayer entries at target client.
	max_unrewarded_relayer_entries_at_target: MessageNonce,
	/// Maximal unconfirmed nonces at target client.
	max_unconfirmed_nonces_at_target: MessageNonce,
	/// Maximal number of messages in the single delivery transaction.
	max_messages_in_single_batch: MessageNonce,
	/// Maximal cumulative messages weight in the single delivery transaction.
	max_messages_weight_in_single_batch: Weight,
	/// Maximal messages size in the single delivery transaction.
	max_messages_size_in_single_batch: u32,
	/// Relayer operating mode.
	relayer_mode: RelayerMode,
	/// Latest confirmed nonces at the source client + the header id where we have first met this nonce.
	latest_confirmed_nonces_at_source: VecDeque<(SourceHeaderIdOf<P>, MessageNonce)>,
	/// Target nonces from the source client.
	target_nonces: Option<TargetClientNonces<DeliveryRaceTargetNoncesData>>,
	/// Basic delivery strategy.
	strategy: MessageDeliveryStrategyBase<P>,
}

type MessageDeliveryStrategyBase<P> = BasicStrategy<
	<P as MessageLane>::SourceHeaderNumber,
	<P as MessageLane>::SourceHeaderHash,
	<P as MessageLane>::TargetHeaderNumber,
	<P as MessageLane>::TargetHeaderHash,
	MessageDetailsMap<<P as MessageLane>::SourceChainBalance>,
	<P as MessageLane>::MessagesProof,
>;

impl<P: MessageLane, SC, TC> std::fmt::Debug for MessageDeliveryStrategy<P, SC, TC> {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		fmt.debug_struct("MessageDeliveryStrategy")
			.field(
				"max_unrewarded_relayer_entries_at_target",
				&self.max_unrewarded_relayer_entries_at_target,
			)
			.field(
				"max_unconfirmed_nonces_at_target",
				&self.max_unconfirmed_nonces_at_target,
			)
			.field("max_messages_in_single_batch", &self.max_messages_in_single_batch)
			.field(
				"max_messages_weight_in_single_batch",
				&self.max_messages_weight_in_single_batch,
			)
			.field(
				"max_messages_size_in_single_batch",
				&self.max_messages_size_in_single_batch,
			)
			.field(
				"latest_confirmed_nonces_at_source",
				&self.latest_confirmed_nonces_at_source,
			)
			.field("target_nonces", &self.target_nonces)
			.field("strategy", &self.strategy)
			.finish()
	}
}

impl<P: MessageLane, SC, TC> MessageDeliveryStrategy<P, SC, TC> {
	/// Returns total weight of all undelivered messages.
	fn total_queued_dispatch_weight(&self) -> Weight {
		self.strategy
			.source_queue()
			.iter()
			.flat_map(|(_, range)| range.values().map(|details| details.dispatch_weight))
			.fold(0, |total, weight| total.saturating_add(weight))
	}
}

#[async_trait]
impl<P, SC, TC> RaceStrategy<SourceHeaderIdOf<P>, TargetHeaderIdOf<P>, P::MessagesProof>
	for MessageDeliveryStrategy<P, SC, TC>
where
	P: MessageLane,
	SC: MessageLaneSourceClient<P>,
	TC: MessageLaneTargetClient<P>,
{
	type SourceNoncesRange = MessageDetailsMap<P::SourceChainBalance>;
	type ProofParameters = MessageProofParameters;
	type TargetNoncesData = DeliveryRaceTargetNoncesData;

	fn is_empty(&self) -> bool {
		self.strategy.is_empty()
	}

	fn required_source_header_at_target(&self, current_best: &SourceHeaderIdOf<P>) -> Option<SourceHeaderIdOf<P>> {
		let header_required_for_messages_delivery = self.strategy.required_source_header_at_target(current_best);
		let header_required_for_reward_confirmations_delivery =
			self.latest_confirmed_nonces_at_source.back().map(|(id, _)| id.clone());
		match (
			header_required_for_messages_delivery,
			header_required_for_reward_confirmations_delivery,
		) {
			(Some(id1), Some(id2)) => Some(if id1.0 > id2.0 { id1 } else { id2 }),
			(a, b) => a.or(b),
		}
	}

	fn best_at_source(&self) -> Option<MessageNonce> {
		self.strategy.best_at_source()
	}

	fn best_at_target(&self) -> Option<MessageNonce> {
		self.strategy.best_at_target()
	}

	fn source_nonces_updated(
		&mut self,
		at_block: SourceHeaderIdOf<P>,
		nonces: SourceClientNonces<Self::SourceNoncesRange>,
	) {
		if let Some(confirmed_nonce) = nonces.confirmed_nonce {
			let is_confirmed_nonce_updated = self
				.latest_confirmed_nonces_at_source
				.back()
				.map(|(_, prev_nonce)| *prev_nonce != confirmed_nonce)
				.unwrap_or(true);
			if is_confirmed_nonce_updated {
				self.latest_confirmed_nonces_at_source
					.push_back((at_block.clone(), confirmed_nonce));
			}
		}
		self.strategy.source_nonces_updated(at_block, nonces)
	}

	fn best_target_nonces_updated(
		&mut self,
		nonces: TargetClientNonces<DeliveryRaceTargetNoncesData>,
		race_state: &mut RaceState<SourceHeaderIdOf<P>, TargetHeaderIdOf<P>, P::MessagesProof>,
	) {
		// best target nonces must always be ge than finalized target nonces
		let mut target_nonces = self.target_nonces.take().unwrap_or_else(|| nonces.clone());
		target_nonces.nonces_data = nonces.nonces_data.clone();
		target_nonces.latest_nonce = std::cmp::max(target_nonces.latest_nonce, nonces.latest_nonce);
		self.target_nonces = Some(target_nonces);

		self.strategy.best_target_nonces_updated(
			TargetClientNonces {
				latest_nonce: nonces.latest_nonce,
				nonces_data: (),
			},
			race_state,
		)
	}

	fn finalized_target_nonces_updated(
		&mut self,
		nonces: TargetClientNonces<DeliveryRaceTargetNoncesData>,
		race_state: &mut RaceState<SourceHeaderIdOf<P>, TargetHeaderIdOf<P>, P::MessagesProof>,
	) {
		if let Some(ref best_finalized_source_header_id_at_best_target) =
			race_state.best_finalized_source_header_id_at_best_target
		{
			let oldest_header_number_to_keep = best_finalized_source_header_id_at_best_target.0;
			while self
				.latest_confirmed_nonces_at_source
				.front()
				.map(|(id, _)| id.0 < oldest_header_number_to_keep)
				.unwrap_or(false)
			{
				self.latest_confirmed_nonces_at_source.pop_front();
			}
		}

		if let Some(ref mut target_nonces) = self.target_nonces {
			target_nonces.latest_nonce = std::cmp::max(target_nonces.latest_nonce, nonces.latest_nonce);
		}

		self.strategy.finalized_target_nonces_updated(
			TargetClientNonces {
				latest_nonce: nonces.latest_nonce,
				nonces_data: (),
			},
			race_state,
		)
	}

	async fn select_nonces_to_deliver(
		&mut self,
		race_state: RaceState<SourceHeaderIdOf<P>, TargetHeaderIdOf<P>, P::MessagesProof>,
	) -> Option<(RangeInclusive<MessageNonce>, Self::ProofParameters)> {
		let best_finalized_source_header_id_at_best_target =
			race_state.best_finalized_source_header_id_at_best_target.clone()?;
		let latest_confirmed_nonce_at_source = self
			.latest_confirmed_nonces_at_source
			.iter()
			.take_while(|(id, _)| id.0 <= best_finalized_source_header_id_at_best_target.0)
			.last()
			.map(|(_, nonce)| *nonce)?;
		let target_nonces = self.target_nonces.as_ref()?;

		// There's additional condition in the message delivery race: target would reject messages
		// if there are too much unconfirmed messages at the inbound lane.

		// The receiving race is responsible to deliver confirmations back to the source chain. So if
		// there's a lot of unconfirmed messages, let's wait until it'll be able to do its job.
		let latest_received_nonce_at_target = target_nonces.latest_nonce;
		let confirmations_missing = latest_received_nonce_at_target.checked_sub(latest_confirmed_nonce_at_source);
		match confirmations_missing {
			Some(confirmations_missing) if confirmations_missing >= self.max_unconfirmed_nonces_at_target => {
				log::debug!(
					target: "bridge",
					"Cannot deliver any more messages from {} to {}. Too many unconfirmed nonces \
					at target: target.latest_received={:?}, source.latest_confirmed={:?}, max={:?}",
					MessageDeliveryRace::<P>::source_name(),
					MessageDeliveryRace::<P>::target_name(),
					latest_received_nonce_at_target,
					latest_confirmed_nonce_at_source,
					self.max_unconfirmed_nonces_at_target,
				);

				return None;
			}
			_ => (),
		}

		// Ok - we may have new nonces to deliver. But target may still reject new messages, because we haven't
		// notified it that (some) messages have been confirmed. So we may want to include updated
		// `source.latest_confirmed` in the proof.
		//
		// Important note: we're including outbound state lane proof whenever there are unconfirmed nonces
		// on the target chain. Other strategy is to include it only if it's absolutely necessary.
		let latest_confirmed_nonce_at_target = target_nonces.nonces_data.confirmed_nonce;
		let outbound_state_proof_required = latest_confirmed_nonce_at_target < latest_confirmed_nonce_at_source;

		// The target node would also reject messages if there are too many entries in the
		// "unrewarded relayers" set. If we are unable to prove new rewards to the target node, then
		// we should wait for confirmations race.
		let unrewarded_relayer_entries_limit_reached =
			target_nonces.nonces_data.unrewarded_relayers.unrewarded_relayer_entries
				>= self.max_unrewarded_relayer_entries_at_target;
		if unrewarded_relayer_entries_limit_reached {
			// so there are already too many unrewarded relayer entries in the set
			//
			// => check if we can prove enough rewards. If not, we should wait for more rewards to be paid
			let number_of_rewards_being_proved =
				latest_confirmed_nonce_at_source.saturating_sub(latest_confirmed_nonce_at_target);
			let enough_rewards_being_proved = number_of_rewards_being_proved
				>= target_nonces.nonces_data.unrewarded_relayers.messages_in_oldest_entry;
			if !enough_rewards_being_proved {
				return None;
			}
		}

		// If we're here, then the confirmations race did its job && sending side now knows that messages
		// have been delivered. Now let's select nonces that we want to deliver.
		//
		// We may deliver at most:
		//
		// max_unconfirmed_nonces_at_target - (latest_received_nonce_at_target - latest_confirmed_nonce_at_target)
		//
		// messages in the batch. But since we're including outbound state proof in the batch, then it
		// may be increased to:
		//
		// max_unconfirmed_nonces_at_target - (latest_received_nonce_at_target - latest_confirmed_nonce_at_source)
		let future_confirmed_nonce_at_target = if outbound_state_proof_required {
			latest_confirmed_nonce_at_source
		} else {
			latest_confirmed_nonce_at_target
		};
		let max_nonces = latest_received_nonce_at_target
			.checked_sub(future_confirmed_nonce_at_target)
			.and_then(|diff| self.max_unconfirmed_nonces_at_target.checked_sub(diff))
			.unwrap_or_default();
		let max_nonces = std::cmp::min(max_nonces, self.max_messages_in_single_batch);
		let max_messages_weight_in_single_batch = self.max_messages_weight_in_single_batch;
		let max_messages_size_in_single_batch = self.max_messages_size_in_single_batch;
		let relayer_mode = self.relayer_mode;
		let lane_source_client = self.lane_source_client.clone();
		let lane_target_client = self.lane_target_client.clone();

		let maximal_source_queue_index = self.strategy.maximal_available_source_queue_index(race_state)?;
		let previous_total_dispatch_weight = self.total_queued_dispatch_weight();
		let source_queue = self.strategy.source_queue();
		let range_end = select_nonces_for_delivery_transaction(
			relayer_mode,
			max_nonces,
			max_messages_weight_in_single_batch,
			max_messages_size_in_single_batch,
			lane_source_client.clone(),
			lane_target_client.clone(),
			source_queue,
			0..maximal_source_queue_index + 1,
		)
		.await?;

		let range_begin = source_queue[0].1.begin();
		let selected_nonces = range_begin..=range_end;
		self.strategy.remove_le_nonces_from_source_queue(range_end);

		let new_total_dispatch_weight = self.total_queued_dispatch_weight();
		let dispatch_weight = previous_total_dispatch_weight - new_total_dispatch_weight;

		Some((
			selected_nonces,
			MessageProofParameters {
				outbound_state_proof_required,
				dispatch_weight,
			},
		))
	}
}

/// From given set of source nonces, that are ready to be delivered, select nonces
/// to fit into single delivery transaction.
///
/// The function returns nonces that are NOT selected for current batch and will be
/// delivered later.
#[allow(clippy::too_many_arguments)]
async fn select_nonces_for_delivery_transaction<P: MessageLane>(
	relayer_mode: RelayerMode,
	max_messages_in_this_batch: MessageNonce,
	max_messages_weight_in_single_batch: Weight,
	max_messages_size_in_single_batch: u32,
	lane_source_client: impl MessageLaneSourceClient<P>,
	lane_target_client: impl MessageLaneTargetClient<P>,
	nonces_queue: &SourceRangesQueue<
		P::SourceHeaderHash,
		P::SourceHeaderNumber,
		MessageDetailsMap<P::SourceChainBalance>,
	>,
	nonces_queue_range: Range<usize>,
) -> Option<MessageNonce> {
	let mut hard_selected_count = 0;
	let mut soft_selected_count = 0;

	let mut selected_weight: Weight = 0;
	let mut selected_unpaid_weight: Weight = 0;
	let mut selected_size: u32 = 0;
	let mut selected_count: MessageNonce = 0;

	let mut total_reward = P::SourceChainBalance::zero();
	let mut total_confirmations_cost = P::SourceChainBalance::zero();
	let mut total_cost = P::SourceChainBalance::zero();

	// technically, multiple confirmations will be delivered in a single transaction,
	// meaning less loses for relayer. But here we don't know the final relayer yet, so
	// we're adding a separate transaction for every message. Normally, this cost is covered
	// by the message sender. Probably reconsider this?
	let confirmation_transaction_cost = if relayer_mode != RelayerMode::Altruistic {
		lane_source_client.estimate_confirmation_transaction().await
	} else {
		Zero::zero()
	};

	let all_ready_nonces = nonces_queue
		.range(nonces_queue_range.clone())
		.flat_map(|(_, ready_nonces)| ready_nonces.iter())
		.enumerate();
	for (index, (nonce, details)) in all_ready_nonces {
		// Since we (hopefully) have some reserves in `max_messages_weight_in_single_batch`
		// and `max_messages_size_in_single_batch`, we may still try to submit transaction
		// with single message if message overflows these limits. The worst case would be if
		// transaction will be rejected by the target runtime, but at least we have tried.

		// limit messages in the batch by weight
		let new_selected_weight = match selected_weight.checked_add(details.dispatch_weight) {
			Some(new_selected_weight) if new_selected_weight <= max_messages_weight_in_single_batch => {
				new_selected_weight
			}
			new_selected_weight if selected_count == 0 => {
				log::warn!(
					target: "bridge",
					"Going to submit message delivery transaction with declared dispatch \
					weight {:?} that overflows maximal configured weight {}",
					new_selected_weight,
					max_messages_weight_in_single_batch,
				);
				new_selected_weight.unwrap_or(Weight::MAX)
			}
			_ => break,
		};

		// limit messages in the batch by size
		let new_selected_size = match selected_size.checked_add(details.size) {
			Some(new_selected_size) if new_selected_size <= max_messages_size_in_single_batch => new_selected_size,
			new_selected_size if selected_count == 0 => {
				log::warn!(
					target: "bridge",
					"Going to submit message delivery transaction with message \
					size {:?} that overflows maximal configured size {}",
					new_selected_size,
					max_messages_size_in_single_batch,
				);
				new_selected_size.unwrap_or(u32::MAX)
			}
			_ => break,
		};

		// limit number of messages in the batch
		let new_selected_count = selected_count + 1;
		if new_selected_count > max_messages_in_this_batch {
			break;
		}

		// If dispatch fee has been paid at the source chain, it means that it is **relayer** who's
		// paying for dispatch at the target chain AND reward must cover this dispatch fee.
		//
		// If dispatch fee is paid at the target chain, it means that it'll be withdrawn from the
		// dispatch origin account AND reward is not covering this fee.
		//
		// So in the latter case we're not adding the dispatch weight to the delivery transaction weight.
		let new_selected_unpaid_weight = match details.dispatch_fee_payment {
			DispatchFeePayment::AtSourceChain => selected_unpaid_weight.saturating_add(details.dispatch_weight),
			DispatchFeePayment::AtTargetChain => selected_unpaid_weight,
		};

		// now the message has passed all 'strong' checks, and we CAN deliver it. But do we WANT
		// to deliver it? It depends on the relayer strategy.
		match relayer_mode {
			RelayerMode::Altruistic => {
				soft_selected_count = index + 1;
			}
			RelayerMode::NoLosses => {
				let delivery_transaction_cost = lane_target_client
					.estimate_delivery_transaction_in_source_tokens(
						0..=(new_selected_count as MessageNonce - 1),
						new_selected_unpaid_weight,
						new_selected_size as u32,
					)
					.await;

				// if it is the first message that makes reward less than cost, let's log it
				// if this message makes batch profitable again, let's log it
				let is_total_reward_less_than_cost = total_reward < total_cost;
				let prev_total_cost = total_cost;
				let prev_total_reward = total_reward;
				total_confirmations_cost = total_confirmations_cost.saturating_add(&confirmation_transaction_cost);
				total_reward = total_reward.saturating_add(&details.reward);
				total_cost = total_confirmations_cost.saturating_add(&delivery_transaction_cost);
				if !is_total_reward_less_than_cost && total_reward < total_cost {
					log::debug!(
						target: "bridge",
						"Message with nonce {} (reward = {:?}) changes total cost {:?}->{:?} and makes it larger than \
						total reward {:?}->{:?}",
						nonce,
						details.reward,
						prev_total_cost,
						total_cost,
						prev_total_reward,
						total_reward,
					);
				} else if is_total_reward_less_than_cost && total_reward >= total_cost {
					log::debug!(
						target: "bridge",
						"Message with nonce {} (reward = {:?}) changes total cost {:?}->{:?} and makes it less than or \
						equal to the total reward {:?}->{:?} (again)",
						nonce,
						details.reward,
						prev_total_cost,
						total_cost,
						prev_total_reward,
						total_reward,
					);
				}

				// NoLosses relayer never want to lose his funds
				if total_reward >= total_cost {
					soft_selected_count = index + 1;
				}
			}
		}

		hard_selected_count = index + 1;
		selected_weight = new_selected_weight;
		selected_unpaid_weight = new_selected_unpaid_weight;
		selected_size = new_selected_size;
		selected_count = new_selected_count;
	}

	let hard_selected_begin_nonce = nonces_queue[nonces_queue_range.start].1.begin();
	if hard_selected_count != soft_selected_count {
		let hard_selected_end_nonce = hard_selected_begin_nonce + hard_selected_count as MessageNonce - 1;
		let soft_selected_begin_nonce = hard_selected_begin_nonce;
		let soft_selected_end_nonce = soft_selected_begin_nonce + soft_selected_count as MessageNonce - 1;
		log::warn!(
			target: "bridge",
			"Relayer may deliver nonces [{:?}; {:?}], but because of its strategy ({:?}) it has selected \
			nonces [{:?}; {:?}].",
			hard_selected_begin_nonce,
			hard_selected_end_nonce,
			relayer_mode,
			soft_selected_begin_nonce,
			soft_selected_end_nonce,
		);

		hard_selected_count = soft_selected_count;
	}

	if hard_selected_count != 0 {
		Some(hard_selected_begin_nonce + hard_selected_count as MessageNonce - 1)
	} else {
		None
	}
}

impl<SourceChainBalance: std::fmt::Debug> NoncesRange for MessageDetailsMap<SourceChainBalance> {
	fn begin(&self) -> MessageNonce {
		self.keys().next().cloned().unwrap_or_default()
	}

	fn end(&self) -> MessageNonce {
		self.keys().next_back().cloned().unwrap_or_default()
	}

	fn greater_than(mut self, nonce: MessageNonce) -> Option<Self> {
		let gte = self.split_off(&(nonce + 1));
		if gte.is_empty() {
			None
		} else {
			Some(gte)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::message_lane_loop::{
		tests::{
			header_id, TestMessageLane, TestMessagesProof, TestSourceChainBalance, TestSourceClient,
			TestSourceHeaderId, TestTargetClient, TestTargetHeaderId, BASE_MESSAGE_DELIVERY_TRANSACTION_COST,
			CONFIRMATION_TRANSACTION_COST,
		},
		MessageDetails,
	};
	use bp_runtime::messages::DispatchFeePayment::*;

	const DEFAULT_DISPATCH_WEIGHT: Weight = 1;
	const DEFAULT_SIZE: u32 = 1;
	const DEFAULT_REWARD: TestSourceChainBalance = CONFIRMATION_TRANSACTION_COST
		+ BASE_MESSAGE_DELIVERY_TRANSACTION_COST
		+ DEFAULT_DISPATCH_WEIGHT
		+ (DEFAULT_SIZE as TestSourceChainBalance);

	type TestRaceState = RaceState<TestSourceHeaderId, TestTargetHeaderId, TestMessagesProof>;
	type TestStrategy = MessageDeliveryStrategy<TestMessageLane, TestSourceClient, TestTargetClient>;

	fn source_nonces(
		new_nonces: RangeInclusive<MessageNonce>,
		confirmed_nonce: MessageNonce,
		reward: TestSourceChainBalance,
		dispatch_fee_payment: DispatchFeePayment,
	) -> SourceClientNonces<MessageDetailsMap<TestSourceChainBalance>> {
		SourceClientNonces {
			new_nonces: new_nonces
				.into_iter()
				.map(|nonce| {
					(
						nonce,
						MessageDetails {
							dispatch_weight: DEFAULT_DISPATCH_WEIGHT,
							size: DEFAULT_SIZE,
							reward,
							dispatch_fee_payment,
						},
					)
				})
				.into_iter()
				.collect(),
			confirmed_nonce: Some(confirmed_nonce),
		}
	}

	fn prepare_strategy() -> (TestRaceState, TestStrategy) {
		let mut race_state = RaceState {
			best_finalized_source_header_id_at_source: Some(header_id(1)),
			best_finalized_source_header_id_at_best_target: Some(header_id(1)),
			best_target_header_id: Some(header_id(1)),
			best_finalized_target_header_id: Some(header_id(1)),
			nonces_to_submit: None,
			nonces_submitted: None,
		};

		let mut race_strategy = TestStrategy {
			relayer_mode: RelayerMode::Altruistic,
			max_unrewarded_relayer_entries_at_target: 4,
			max_unconfirmed_nonces_at_target: 4,
			max_messages_in_single_batch: 4,
			max_messages_weight_in_single_batch: 4,
			max_messages_size_in_single_batch: 4,
			latest_confirmed_nonces_at_source: vec![(header_id(1), 19)].into_iter().collect(),
			lane_source_client: TestSourceClient::default(),
			lane_target_client: TestTargetClient::default(),
			target_nonces: Some(TargetClientNonces {
				latest_nonce: 19,
				nonces_data: DeliveryRaceTargetNoncesData {
					confirmed_nonce: 19,
					unrewarded_relayers: UnrewardedRelayersState {
						unrewarded_relayer_entries: 0,
						messages_in_oldest_entry: 0,
						total_messages: 0,
					},
				},
			}),
			strategy: BasicStrategy::new(),
		};

		race_strategy
			.strategy
			.source_nonces_updated(header_id(1), source_nonces(20..=23, 19, DEFAULT_REWARD, AtSourceChain));

		let target_nonces = TargetClientNonces {
			latest_nonce: 19,
			nonces_data: (),
		};
		race_strategy
			.strategy
			.best_target_nonces_updated(target_nonces.clone(), &mut race_state);
		race_strategy
			.strategy
			.finalized_target_nonces_updated(target_nonces, &mut race_state);

		(race_state, race_strategy)
	}

	fn proof_parameters(state_required: bool, weight: Weight) -> MessageProofParameters {
		MessageProofParameters {
			outbound_state_proof_required: state_required,
			dispatch_weight: weight,
		}
	}

	#[test]
	fn weights_map_works_as_nonces_range() {
		fn build_map(range: RangeInclusive<MessageNonce>) -> MessageDetailsMap<TestSourceChainBalance> {
			range
				.map(|idx| {
					(
						idx,
						MessageDetails {
							dispatch_weight: idx,
							size: idx as _,
							reward: idx as _,
							dispatch_fee_payment: AtSourceChain,
						},
					)
				})
				.collect()
		}

		let map = build_map(20..=30);

		assert_eq!(map.begin(), 20);
		assert_eq!(map.end(), 30);
		assert_eq!(map.clone().greater_than(10), Some(build_map(20..=30)));
		assert_eq!(map.clone().greater_than(19), Some(build_map(20..=30)));
		assert_eq!(map.clone().greater_than(20), Some(build_map(21..=30)));
		assert_eq!(map.clone().greater_than(25), Some(build_map(26..=30)));
		assert_eq!(map.clone().greater_than(29), Some(build_map(30..=30)));
		assert_eq!(map.greater_than(30), None);
	}

	#[async_std::test]
	async fn message_delivery_strategy_selects_messages_to_deliver() {
		let (state, mut strategy) = prepare_strategy();

		// both sides are ready to relay new messages
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=23), proof_parameters(false, 4)))
		);
	}

	#[async_std::test]
	async fn message_delivery_strategy_selects_nothing_if_too_many_confirmations_missing() {
		let (state, mut strategy) = prepare_strategy();

		// if there are already `max_unconfirmed_nonces_at_target` messages on target,
		// we need to wait until confirmations will be delivered by receiving race
		strategy.latest_confirmed_nonces_at_source = vec![(
			header_id(1),
			strategy.target_nonces.as_ref().unwrap().latest_nonce - strategy.max_unconfirmed_nonces_at_target,
		)]
		.into_iter()
		.collect();
		assert_eq!(strategy.select_nonces_to_deliver(state).await, None);
	}

	#[async_std::test]
	async fn message_delivery_strategy_includes_outbound_state_proof_when_new_nonces_are_available() {
		let (state, mut strategy) = prepare_strategy();

		// if there are new confirmed nonces on source, we want to relay this information
		// to target to prune rewards queue
		let prev_confirmed_nonce_at_source = strategy.latest_confirmed_nonces_at_source.back().unwrap().1;
		strategy.target_nonces.as_mut().unwrap().nonces_data.confirmed_nonce = prev_confirmed_nonce_at_source - 1;
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=23), proof_parameters(true, 4)))
		);
	}

	#[async_std::test]
	async fn message_delivery_strategy_selects_nothing_if_there_are_too_many_unrewarded_relayers() {
		let (state, mut strategy) = prepare_strategy();

		// if there are already `max_unrewarded_relayer_entries_at_target` entries at target,
		// we need to wait until rewards will be paid
		{
			let mut unrewarded_relayers = &mut strategy.target_nonces.as_mut().unwrap().nonces_data.unrewarded_relayers;
			unrewarded_relayers.unrewarded_relayer_entries = strategy.max_unrewarded_relayer_entries_at_target;
			unrewarded_relayers.messages_in_oldest_entry = 4;
		}
		assert_eq!(strategy.select_nonces_to_deliver(state).await, None);
	}

	#[async_std::test]
	async fn message_delivery_strategy_selects_nothing_if_proved_rewards_is_not_enough_to_remove_oldest_unrewarded_entry(
	) {
		let (state, mut strategy) = prepare_strategy();

		// if there are already `max_unrewarded_relayer_entries_at_target` entries at target,
		// we need to prove at least `messages_in_oldest_entry` rewards
		let prev_confirmed_nonce_at_source = strategy.latest_confirmed_nonces_at_source.back().unwrap().1;
		{
			let mut nonces_data = &mut strategy.target_nonces.as_mut().unwrap().nonces_data;
			nonces_data.confirmed_nonce = prev_confirmed_nonce_at_source - 1;
			let mut unrewarded_relayers = &mut nonces_data.unrewarded_relayers;
			unrewarded_relayers.unrewarded_relayer_entries = strategy.max_unrewarded_relayer_entries_at_target;
			unrewarded_relayers.messages_in_oldest_entry = 4;
		}
		assert_eq!(strategy.select_nonces_to_deliver(state).await, None);
	}

	#[async_std::test]
	async fn message_delivery_strategy_includes_outbound_state_proof_if_proved_rewards_is_enough() {
		let (state, mut strategy) = prepare_strategy();

		// if there are already `max_unrewarded_relayer_entries_at_target` entries at target,
		// we need to prove at least `messages_in_oldest_entry` rewards
		let prev_confirmed_nonce_at_source = strategy.latest_confirmed_nonces_at_source.back().unwrap().1;
		{
			let mut nonces_data = &mut strategy.target_nonces.as_mut().unwrap().nonces_data;
			nonces_data.confirmed_nonce = prev_confirmed_nonce_at_source - 3;
			let mut unrewarded_relayers = &mut nonces_data.unrewarded_relayers;
			unrewarded_relayers.unrewarded_relayer_entries = strategy.max_unrewarded_relayer_entries_at_target;
			unrewarded_relayers.messages_in_oldest_entry = 3;
		}
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=23), proof_parameters(true, 4)))
		);
	}

	#[async_std::test]
	async fn message_delivery_strategy_limits_batch_by_messages_weight() {
		let (state, mut strategy) = prepare_strategy();

		// not all queued messages may fit in the batch, because batch has max weight
		strategy.max_messages_weight_in_single_batch = 3;
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=22), proof_parameters(false, 3)))
		);
	}

	#[async_std::test]
	async fn message_delivery_strategy_accepts_single_message_even_if_its_weight_overflows_maximal_weight() {
		let (state, mut strategy) = prepare_strategy();

		// first message doesn't fit in the batch, because it has weight (10) that overflows max weight (4)
		strategy.strategy.source_queue_mut()[0]
			.1
			.get_mut(&20)
			.unwrap()
			.dispatch_weight = 10;
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=20), proof_parameters(false, 10)))
		);
	}

	#[async_std::test]
	async fn message_delivery_strategy_limits_batch_by_messages_size() {
		let (state, mut strategy) = prepare_strategy();

		// not all queued messages may fit in the batch, because batch has max weight
		strategy.max_messages_size_in_single_batch = 3;
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=22), proof_parameters(false, 3)))
		);
	}

	#[async_std::test]
	async fn message_delivery_strategy_accepts_single_message_even_if_its_weight_overflows_maximal_size() {
		let (state, mut strategy) = prepare_strategy();

		// first message doesn't fit in the batch, because it has weight (10) that overflows max weight (4)
		strategy.strategy.source_queue_mut()[0].1.get_mut(&20).unwrap().size = 10;
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=20), proof_parameters(false, 1)))
		);
	}

	#[async_std::test]
	async fn message_delivery_strategy_limits_batch_by_messages_count_when_there_is_upper_limit() {
		let (state, mut strategy) = prepare_strategy();

		// not all queued messages may fit in the batch, because batch has max number of messages limit
		strategy.max_messages_in_single_batch = 3;
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=22), proof_parameters(false, 3)))
		);
	}

	#[async_std::test]
	async fn message_delivery_strategy_limits_batch_by_messages_count_when_there_are_unconfirmed_nonces() {
		let (state, mut strategy) = prepare_strategy();

		// 1 delivery confirmation from target to source is still missing, so we may only
		// relay 3 new messages
		let prev_confirmed_nonce_at_source = strategy.latest_confirmed_nonces_at_source.back().unwrap().1;
		strategy.latest_confirmed_nonces_at_source = vec![(header_id(1), prev_confirmed_nonce_at_source - 1)]
			.into_iter()
			.collect();
		strategy.target_nonces.as_mut().unwrap().nonces_data.confirmed_nonce = prev_confirmed_nonce_at_source - 1;
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=22), proof_parameters(false, 3)))
		);
	}

	#[async_std::test]
	async fn message_delivery_strategy_waits_for_confirmed_nonce_header_to_appear_on_target() {
		// 1 delivery confirmation from target to source is still missing, so we may deliver
		// reward confirmation with our message delivery transaction. But the problem is that
		// the reward has been paid at header 2 && this header is still unknown to target node.
		//
		// => so we can't deliver more than 3 messages
		let (mut state, mut strategy) = prepare_strategy();
		let prev_confirmed_nonce_at_source = strategy.latest_confirmed_nonces_at_source.back().unwrap().1;
		strategy.latest_confirmed_nonces_at_source = vec![
			(header_id(1), prev_confirmed_nonce_at_source - 1),
			(header_id(2), prev_confirmed_nonce_at_source),
		]
		.into_iter()
		.collect();
		strategy.target_nonces.as_mut().unwrap().nonces_data.confirmed_nonce = prev_confirmed_nonce_at_source - 1;
		state.best_finalized_source_header_id_at_best_target = Some(header_id(1));
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=22), proof_parameters(false, 3)))
		);

		// the same situation, but the header 2 is known to the target node, so we may deliver reward confirmation
		let (mut state, mut strategy) = prepare_strategy();
		let prev_confirmed_nonce_at_source = strategy.latest_confirmed_nonces_at_source.back().unwrap().1;
		strategy.latest_confirmed_nonces_at_source = vec![
			(header_id(1), prev_confirmed_nonce_at_source - 1),
			(header_id(2), prev_confirmed_nonce_at_source),
		]
		.into_iter()
		.collect();
		strategy.target_nonces.as_mut().unwrap().nonces_data.confirmed_nonce = prev_confirmed_nonce_at_source - 1;
		state.best_finalized_source_header_id_at_source = Some(header_id(2));
		state.best_finalized_source_header_id_at_best_target = Some(header_id(2));
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=23), proof_parameters(true, 4)))
		);
	}

	#[async_std::test]
	async fn source_header_is_required_when_confirmations_are_required() {
		// let's prepare situation when:
		// - all messages [20; 23] have been generated at source block#1;
		let (mut state, mut strategy) = prepare_strategy();
		// - messages [20; 21] have been delivered, but messages [11; 20] can't be delivered because of unrewarded
		//   relayers vector capacity;
		strategy.max_unconfirmed_nonces_at_target = 2;
		assert_eq!(
			strategy.select_nonces_to_deliver(state.clone()).await,
			Some(((20..=21), proof_parameters(false, 2)))
		);
		strategy.finalized_target_nonces_updated(
			TargetClientNonces {
				latest_nonce: 21,
				nonces_data: DeliveryRaceTargetNoncesData {
					confirmed_nonce: 19,
					unrewarded_relayers: UnrewardedRelayersState {
						unrewarded_relayer_entries: 2,
						messages_in_oldest_entry: 2,
						total_messages: 2,
					},
				},
			},
			&mut state,
		);
		assert_eq!(strategy.select_nonces_to_deliver(state).await, None);
		// - messages [1; 10] receiving confirmation has been delivered at source block#2;
		strategy.source_nonces_updated(
			header_id(2),
			SourceClientNonces {
				new_nonces: MessageDetailsMap::new(),
				confirmed_nonce: Some(21),
			},
		);
		// - so now we'll need to relay source block#11 to be able to accept messages [11; 20].
		assert_eq!(
			strategy.required_source_header_at_target(&header_id(1)),
			Some(header_id(2))
		);
	}

	#[async_std::test]
	async fn no_losses_relayer_is_delivering_messages_if_cost_is_equal_to_reward() {
		let (state, mut strategy) = prepare_strategy();
		strategy.relayer_mode = RelayerMode::NoLosses;

		// so now we have:
		// - 20..=23 with reward = cost
		// => strategy shall select all 20..=23
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=23), proof_parameters(false, 4)))
		);
	}

	#[async_std::test]
	async fn no_losses_relayer_is_not_delivering_messages_if_cost_is_larger_than_reward() {
		let (mut state, mut strategy) = prepare_strategy();
		let nonces = source_nonces(
			24..=25,
			19,
			DEFAULT_REWARD - BASE_MESSAGE_DELIVERY_TRANSACTION_COST,
			AtSourceChain,
		);
		strategy.strategy.source_nonces_updated(header_id(2), nonces);
		state.best_finalized_source_header_id_at_best_target = Some(header_id(2));
		strategy.relayer_mode = RelayerMode::NoLosses;

		// so now we have:
		// - 20..=23 with reward = cost
		// - 24..=25 with reward less than cost
		// => strategy shall only select 20..=23
		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=23), proof_parameters(false, 4)))
		);
	}

	#[async_std::test]
	async fn no_losses_relayer_is_delivering_unpaid_messages() {
		async fn test_with_dispatch_fee_payment(
			dispatch_fee_payment: DispatchFeePayment,
		) -> Option<(RangeInclusive<MessageNonce>, MessageProofParameters)> {
			let (mut state, mut strategy) = prepare_strategy();
			let nonces = source_nonces(
				24..=24,
				19,
				DEFAULT_REWARD - DEFAULT_DISPATCH_WEIGHT,
				dispatch_fee_payment,
			);
			strategy.strategy.source_nonces_updated(header_id(2), nonces);
			state.best_finalized_source_header_id_at_best_target = Some(header_id(2));
			strategy.max_unrewarded_relayer_entries_at_target = 100;
			strategy.max_unconfirmed_nonces_at_target = 100;
			strategy.max_messages_in_single_batch = 100;
			strategy.max_messages_weight_in_single_batch = 100;
			strategy.max_messages_size_in_single_batch = 100;
			strategy.relayer_mode = RelayerMode::NoLosses;

			// so now we have:
			// - 20..=23 with reward = cost
			// - 24..=24 with reward less than cost, but we're deducting `DEFAULT_DISPATCH_WEIGHT` from the
			//   cost, so it should be fine;
			// => when MSG#24 fee is paid at the target chain, strategy shall select all 20..=24
			// => when MSG#25 fee is paid at the source chain, strategy shall only select 20..=23
			strategy.select_nonces_to_deliver(state).await
		}

		assert_eq!(
			test_with_dispatch_fee_payment(AtTargetChain).await,
			Some(((20..=24), proof_parameters(false, 5)))
		);
		assert_eq!(
			test_with_dispatch_fee_payment(AtSourceChain).await,
			Some(((20..=23), proof_parameters(false, 4)))
		);
	}

	#[async_std::test]
	async fn relayer_uses_flattened_view_of_the_source_queue_to_select_nonces() {
		// Real scenario that has happened on test deployments:
		// 1) relayer witnessed M1 at block 1 => it has separate entry in the `source_queue`
		// 2) relayer witnessed M2 at block 2 => it has separate entry in the `source_queue`
		// 3) if block 2 is known to the target node, then both M1 and M2 are selected for single delivery,
		//    even though weight(M1+M2) > larger than largest allowed weight
		//
		// This was happening because selector (`select_nonces_for_delivery_transaction`) has been called
		// for every `source_queue` entry separately without preserving any context.
		let (mut state, mut strategy) = prepare_strategy();
		let nonces = source_nonces(24..=25, 19, DEFAULT_REWARD, AtSourceChain);
		strategy.strategy.source_nonces_updated(header_id(2), nonces);
		strategy.max_unrewarded_relayer_entries_at_target = 100;
		strategy.max_unconfirmed_nonces_at_target = 100;
		strategy.max_messages_in_single_batch = 5;
		strategy.max_messages_weight_in_single_batch = 100;
		strategy.max_messages_size_in_single_batch = 100;
		state.best_finalized_source_header_id_at_best_target = Some(header_id(2));

		assert_eq!(
			strategy.select_nonces_to_deliver(state).await,
			Some(((20..=24), proof_parameters(false, 5)))
		);
	}
}
