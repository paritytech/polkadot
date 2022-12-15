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

use std::collections::{HashMap, HashSet};

use futures::{channel::mpsc, future::RemoteHandle, Future, FutureExt, SinkExt};

use polkadot_node_network_protocol::{
	request_response::{
		outgoing::RequestError,
		v1::{DisputeRequest, DisputeResponse},
		OutgoingRequest, OutgoingResult, Recipient, Requests,
	},
	IfDisconnected,
};
use polkadot_node_subsystem::{messages::NetworkBridgeTxMessage, overseer};
use polkadot_node_subsystem_util::{metrics, runtime::RuntimeInfo};
use polkadot_primitives::v2::{
	AuthorityDiscoveryId, CandidateHash, Hash, SessionIndex, ValidatorIndex,
};

use super::error::{FatalError, Result};

use crate::{
	metrics::{FAILED, SUCCEEDED},
	Metrics, LOG_TARGET,
};

/// Delivery status for a particular dispute.
///
/// Keeps track of all the validators that have to be reached for a dispute.
///
/// The unit of work for a `SendTask` is an authority/validator.
pub struct SendTask {
	/// The request we are supposed to get out to all `parachain` validators of the dispute's session
	/// and to all current authorities.
	request: DisputeRequest,

	/// The set of authorities we need to send our messages to. This set will change at session
	/// boundaries. It will always be at least the `parachain` validators of the session where the
	/// dispute happened and the authorities of the current sessions as determined by active heads.
	deliveries: HashMap<AuthorityDiscoveryId, DeliveryStatus>,

	/// Whether we have any tasks failed since the last refresh.
	has_failed_sends: bool,

	/// Sender to be cloned for tasks.
	tx: mpsc::Sender<TaskFinish>,
}

/// Status of a particular vote/statement delivery to a particular validator.
enum DeliveryStatus {
	/// Request is still in flight.
	Pending(RemoteHandle<()>),
	/// Succeeded - no need to send request to this peer anymore.
	Succeeded,
}

/// A sending task finishes with this result:
#[derive(Debug)]
pub struct TaskFinish {
	/// The candidate this task was running for.
	pub candidate_hash: CandidateHash,
	/// The authority the request was sent to.
	pub receiver: AuthorityDiscoveryId,
	/// The result of the delivery attempt.
	pub result: TaskResult,
}

#[derive(Debug)]
pub enum TaskResult {
	/// Task succeeded in getting the request to its peer.
	Succeeded,
	/// Task was not able to get the request out to its peer.
	///
	/// It should be retried in that case.
	Failed(RequestError),
}

impl TaskResult {
	pub fn as_metrics_label(&self) -> &'static str {
		match self {
			Self::Succeeded => SUCCEEDED,
			Self::Failed(_) => FAILED,
		}
	}
}

#[overseer::contextbounds(DisputeDistribution, prefix = self::overseer)]
impl SendTask {
	/// Initiates sending a dispute message to peers.
	///
	/// Creation of new `SendTask`s is subject to rate limiting. As each `SendTask` will trigger
	/// sending a message to each validator, hence for employing a per-peer rate limit, we need to
	/// limit the construction of new `SendTask`s.
	pub async fn new<Context>(
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		active_sessions: &HashMap<SessionIndex, Hash>,
		tx: mpsc::Sender<TaskFinish>,
		request: DisputeRequest,
		metrics: &Metrics,
	) -> Result<Self> {
		let mut send_task =
			Self { request, deliveries: HashMap::new(), has_failed_sends: false, tx };
		send_task.refresh_sends(ctx, runtime, active_sessions, metrics).await?;
		Ok(send_task)
	}

	/// Make sure we are sending to all relevant authorities.
	///
	/// This function is called at construction and should also be called whenever a session change
	/// happens and on a regular basis to ensure we are retrying failed attempts.
	///
	/// This might resend to validators and is thus subject to any rate limiting we might want.
	/// Calls to this function for different instances should be rate limited according to
	/// `SEND_RATE_LIMIT`.
	///
	/// Returns: `True` if this call resulted in new requests.
	pub async fn refresh_sends<Context>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		active_sessions: &HashMap<SessionIndex, Hash>,
		metrics: &Metrics,
	) -> Result<bool> {
		let new_authorities = self.get_relevant_validators(ctx, runtime, active_sessions).await?;

		// Note this will also contain all authorities for which sending failed previously:
		let add_authorities: Vec<_> = new_authorities
			.iter()
			.filter(|a| !self.deliveries.contains_key(a))
			.map(Clone::clone)
			.collect();

		// Get rid of dead/irrelevant tasks/statuses:
		gum::trace!(
			target: LOG_TARGET,
			already_running_deliveries = ?self.deliveries.len(),
			"Cleaning up deliveries"
		);
		self.deliveries.retain(|k, _| new_authorities.contains(k));

		// Start any new tasks that are needed:
		gum::trace!(
			target: LOG_TARGET,
			new_and_failed_authorities = ?add_authorities.len(),
			overall_authority_set_size = ?new_authorities.len(),
			already_running_deliveries = ?self.deliveries.len(),
			"Starting new send requests for authorities."
		);
		let new_statuses =
			send_requests(ctx, self.tx.clone(), add_authorities, self.request.clone(), metrics)
				.await?;

		let was_empty = new_statuses.is_empty();
		gum::trace!(
			target: LOG_TARGET,
			sent_requests = ?new_statuses.len(),
			"Requests dispatched."
		);

		self.has_failed_sends = false;
		self.deliveries.extend(new_statuses.into_iter());
		Ok(!was_empty)
	}

	/// Whether any sends have failed since the last refresh.
	pub fn has_failed_sends(&self) -> bool {
		self.has_failed_sends
	}

	/// Handle a finished response waiting task.
	///
	/// Called by `DisputeSender` upon reception of the corresponding message from our spawned `wait_response_task`.
	pub fn on_finished_send(&mut self, authority: &AuthorityDiscoveryId, result: TaskResult) {
		match result {
			TaskResult::Failed(err) => {
				gum::trace!(
					target: LOG_TARGET,
					?authority,
					candidate_hash = %self.request.0.candidate_receipt.hash(),
					%err,
					"Error sending dispute statements to node."
				);

				self.has_failed_sends = true;
				// Remove state, so we know what to try again:
				self.deliveries.remove(authority);
			},
			TaskResult::Succeeded => {
				let status = match self.deliveries.get_mut(&authority) {
					None => {
						// Can happen when a sending became irrelevant while the response was already
						// queued.
						gum::debug!(
							target: LOG_TARGET,
							candidate = ?self.request.0.candidate_receipt.hash(),
							?authority,
							?result,
							"Received `FromSendingTask::Finished` for non existing task."
						);
						return
					},
					Some(status) => status,
				};
				// We are done here:
				*status = DeliveryStatus::Succeeded;
			},
		}
	}

	/// Determine all validators that should receive the given dispute requests.
	///
	/// This is all `parachain` validators of the session the candidate occurred and all authorities
	/// of all currently active sessions, determined by currently active heads.
	async fn get_relevant_validators<Context>(
		&self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		active_sessions: &HashMap<SessionIndex, Hash>,
	) -> Result<HashSet<AuthorityDiscoveryId>> {
		let ref_head = self.request.0.candidate_receipt.descriptor.relay_parent;
		// Retrieve all authorities which participated in the parachain consensus of the session
		// in which the candidate was backed.
		let info = runtime
			.get_session_info_by_index(ctx.sender(), ref_head, self.request.0.session_index)
			.await?;
		let session_info = &info.session_info;
		let validator_count = session_info.validators.len();
		let mut authorities: HashSet<_> = session_info
			.discovery_keys
			.iter()
			.take(validator_count)
			.enumerate()
			.filter(|(i, _)| Some(ValidatorIndex(*i as _)) != info.validator_info.our_index)
			.map(|(_, v)| v.clone())
			.collect();

		// Retrieve all authorities for the current session as indicated by the active
		// heads we are tracking.
		for (session_index, head) in active_sessions.iter() {
			let info =
				runtime.get_session_info_by_index(ctx.sender(), *head, *session_index).await?;
			let session_info = &info.session_info;
			let new_set = session_info
				.discovery_keys
				.iter()
				.enumerate()
				.filter(|(i, _)| Some(ValidatorIndex(*i as _)) != info.validator_info.our_index)
				.map(|(_, v)| v.clone());
			authorities.extend(new_set);
		}
		Ok(authorities)
	}
}

/// Start sending of the given message to all given authorities.
///
/// And spawn tasks for handling the response.
#[overseer::contextbounds(DisputeDistribution, prefix = self::overseer)]
async fn send_requests<Context>(
	ctx: &mut Context,
	tx: mpsc::Sender<TaskFinish>,
	receivers: Vec<AuthorityDiscoveryId>,
	req: DisputeRequest,
	metrics: &Metrics,
) -> Result<HashMap<AuthorityDiscoveryId, DeliveryStatus>> {
	let mut statuses = HashMap::with_capacity(receivers.len());
	let mut reqs = Vec::with_capacity(receivers.len());

	for receiver in receivers {
		let (outgoing, pending_response) =
			OutgoingRequest::new(Recipient::Authority(receiver.clone()), req.clone());

		reqs.push(Requests::DisputeSendingV1(outgoing));

		let fut = wait_response_task(
			pending_response,
			req.0.candidate_receipt.hash(),
			receiver.clone(),
			tx.clone(),
			metrics.time_dispute_request(),
		);

		let (remote, remote_handle) = fut.remote_handle();
		ctx.spawn("dispute-sender", remote.boxed()).map_err(FatalError::SpawnTask)?;
		statuses.insert(receiver, DeliveryStatus::Pending(remote_handle));
	}

	let msg = NetworkBridgeTxMessage::SendRequests(reqs, IfDisconnected::ImmediateError);
	ctx.send_message(msg).await;
	Ok(statuses)
}

/// Future to be spawned in a task for awaiting a response.
async fn wait_response_task(
	pending_response: impl Future<Output = OutgoingResult<DisputeResponse>>,
	candidate_hash: CandidateHash,
	receiver: AuthorityDiscoveryId,
	mut tx: mpsc::Sender<TaskFinish>,
	_timer: Option<metrics::prometheus::prometheus::HistogramTimer>,
) {
	let result = pending_response.await;
	let msg = match result {
		Err(err) => TaskFinish { candidate_hash, receiver, result: TaskResult::Failed(err) },
		Ok(DisputeResponse::Confirmed) =>
			TaskFinish { candidate_hash, receiver, result: TaskResult::Succeeded },
	};
	if let Err(err) = tx.feed(msg).await {
		gum::debug!(
			target: LOG_TARGET,
			%err,
			"Failed to notify subsystem about dispute sending result."
		);
	}
}
