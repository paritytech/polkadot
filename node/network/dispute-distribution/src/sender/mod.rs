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


use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::Entry;

use futures::Future;
use futures::channel::mpsc;
use futures::future::RemoteHandle;

use polkadot_node_network_protocol::IfDisconnected;
use polkadot_node_network_protocol::request_response::OutgoingRequest;
use polkadot_node_network_protocol::request_response::OutgoingResult;
use polkadot_node_network_protocol::request_response::Recipient;
use polkadot_node_network_protocol::request_response::Requests;
use polkadot_node_network_protocol::request_response::v1::DisputeResponse;
use polkadot_node_primitives::DisputeMessage;
use polkadot_primitives::v1::{SessionIndex, AuthorityDiscoveryId};
use polkadot_subsystem::messages::AllMessages;
use polkadot_subsystem::messages::NetworkBridgeMessage;
use polkadot_node_network_protocol::request_response::v1::DisputeRequest;
use polkadot_primitives::v1::CandidateHash;
use polkadot_primitives::v1::Hash;
use polkadot_subsystem::SubsystemContext;

/// Error and [`Result`] type for sender
mod error;
use error::Fatal;
use error::Result;

use polkadot_node_subsystem_util::runtime::RuntimeInfo;

use crate::LOG_TARGET;

use self::error::NonFatal;

/// Sending of disputes to all relevant validator nodes.
pub struct DisputeSender {
	/// All heads we currently consider active.
	active_heads: Vec<Hash>,

	/// All ongoing dispute sendings this subsystem is aware of.
	sendings: HashMap<CandidateHash, DisputeInfo>,

	/// Sender to be cloned for `SendTask`s.
	tx: mpsc::Sender<FromSendingTask>,

	/// Receive messages from `SendTask`.
	rx: mpsc::Receiver<FromSendingTask>,
}

/// Dispute state for a particular disputed candidate.
struct DisputeInfo {
	/// The request we are supposed to get out to all parachain validators of the dispute's session
	/// and to all current authorities.
	request: DisputeRequest,
	/// The set of authorities we need to send our messages to. This set will change at session
	/// boundaries. It will always be at least the parachain validators of the session where the
	/// dispute happened and the authorities of the current sessions as determined by active heads.
	deliveries: HashMap<AuthorityDiscoveryId, DeliveryStatus>,
}

/// Status of a particular vote/statement delivery to a particular validator.
enum DeliveryStatus {
	/// Request is still in flight.
	Pending(RemoteHandle<()>),
	/// Request failed - waiting for retry.
	Failed,
	/// Succeeded - no need to send request to this peer anymore.
	Succeeded,
}

/// Messages from tasks trying to get disputes delievered.
enum FromSendingTask {
	/// Delivery of statements for given candidate succeeded for this authority.
	Succeeded(CandidateHash, AuthorityDiscoveryId),
	/// Delivery of statements for given candidate failed for this authority.
	///
	/// We should retry.
	Failed(CandidateHash, AuthorityDiscoveryId),
}

impl DisputeSender
{
	/// Initiates sending a dispute message to peers.
	async fn start_send_dispute<Context>(
		&mut self,
		ctx: &mut Context, 
		runtime: &mut RuntimeInfo,
		msg: DisputeMessage,
	) -> Result<()> {
		let req: DisputeRequest = msg.into();
		match self.sendings.entry(req.0.candidate_hash) {
			Entry::Occupied(_) => {
				tracing::warn!(
					target: LOG_TARGET,
					candidate_hash = ?req.0.candidate_hash,
					"Double dispute participation - not supposed to happen."
				);
			}
			Entry::Vacant(vacant) => {
				let mut dispute_info = DisputeInfo {
					request: req,
					deliveries: HashMap::new(),
				};
				self.update_dispute_sending(
					ctx,
					runtime,
					&mut dispute_info,
				).await?;
				vacant.insert(dispute_info);
			}
		}
		Ok(())
	}

	/// Initiate requests for notifying nodes about a dispute to all concerned authorities.
	///
	/// Any missing tasks will be added/started and obsolete tasks will be killed.
	async fn update_dispute_sending<Context>(
		&self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		dispute_info: &mut DisputeInfo
	) -> Result<()> {
		let req = &dispute_info.request;
		let new_authorities = self.get_relevant_validators(ctx, runtime, &req).await?;
		let old_authorities = dispute_info.deliveries.keys();

		let add_authorities = new_authorities
			.difference(old_authorities)
			.collect();

		// Get rid of dead/irrelevant tasks/statuses:
		dispute_info.deliveries.retain(|k, _| new_authorities.contains(k));

		// Start any new tasks that are needed:
		let new_statuses = send_requests(
			ctx,
			self.tx.clone(),
			add_authorities,
			req.clone(),
		).await?;

		dispute_info.delivieres.extend(new_statuses.into_iter());
	}


	/// Determine all validators that should receive the given dispute requests.
	///
	/// This is all parachain validators of the session the candidate occurred and all authorities
	/// of all currently active sessions, determined by currently active heads.
	async fn get_relevant_validators<Context: SubsystemContext>(
		&self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		req: &DisputeRequest,
	) -> Result<HashSet<AuthorityDiscoveryId>> {
		// We need some relay chain head for context for receiving session info information:
		let ref_head = self.active_heads.get(0).ok_or(NonFatal::NoActiveHeads)?;
		// Parachain validators:
		let info = runtime.get_session_info_by_index(ref_head, req.session_index).await?;
		let session_info = info.session_info;
		let validator_count = session_info.validators.len();
		let mut authorities: HashSet<_> = session_info
			.discovery_keys
			.iter()
			.take(validator_count)
			.enumerate()
			.filter(|(i, _)| Some(i) != info.validator_info.our_index)
			.map(|(_, v)| v)
			.collect();

		// Current authorities:
		for head in self.active_heads {
			let info = runtime.get_session_info(head).await?;
			let session_info = info.session_info;
			let new_set = session_info
				.discovery_keys
				.iter()
				.enumerate()
				.filter(|(i, _)| Some(i) != info.validator_info.our_index)
				.map(|(_, v)| v);
			authorities.extend(new_set);
		}
		authorities
	}

	/// Retrieve the currently active sessions.
	async fn get_active_session_indeces<Context: SubsystemContext>(
		&self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
	) -> HashSet<SessionIndex> {
		let mut indeces = Vec::new();
		for head in self.active_heads {
			let session_index = runtime.get_session_index(ctx, head).await?;
			indeces.push(session_index);
		}
		Ok(indeces)
	}
}


/// Start sending of the given msg to all given authorities.
async fn send_requests<Context>(
	ctx: &mut Context,
	tx: mpsc::Sender<FromSendingTask>,
	receivers: Vec<AuthorityDiscoveryId>,
	req: DisputeRequest,
) -> Result<HashMap<AuthorityDiscoveryId, DeliveryStatus>> {
	let mut statuses = HashMap::with_capacity(receivers.len());
	let mut reqs = Vec::with_capacity(receivers.len());

	for receiver in receivers {
		let (outgoing, pending_response) = OutgoingRequest::new(
			Recipient::AuthorityDiscoveryId(receiver),
			req,
		);

		reqs.push(Requests::DisputeSending(outgoing));

		let receiver = wait_response_task(
			pending_response,
			&req.candidate_hash,
			&receiver,
			tx,
		);

		let (remote, remote_handle) = receiver.remote_handle();
		ctx.spawn("dispute-sender", remote)
			.await
			.map_err(Fatal::SpawnTask)?;
		statuses.insert(receiver, DeliveryStatus::Pending(remote_handle));
	}

	let msg = NetworkBridgeMessage::SendRequests(
		reqs,
		// We should be connected, but the hell - if not, try!
		IfDisconnected::TryConnect,
	);
	ctx.send_message(AllMessages::NetworkBridge(msg)).await;
	Ok(statuses)
}

/// Future to be spawned in a task for awaiting a response.
async fn wait_response_task(
	pending_response: impl Future<Output = OutgoingResult<DisputeResponse>>,
	candidate_hash: CandidateHash,
	receiver: AuthorityDiscoveryId,
	tx: mpsc::Sender<FromSendingTask>,
) {
	let result = pending_response.await;
	let msg = match result {
		Err(err) => {
			tracing::warn!(
				target: LOG_TARGET,
				%candidate_hash,
				%receiver,
				%err,
				"Error sending dispute statements to node."
			);
			FromSendingTask::Failed(candidate_hash, receiver)
		}
		Ok(DisputeResponse::Confirmed) => {
			tracing::trace!(
				target: LOG_TARGET,
				%candidate_hash,
				%receiver,
				"Sending dispute message succeeded"
			);
			FromSendingTask::Succeeded(candidate_hash, receiver)
		}
	};
	if let Err(err) = tx.seed(msg).await {
		tracing::debug!(
			target: LOG_TARGET,
			%err,
			"Failed to notify susystem about dispute sending result."
		);
	}
}
