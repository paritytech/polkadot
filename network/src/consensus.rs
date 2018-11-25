// Copyright 2017 Parity Technologies (UK) Ltd.
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

//! The "consensus" networking code built on top of the base network service.
//!
//! This fulfills the `polkadot_consensus::Network` trait, providing a hook to be called
//! each time consensus begins on a new chain head.

use sr_primitives::traits::ProvideRuntimeApi;
use substrate_network::consensus_gossip::ConsensusMessage;
use polkadot_consensus::{Network, SharedTable, Collators};
use polkadot_primitives::{AccountId, Block, Hash, SessionKey};
use polkadot_primitives::parachain::{Id as ParaId, Collation, ParachainHost};
use codec::Decode;

use futures::prelude::*;
use futures::sync::mpsc;

use std::sync::Arc;

use tokio::runtime::TaskExecutor;
use parking_lot::Mutex;

use super::{NetworkService, Knowledge, CurrentConsensus};
use router::Router;

// task that processes all gossipped consensus messages,
// checking signatures
struct MessageProcessTask<P> {
	inner_stream: mpsc::UnboundedReceiver<ConsensusMessage>,
	parent_hash: Hash,
	table_router: Router<P>,
}

impl<P: ProvideRuntimeApi + Send + Sync + 'static> MessageProcessTask<P>
	where P::Api: ParachainHost<Block>,
{
	fn process_message(&self, msg: ConsensusMessage) -> Option<Async<()>> {
		use polkadot_consensus::SignedStatement;

		debug!(target: "consensus", "Processing consensus statement for live consensus");
		if let Some(statement) = SignedStatement::decode(&mut msg.as_slice()) {
			if ::polkadot_consensus::check_statement(
				&statement.statement,
				&statement.signature,
				statement.sender,
				&self.parent_hash
			) {
				self.table_router.import_statement(statement);
			}
		}

		None
	}
}

impl<P: ProvideRuntimeApi + Send + Sync + 'static> Future for MessageProcessTask<P>
	where P::Api: ParachainHost<Block>,
{
	type Item = ();
	type Error = ();

	fn poll(&mut self) -> Poll<(), ()> {
		loop {
			match self.inner_stream.poll() {
				Ok(Async::Ready(Some(val))) => if let Some(async) = self.process_message(val) {
					return Ok(async);
				},
				Ok(Async::Ready(None)) => return Ok(Async::Ready(())),
				Ok(Async::NotReady) => return Ok(Async::NotReady),
				Err(e) => debug!(target: "p_net", "Error getting consensus message: {:?}", e),
			}
		}
	}
}

/// Wrapper around the network service
pub struct ConsensusNetwork<P> {
	network: Arc<NetworkService>,
	api: Arc<P>,
}

impl<P> ConsensusNetwork<P> {
	/// Create a new consensus networking object.
	pub fn new(network: Arc<NetworkService>, api: Arc<P>) -> Self {
		ConsensusNetwork { network, api }
	}
}

impl<P> Clone for ConsensusNetwork<P> {
	fn clone(&self) -> Self {
		ConsensusNetwork {
			network: self.network.clone(),
			api: self.api.clone(),
		}
	}
}

/// A long-lived network which can create parachain statement  routing processes on demand.
impl<P: ProvideRuntimeApi + Send + Sync + 'static> Network for ConsensusNetwork<P>
	where P::Api: ParachainHost<Block>,
{
	type TableRouter = Router<P>;

	/// Instantiate a table router using the given shared table.
	fn communication_for(
		&self,
		_validators: &[SessionKey],
		table: Arc<SharedTable>,
		task_executor: TaskExecutor
	) -> Self::TableRouter {
		let parent_hash = table.consensus_parent_hash().clone();

		let knowledge = Arc::new(Mutex::new(Knowledge::new()));

		let local_session_key = table.session_key();
		let table_router = Router::new(
			table,
			self.network.clone(),
			self.api.clone(),
			task_executor.clone(),
			parent_hash,
			knowledge.clone(),
		);

		let attestation_topic = table_router.gossip_topic();

		// spin up a task in the background that processes all incoming statements
		// TODO: propagate statements on a timer?
		let inner_stream = self.network.consensus_gossip().write().messages_for(attestation_topic);
		task_executor.spawn(self.network.with_spec(|spec, ctx| {
			spec.new_consensus(ctx, CurrentConsensus {
				knowledge,
				parent_hash,
				local_session_key,
			});

			MessageProcessTask {
				inner_stream,
				parent_hash,
				table_router: table_router.clone(),
			}
		}));

		table_router
	}
}

/// Error when the network appears to be down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkDown;

/// A future that resolves when a collation is received.
pub struct AwaitingCollation(::futures::sync::oneshot::Receiver<Collation>);

impl Future for AwaitingCollation {
	type Item = Collation;
	type Error = NetworkDown;

	fn poll(&mut self) -> Poll<Collation, NetworkDown> {
		self.0.poll().map_err(|_| NetworkDown)
	}
}


impl<P: ProvideRuntimeApi + Send + Sync + 'static> Collators for ConsensusNetwork<P>
	where P::Api: ParachainHost<Block>,
{
	type Error = NetworkDown;
	type Collation = AwaitingCollation;

	fn collate(&self, parachain: ParaId, relay_parent: Hash) -> Self::Collation {
		AwaitingCollation(
			self.network.with_spec(|spec, _| spec.await_collation(relay_parent, parachain))
		)
	}

	fn note_bad_collator(&self, collator: AccountId) {
		self.network.with_spec(|spec, ctx| spec.disconnect_bad_collator(ctx, collator));
	}
}
