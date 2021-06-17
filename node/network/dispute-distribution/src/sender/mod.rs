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


/// For each ongoing dispute we have a `SendTask` which takes care of it.
///
/// It is going to spawn real tasks as it sees fit for getting the votes of the particular dispute
/// out.
mod send_task;
use send_task::SendTask;
pub use send_task::FromSendingTask;

/// Error and [`Result`] type for sender
mod error;
pub use error::{Result, Error, Fatal, NonFatal};

use polkadot_node_subsystem_util::runtime::RuntimeInfo;

use crate::LOG_TARGET;

/// Sending of disputes to all relevant validator nodes.
pub struct DisputeSender {
	/// All heads we currently consider active.
	active_heads: Vec<Hash>,

	/// All ongoing dispute sendings this subsystem is aware of.
	sendings: HashMap<CandidateHash, SendTask>,

	/// Sender to be cloned for `SendTask`s.
	tx: mpsc::Sender<FromSendingTask>,

	//// Receive messages from `SendTask`.
	// rx: mpsc::Receiver<FromSendingTask>,
}

impl DisputeSender
{
	pub fn new(tx: mpsc::Sender<FromSendingTask>) -> Self {
		Self {
			active_heads: Vec::new(),
			sendings: HashMap::new(),
			tx,
		}
	}

	/// Initiates sending a dispute message to peers.
	pub async fn start_sending<Context: SubsystemContext>(
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
				return Ok(())
			}
			Entry::Vacant(vacant) => {
				let send_task = SendTask::new(
					ctx,
					runtime,
					&self.active_heads,
					self.tx.clone(),
					req,
				).await?;
				vacant.insert(send_task);
			}
		}
		Ok(())
	}

	/// Receive message from a sending task.
	pub async fn on_task_message(&mut self, msg: FromSendingTask) {
		match msg {
			FromSendingTask::Finished(candidate_hash, authority, result) => {
				let task = match self.sendings.get_mut(&candidate_hash) {
					None => {
						// Can happen when a dispute ends, with messages still in queue:
						tracing::trace!(
							target: LOG_TARGET,
							?result,
							"Received `FromSendingTask::Finished` for non existing dispute."
						);
						return
					}
					Some(task) => task,
				};
				task.on_finished_send(&authority, result);
			}
		}
	}
}

/// Retrieve the currently active sessions.
async fn get_active_session_indeces<Context: SubsystemContext>(
	ctx: &mut Context,
	runtime: &mut RuntimeInfo,
	active_heads: &Vec<Hash>,
) -> Result<HashSet<SessionIndex>> {
	let mut indeces = HashSet::new();
	for head in active_heads {
		let session_index = runtime.get_session_index(ctx, *head).await?;
		indeces.insert(session_index);
	}
	Ok(indeces)
}
