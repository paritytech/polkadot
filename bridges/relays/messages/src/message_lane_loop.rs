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

//! Message delivery loop. Designed to work with messages pallet.
//!
//! Single relay instance delivers messages of single lane in single direction.
//! To serve two-way lane, you would need two instances of relay.
//! To serve N two-way lanes, you would need N*2 instances of relay.
//!
//! Please keep in mind that the best header in this file is actually best
//! finalized header. I.e. when talking about headers in lane context, we
//! only care about finalized headers.

use crate::message_lane::{MessageLane, SourceHeaderIdOf, TargetHeaderIdOf};
use crate::message_race_delivery::run as run_message_delivery_race;
use crate::message_race_receiving::run as run_message_receiving_race;
use crate::metrics::MessageLaneLoopMetrics;

use async_trait::async_trait;
use bp_messages::{LaneId, MessageNonce, UnrewardedRelayersState, Weight};
use futures::{channel::mpsc::unbounded, future::FutureExt, stream::StreamExt};
use relay_utils::{
	interval,
	metrics::{GlobalMetrics, MetricsParams},
	process_future_result,
	relay_loop::Client as RelayClient,
	retry_backoff, FailedClient,
};
use std::{collections::BTreeMap, fmt::Debug, future::Future, ops::RangeInclusive, time::Duration};

/// Message lane loop configuration params.
#[derive(Debug, Clone)]
pub struct Params {
	/// Id of lane this loop is servicing.
	pub lane: LaneId,
	/// Interval at which we ask target node about its updates.
	pub source_tick: Duration,
	/// Interval at which we ask target node about its updates.
	pub target_tick: Duration,
	/// Delay between moments when connection error happens and our reconnect attempt.
	pub reconnect_delay: Duration,
	/// The loop will auto-restart if there has been no updates during this period.
	pub stall_timeout: Duration,
	/// Message delivery race parameters.
	pub delivery_params: MessageDeliveryParams,
}

/// Message delivery race parameters.
#[derive(Debug, Clone)]
pub struct MessageDeliveryParams {
	/// Maximal number of unconfirmed relayer entries at the inbound lane. If there's that number of entries
	/// in the `InboundLaneData::relayers` set, all new messages will be rejected until reward payment will
	/// be proved (by including outbound lane state to the message delivery transaction).
	pub max_unrewarded_relayer_entries_at_target: MessageNonce,
	/// Message delivery race will stop delivering messages if there are `max_unconfirmed_nonces_at_target`
	/// unconfirmed nonces on the target node. The race would continue once they're confirmed by the
	/// receiving race.
	pub max_unconfirmed_nonces_at_target: MessageNonce,
	/// Maximal number of relayed messages in single delivery transaction.
	pub max_messages_in_single_batch: MessageNonce,
	/// Maximal cumulative dispatch weight of relayed messages in single delivery transaction.
	pub max_messages_weight_in_single_batch: Weight,
	/// Maximal cumulative size of relayed messages in single delivery transaction.
	pub max_messages_size_in_single_batch: usize,
}

/// Message weights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MessageWeights {
	/// Message dispatch weight.
	pub weight: Weight,
	/// Message size (number of bytes in encoded payload).
	pub size: usize,
}

/// Messages weights map.
pub type MessageWeightsMap = BTreeMap<MessageNonce, MessageWeights>;

/// Message delivery race proof parameters.
#[derive(Debug, PartialEq)]
pub struct MessageProofParameters {
	/// Include outbound lane state proof?
	pub outbound_state_proof_required: bool,
	/// Cumulative dispatch weight of messages that we're building proof for.
	pub dispatch_weight: Weight,
}

/// Source client trait.
#[async_trait]
pub trait SourceClient<P: MessageLane>: RelayClient {
	/// Returns state of the client.
	async fn state(&self) -> Result<SourceClientState<P>, Self::Error>;

	/// Get nonce of instance of latest generated message.
	async fn latest_generated_nonce(
		&self,
		id: SourceHeaderIdOf<P>,
	) -> Result<(SourceHeaderIdOf<P>, MessageNonce), Self::Error>;
	/// Get nonce of the latest message, which receiving has been confirmed by the target chain.
	async fn latest_confirmed_received_nonce(
		&self,
		id: SourceHeaderIdOf<P>,
	) -> Result<(SourceHeaderIdOf<P>, MessageNonce), Self::Error>;

	/// Returns mapping of message nonces, generated on this client, to their weights.
	///
	/// Some weights may be missing from returned map, if corresponding messages were pruned at
	/// the source chain.
	async fn generated_messages_weights(
		&self,
		id: SourceHeaderIdOf<P>,
		nonces: RangeInclusive<MessageNonce>,
	) -> Result<MessageWeightsMap, Self::Error>;

	/// Prove messages in inclusive range [begin; end].
	async fn prove_messages(
		&self,
		id: SourceHeaderIdOf<P>,
		nonces: RangeInclusive<MessageNonce>,
		proof_parameters: MessageProofParameters,
	) -> Result<(SourceHeaderIdOf<P>, RangeInclusive<MessageNonce>, P::MessagesProof), Self::Error>;

	/// Submit messages receiving proof.
	async fn submit_messages_receiving_proof(
		&self,
		generated_at_block: TargetHeaderIdOf<P>,
		proof: P::MessagesReceivingProof,
	) -> Result<(), Self::Error>;

	/// We need given finalized target header on source to continue synchronization.
	async fn require_target_header_on_source(&self, id: TargetHeaderIdOf<P>);
}

/// Target client trait.
#[async_trait]
pub trait TargetClient<P: MessageLane>: RelayClient {
	/// Returns state of the client.
	async fn state(&self) -> Result<TargetClientState<P>, Self::Error>;

	/// Get nonce of latest received message.
	async fn latest_received_nonce(
		&self,
		id: TargetHeaderIdOf<P>,
	) -> Result<(TargetHeaderIdOf<P>, MessageNonce), Self::Error>;

	/// Get nonce of latest confirmed message.
	async fn latest_confirmed_received_nonce(
		&self,
		id: TargetHeaderIdOf<P>,
	) -> Result<(TargetHeaderIdOf<P>, MessageNonce), Self::Error>;
	/// Get state of unrewarded relayers set at the inbound lane.
	async fn unrewarded_relayers_state(
		&self,
		id: TargetHeaderIdOf<P>,
	) -> Result<(TargetHeaderIdOf<P>, UnrewardedRelayersState), Self::Error>;

	/// Prove messages receiving at given block.
	async fn prove_messages_receiving(
		&self,
		id: TargetHeaderIdOf<P>,
	) -> Result<(TargetHeaderIdOf<P>, P::MessagesReceivingProof), Self::Error>;

	/// Submit messages proof.
	async fn submit_messages_proof(
		&self,
		generated_at_header: SourceHeaderIdOf<P>,
		nonces: RangeInclusive<MessageNonce>,
		proof: P::MessagesProof,
	) -> Result<RangeInclusive<MessageNonce>, Self::Error>;

	/// We need given finalized source header on target to continue synchronization.
	async fn require_source_header_on_target(&self, id: SourceHeaderIdOf<P>);
}

/// State of the client.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClientState<SelfHeaderId, PeerHeaderId> {
	/// Best header id of this chain.
	pub best_self: SelfHeaderId,
	/// Best finalized header id of this chain.
	pub best_finalized_self: SelfHeaderId,
	/// Best finalized header id of the peer chain read at the best block of this chain (at `best_finalized_self`).
	pub best_finalized_peer_at_best_self: PeerHeaderId,
}

/// State of source client in one-way message lane.
pub type SourceClientState<P> = ClientState<SourceHeaderIdOf<P>, TargetHeaderIdOf<P>>;

/// State of target client in one-way message lane.
pub type TargetClientState<P> = ClientState<TargetHeaderIdOf<P>, SourceHeaderIdOf<P>>;

/// Both clients state.
#[derive(Debug, Default)]
pub struct ClientsState<P: MessageLane> {
	/// Source client state.
	pub source: Option<SourceClientState<P>>,
	/// Target client state.
	pub target: Option<TargetClientState<P>>,
}

/// Return prefix that will be used by default to expose Prometheus metrics of the finality proofs sync loop.
pub fn metrics_prefix<P: MessageLane>(lane: &LaneId) -> String {
	format!(
		"{}_to_{}_MessageLane_{}",
		P::SOURCE_NAME,
		P::TARGET_NAME,
		hex::encode(lane)
	)
}

/// Run message lane service loop.
pub async fn run<P: MessageLane>(
	params: Params,
	source_client: impl SourceClient<P>,
	target_client: impl TargetClient<P>,
	metrics_params: MetricsParams,
	exit_signal: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
	let exit_signal = exit_signal.shared();
	relay_utils::relay_loop(source_client, target_client)
		.reconnect_delay(params.reconnect_delay)
		.with_metrics(Some(metrics_prefix::<P>(&params.lane)), metrics_params)
		.loop_metric(|registry, prefix| MessageLaneLoopMetrics::new(registry, prefix))?
		.standalone_metric(|registry, prefix| GlobalMetrics::new(registry, prefix))?
		.expose()
		.await?
		.run(
			metrics_prefix::<P>(&params.lane),
			move |source_client, target_client, metrics| {
				run_until_connection_lost(
					params.clone(),
					source_client,
					target_client,
					metrics,
					exit_signal.clone(),
				)
			},
		)
		.await
}

/// Run one-way message delivery loop until connection with target or source node is lost, or exit signal is received.
async fn run_until_connection_lost<P: MessageLane, SC: SourceClient<P>, TC: TargetClient<P>>(
	params: Params,
	source_client: SC,
	target_client: TC,
	metrics_msg: Option<MessageLaneLoopMetrics>,
	exit_signal: impl Future<Output = ()>,
) -> Result<(), FailedClient> {
	let mut source_retry_backoff = retry_backoff();
	let mut source_client_is_online = false;
	let mut source_state_required = true;
	let source_state = source_client.state().fuse();
	let source_go_offline_future = futures::future::Fuse::terminated();
	let source_tick_stream = interval(params.source_tick).fuse();

	let mut target_retry_backoff = retry_backoff();
	let mut target_client_is_online = false;
	let mut target_state_required = true;
	let target_state = target_client.state().fuse();
	let target_go_offline_future = futures::future::Fuse::terminated();
	let target_tick_stream = interval(params.target_tick).fuse();

	let (
		(delivery_source_state_sender, delivery_source_state_receiver),
		(delivery_target_state_sender, delivery_target_state_receiver),
	) = (unbounded(), unbounded());
	let delivery_race_loop = run_message_delivery_race(
		source_client.clone(),
		delivery_source_state_receiver,
		target_client.clone(),
		delivery_target_state_receiver,
		params.stall_timeout,
		metrics_msg.clone(),
		params.delivery_params,
	)
	.fuse();

	let (
		(receiving_source_state_sender, receiving_source_state_receiver),
		(receiving_target_state_sender, receiving_target_state_receiver),
	) = (unbounded(), unbounded());
	let receiving_race_loop = run_message_receiving_race(
		source_client.clone(),
		receiving_source_state_receiver,
		target_client.clone(),
		receiving_target_state_receiver,
		params.stall_timeout,
		metrics_msg.clone(),
	)
	.fuse();

	let exit_signal = exit_signal.fuse();

	futures::pin_mut!(
		source_state,
		source_go_offline_future,
		source_tick_stream,
		target_state,
		target_go_offline_future,
		target_tick_stream,
		delivery_race_loop,
		receiving_race_loop,
		exit_signal
	);

	loop {
		futures::select! {
			new_source_state = source_state => {
				source_state_required = false;

				source_client_is_online = process_future_result(
					new_source_state,
					&mut source_retry_backoff,
					|new_source_state| {
						log::debug!(
							target: "bridge",
							"Received state from {} node: {:?}",
							P::SOURCE_NAME,
							new_source_state,
						);
						let _ = delivery_source_state_sender.unbounded_send(new_source_state.clone());
						let _ = receiving_source_state_sender.unbounded_send(new_source_state.clone());

						if let Some(metrics_msg) = metrics_msg.as_ref() {
							metrics_msg.update_source_state::<P>(new_source_state);
						}
					},
					&mut source_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving state from {} node", P::SOURCE_NAME),
				).fail_if_connection_error(FailedClient::Source)?;
			},
			_ = source_go_offline_future => {
				source_client_is_online = true;
			},
			_ = source_tick_stream.next() => {
				source_state_required = true;
			},
			new_target_state = target_state => {
				target_state_required = false;

				target_client_is_online = process_future_result(
					new_target_state,
					&mut target_retry_backoff,
					|new_target_state| {
						log::debug!(
							target: "bridge",
							"Received state from {} node: {:?}",
							P::TARGET_NAME,
							new_target_state,
						);
						let _ = delivery_target_state_sender.unbounded_send(new_target_state.clone());
						let _ = receiving_target_state_sender.unbounded_send(new_target_state.clone());

						if let Some(metrics_msg) = metrics_msg.as_ref() {
							metrics_msg.update_target_state::<P>(new_target_state);
						}
					},
					&mut target_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving state from {} node", P::TARGET_NAME),
				).fail_if_connection_error(FailedClient::Target)?;
			},
			_ = target_go_offline_future => {
				target_client_is_online = true;
			},
			_ = target_tick_stream.next() => {
				target_state_required = true;
			},

			delivery_error = delivery_race_loop => {
				match delivery_error {
					Ok(_) => unreachable!("only ends with error; qed"),
					Err(err) => return Err(err),
				}
			},
			receiving_error = receiving_race_loop => {
				match receiving_error {
					Ok(_) => unreachable!("only ends with error; qed"),
					Err(err) => return Err(err),
				}
			},

			() = exit_signal => {
				return Ok(());
			}
		}

		if source_client_is_online && source_state_required {
			log::debug!(target: "bridge", "Asking {} node about its state", P::SOURCE_NAME);
			source_state.set(source_client.state().fuse());
			source_client_is_online = false;
		}

		if target_client_is_online && target_state_required {
			log::debug!(target: "bridge", "Asking {} node about its state", P::TARGET_NAME);
			target_state.set(target_client.state().fuse());
			target_client_is_online = false;
		}
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use futures::stream::StreamExt;
	use parking_lot::Mutex;
	use relay_utils::{HeaderId, MaybeConnectionError};
	use std::sync::Arc;

	pub fn header_id(number: TestSourceHeaderNumber) -> TestSourceHeaderId {
		HeaderId(number, number)
	}

	pub type TestSourceHeaderId = HeaderId<TestSourceHeaderNumber, TestSourceHeaderHash>;
	pub type TestTargetHeaderId = HeaderId<TestTargetHeaderNumber, TestTargetHeaderHash>;

	pub type TestMessagesProof = (RangeInclusive<MessageNonce>, Option<MessageNonce>);
	pub type TestMessagesReceivingProof = MessageNonce;

	pub type TestSourceHeaderNumber = u64;
	pub type TestSourceHeaderHash = u64;

	pub type TestTargetHeaderNumber = u64;
	pub type TestTargetHeaderHash = u64;

	#[derive(Debug)]
	pub struct TestError;

	impl MaybeConnectionError for TestError {
		fn is_connection_error(&self) -> bool {
			true
		}
	}

	#[derive(Clone)]
	pub struct TestMessageLane;

	impl MessageLane for TestMessageLane {
		const SOURCE_NAME: &'static str = "TestSource";
		const TARGET_NAME: &'static str = "TestTarget";

		type MessagesProof = TestMessagesProof;
		type MessagesReceivingProof = TestMessagesReceivingProof;

		type SourceHeaderNumber = TestSourceHeaderNumber;
		type SourceHeaderHash = TestSourceHeaderHash;

		type TargetHeaderNumber = TestTargetHeaderNumber;
		type TargetHeaderHash = TestTargetHeaderHash;
	}

	#[derive(Debug, Default, Clone)]
	pub struct TestClientData {
		is_source_fails: bool,
		is_source_reconnected: bool,
		source_state: SourceClientState<TestMessageLane>,
		source_latest_generated_nonce: MessageNonce,
		source_latest_confirmed_received_nonce: MessageNonce,
		submitted_messages_receiving_proofs: Vec<TestMessagesReceivingProof>,
		is_target_fails: bool,
		is_target_reconnected: bool,
		target_state: SourceClientState<TestMessageLane>,
		target_latest_received_nonce: MessageNonce,
		target_latest_confirmed_received_nonce: MessageNonce,
		submitted_messages_proofs: Vec<TestMessagesProof>,
		target_to_source_header_required: Option<TestTargetHeaderId>,
		target_to_source_header_requirements: Vec<TestTargetHeaderId>,
		source_to_target_header_required: Option<TestSourceHeaderId>,
		source_to_target_header_requirements: Vec<TestSourceHeaderId>,
	}

	#[derive(Clone)]
	pub struct TestSourceClient {
		data: Arc<Mutex<TestClientData>>,
		tick: Arc<dyn Fn(&mut TestClientData) + Send + Sync>,
	}

	#[async_trait]
	impl RelayClient for TestSourceClient {
		type Error = TestError;

		async fn reconnect(&mut self) -> Result<(), TestError> {
			{
				let mut data = self.data.lock();
				(self.tick)(&mut *data);
				data.is_source_reconnected = true;
			}
			Ok(())
		}
	}

	#[async_trait]
	impl SourceClient<TestMessageLane> for TestSourceClient {
		async fn state(&self) -> Result<SourceClientState<TestMessageLane>, TestError> {
			let mut data = self.data.lock();
			(self.tick)(&mut *data);
			if data.is_source_fails {
				return Err(TestError);
			}
			Ok(data.source_state.clone())
		}

		async fn latest_generated_nonce(
			&self,
			id: SourceHeaderIdOf<TestMessageLane>,
		) -> Result<(SourceHeaderIdOf<TestMessageLane>, MessageNonce), TestError> {
			let mut data = self.data.lock();
			(self.tick)(&mut *data);
			if data.is_source_fails {
				return Err(TestError);
			}
			Ok((id, data.source_latest_generated_nonce))
		}

		async fn latest_confirmed_received_nonce(
			&self,
			id: SourceHeaderIdOf<TestMessageLane>,
		) -> Result<(SourceHeaderIdOf<TestMessageLane>, MessageNonce), TestError> {
			let mut data = self.data.lock();
			(self.tick)(&mut *data);
			Ok((id, data.source_latest_confirmed_received_nonce))
		}

		async fn generated_messages_weights(
			&self,
			_id: SourceHeaderIdOf<TestMessageLane>,
			nonces: RangeInclusive<MessageNonce>,
		) -> Result<MessageWeightsMap, TestError> {
			Ok(nonces
				.map(|nonce| (nonce, MessageWeights { weight: 1, size: 1 }))
				.collect())
		}

		async fn prove_messages(
			&self,
			id: SourceHeaderIdOf<TestMessageLane>,
			nonces: RangeInclusive<MessageNonce>,
			proof_parameters: MessageProofParameters,
		) -> Result<
			(
				SourceHeaderIdOf<TestMessageLane>,
				RangeInclusive<MessageNonce>,
				TestMessagesProof,
			),
			TestError,
		> {
			let mut data = self.data.lock();
			(self.tick)(&mut *data);
			Ok((
				id,
				nonces.clone(),
				(
					nonces,
					if proof_parameters.outbound_state_proof_required {
						Some(data.source_latest_confirmed_received_nonce)
					} else {
						None
					},
				),
			))
		}

		async fn submit_messages_receiving_proof(
			&self,
			_generated_at_block: TargetHeaderIdOf<TestMessageLane>,
			proof: TestMessagesReceivingProof,
		) -> Result<(), TestError> {
			let mut data = self.data.lock();
			(self.tick)(&mut *data);
			data.source_state.best_self =
				HeaderId(data.source_state.best_self.0 + 1, data.source_state.best_self.1 + 1);
			data.source_state.best_finalized_self = data.source_state.best_self;
			data.submitted_messages_receiving_proofs.push(proof);
			data.source_latest_confirmed_received_nonce = proof;
			Ok(())
		}

		async fn require_target_header_on_source(&self, id: TargetHeaderIdOf<TestMessageLane>) {
			let mut data = self.data.lock();
			data.target_to_source_header_required = Some(id);
			data.target_to_source_header_requirements.push(id);
			(self.tick)(&mut *data);
		}
	}

	#[derive(Clone)]
	pub struct TestTargetClient {
		data: Arc<Mutex<TestClientData>>,
		tick: Arc<dyn Fn(&mut TestClientData) + Send + Sync>,
	}

	#[async_trait]
	impl RelayClient for TestTargetClient {
		type Error = TestError;

		async fn reconnect(&mut self) -> Result<(), TestError> {
			{
				let mut data = self.data.lock();
				(self.tick)(&mut *data);
				data.is_target_reconnected = true;
			}
			Ok(())
		}
	}

	#[async_trait]
	impl TargetClient<TestMessageLane> for TestTargetClient {
		async fn state(&self) -> Result<TargetClientState<TestMessageLane>, TestError> {
			let mut data = self.data.lock();
			(self.tick)(&mut *data);
			if data.is_target_fails {
				return Err(TestError);
			}
			Ok(data.target_state.clone())
		}

		async fn latest_received_nonce(
			&self,
			id: TargetHeaderIdOf<TestMessageLane>,
		) -> Result<(TargetHeaderIdOf<TestMessageLane>, MessageNonce), TestError> {
			let mut data = self.data.lock();
			(self.tick)(&mut *data);
			if data.is_target_fails {
				return Err(TestError);
			}
			Ok((id, data.target_latest_received_nonce))
		}

		async fn unrewarded_relayers_state(
			&self,
			id: TargetHeaderIdOf<TestMessageLane>,
		) -> Result<(TargetHeaderIdOf<TestMessageLane>, UnrewardedRelayersState), TestError> {
			Ok((
				id,
				UnrewardedRelayersState {
					unrewarded_relayer_entries: 0,
					messages_in_oldest_entry: 0,
					total_messages: 0,
				},
			))
		}

		async fn latest_confirmed_received_nonce(
			&self,
			id: TargetHeaderIdOf<TestMessageLane>,
		) -> Result<(TargetHeaderIdOf<TestMessageLane>, MessageNonce), TestError> {
			let mut data = self.data.lock();
			(self.tick)(&mut *data);
			if data.is_target_fails {
				return Err(TestError);
			}
			Ok((id, data.target_latest_confirmed_received_nonce))
		}

		async fn prove_messages_receiving(
			&self,
			id: TargetHeaderIdOf<TestMessageLane>,
		) -> Result<(TargetHeaderIdOf<TestMessageLane>, TestMessagesReceivingProof), TestError> {
			Ok((id, self.data.lock().target_latest_received_nonce))
		}

		async fn submit_messages_proof(
			&self,
			_generated_at_header: SourceHeaderIdOf<TestMessageLane>,
			nonces: RangeInclusive<MessageNonce>,
			proof: TestMessagesProof,
		) -> Result<RangeInclusive<MessageNonce>, TestError> {
			let mut data = self.data.lock();
			(self.tick)(&mut *data);
			if data.is_target_fails {
				return Err(TestError);
			}
			data.target_state.best_self =
				HeaderId(data.target_state.best_self.0 + 1, data.target_state.best_self.1 + 1);
			data.target_state.best_finalized_self = data.target_state.best_self;
			data.target_latest_received_nonce = *proof.0.end();
			if let Some(target_latest_confirmed_received_nonce) = proof.1 {
				data.target_latest_confirmed_received_nonce = target_latest_confirmed_received_nonce;
			}
			data.submitted_messages_proofs.push(proof);
			Ok(nonces)
		}

		async fn require_source_header_on_target(&self, id: SourceHeaderIdOf<TestMessageLane>) {
			let mut data = self.data.lock();
			data.source_to_target_header_required = Some(id);
			data.source_to_target_header_requirements.push(id);
			(self.tick)(&mut *data);
		}
	}

	fn run_loop_test(
		data: TestClientData,
		source_tick: Arc<dyn Fn(&mut TestClientData) + Send + Sync>,
		target_tick: Arc<dyn Fn(&mut TestClientData) + Send + Sync>,
		exit_signal: impl Future<Output = ()> + 'static + Send,
	) -> TestClientData {
		async_std::task::block_on(async {
			let data = Arc::new(Mutex::new(data));

			let source_client = TestSourceClient {
				data: data.clone(),
				tick: source_tick,
			};
			let target_client = TestTargetClient {
				data: data.clone(),
				tick: target_tick,
			};
			let _ = run(
				Params {
					lane: [0, 0, 0, 0],
					source_tick: Duration::from_millis(100),
					target_tick: Duration::from_millis(100),
					reconnect_delay: Duration::from_millis(0),
					stall_timeout: Duration::from_millis(60 * 1000),
					delivery_params: MessageDeliveryParams {
						max_unrewarded_relayer_entries_at_target: 4,
						max_unconfirmed_nonces_at_target: 4,
						max_messages_in_single_batch: 4,
						max_messages_weight_in_single_batch: 4,
						max_messages_size_in_single_batch: 4,
					},
				},
				source_client,
				target_client,
				MetricsParams::disabled(),
				exit_signal,
			)
			.await;
			let result = data.lock().clone();
			result
		})
	}

	#[test]
	fn message_lane_loop_is_able_to_recover_from_connection_errors() {
		// with this configuration, source client will return Err, making source client
		// reconnect. Then the target client will fail with Err + reconnect. Then we finally
		// able to deliver messages.
		let (exit_sender, exit_receiver) = unbounded();
		let result = run_loop_test(
			TestClientData {
				is_source_fails: true,
				source_state: ClientState {
					best_self: HeaderId(0, 0),
					best_finalized_self: HeaderId(0, 0),
					best_finalized_peer_at_best_self: HeaderId(0, 0),
				},
				source_latest_generated_nonce: 1,
				target_state: ClientState {
					best_self: HeaderId(0, 0),
					best_finalized_self: HeaderId(0, 0),
					best_finalized_peer_at_best_self: HeaderId(0, 0),
				},
				target_latest_received_nonce: 0,
				..Default::default()
			},
			Arc::new(|data: &mut TestClientData| {
				if data.is_source_reconnected {
					data.is_source_fails = false;
					data.is_target_fails = true;
				}
			}),
			Arc::new(move |data: &mut TestClientData| {
				if data.is_target_reconnected {
					data.is_target_fails = false;
				}
				if data.target_state.best_finalized_peer_at_best_self.0 < 10 {
					data.target_state.best_finalized_peer_at_best_self = HeaderId(
						data.target_state.best_finalized_peer_at_best_self.0 + 1,
						data.target_state.best_finalized_peer_at_best_self.0 + 1,
					);
				}
				if !data.submitted_messages_proofs.is_empty() {
					exit_sender.unbounded_send(()).unwrap();
				}
			}),
			exit_receiver.into_future().map(|(_, _)| ()),
		);

		assert_eq!(result.submitted_messages_proofs, vec![(1..=1, None)],);
	}

	#[test]
	fn message_lane_loop_works() {
		let (exit_sender, exit_receiver) = unbounded();
		let result = run_loop_test(
			TestClientData {
				source_state: ClientState {
					best_self: HeaderId(10, 10),
					best_finalized_self: HeaderId(10, 10),
					best_finalized_peer_at_best_self: HeaderId(0, 0),
				},
				source_latest_generated_nonce: 10,
				target_state: ClientState {
					best_self: HeaderId(0, 0),
					best_finalized_self: HeaderId(0, 0),
					best_finalized_peer_at_best_self: HeaderId(0, 0),
				},
				target_latest_received_nonce: 0,
				..Default::default()
			},
			Arc::new(|data: &mut TestClientData| {
				// blocks are produced on every tick
				data.source_state.best_self =
					HeaderId(data.source_state.best_self.0 + 1, data.source_state.best_self.1 + 1);
				data.source_state.best_finalized_self = data.source_state.best_self;
				// headers relay must only be started when we need new target headers at source node
				if data.target_to_source_header_required.is_some() {
					assert!(data.source_state.best_finalized_peer_at_best_self.0 < data.target_state.best_self.0);
					data.target_to_source_header_required = None;
				}
				// syncing target headers -> source chain
				if let Some(last_requirement) = data.target_to_source_header_requirements.last() {
					if *last_requirement != data.source_state.best_finalized_peer_at_best_self {
						data.source_state.best_finalized_peer_at_best_self = *last_requirement;
					}
				}
			}),
			Arc::new(move |data: &mut TestClientData| {
				// blocks are produced on every tick
				data.target_state.best_self =
					HeaderId(data.target_state.best_self.0 + 1, data.target_state.best_self.1 + 1);
				data.target_state.best_finalized_self = data.target_state.best_self;
				// headers relay must only be started when we need new source headers at target node
				if data.source_to_target_header_required.is_some() {
					assert!(data.target_state.best_finalized_peer_at_best_self.0 < data.source_state.best_self.0);
					data.source_to_target_header_required = None;
				}
				// syncing source headers -> target chain
				if let Some(last_requirement) = data.source_to_target_header_requirements.last() {
					if *last_requirement != data.target_state.best_finalized_peer_at_best_self {
						data.target_state.best_finalized_peer_at_best_self = *last_requirement;
					}
				}
				// if source has received all messages receiving confirmations => stop
				if data.source_latest_confirmed_received_nonce == 10 {
					exit_sender.unbounded_send(()).unwrap();
				}
			}),
			exit_receiver.into_future().map(|(_, _)| ()),
		);

		// there are no strict restrictions on when reward confirmation should come
		// (because `max_unconfirmed_nonces_at_target` is `100` in tests and this confirmation
		// depends on the state of both clients)
		// => we do not check it here
		assert_eq!(result.submitted_messages_proofs[0].0, 1..=4);
		assert_eq!(result.submitted_messages_proofs[1].0, 5..=8);
		assert_eq!(result.submitted_messages_proofs[2].0, 9..=10);
		assert!(!result.submitted_messages_receiving_proofs.is_empty());

		// check that we have at least once required new source->target or target->source headers
		assert!(!result.target_to_source_header_requirements.is_empty());
		assert!(!result.source_to_target_header_requirements.is_empty());
	}
}
