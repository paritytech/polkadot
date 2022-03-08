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

//! enforcement strategy

use num_traits::Zero;

use bp_messages::{MessageNonce, Weight};
use bp_runtime::messages::DispatchFeePayment;

use crate::{
	message_lane::MessageLane,
	message_lane_loop::{
		MessageDetails, SourceClient as MessageLaneSourceClient,
		TargetClient as MessageLaneTargetClient,
	},
	message_race_loop::NoncesRange,
	relay_strategy::{RelayMessagesBatchReference, RelayReference, RelayStrategy},
};

/// Do hard check and run soft check strategy
#[derive(Clone)]
pub struct EnforcementStrategy<Strategy: RelayStrategy> {
	strategy: Strategy,
}

impl<Strategy: RelayStrategy> EnforcementStrategy<Strategy> {
	pub fn new(strategy: Strategy) -> Self {
		Self { strategy }
	}
}

impl<Strategy: RelayStrategy> EnforcementStrategy<Strategy> {
	pub async fn decide<
		P: MessageLane,
		SourceClient: MessageLaneSourceClient<P>,
		TargetClient: MessageLaneTargetClient<P>,
	>(
		&mut self,
		reference: RelayMessagesBatchReference<P, SourceClient, TargetClient>,
	) -> Option<MessageNonce> {
		let mut hard_selected_count = 0;
		let mut soft_selected_count = 0;

		let mut selected_weight: Weight = 0;
		let mut selected_count: MessageNonce = 0;

		let hard_selected_begin_nonce =
			reference.nonces_queue[reference.nonces_queue_range.start].1.begin();

		// relay reference
		let mut relay_reference = RelayReference {
			lane_source_client: reference.lane_source_client.clone(),
			lane_target_client: reference.lane_target_client.clone(),

			selected_reward: P::SourceChainBalance::zero(),
			selected_cost: P::SourceChainBalance::zero(),
			selected_size: 0,

			total_reward: P::SourceChainBalance::zero(),
			total_confirmations_cost: P::SourceChainBalance::zero(),
			total_cost: P::SourceChainBalance::zero(),

			hard_selected_begin_nonce,
			selected_prepaid_nonces: 0,
			selected_unpaid_weight: 0,

			index: 0,
			nonce: 0,
			details: MessageDetails {
				dispatch_weight: 0,
				size: 0,
				reward: P::SourceChainBalance::zero(),
				dispatch_fee_payment: DispatchFeePayment::AtSourceChain,
			},
		};

		let all_ready_nonces = reference
			.nonces_queue
			.range(reference.nonces_queue_range.clone())
			.flat_map(|(_, ready_nonces)| ready_nonces.iter())
			.enumerate();
		for (index, (nonce, details)) in all_ready_nonces {
			relay_reference.index = index;
			relay_reference.nonce = *nonce;
			relay_reference.details = *details;

			// Since we (hopefully) have some reserves in `max_messages_weight_in_single_batch`
			// and `max_messages_size_in_single_batch`, we may still try to submit transaction
			// with single message if message overflows these limits. The worst case would be if
			// transaction will be rejected by the target runtime, but at least we have tried.

			// limit messages in the batch by weight
			let new_selected_weight = match selected_weight.checked_add(details.dispatch_weight) {
				Some(new_selected_weight)
					if new_selected_weight <= reference.max_messages_weight_in_single_batch =>
					new_selected_weight,
				new_selected_weight if selected_count == 0 => {
					log::warn!(
						target: "bridge",
						"Going to submit message delivery transaction with declared dispatch \
						weight {:?} that overflows maximal configured weight {}",
						new_selected_weight,
						reference.max_messages_weight_in_single_batch,
					);
					new_selected_weight.unwrap_or(Weight::MAX)
				},
				_ => break,
			};

			// limit messages in the batch by size
			let new_selected_size = match relay_reference.selected_size.checked_add(details.size) {
				Some(new_selected_size)
					if new_selected_size <= reference.max_messages_size_in_single_batch =>
					new_selected_size,
				new_selected_size if selected_count == 0 => {
					log::warn!(
						target: "bridge",
						"Going to submit message delivery transaction with message \
						size {:?} that overflows maximal configured size {}",
						new_selected_size,
						reference.max_messages_size_in_single_batch,
					);
					new_selected_size.unwrap_or(u32::MAX)
				},
				_ => break,
			};

			// limit number of messages in the batch
			let new_selected_count = selected_count + 1;
			if new_selected_count > reference.max_messages_in_this_batch {
				break
			}
			relay_reference.selected_size = new_selected_size;

			// If dispatch fee has been paid at the source chain, it means that it is **relayer**
			// who's paying for dispatch at the target chain AND reward must cover this dispatch
			// fee.
			//
			// If dispatch fee is paid at the target chain, it means that it'll be withdrawn from
			// the dispatch origin account AND reward is not covering this fee.
			//
			// So in the latter case we're not adding the dispatch weight to the delivery
			// transaction weight.
			let mut new_selected_prepaid_nonces = relay_reference.selected_prepaid_nonces;
			let new_selected_unpaid_weight = match details.dispatch_fee_payment {
				DispatchFeePayment::AtSourceChain => {
					new_selected_prepaid_nonces += 1;
					relay_reference.selected_unpaid_weight.saturating_add(details.dispatch_weight)
				},
				DispatchFeePayment::AtTargetChain => relay_reference.selected_unpaid_weight,
			};
			relay_reference.selected_prepaid_nonces = new_selected_prepaid_nonces;
			relay_reference.selected_unpaid_weight = new_selected_unpaid_weight;

			// now the message has passed all 'strong' checks, and we CAN deliver it. But do we WANT
			// to deliver it? It depends on the relayer strategy.
			if self.strategy.decide(&mut relay_reference).await {
				soft_selected_count = index + 1;
			}

			hard_selected_count = index + 1;
			selected_weight = new_selected_weight;
			selected_count = new_selected_count;
		}

		if hard_selected_count != soft_selected_count {
			let hard_selected_end_nonce =
				hard_selected_begin_nonce + hard_selected_count as MessageNonce - 1;
			let soft_selected_begin_nonce = hard_selected_begin_nonce;
			let soft_selected_end_nonce =
				soft_selected_begin_nonce + soft_selected_count as MessageNonce - 1;
			log::warn!(
				target: "bridge",
				"Relayer may deliver nonces [{:?}; {:?}], but because of its strategy it has selected \
				nonces [{:?}; {:?}].",
				hard_selected_begin_nonce,
				hard_selected_end_nonce,
				soft_selected_begin_nonce,
				soft_selected_end_nonce,
			);

			hard_selected_count = soft_selected_count;
		}

		if hard_selected_count != 0 {
			if relay_reference.selected_reward != P::SourceChainBalance::zero() &&
				relay_reference.selected_cost != P::SourceChainBalance::zero()
			{
				log::trace!(
					target: "bridge",
					"Expected reward from delivering nonces [{:?}; {:?}] is: {:?} - {:?} = {:?}",
					hard_selected_begin_nonce,
					hard_selected_begin_nonce + hard_selected_count as MessageNonce - 1,
					&relay_reference.selected_reward,
					&relay_reference.selected_cost,
					relay_reference.selected_reward - relay_reference.selected_cost,
				);
			}

			Some(hard_selected_begin_nonce + hard_selected_count as MessageNonce - 1)
		} else {
			None
		}
	}
}
