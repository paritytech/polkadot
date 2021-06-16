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


mod send_task;
use send_task::SendTask;
pub use send_task::FromSendingTask;

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
	async fn start_send_dispute<Context: SubsystemContext>(
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
}

/// Retrieve the currently active sessions.
async fn get_active_session_indeces<Context: SubsystemContext>(
	ctx: &mut Context,
	runtime: &mut RuntimeInfo,
	active_heads: &Vec<Hash>,
) -> HashSet<SessionIndex> {
	let mut indeces = HashSet::new();
	for head in active_heads {
		let session_index = runtime.get_session_index(ctx, head).await?;
		indeces.insert(session_index);
	}
	Ok(indeces)
}
