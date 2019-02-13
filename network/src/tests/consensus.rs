// Copyright 2019 Parity Technologies (UK) Ltd.
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

//! Tests and helpers for consensus networking.

use consensus::{Knowledge, NetworkService};
use substrate_network::{consensus_gossip::ConsensusMessage, Context as NetContext};
use substrate_primitives::{Ed25519AuthorityId, NativeOrEncoded};
use router::Router;
use {PolkadotProtocol};

use polkadot_consensus::SharedTable;
use polkadot_primitives::{AccountId, Block, Hash, Header, BlockId};
use polkadot_primitives::parachain::{Id as ParaId, DutyRoster, ParachainHost};
use parking_lot::Mutex;
use substrate_client::error::Result as ClientResult;
use substrate_client::runtime_api::{Core, RuntimeVersion, ApiExt};
use sr_primitives::ExecutionContext;
use sr_primitives::traits::{ApiRef, ProvideRuntimeApi};

use std::sync::Arc;
use futures::{prelude::*, sync::mpsc};

use super::TestContext;

struct GossipRouter {
	incoming_messages: mpsc::UnboundedReceiver<(Hash, ConsensusMessage)>,
	incoming_streams: mpsc::UnboundedReceiver<(Hash, mpsc::UnboundedSender<ConsensusMessage>)>,
	outgoing: Vec<(Hash, mpsc::UnboundedSender<ConsensusMessage>)>,
	messages: Vec<(Hash, ConsensusMessage)>,
}

impl GossipRouter {
	fn add_message(&mut self, topic: Hash, message: ConsensusMessage) {
		self.outgoing.retain(|&(ref o_topic, ref sender)| {
			o_topic != &topic || sender.unbounded_send(message.clone()).is_ok()
		});
		self.messages.push((topic, message));
	}

	fn add_outgoing(&mut self, topic: Hash, sender: mpsc::UnboundedSender<ConsensusMessage>) {
		for message in self.messages.iter()
			.filter(|&&(ref t, _)| t == &topic)
			.map(|&(_, ref msg)| msg.clone())
		{
			if let Err(_) = sender.unbounded_send(message) { return }
		}

		self.outgoing.push((topic, sender));
	}
}

impl Future for GossipRouter {
	type Item = ();
	type Error = ();

	fn poll(&mut self) -> Poll<(), ()> {
		loop {
			match self.incoming_messages.poll().unwrap() {
				Async::Ready(Some((topic, message))) => self.add_message(topic, message),
				Async::Ready(None) => panic!("ended early."),
				Async::NotReady => break,
			}
		}

		loop {
			match self.incoming_streams.poll().unwrap() {
				Async::Ready(Some((topic, sender))) => self.add_outgoing(topic, sender),
				Async::Ready(None) => panic!("ended early."),
				Async::NotReady => break,
			}
		}

		Ok(Async::NotReady)
	}
}


#[derive(Clone)]
struct GossipHandle {
	send_message: mpsc::UnboundedSender<(Hash, ConsensusMessage)>,
	send_listener: mpsc::UnboundedSender<(Hash, mpsc::UnboundedSender<ConsensusMessage>)>,
}

fn make_gossip() -> (GossipRouter, GossipHandle) {
	let (message_tx, message_rx) = mpsc::unbounded();
	let (listener_tx, listener_rx) = mpsc::unbounded();

	(
		GossipRouter {
			incoming_messages: message_rx,
			incoming_streams: listener_rx,
			outgoing: Vec::new(),
			messages: Vec::new(),
		},
		GossipHandle { send_message: message_tx, send_listener: listener_tx },
	)
}

struct Network {
	proto: Arc<Mutex<PolkadotProtocol>>,
	gossip: GossipHandle,
}

impl NetworkService for Network {
	fn gossip_messages_for(&self, topic: Hash) -> mpsc::UnboundedReceiver<ConsensusMessage> {
		let (tx, rx) = mpsc::unbounded();
		let _  = self.gossip.send_listener.unbounded_send((topic, tx));
		rx
	}

	fn gossip_message(&self, topic: Hash, message: ConsensusMessage, _broadcast: bool) {
		let _ = self.gossip.send_message.unbounded_send((topic, message));
	}

	fn drop_gossip(&self, _topic: Hash) {}

	fn with_spec<F: Send + 'static>(&self, with: F)
		where F: FnOnce(&mut PolkadotProtocol, &mut NetContext<Block>)
	{
		let mut context = TestContext::default();
		let res = with(&mut *self.proto.lock(), &mut context);
		// TODO: send context to worker for message routing.
		res
	}
}

#[derive(Clone)]
struct TestApi;

struct RuntimeApi {
	inner: TestApi,
}

impl ProvideRuntimeApi for TestApi {
	type Api = RuntimeApi;

	fn runtime_api<'a>(&'a self) -> ApiRef<'a, Self::Api> {
		RuntimeApi { inner: self.clone() }.into()
	}
}

impl Core<Block> for RuntimeApi {
	fn version_runtime_api_impl(
		&self,
		_: &BlockId,
		_: ExecutionContext,
		_: Option<()>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<RuntimeVersion>> {
		unimplemented!("Not required for testing!")
	}

	fn authorities_runtime_api_impl(
		&self,
		_: &BlockId,
		_: ExecutionContext,
		_: Option<()>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Vec<Ed25519AuthorityId>>> {
		unimplemented!("Not required for testing!")
	}

	fn execute_block_runtime_api_impl(
		&self,
		_: &BlockId,
		_: ExecutionContext,
		_: Option<(Block)>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<()>> {
		unimplemented!("Not required for testing!")
	}

	fn initialise_block_runtime_api_impl(
		&self,
		_: &BlockId,
		_: ExecutionContext,
		_: Option<&Header>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<()>> {
		unimplemented!("Not required for testing!")
	}
}

impl ApiExt<Block> for RuntimeApi {
	fn map_api_result<F: FnOnce(&Self) -> Result<R, E>, R, E>(
		&self,
		_: F
	) -> Result<R, E> {
		unimplemented!("Not required for testing!")
	}

	fn runtime_version_at(&self, _: &BlockId) -> ClientResult<RuntimeVersion> {
		unimplemented!("Not required for testing!")
	}
}

impl ParachainHost<Block> for RuntimeApi {
	fn validators_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<()>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Vec<AccountId>>> {
		Ok(NativeOrEncoded::Native(Vec::new()))
	}

	fn duty_roster_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<()>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<DutyRoster>> {
		Ok(NativeOrEncoded::Native(DutyRoster {
			validator_duty: Vec::new(),
		}))
	}

	fn active_parachains_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<()>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Vec<ParaId>>> {
		Ok(NativeOrEncoded::Native(Vec::new()))
	}

	fn parachain_head_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<ParaId>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<Vec<u8>>>> {
		Ok(NativeOrEncoded::Native(None))
	}

	fn parachain_code_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<ParaId>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<Vec<u8>>>> {
		Ok(NativeOrEncoded::Native(None))
	}

	fn ingress_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<ParaId>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<Vec<(ParaId, Hash)>>>> {
		Ok(NativeOrEncoded::Native(None))
	}
}
