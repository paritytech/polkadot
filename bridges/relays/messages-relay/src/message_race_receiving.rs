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

//! Message receiving race delivers proof-of-messages-delivery from lane.target to lane.source.

use crate::message_lane::{MessageLane, SourceHeaderIdOf, TargetHeaderIdOf};
use crate::message_lane_loop::{
	SourceClient as MessageLaneSourceClient, SourceClientState, TargetClient as MessageLaneTargetClient,
	TargetClientState,
};
use crate::message_race_loop::{
	MessageRace, NoncesRange, SourceClient, SourceClientNonces, TargetClient, TargetClientNonces,
};
use crate::message_race_strategy::BasicStrategy;
use crate::metrics::MessageLaneLoopMetrics;

use async_trait::async_trait;
use bp_message_lane::MessageNonce;
use futures::stream::FusedStream;
use relay_utils::FailedClient;
use std::{marker::PhantomData, ops::RangeInclusive, time::Duration};

/// Message receiving confirmations delivery strategy.
type ReceivingConfirmationsBasicStrategy<P> = BasicStrategy<
	<P as MessageLane>::TargetHeaderNumber,
	<P as MessageLane>::TargetHeaderHash,
	<P as MessageLane>::SourceHeaderNumber,
	<P as MessageLane>::SourceHeaderHash,
	RangeInclusive<MessageNonce>,
	<P as MessageLane>::MessagesReceivingProof,
>;

/// Run receiving confirmations race.
pub async fn run<P: MessageLane>(
	source_client: impl MessageLaneSourceClient<P>,
	source_state_updates: impl FusedStream<Item = SourceClientState<P>>,
	target_client: impl MessageLaneTargetClient<P>,
	target_state_updates: impl FusedStream<Item = TargetClientState<P>>,
	stall_timeout: Duration,
	metrics_msg: Option<MessageLaneLoopMetrics>,
) -> Result<(), FailedClient> {
	crate::message_race_loop::run(
		ReceivingConfirmationsRaceSource {
			client: target_client,
			metrics_msg: metrics_msg.clone(),
			_phantom: Default::default(),
		},
		target_state_updates,
		ReceivingConfirmationsRaceTarget {
			client: source_client,
			metrics_msg,
			_phantom: Default::default(),
		},
		source_state_updates,
		stall_timeout,
		ReceivingConfirmationsBasicStrategy::<P>::new(),
	)
	.await
}

/// Messages receiving confirmations race.
struct ReceivingConfirmationsRace<P>(std::marker::PhantomData<P>);

impl<P: MessageLane> MessageRace for ReceivingConfirmationsRace<P> {
	type SourceHeaderId = TargetHeaderIdOf<P>;
	type TargetHeaderId = SourceHeaderIdOf<P>;

	type MessageNonce = MessageNonce;
	type Proof = P::MessagesReceivingProof;

	fn source_name() -> String {
		format!("{}::ReceivingConfirmationsDelivery", P::TARGET_NAME)
	}

	fn target_name() -> String {
		format!("{}::ReceivingConfirmationsDelivery", P::SOURCE_NAME)
	}
}

/// Message receiving confirmations race source, which is a target of the lane.
struct ReceivingConfirmationsRaceSource<P: MessageLane, C> {
	client: C,
	metrics_msg: Option<MessageLaneLoopMetrics>,
	_phantom: PhantomData<P>,
}

#[async_trait]
impl<P, C> SourceClient<ReceivingConfirmationsRace<P>> for ReceivingConfirmationsRaceSource<P, C>
where
	P: MessageLane,
	C: MessageLaneTargetClient<P>,
{
	type Error = C::Error;
	type NoncesRange = RangeInclusive<MessageNonce>;
	type ProofParameters = ();

	async fn nonces(
		&self,
		at_block: TargetHeaderIdOf<P>,
		prev_latest_nonce: MessageNonce,
	) -> Result<(TargetHeaderIdOf<P>, SourceClientNonces<Self::NoncesRange>), Self::Error> {
		let (at_block, latest_received_nonce) = self.client.latest_received_nonce(at_block).await?;
		if let Some(metrics_msg) = self.metrics_msg.as_ref() {
			metrics_msg.update_target_latest_received_nonce::<P>(latest_received_nonce);
		}
		Ok((
			at_block,
			SourceClientNonces {
				new_nonces: prev_latest_nonce + 1..=latest_received_nonce,
				confirmed_nonce: None,
			},
		))
	}

	#[allow(clippy::unit_arg)]
	async fn generate_proof(
		&self,
		at_block: TargetHeaderIdOf<P>,
		nonces: RangeInclusive<MessageNonce>,
		_proof_parameters: Self::ProofParameters,
	) -> Result<
		(
			TargetHeaderIdOf<P>,
			RangeInclusive<MessageNonce>,
			P::MessagesReceivingProof,
		),
		Self::Error,
	> {
		self.client
			.prove_messages_receiving(at_block)
			.await
			.map(|(at_block, proof)| (at_block, nonces, proof))
	}
}

/// Message receiving confirmations race target, which is a source of the lane.
struct ReceivingConfirmationsRaceTarget<P: MessageLane, C> {
	client: C,
	metrics_msg: Option<MessageLaneLoopMetrics>,
	_phantom: PhantomData<P>,
}

#[async_trait]
impl<P, C> TargetClient<ReceivingConfirmationsRace<P>> for ReceivingConfirmationsRaceTarget<P, C>
where
	P: MessageLane,
	C: MessageLaneSourceClient<P>,
{
	type Error = C::Error;
	type TargetNoncesData = ();

	async fn nonces(
		&self,
		at_block: SourceHeaderIdOf<P>,
		update_metrics: bool,
	) -> Result<(SourceHeaderIdOf<P>, TargetClientNonces<()>), Self::Error> {
		let (at_block, latest_confirmed_nonce) = self.client.latest_confirmed_received_nonce(at_block).await?;
		if update_metrics {
			if let Some(metrics_msg) = self.metrics_msg.as_ref() {
				metrics_msg.update_source_latest_confirmed_nonce::<P>(latest_confirmed_nonce);
			}
		}
		Ok((
			at_block,
			TargetClientNonces {
				latest_nonce: latest_confirmed_nonce,
				nonces_data: (),
			},
		))
	}

	async fn submit_proof(
		&self,
		generated_at_block: TargetHeaderIdOf<P>,
		nonces: RangeInclusive<MessageNonce>,
		proof: P::MessagesReceivingProof,
	) -> Result<RangeInclusive<MessageNonce>, Self::Error> {
		self.client
			.submit_messages_receiving_proof(generated_at_block, proof)
			.await?;
		Ok(nonces)
	}
}

impl NoncesRange for RangeInclusive<MessageNonce> {
	fn begin(&self) -> MessageNonce {
		*RangeInclusive::<MessageNonce>::start(self)
	}

	fn end(&self) -> MessageNonce {
		*RangeInclusive::<MessageNonce>::end(self)
	}

	fn greater_than(self, nonce: MessageNonce) -> Option<Self> {
		let next_nonce = nonce + 1;
		let end = *self.end();
		if next_nonce > end {
			None
		} else {
			Some(std::cmp::max(self.begin(), next_nonce)..=end)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn range_inclusive_works_as_nonces_range() {
		let range = 20..=30;

		assert_eq!(NoncesRange::begin(&range), 20);
		assert_eq!(NoncesRange::end(&range), 30);
		assert_eq!(range.clone().greater_than(10), Some(20..=30));
		assert_eq!(range.clone().greater_than(19), Some(20..=30));
		assert_eq!(range.clone().greater_than(20), Some(21..=30));
		assert_eq!(range.clone().greater_than(25), Some(26..=30));
		assert_eq!(range.clone().greater_than(29), Some(30..=30));
		assert_eq!(range.greater_than(30), None);
	}
}
