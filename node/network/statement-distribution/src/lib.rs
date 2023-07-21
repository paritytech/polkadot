// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! The Statement Distribution Subsystem.
//!
//! This is responsible for distributing signed statements about candidate
//! validity among validators.

// #![deny(unused_crate_dependencies)]
#![warn(missing_docs)]

use error::{log_error, FatalResult};
use std::time::Duration;

use polkadot_node_network_protocol::{
	request_response::{
		v1 as request_v1, vstaging::AttestedCandidateRequest, IncomingRequestReceiver,
	},
	vstaging as protocol_vstaging, Versioned,
};
use polkadot_node_primitives::StatementWithPVD;
use polkadot_node_subsystem::{
	messages::{NetworkBridgeEvent, StatementDistributionMessage},
	overseer, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, SpawnedSubsystem, SubsystemError,
};
use polkadot_node_subsystem_util::{
	rand,
	reputation::{ReputationAggregator, REPUTATION_CHANGE_INTERVAL},
	runtime::{prospective_parachains_mode, ProspectiveParachainsMode},
};

use futures::{channel::mpsc, prelude::*};
use sp_keystore::KeystorePtr;

use fatality::Nested;

mod error;
pub use error::{Error, FatalError, JfyiError, Result};

/// Metrics for the statement distribution
pub(crate) mod metrics;
use metrics::Metrics;

mod legacy_v1;
use legacy_v1::{
	respond as v1_respond_task, RequesterMessage as V1RequesterMessage,
	ResponderMessage as V1ResponderMessage,
};

mod vstaging;

const LOG_TARGET: &str = "parachain::statement-distribution";

/// The statement distribution subsystem.
pub struct StatementDistributionSubsystem<R> {
	/// Pointer to a keystore, which is required for determining this node's validator index.
	keystore: KeystorePtr,
	/// Receiver for incoming large statement requests.
	v1_req_receiver: Option<IncomingRequestReceiver<request_v1::StatementFetchingRequest>>,
	/// Receiver for incoming candidate requests.
	req_receiver: Option<IncomingRequestReceiver<AttestedCandidateRequest>>,
	/// Prometheus metrics
	metrics: Metrics,
	/// Pseudo-random generator for peers selection logic
	rng: R,
	/// Aggregated reputation change
	reputation: ReputationAggregator,
}

#[overseer::subsystem(StatementDistribution, error=SubsystemError, prefix=self::overseer)]
impl<Context, R: rand::Rng + Send + Sync + 'static> StatementDistributionSubsystem<R> {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		// Swallow error because failure is fatal to the node and we log with more precision
		// within `run`.
		SpawnedSubsystem {
			name: "statement-distribution-subsystem",
			future: self
				.run(ctx)
				.map_err(|e| SubsystemError::with_origin("statement-distribution", e))
				.boxed(),
		}
	}
}

/// Messages to be handled in this subsystem.
enum MuxedMessage {
	/// Messages from other subsystems.
	Subsystem(FatalResult<FromOrchestra<StatementDistributionMessage>>),
	/// Messages from spawned v1 (legacy) requester background tasks.
	V1Requester(Option<V1RequesterMessage>),
	/// Messages from spawned v1 (legacy) responder background task.
	V1Responder(Option<V1ResponderMessage>),
	/// Messages from candidate responder background task.
	Responder(Option<vstaging::ResponderMessage>),
	/// Messages from answered requests.
	Response(vstaging::UnhandledResponse),
	/// Message that a request is ready to be retried. This just acts as a signal that we should
	/// dispatch all pending requests again.
	RetryRequest(()),
}

#[overseer::contextbounds(StatementDistribution, prefix = self::overseer)]
impl MuxedMessage {
	async fn receive<Context>(
		ctx: &mut Context,
		state: &mut vstaging::State,
		from_v1_requester: &mut mpsc::Receiver<V1RequesterMessage>,
		from_v1_responder: &mut mpsc::Receiver<V1ResponderMessage>,
		from_responder: &mut mpsc::Receiver<vstaging::ResponderMessage>,
	) -> MuxedMessage {
		let (request_manager, response_manager) = state.request_and_response_managers();
		// We are only fusing here to make `select` happy, in reality we will quit if one of those
		// streams end:
		let from_orchestra = ctx.recv().fuse();
		let from_v1_requester = from_v1_requester.next();
		let from_v1_responder = from_v1_responder.next();
		let from_responder = from_responder.next();
		let receive_response = vstaging::receive_response(response_manager).fuse();
		let retry_request = vstaging::next_retry(request_manager).fuse();
		futures::pin_mut!(
			from_orchestra,
			from_v1_requester,
			from_v1_responder,
			from_responder,
			receive_response,
			retry_request,
		);
		futures::select! {
			msg = from_orchestra => MuxedMessage::Subsystem(msg.map_err(FatalError::SubsystemReceive)),
			msg = from_v1_requester => MuxedMessage::V1Requester(msg),
			msg = from_v1_responder => MuxedMessage::V1Responder(msg),
			msg = from_responder => MuxedMessage::Responder(msg),
			msg = receive_response => MuxedMessage::Response(msg),
			msg = retry_request => MuxedMessage::RetryRequest(msg),
		}
	}
}

#[overseer::contextbounds(StatementDistribution, prefix = self::overseer)]
impl<R: rand::Rng> StatementDistributionSubsystem<R> {
	/// Create a new Statement Distribution Subsystem
	pub fn new(
		keystore: KeystorePtr,
		v1_req_receiver: IncomingRequestReceiver<request_v1::StatementFetchingRequest>,
		req_receiver: IncomingRequestReceiver<AttestedCandidateRequest>,
		metrics: Metrics,
		rng: R,
	) -> Self {
		Self {
			keystore,
			v1_req_receiver: Some(v1_req_receiver),
			req_receiver: Some(req_receiver),
			metrics,
			rng,
			reputation: Default::default(),
		}
	}

	async fn run<Context>(self, ctx: Context) -> std::result::Result<(), FatalError> {
		self.run_inner(ctx, REPUTATION_CHANGE_INTERVAL).await
	}

	async fn run_inner<Context>(
		mut self,
		mut ctx: Context,
		reputation_interval: Duration,
	) -> std::result::Result<(), FatalError> {
		let new_reputation_delay = || futures_timer::Delay::new(reputation_interval).fuse();
		let mut reputation_delay = new_reputation_delay();

		let mut legacy_v1_state = crate::legacy_v1::State::new(self.keystore.clone());
		let mut state = crate::vstaging::State::new(self.keystore.clone());

		// Sender/Receiver for getting news from our statement fetching tasks.
		let (v1_req_sender, mut v1_req_receiver) = mpsc::channel(1);
		// Sender/Receiver for getting news from our responder task.
		let (v1_res_sender, mut v1_res_receiver) = mpsc::channel(1);

		let mut warn_freq = gum::Freq::new();

		ctx.spawn(
			"large-statement-responder",
			v1_respond_task(
				self.v1_req_receiver.take().expect("Mandatory argument to new. qed"),
				v1_res_sender.clone(),
			)
			.boxed(),
		)
		.map_err(FatalError::SpawnTask)?;

		// Sender/receiver for getting news from our candidate responder task.
		let (res_sender, mut res_receiver) = mpsc::channel(1);

		ctx.spawn(
			"candidate-responder",
			vstaging::respond_task(
				self.req_receiver.take().expect("Mandatory argument to new. qed"),
				res_sender.clone(),
			)
			.boxed(),
		)
		.map_err(FatalError::SpawnTask)?;

		loop {
			// Wait for the next message.
			let message = futures::select! {
				_ = reputation_delay => {
					self.reputation.send(ctx.sender()).await;
					reputation_delay = new_reputation_delay();
					continue
				},
				message = MuxedMessage::receive(
					&mut ctx,
					&mut state,
					&mut v1_req_receiver,
					&mut v1_res_receiver,
					&mut res_receiver,
				).fuse() => {
					message
				}
			};

			match message {
				MuxedMessage::Subsystem(result) => {
					let result = self
						.handle_subsystem_message(
							&mut ctx,
							&mut state,
							&mut legacy_v1_state,
							&v1_req_sender,
							result?,
						)
						.await;
					match result.into_nested()? {
						Ok(true) => break,
						Ok(false) => {},
						Err(jfyi) => gum::debug!(target: LOG_TARGET, error = ?jfyi),
					}
				},
				MuxedMessage::V1Requester(result) => {
					let result = crate::legacy_v1::handle_requester_message(
						&mut ctx,
						&mut legacy_v1_state,
						&v1_req_sender,
						&mut self.rng,
						result.ok_or(FatalError::RequesterReceiverFinished)?,
						&self.metrics,
						&mut self.reputation,
					)
					.await;
					log_error(
						result.map_err(From::from),
						"handle_requester_message",
						&mut warn_freq,
					)?;
				},
				MuxedMessage::V1Responder(result) => {
					let result = crate::legacy_v1::handle_responder_message(
						&mut legacy_v1_state,
						result.ok_or(FatalError::ResponderReceiverFinished)?,
					)
					.await;
					log_error(
						result.map_err(From::from),
						"handle_responder_message",
						&mut warn_freq,
					)?;
				},
				MuxedMessage::Responder(result) => {
					vstaging::answer_request(
						&mut state,
						result.ok_or(FatalError::RequesterReceiverFinished)?,
					);
				},
				MuxedMessage::Response(result) => {
					vstaging::handle_response(&mut ctx, &mut state, result, &mut self.reputation)
						.await;
				},
				MuxedMessage::RetryRequest(()) => {
					// A pending request is ready to retry. This is only a signal to call
					// `dispatch_requests` again.
					()
				},
			};

			vstaging::dispatch_requests(&mut ctx, &mut state).await;
		}
		Ok(())
	}

	async fn handle_subsystem_message<Context>(
		&mut self,
		ctx: &mut Context,
		state: &mut vstaging::State,
		legacy_v1_state: &mut legacy_v1::State,
		v1_req_sender: &mpsc::Sender<V1RequesterMessage>,
		message: FromOrchestra<StatementDistributionMessage>,
	) -> Result<bool> {
		let metrics = &self.metrics;

		match message {
			FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated,
				deactivated,
			})) => {
				let _timer = metrics.time_active_leaves_update();

				// vstaging should handle activated first because of implicit view.
				if let Some(ref activated) = activated {
					let mode = prospective_parachains_mode(ctx.sender(), activated.hash).await?;
					if let ProspectiveParachainsMode::Enabled { .. } = mode {
						vstaging::handle_active_leaves_update(ctx, state, activated, mode).await?;
					} else if let ProspectiveParachainsMode::Disabled = mode {
						for deactivated in &deactivated {
							crate::legacy_v1::handle_deactivate_leaf(legacy_v1_state, *deactivated);
						}

						crate::legacy_v1::handle_activated_leaf(
							ctx,
							legacy_v1_state,
							activated.clone(),
						)
						.await?;
					}
				} else {
					for deactivated in &deactivated {
						crate::legacy_v1::handle_deactivate_leaf(legacy_v1_state, *deactivated);
					}
					vstaging::handle_deactivate_leaves(state, &deactivated);
				}
			},
			FromOrchestra::Signal(OverseerSignal::BlockFinalized(..)) => {
				// do nothing
			},
			FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(true),
			FromOrchestra::Communication { msg } => match msg {
				StatementDistributionMessage::Share(relay_parent, statement) => {
					let _timer = metrics.time_share();

					// pass to legacy if legacy state contains head.
					if legacy_v1_state.contains_relay_parent(&relay_parent) {
						crate::legacy_v1::share_local_statement(
							ctx,
							legacy_v1_state,
							relay_parent,
							StatementWithPVD::drop_pvd_from_signed(statement),
							&mut self.rng,
							metrics,
						)
						.await?;
					} else {
						vstaging::share_local_statement(
							ctx,
							state,
							relay_parent,
							statement,
							&mut self.reputation,
						)
						.await?;
					}
				},
				StatementDistributionMessage::NetworkBridgeUpdate(event) => {
					// pass all events to both protocols except for messages,
					// which are filtered.
					enum VersionTarget {
						Legacy,
						Current,
						Both,
					}

					impl VersionTarget {
						fn targets_legacy(&self) -> bool {
							match self {
								&VersionTarget::Legacy | &VersionTarget::Both => true,
								_ => false,
							}
						}

						fn targets_current(&self) -> bool {
							match self {
								&VersionTarget::Current | &VersionTarget::Both => true,
								_ => false,
							}
						}
					}

					let target = match &event {
						NetworkBridgeEvent::PeerMessage(_, message) => match message {
							Versioned::VStaging(
								protocol_vstaging::StatementDistributionMessage::V1Compatibility(_),
							) => VersionTarget::Legacy,
							Versioned::V1(_) => VersionTarget::Legacy,
							Versioned::VStaging(_) => VersionTarget::Current,
						},
						_ => VersionTarget::Both,
					};

					if target.targets_legacy() {
						crate::legacy_v1::handle_network_update(
							ctx,
							legacy_v1_state,
							v1_req_sender,
							event.clone(),
							&mut self.rng,
							metrics,
							&mut self.reputation,
						)
						.await;
					}

					if target.targets_current() {
						// pass to vstaging.
						vstaging::handle_network_update(ctx, state, event, &mut self.reputation)
							.await;
					}
				},
				StatementDistributionMessage::Backed(candidate_hash) => {
					crate::vstaging::handle_backed_candidate_message(ctx, state, candidate_hash)
						.await;
				},
			},
		}
		Ok(false)
	}
}
