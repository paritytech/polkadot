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

//! Tests and helpers for validation networking.

use validation::NetworkService;
use substrate_network::Context as NetContext;
use substrate_primitives::{NativeOrEncoded, ExecutionContext};
use substrate_keyring::AuthorityKeyring;
use {PolkadotProtocol};

use polkadot_validation::{SharedTable, MessagesFrom, Network};
use polkadot_primitives::{SessionKey, Block, Hash, Header, BlockId};
use polkadot_primitives::parachain::{
	Id as ParaId, Chain, DutyRoster, ParachainHost, OutgoingMessage,
	ValidatorId, ConsolidatedIngressRoots,
};
use parking_lot::Mutex;
use substrate_client::error::Result as ClientResult;
use substrate_client::runtime_api::{Core, RuntimeVersion, ApiExt};
use sr_primitives::traits::{ApiRef, ProvideRuntimeApi};

use std::collections::HashMap;
use std::sync::Arc;
use futures::{prelude::*, sync::mpsc};
use tokio::runtime::{Runtime, TaskExecutor};

use super::TestContext;

#[derive(Clone, Copy)]
struct NeverExit;

impl Future for NeverExit {
	type Item = ();
	type Error = ();

	fn poll(&mut self) -> Poll<(), ()> {
		Ok(Async::NotReady)
	}
}

struct GossipRouter {
	incoming_messages: mpsc::UnboundedReceiver<(Hash, Vec<u8>)>,
	incoming_streams: mpsc::UnboundedReceiver<(Hash, mpsc::UnboundedSender<Vec<u8>>)>,
	outgoing: Vec<(Hash, mpsc::UnboundedSender<Vec<u8>>)>,
	messages: Vec<(Hash, Vec<u8>)>,
}

impl GossipRouter {
	fn add_message(&mut self, topic: Hash, message: Vec<u8>) {
		self.outgoing.retain(|&(ref o_topic, ref sender)| {
			o_topic != &topic || sender.unbounded_send(message.clone()).is_ok()
		});
		self.messages.push((topic, message));
	}

	fn add_outgoing(&mut self, topic: Hash, sender: mpsc::UnboundedSender<Vec<u8>>) {
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
	send_message: mpsc::UnboundedSender<(Hash, Vec<u8>)>,
	send_listener: mpsc::UnboundedSender<(Hash, mpsc::UnboundedSender<Vec<u8>>)>,
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

struct TestNetwork {
	proto: Arc<Mutex<PolkadotProtocol>>,
	gossip: GossipHandle,
}

impl NetworkService for TestNetwork {
	fn gossip_messages_for(&self, topic: Hash) -> mpsc::UnboundedReceiver<Vec<u8>> {
		let (tx, rx) = mpsc::unbounded();
		let _  = self.gossip.send_listener.unbounded_send((topic, tx));
		rx
	}

	fn gossip_message(&self, topic: Hash, message: Vec<u8>) {
		let _ = self.gossip.send_message.unbounded_send((topic, message));
	}

	fn drop_gossip(&self, _topic: Hash) {}

	fn with_spec<F: Send + 'static>(&self, with: F)
		where F: FnOnce(&mut PolkadotProtocol, &mut NetContext<Block>)
	{
		let mut context = TestContext::default();
		let res = with(&mut *self.proto.lock(), &mut context);
		// TODO: send context to worker for message routing.
		// https://github.com/paritytech/polkadot/issues/215
		res
	}
}

#[derive(Default)]
struct ApiData {
	validators: Vec<ValidatorId>,
	duties: Vec<Chain>,
	active_parachains: Vec<ParaId>,
	ingress: HashMap<ParaId, ConsolidatedIngressRoots>,
}

#[derive(Default, Clone)]
struct TestApi {
	data: Arc<Mutex<ApiData>>,
}

struct RuntimeApi {
	data: Arc<Mutex<ApiData>>,
}

impl ProvideRuntimeApi for TestApi {
	type Api = RuntimeApi;

	fn runtime_api<'a>(&'a self) -> ApiRef<'a, Self::Api> {
		RuntimeApi { data: self.data.clone() }.into()
	}
}

impl Core<Block> for RuntimeApi {
	fn Core_version_runtime_api_impl(
		&self,
		_: &BlockId,
		_: ExecutionContext,
		_: Option<()>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<RuntimeVersion>> {
		unimplemented!("Not required for testing!")
	}

	fn Core_authorities_runtime_api_impl(
		&self,
		_: &BlockId,
		_: ExecutionContext,
		_: Option<()>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Vec<SessionKey>>> {
		unimplemented!("Not required for testing!")
	}

	fn Core_execute_block_runtime_api_impl(
		&self,
		_: &BlockId,
		_: ExecutionContext,
		_: Option<(Block)>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<()>> {
		unimplemented!("Not required for testing!")
	}

	fn Core_initialize_block_runtime_api_impl(
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
	fn ParachainHost_validators_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<()>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Vec<ValidatorId>>> {
		Ok(NativeOrEncoded::Native(self.data.lock().validators.clone()))
	}

	fn ParachainHost_duty_roster_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<()>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<DutyRoster>> {

		Ok(NativeOrEncoded::Native(DutyRoster {
			validator_duty: self.data.lock().duties.clone(),
		}))
	}

	fn ParachainHost_active_parachains_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<()>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Vec<ParaId>>> {
		Ok(NativeOrEncoded::Native(self.data.lock().active_parachains.clone()))
	}

	fn ParachainHost_parachain_head_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<ParaId>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<Vec<u8>>>> {
		Ok(NativeOrEncoded::Native(Some(Vec::new())))
	}

	fn ParachainHost_parachain_code_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<ParaId>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<Vec<u8>>>> {
		Ok(NativeOrEncoded::Native(Some(Vec::new())))
	}

	fn ParachainHost_ingress_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		id: Option<ParaId>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<ConsolidatedIngressRoots>>> {
		let id = id.unwrap();
		Ok(NativeOrEncoded::Native(self.data.lock().ingress.get(&id).cloned()))
	}
}

type TestValidationNetwork = ::validation::ValidationNetwork<
	TestApi,
	NeverExit,
	TestNetwork,
	TaskExecutor,
>;

struct Built {
	gossip: GossipRouter,
	api_handle: Arc<Mutex<ApiData>>,
	networks: Vec<TestValidationNetwork>,
}

fn build_network(n: usize, executor: TaskExecutor) -> Built {
	let (gossip_router, gossip_handle) = make_gossip();
	let api_handle = Arc::new(Mutex::new(Default::default()));
	let runtime_api = Arc::new(TestApi { data: api_handle.clone() });

	let networks = (0..n).map(|_| {
		let net = Arc::new(TestNetwork {
			proto: Arc::new(Mutex::new(PolkadotProtocol::new(None))),
			gossip: gossip_handle.clone(),
		});

		let message_val = crate::gossip::RegisteredMessageValidator::new_test(
			|_hash: &_| Some(crate::gossip::Known::Leaf),
		);

		TestValidationNetwork::new(
			net,
			NeverExit,
			message_val,
			runtime_api.clone(),
			executor.clone(),
		)
	});

	let networks: Vec<_> = networks.collect();

	Built {
		gossip: gossip_router,
		api_handle,
		networks,
	}
}

#[derive(Default)]
struct IngressBuilder {
	egress: HashMap<(ParaId, ParaId), Vec<Vec<u8>>>,
}

impl IngressBuilder {
	fn add_messages(&mut self, source: ParaId, messages: &[OutgoingMessage]) {
		for message in messages {
			let target = message.target;
			self.egress.entry((source, target)).or_insert_with(Vec::new).push(message.data.clone());
		}
	}

	fn build(self) -> HashMap<ParaId, ConsolidatedIngressRoots> {
		let mut map = HashMap::new();
		for ((source, target), messages) in self.egress {
			map.entry(target).or_insert_with(Vec::new)
				.push((source, polkadot_validation::message_queue_root(&messages)));
		}

		for roots in map.values_mut() {
			roots.sort_by_key(|&(para_id, _)| para_id);
		}

		map.into_iter().map(|(k, v)| (k, ConsolidatedIngressRoots(v))).collect()
	}
}

fn make_table(data: &ApiData, local_key: &AuthorityKeyring, parent_hash: Hash) -> Arc<SharedTable> {
	use ::av_store::Store;

	let store = Store::new_in_memory();
	let (group_info, _) = ::polkadot_validation::make_group_info(
		DutyRoster { validator_duty: data.duties.clone() },
		&data.validators, // only possible as long as parachain crypto === aura crypto
		SessionKey::from(*local_key)
	).unwrap();

	Arc::new(SharedTable::new(
		group_info,
		Arc::new(local_key.pair()),
		parent_hash,
		store,
	))
}

#[test]
fn ingress_fetch_works() {
	let mut runtime = Runtime::new().unwrap();
	let built = build_network(3, runtime.executor());

	let id_a: ParaId = 1.into();
	let id_b: ParaId = 2.into();
	let id_c: ParaId = 3.into();

	let key_a = AuthorityKeyring::Alice;
	let key_b = AuthorityKeyring::Bob;
	let key_c = AuthorityKeyring::Charlie;

	let messages_from_a = vec![
		OutgoingMessage { target: id_b, data: vec![1, 2, 3] },
		OutgoingMessage { target: id_b, data: vec![3, 4, 5] },
		OutgoingMessage { target: id_c, data: vec![9, 9, 9] },
	];

	let messages_from_b = vec![
		OutgoingMessage { target: id_a, data: vec![1, 1, 1, 1, 1,] },
		OutgoingMessage { target: id_c, data: b"hello world".to_vec() },
	];

	let messages_from_c = vec![
		OutgoingMessage { target: id_a, data: b"dog42".to_vec() },
		OutgoingMessage { target: id_b, data: b"dogglesworth".to_vec() },
	];

	let ingress = {
		let mut builder = IngressBuilder::default();
		builder.add_messages(id_a, &messages_from_a);
		builder.add_messages(id_b, &messages_from_b);
		builder.add_messages(id_c, &messages_from_c);

		builder.build()
	};

	let parent_hash = [1; 32].into();

	let (router_a, router_b, router_c) = {
		let validators: Vec<ValidatorId> = vec![
			key_a.into(),
			key_b.into(),
			key_c.into(),
		];

		// NOTE: this is possible only because we are currently asserting that parachain validators
		// share their crypto with the (Aura) authority set. Once that assumption breaks, so will this
		// code.
		let authorities = validators.clone();

		let mut api_handle = built.api_handle.lock();
		*api_handle = ApiData {
			active_parachains: vec![id_a, id_b, id_c],
			duties: vec![Chain::Parachain(id_a), Chain::Parachain(id_b), Chain::Parachain(id_c)],
			validators,
			ingress,
		};

		(
			built.networks[0].communication_for(
				make_table(&*api_handle, &key_a, parent_hash),
				vec![MessagesFrom::from_messages(id_a, messages_from_a)],
				&authorities,
			),
			built.networks[1].communication_for(
				make_table(&*api_handle, &key_b, parent_hash),
				vec![MessagesFrom::from_messages(id_b, messages_from_b)],
				&authorities,
			),
			built.networks[2].communication_for(
				make_table(&*api_handle, &key_c, parent_hash),
				vec![MessagesFrom::from_messages(id_c, messages_from_c)],
				&authorities,
			),
		)
	};

	// make sure everyone can get ingress for their own parachain.
	let fetch_a = router_a.then(move |r| r.unwrap().fetcher()
		.fetch_incoming(id_a).map_err(|_| format!("Could not fetch ingress_a")));
	let fetch_b = router_b.then(move |r| r.unwrap().fetcher()
		.fetch_incoming(id_b).map_err(|_| format!("Could not fetch ingress_b")));
	let fetch_c = router_c.then(move |r| r.unwrap().fetcher()
		.fetch_incoming(id_c).map_err(|_| format!("Could not fetch ingress_c")));

	let work = fetch_a.join3(fetch_b, fetch_c);
	runtime.spawn(built.gossip.then(|_| Ok(()))); // in background.
	runtime.block_on(work).unwrap();
}
