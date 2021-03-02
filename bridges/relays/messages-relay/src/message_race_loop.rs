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

//! Loop that is serving single race within message lane. This could be
//! message delivery race, receiving confirmations race or processing
//! confirmations race.
//!
//! The idea of the race is simple - we have `nonce`-s on source and target
//! nodes. We're trying to prove that the source node has this nonce (and
//! associated data - like messages, lane state, etc) to the target node by
//! generating and submitting proof.

use crate::message_lane_loop::ClientState;

use async_trait::async_trait;
use bp_message_lane::MessageNonce;
use futures::{
	future::FutureExt,
	stream::{FusedStream, StreamExt},
};
use relay_utils::{process_future_result, retry_backoff, FailedClient, MaybeConnectionError};
use std::{
	fmt::Debug,
	ops::RangeInclusive,
	time::{Duration, Instant},
};

/// One of races within lane.
pub trait MessageRace {
	/// Header id of the race source.
	type SourceHeaderId: Debug + Clone + PartialEq;
	/// Header id of the race source.
	type TargetHeaderId: Debug + Clone + PartialEq;

	/// Message nonce used in the race.
	type MessageNonce: Debug + Clone;
	/// Proof that is generated and delivered in this race.
	type Proof: Debug + Clone;

	/// Name of the race source.
	fn source_name() -> String;
	/// Name of the race target.
	fn target_name() -> String;
}

/// State of race source client.
type SourceClientState<P> = ClientState<<P as MessageRace>::SourceHeaderId, <P as MessageRace>::TargetHeaderId>;

/// State of race target client.
type TargetClientState<P> = ClientState<<P as MessageRace>::TargetHeaderId, <P as MessageRace>::SourceHeaderId>;

/// Inclusive nonces range.
pub trait NoncesRange: Debug + Sized {
	/// Get begin of the range.
	fn begin(&self) -> MessageNonce;
	/// Get end of the range.
	fn end(&self) -> MessageNonce;
	/// Returns new range with current range nonces that are greater than the passed `nonce`.
	/// If there are no such nonces, `None` is returned.
	fn greater_than(self, nonce: MessageNonce) -> Option<Self>;
}

/// Nonces on the race source client.
#[derive(Debug, Clone)]
pub struct SourceClientNonces<NoncesRange> {
	/// New nonces range known to the client. `New` here means all nonces generated after
	/// `prev_latest_nonce` passed to the `SourceClient::nonces` method.
	pub new_nonces: NoncesRange,
	/// Latest nonce that is confirmed to the bridged client. This nonce only makes
	/// sense in some races. In other races it is `None`.
	pub confirmed_nonce: Option<MessageNonce>,
}

/// Nonces on the race target client.
#[derive(Debug, Clone)]
pub struct TargetClientNonces<TargetNoncesData> {
	/// Latest nonce that is known to the target client.
	pub latest_nonce: MessageNonce,
	/// Additional data from target node that may be used by the race.
	pub nonces_data: TargetNoncesData,
}

/// One of message lane clients, which is source client for the race.
#[async_trait]
pub trait SourceClient<P: MessageRace> {
	/// Type of error this clients returns.
	type Error: std::fmt::Debug + MaybeConnectionError;
	/// Type of nonces range returned by the source client.
	type NoncesRange: NoncesRange;
	/// Additional proof parameters required to generate proof.
	type ProofParameters;

	/// Return nonces that are known to the source client.
	async fn nonces(
		&self,
		at_block: P::SourceHeaderId,
		prev_latest_nonce: MessageNonce,
	) -> Result<(P::SourceHeaderId, SourceClientNonces<Self::NoncesRange>), Self::Error>;
	/// Generate proof for delivering to the target client.
	async fn generate_proof(
		&self,
		at_block: P::SourceHeaderId,
		nonces: RangeInclusive<MessageNonce>,
		proof_parameters: Self::ProofParameters,
	) -> Result<(P::SourceHeaderId, RangeInclusive<MessageNonce>, P::Proof), Self::Error>;
}

/// One of message lane clients, which is target client for the race.
#[async_trait]
pub trait TargetClient<P: MessageRace> {
	/// Type of error this clients returns.
	type Error: std::fmt::Debug + MaybeConnectionError;
	/// Type of the additional data from the target client, used by the race.
	type TargetNoncesData: std::fmt::Debug;

	/// Return nonces that are known to the target client.
	async fn nonces(
		&self,
		at_block: P::TargetHeaderId,
		update_metrics: bool,
	) -> Result<(P::TargetHeaderId, TargetClientNonces<Self::TargetNoncesData>), Self::Error>;
	/// Submit proof to the target client.
	async fn submit_proof(
		&self,
		generated_at_block: P::SourceHeaderId,
		nonces: RangeInclusive<MessageNonce>,
		proof: P::Proof,
	) -> Result<RangeInclusive<MessageNonce>, Self::Error>;
}

/// Race strategy.
pub trait RaceStrategy<SourceHeaderId, TargetHeaderId, Proof>: Debug {
	/// Type of nonces range expected from the source client.
	type SourceNoncesRange: NoncesRange;
	/// Additional proof parameters required to generate proof.
	type ProofParameters;
	/// Additional data expected from the target client.
	type TargetNoncesData;

	/// Should return true if nothing has to be synced.
	fn is_empty(&self) -> bool;
	/// Return best nonce at source node.
	///
	/// `Some` is returned only if we are sure that the value is greater or equal
	/// than the result of `best_at_target`.
	fn best_at_source(&self) -> Option<MessageNonce>;
	/// Return best nonce at target node.
	///
	/// May return `None` if value is yet unknown.
	fn best_at_target(&self) -> Option<MessageNonce>;

	/// Called when nonces are updated at source node of the race.
	fn source_nonces_updated(&mut self, at_block: SourceHeaderId, nonces: SourceClientNonces<Self::SourceNoncesRange>);
	/// Called when best nonces are updated at target node of the race.
	fn best_target_nonces_updated(
		&mut self,
		nonces: TargetClientNonces<Self::TargetNoncesData>,
		race_state: &mut RaceState<SourceHeaderId, TargetHeaderId, Proof>,
	);
	/// Called when finalized nonces are updated at target node of the race.
	fn finalized_target_nonces_updated(
		&mut self,
		nonces: TargetClientNonces<Self::TargetNoncesData>,
		race_state: &mut RaceState<SourceHeaderId, TargetHeaderId, Proof>,
	);
	/// Should return `Some(nonces)` if we need to deliver proof of `nonces` (and associated
	/// data) from source to target node.
	/// Additionally, parameters required to generate proof are returned.
	fn select_nonces_to_deliver(
		&mut self,
		race_state: &RaceState<SourceHeaderId, TargetHeaderId, Proof>,
	) -> Option<(RangeInclusive<MessageNonce>, Self::ProofParameters)>;
}

/// State of the race.
#[derive(Debug)]
pub struct RaceState<SourceHeaderId, TargetHeaderId, Proof> {
	/// Best finalized source header id at the source client.
	pub best_finalized_source_header_id_at_source: Option<SourceHeaderId>,
	/// Best finalized source header id at the best block on the target
	/// client (at the `best_finalized_source_header_id_at_best_target`).
	pub best_finalized_source_header_id_at_best_target: Option<SourceHeaderId>,
	/// Best header id at the target client.
	pub best_target_header_id: Option<TargetHeaderId>,
	/// Best finalized header id at the target client.
	pub best_finalized_target_header_id: Option<TargetHeaderId>,
	/// Range of nonces that we have selected to submit.
	pub nonces_to_submit: Option<(SourceHeaderId, RangeInclusive<MessageNonce>, Proof)>,
	/// Range of nonces that is currently submitted.
	pub nonces_submitted: Option<RangeInclusive<MessageNonce>>,
}

/// Run race loop until connection with target or source node is lost.
pub async fn run<P: MessageRace, SC: SourceClient<P>, TC: TargetClient<P>>(
	race_source: SC,
	race_source_updated: impl FusedStream<Item = SourceClientState<P>>,
	race_target: TC,
	race_target_updated: impl FusedStream<Item = TargetClientState<P>>,
	stall_timeout: Duration,
	mut strategy: impl RaceStrategy<
		P::SourceHeaderId,
		P::TargetHeaderId,
		P::Proof,
		SourceNoncesRange = SC::NoncesRange,
		ProofParameters = SC::ProofParameters,
		TargetNoncesData = TC::TargetNoncesData,
	>,
) -> Result<(), FailedClient> {
	let mut progress_context = Instant::now();
	let mut race_state = RaceState::default();
	let mut stall_countdown = Instant::now();

	let mut source_retry_backoff = retry_backoff();
	let mut source_client_is_online = true;
	let mut source_nonces_required = false;
	let source_nonces = futures::future::Fuse::terminated();
	let source_generate_proof = futures::future::Fuse::terminated();
	let source_go_offline_future = futures::future::Fuse::terminated();

	let mut target_retry_backoff = retry_backoff();
	let mut target_client_is_online = true;
	let mut target_best_nonces_required = false;
	let mut target_finalized_nonces_required = false;
	let target_best_nonces = futures::future::Fuse::terminated();
	let target_finalized_nonces = futures::future::Fuse::terminated();
	let target_submit_proof = futures::future::Fuse::terminated();
	let target_go_offline_future = futures::future::Fuse::terminated();

	futures::pin_mut!(
		race_source_updated,
		source_nonces,
		source_generate_proof,
		source_go_offline_future,
		race_target_updated,
		target_best_nonces,
		target_finalized_nonces,
		target_submit_proof,
		target_go_offline_future,
	);

	loop {
		futures::select! {
			// when headers ids are updated
			source_state = race_source_updated.next() => {
				if let Some(source_state) = source_state {
					let is_source_state_updated = race_state.best_finalized_source_header_id_at_source.as_ref()
						!= Some(&source_state.best_finalized_self);
					if is_source_state_updated {
						source_nonces_required = true;
						race_state.best_finalized_source_header_id_at_source = Some(source_state.best_finalized_self);
					}
				}
			},
			target_state = race_target_updated.next() => {
				if let Some(target_state) = target_state {
					let is_target_best_state_updated = race_state.best_target_header_id.as_ref()
						!= Some(&target_state.best_self);

					if is_target_best_state_updated {
						target_best_nonces_required = true;
						race_state.best_target_header_id = Some(target_state.best_self);
						race_state.best_finalized_source_header_id_at_best_target
							= Some(target_state.best_finalized_peer_at_best_self);
					}

					let is_target_finalized_state_updated = race_state.best_finalized_target_header_id.as_ref()
						!= Some(&target_state.best_finalized_self);
					if  is_target_finalized_state_updated {
						target_finalized_nonces_required = true;
						race_state.best_finalized_target_header_id = Some(target_state.best_finalized_self);
					}
				}
			},

			// when nonces are updated
			nonces = source_nonces => {
				source_nonces_required = false;

				source_client_is_online = process_future_result(
					nonces,
					&mut source_retry_backoff,
					|(at_block, nonces)| {
						log::debug!(
							target: "bridge",
							"Received nonces from {}: {:?}",
							P::source_name(),
							nonces,
						);

						strategy.source_nonces_updated(at_block, nonces);
					},
					&mut source_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving nonces from {}", P::source_name()),
				).fail_if_connection_error(FailedClient::Source)?;
			},
			nonces = target_best_nonces => {
				target_best_nonces_required = false;

				target_client_is_online = process_future_result(
					nonces,
					&mut target_retry_backoff,
					|(_, nonces)| {
						log::debug!(
							target: "bridge",
							"Received best nonces from {}: {:?}",
							P::target_name(),
							nonces,
						);

						let prev_best_at_target = strategy.best_at_target();
						strategy.best_target_nonces_updated(nonces, &mut race_state);
						if strategy.best_at_target() != prev_best_at_target {
							stall_countdown = Instant::now();
						}
					},
					&mut target_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving best nonces from {}", P::target_name()),
				).fail_if_connection_error(FailedClient::Target)?;
			},
			nonces = target_finalized_nonces => {
				target_finalized_nonces_required = false;

				target_client_is_online = process_future_result(
					nonces,
					&mut target_retry_backoff,
					|(_, nonces)| {
						log::debug!(
							target: "bridge",
							"Received finalized nonces from {}: {:?}",
							P::target_name(),
							nonces,
						);

						strategy.finalized_target_nonces_updated(nonces, &mut race_state);
					},
					&mut target_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving finalized nonces from {}", P::target_name()),
				).fail_if_connection_error(FailedClient::Target)?;
			},

			// proof generation and submission
			proof = source_generate_proof => {
				source_client_is_online = process_future_result(
					proof,
					&mut source_retry_backoff,
					|(at_block, nonces_range, proof)| {
						log::debug!(
							target: "bridge",
							"Received proof for nonces in range {:?} from {}",
							nonces_range,
							P::source_name(),
						);

						race_state.nonces_to_submit = Some((at_block, nonces_range, proof));
					},
					&mut source_go_offline_future,
					async_std::task::sleep,
					|| format!("Error generating proof at {}", P::source_name()),
				).fail_if_connection_error(FailedClient::Source)?;
			},
			proof_submit_result = target_submit_proof => {
				target_client_is_online = process_future_result(
					proof_submit_result,
					&mut target_retry_backoff,
					|nonces_range| {
						log::debug!(
							target: "bridge",
							"Successfully submitted proof of nonces {:?} to {}",
							nonces_range,
							P::target_name(),
						);

						race_state.nonces_to_submit = None;
						race_state.nonces_submitted = Some(nonces_range);
						stall_countdown = Instant::now();
					},
					&mut target_go_offline_future,
					async_std::task::sleep,
					|| format!("Error submitting proof {}", P::target_name()),
				).fail_if_connection_error(FailedClient::Target)?;
			},

			// when we're ready to retry request
			_ = source_go_offline_future => {
				source_client_is_online = true;
			},
			_ = target_go_offline_future => {
				target_client_is_online = true;
			},
		}

		progress_context = print_race_progress::<P, _>(progress_context, &strategy);

		if stall_countdown.elapsed() > stall_timeout {
			log::warn!(
				target: "bridge",
				"{} -> {} race has stalled. State: {:?}. Strategy: {:?}",
				P::source_name(),
				P::target_name(),
				race_state,
				strategy,
			);

			return Err(FailedClient::Both);
		} else if race_state.nonces_to_submit.is_none() && race_state.nonces_submitted.is_none() && strategy.is_empty()
		{
			stall_countdown = Instant::now();
		}

		if source_client_is_online {
			source_client_is_online = false;

			let nonces_to_deliver = select_nonces_to_deliver(&race_state, &mut strategy);
			let best_at_source = strategy.best_at_source();

			if let Some((at_block, nonces_range, proof_parameters)) = nonces_to_deliver {
				log::debug!(
					target: "bridge",
					"Asking {} to prove nonces in range {:?} at block {:?}",
					P::source_name(),
					nonces_range,
					at_block,
				);
				source_generate_proof.set(
					race_source
						.generate_proof(at_block, nonces_range, proof_parameters)
						.fuse(),
				);
			} else if source_nonces_required && best_at_source.is_some() {
				log::debug!(target: "bridge", "Asking {} about message nonces", P::source_name());
				let at_block = race_state
					.best_finalized_source_header_id_at_source
					.as_ref()
					.expect(
						"source_nonces_required is only true when\
						best_finalized_source_header_id_at_source is Some; qed",
					)
					.clone();
				source_nonces.set(
					race_source
						.nonces(at_block, best_at_source.expect("guaranteed by if condition; qed"))
						.fuse(),
				);
			} else {
				source_client_is_online = true;
			}
		}

		if target_client_is_online {
			target_client_is_online = false;

			if let Some((at_block, nonces_range, proof)) = race_state.nonces_to_submit.as_ref() {
				log::debug!(
					target: "bridge",
					"Going to submit proof of messages in range {:?} to {} node",
					nonces_range,
					P::target_name(),
				);
				target_submit_proof.set(
					race_target
						.submit_proof(at_block.clone(), nonces_range.clone(), proof.clone())
						.fuse(),
				);
			} else if target_best_nonces_required {
				log::debug!(target: "bridge", "Asking {} about best message nonces", P::target_name());
				let at_block = race_state
					.best_target_header_id
					.as_ref()
					.expect("target_best_nonces_required is only true when best_target_header_id is Some; qed")
					.clone();
				target_best_nonces.set(race_target.nonces(at_block, false).fuse());
			} else if target_finalized_nonces_required {
				log::debug!(target: "bridge", "Asking {} about finalized message nonces", P::target_name());
				let at_block = race_state
					.best_finalized_target_header_id
					.as_ref()
					.expect(
						"target_finalized_nonces_required is only true when\
						best_finalized_target_header_id is Some; qed",
					)
					.clone();
				target_finalized_nonces.set(race_target.nonces(at_block, true).fuse());
			} else {
				target_client_is_online = true;
			}
		}
	}
}

impl<SourceHeaderId, TargetHeaderId, Proof> Default for RaceState<SourceHeaderId, TargetHeaderId, Proof> {
	fn default() -> Self {
		RaceState {
			best_finalized_source_header_id_at_source: None,
			best_finalized_source_header_id_at_best_target: None,
			best_target_header_id: None,
			best_finalized_target_header_id: None,
			nonces_to_submit: None,
			nonces_submitted: None,
		}
	}
}

/// Print race progress.
fn print_race_progress<P, S>(prev_time: Instant, strategy: &S) -> Instant
where
	P: MessageRace,
	S: RaceStrategy<P::SourceHeaderId, P::TargetHeaderId, P::Proof>,
{
	let now_time = Instant::now();

	let need_update = now_time.saturating_duration_since(prev_time) > Duration::from_secs(10);
	if !need_update {
		return prev_time;
	}

	let now_best_nonce_at_source = strategy.best_at_source();
	let now_best_nonce_at_target = strategy.best_at_target();
	log::info!(
		target: "bridge",
		"Synced {:?} of {:?} nonces in {} -> {} race",
		now_best_nonce_at_target,
		now_best_nonce_at_source,
		P::source_name(),
		P::target_name(),
	);
	now_time
}

fn select_nonces_to_deliver<SourceHeaderId, TargetHeaderId, Proof, Strategy>(
	race_state: &RaceState<SourceHeaderId, TargetHeaderId, Proof>,
	strategy: &mut Strategy,
) -> Option<(SourceHeaderId, RangeInclusive<MessageNonce>, Strategy::ProofParameters)>
where
	SourceHeaderId: Clone,
	Strategy: RaceStrategy<SourceHeaderId, TargetHeaderId, Proof>,
{
	race_state
		.best_finalized_source_header_id_at_best_target
		.as_ref()
		.and_then(|best_finalized_source_header_id_at_best_target| {
			strategy
				.select_nonces_to_deliver(&race_state)
				.map(|(nonces_range, proof_parameters)| {
					(
						best_finalized_source_header_id_at_best_target.clone(),
						nonces_range,
						proof_parameters,
					)
				})
		})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::message_race_strategy::BasicStrategy;
	use relay_utils::HeaderId;

	#[test]
	fn proof_is_generated_at_best_block_known_to_target_node() {
		const GENERATED_AT: u64 = 6;
		const BEST_AT_SOURCE: u64 = 10;
		const BEST_AT_TARGET: u64 = 8;

		// target node only knows about source' BEST_AT_TARGET block
		// source node has BEST_AT_SOURCE > BEST_AT_TARGET block
		let mut race_state = RaceState::<_, _, ()> {
			best_finalized_source_header_id_at_source: Some(HeaderId(BEST_AT_SOURCE, BEST_AT_SOURCE)),
			best_finalized_source_header_id_at_best_target: Some(HeaderId(BEST_AT_TARGET, BEST_AT_TARGET)),
			best_target_header_id: Some(HeaderId(0, 0)),
			best_finalized_target_header_id: Some(HeaderId(0, 0)),
			nonces_to_submit: None,
			nonces_submitted: None,
		};

		// we have some nonces to deliver and they're generated at GENERATED_AT < BEST_AT_SOURCE
		let mut strategy = BasicStrategy::new();
		strategy.source_nonces_updated(
			HeaderId(GENERATED_AT, GENERATED_AT),
			SourceClientNonces {
				new_nonces: 0..=10,
				confirmed_nonce: None,
			},
		);
		strategy.best_target_nonces_updated(
			TargetClientNonces {
				latest_nonce: 5u64,
				nonces_data: (),
			},
			&mut race_state,
		);

		// the proof will be generated on source, but using BEST_AT_TARGET block
		assert_eq!(
			select_nonces_to_deliver(&race_state, &mut strategy),
			Some((HeaderId(BEST_AT_TARGET, BEST_AT_TARGET), 6..=10, (),))
		);
	}
}
