// Copyright 2021 Parity Technologies (UK) Ltd.
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

#![deny(unused_crate_dependencies)]
#![warn(missing_docs)]

use error::{log_error, FatalResult};

use polkadot_node_network_protocol::{
	request_response::{v1 as request_v1, IncomingRequestReceiver},
	vstaging as protocol_vstaging, Versioned,
};
use polkadot_node_primitives::StatementWithPVD;
use polkadot_node_subsystem::{
	messages::{NetworkBridgeEvent, StatementDistributionMessage},
	overseer, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, SpawnedSubsystem, SubsystemError,
};
use polkadot_node_subsystem_util::rand;

use futures::{channel::mpsc, prelude::*};
use sp_keystore::SyncCryptoStorePtr;

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
	keystore: SyncCryptoStorePtr,
	/// Receiver for incoming large statement requests.
	v1_req_receiver: Option<IncomingRequestReceiver<request_v1::StatementFetchingRequest>>,
	/// Prometheus metrics
	metrics: Metrics,
	/// Pseudo-random generator for peers selection logic
	rng: R,
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
}

#[overseer::contextbounds(StatementDistribution, prefix = self::overseer)]
impl MuxedMessage {
	async fn receive<Context>(
		ctx: &mut Context,
		from_v1_requester: &mut mpsc::Receiver<V1RequesterMessage>,
		from_v1_responder: &mut mpsc::Receiver<V1ResponderMessage>,
	) -> MuxedMessage {
		// We are only fusing here to make `select` happy, in reality we will quit if one of those
		// streams end:
		let from_orchestra = ctx.recv().fuse();
		let from_v1_requester = from_v1_requester.next();
		let from_v1_responder = from_v1_responder.next();
		futures::pin_mut!(from_orchestra, from_v1_requester, from_v1_responder);
		futures::select! {
			msg = from_orchestra => MuxedMessage::Subsystem(msg.map_err(FatalError::SubsystemReceive)),
			msg = from_v1_requester => MuxedMessage::V1Requester(msg),
			msg = from_v1_responder => MuxedMessage::V1Responder(msg),
		}
	}
}

#[overseer::contextbounds(StatementDistribution, prefix = self::overseer)]
impl<R: rand::Rng> StatementDistributionSubsystem<R> {
	/// Create a new Statement Distribution Subsystem
	pub fn new(
		keystore: SyncCryptoStorePtr,
		v1_req_receiver: IncomingRequestReceiver<request_v1::StatementFetchingRequest>,
		metrics: Metrics,
		rng: R,
	) -> Self {
		Self { keystore, v1_req_receiver: Some(v1_req_receiver), metrics, rng }
	}

	async fn run<Context>(mut self, mut ctx: Context) -> std::result::Result<(), FatalError> {
		let mut legacy_v1_state = crate::legacy_v1::State::new(self.keystore.clone());

		// Sender/Receiver for getting news from our statement fetching tasks.
		let (v1_req_sender, mut v1_req_receiver) = mpsc::channel(1);
		// Sender/Receiver for getting news from our responder task.
		let (v1_res_sender, mut v1_res_receiver) = mpsc::channel(1);

		ctx.spawn(
			"large-statement-responder",
			v1_respond_task(
				self.v1_req_receiver.take().expect("Mandatory argument to new. qed"),
				v1_res_sender.clone(),
			)
			.boxed(),
		)
		.map_err(FatalError::SpawnTask)?;

		loop {
			let message =
				MuxedMessage::receive(&mut ctx, &mut v1_req_receiver, &mut v1_res_receiver).await;
			match message {
				MuxedMessage::Subsystem(result) => {
					let result = self
						.handle_subsystem_message(
							&mut ctx,
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
					)
					.await;
					log_error(result.map_err(From::from), "handle_requester_message")?;
				},
				MuxedMessage::V1Responder(result) => {
					let result = crate::legacy_v1::handle_responder_message(
						&mut legacy_v1_state,
						result.ok_or(FatalError::ResponderReceiverFinished)?,
					)
					.await;
					log_error(result.map_err(From::from), "handle_responder_message")?;
				},
			};
		}
		Ok(())
	}

	async fn handle_subsystem_message<Context>(
		&mut self,
		ctx: &mut Context,
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

				// TODO [now]: vstaging should handle activated first
				// because of implicit view.
				for deactivated in deactivated {
					crate::legacy_v1::handle_deactivate_leaf(legacy_v1_state, deactivated);
				}

				if let Some(activated) = activated {
					// TODO [now]: legacy, activate only if no prospective parachains support.
					crate::legacy_v1::handle_activated_leaf(ctx, legacy_v1_state, activated)
						.await?;
				}
			},
			FromOrchestra::Signal(OverseerSignal::BlockFinalized(..)) => {
				// do nothing
			},
			FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(true),
			FromOrchestra::Communication { msg } =>
				match msg {
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
						}
					},
					StatementDistributionMessage::NetworkBridgeUpdate(event) => {
						// pass to legacy, but not if the message isn't
						// v1.
						let legacy = match &event {
							&NetworkBridgeEvent::PeerMessage(_, ref message) => match message {
								Versioned::VStaging(protocol_vstaging::StatementDistributionMessage::V1Compatibility(_)) => true,
								Versioned::V1(_) => true,
								Versioned::VStaging(_) => false,
							},
							_ => true,
						};

						if legacy {
							crate::legacy_v1::handle_network_update(
								ctx,
								legacy_v1_state,
								v1_req_sender,
								event,
								&mut self.rng,
								metrics,
							)
							.await;
						}

						// TODO [now]: pass to vstaging, but not if the message is
						// v1 or the connecting peer is v1.
					},
					StatementDistributionMessage::Backed { para_id, para_head } => {
						// TODO [now]: pass to vstaging
					},
				},
		}
		Ok(false)
	}
}
