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

//! Rational relay strategy

use async_trait::async_trait;
use num_traits::SaturatingAdd;

use bp_messages::MessageNonce;

use crate::{
	message_lane::MessageLane,
	message_lane_loop::{
		SourceClient as MessageLaneSourceClient, TargetClient as MessageLaneTargetClient,
	},
	relay_strategy::{RelayReference, RelayStrategy},
};

/// The relayer will deliver all messages and confirmations as long as he's not losing any
/// funds.
#[derive(Clone)]
pub struct RationalStrategy;

#[async_trait]
impl RelayStrategy for RationalStrategy {
	async fn decide<
		P: MessageLane,
		SourceClient: MessageLaneSourceClient<P>,
		TargetClient: MessageLaneTargetClient<P>,
	>(
		&mut self,
		reference: &mut RelayReference<P, SourceClient, TargetClient>,
	) -> bool {
		// technically, multiple confirmations will be delivered in a single transaction,
		// meaning less loses for relayer. But here we don't know the final relayer yet, so
		// we're adding a separate transaction for every message. Normally, this cost is covered
		// by the message sender. Probably reconsider this?
		let confirmation_transaction_cost =
			reference.lane_source_client.estimate_confirmation_transaction().await;

		let delivery_transaction_cost = match reference
			.lane_target_client
			.estimate_delivery_transaction_in_source_tokens(
				reference.hard_selected_begin_nonce..=
					(reference.hard_selected_begin_nonce + reference.index as MessageNonce),
				reference.selected_prepaid_nonces,
				reference.selected_unpaid_weight,
				reference.selected_size as u32,
			)
			.await
		{
			Ok(v) => v,
			Err(err) => {
				log::debug!(
					target: "bridge",
					"Failed to estimate delivery transaction cost: {:?}. No nonces selected for delivery",
					err,
				);
				return false
			},
		};

		// if it is the first message that makes reward less than cost, let's log it
		// if this message makes batch profitable again, let's log it
		let is_total_reward_less_than_cost = reference.total_reward < reference.total_cost;
		let prev_total_cost = reference.total_cost;
		let prev_total_reward = reference.total_reward;
		reference.total_confirmations_cost = reference
			.total_confirmations_cost
			.saturating_add(&confirmation_transaction_cost);
		reference.total_reward = reference.total_reward.saturating_add(&reference.details.reward);
		reference.total_cost =
			reference.total_confirmations_cost.saturating_add(&delivery_transaction_cost);
		if !is_total_reward_less_than_cost && reference.total_reward < reference.total_cost {
			log::debug!(
				target: "bridge",
				"Message with nonce {} (reward = {:?}) changes total cost {:?}->{:?} and makes it larger than \
				total reward {:?}->{:?}",
				reference.nonce,
				reference.details.reward,
				prev_total_cost,
				reference.total_cost,
				prev_total_reward,
				reference.total_reward,
			);
		} else if is_total_reward_less_than_cost && reference.total_reward >= reference.total_cost {
			log::debug!(
				target: "bridge",
				"Message with nonce {} (reward = {:?}) changes total cost {:?}->{:?} and makes it less than or \
				equal to the total reward {:?}->{:?} (again)",
				reference.nonce,
				reference.details.reward,
				prev_total_cost,
				reference.total_cost,
				prev_total_reward,
				reference.total_reward,
			);
		}

		// Rational relayer never want to lose his funds
		if reference.total_reward >= reference.total_cost {
			reference.selected_reward = reference.total_reward;
			reference.selected_cost = reference.total_cost;
			return true
		}

		false
	}
}
