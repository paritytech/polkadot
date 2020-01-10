// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

#![allow(unused)]

use crate::gossip::GossipMessage;
use sc_network::{Context as NetContext, PeerId};
use sc_network_gossip::TopicNotification;
use sp_core::{NativeOrEncoded, ExecutionContext};
use sp_keyring::Sr25519Keyring;
use crate::{PolkadotProtocol, NetworkService, GossipMessageStream};

use polkadot_validation::{SharedTable, Network};
use polkadot_primitives::{Block, BlockNumber, Hash, Header, BlockId};
use polkadot_primitives::parachain::{
	Id as ParaId, Chain, DutyRoster, ParachainHost, TargetedMessage,
	ValidatorId, StructuredUnroutedIngress, BlockIngressRoots, Status,
	FeeSchedule, HeadData, Retriable, CollatorId, ErasureChunk, CandidateReceipt,
};
use parking_lot::Mutex;
use sp_blockchain::Result as ClientResult;
use sp_api::{ApiRef, Core, RuntimeVersion, StorageProof, ApiErrorExt, ApiExt, ProvideRuntimeApi};
use sp_runtime::traits::Block as BlockT;

use std::collections::HashMap;
use std::sync::Arc;
use std::pin::Pin;
use std::task::{Poll, Context};
use futures::{prelude::*, channel::mpsc, future::{select, Either}};
use codec::Encode;

use super::{TestContext, TestChainContext};

type TaskExecutor = Arc<dyn futures::task::Spawn + Send + Sync>;

#[derive(Clone, Copy)]
struct NeverExit;

impl Future for NeverExit {
	type Output = ();

	fn poll(self: Pin<&mut Self>, _: &mut Context) -> Poll<Self::Output> {
		Poll::Pending
	}
}

fn clone_gossip(n: &TopicNotification) -> TopicNotification {
	TopicNotification {
		message: n.message.clone(),
		sender: n.sender.clone(),
	}
}

async fn gossip_router(
	mut incoming_messages: mpsc::UnboundedReceiver<(Hash, TopicNotification)>,
	mut incoming_streams: mpsc::UnboundedReceiver<(Hash, mpsc::UnboundedSender<TopicNotification>)>
) {
	let mut outgoing: Vec<(Hash, mpsc::UnboundedSender<TopicNotification>)> = Vec::new();
	let mut messages = Vec::new();

	loop {
		match select(incoming_messages.next(), incoming_streams.next()).await {
			Either::Left((Some((topic, message)), _)) => {
				outgoing.retain(|&(ref o_topic, ref sender)| {
					o_topic != &topic || sender.unbounded_send(clone_gossip(&message)).is_ok()
				});
				messages.push((topic, message));
			},
			Either::Right((Some((topic, sender)), _)) => {
				for message in messages.iter()
					.filter(|&&(ref t, _)| t == &topic)
					.map(|&(_, ref msg)| clone_gossip(msg))
				{
					if let Err(_) = sender.unbounded_send(message) { return }
				}

				outgoing.push((topic, sender));
			},
			Either::Left((None, _)) | Either::Right((None, _)) =>  panic!("ended early.")
		}
	}
}

#[derive(Clone)]
struct GossipHandle {
	send_message: mpsc::UnboundedSender<(Hash, TopicNotification)>,
	send_listener: mpsc::UnboundedSender<(Hash, mpsc::UnboundedSender<TopicNotification>)>,
}

fn make_gossip() -> (impl Future<Output = ()>, GossipHandle) {
	let (message_tx, message_rx) = mpsc::unbounded();
	let (listener_tx, listener_rx) = mpsc::unbounded();

	(
		gossip_router(message_rx, listener_rx),
		GossipHandle { send_message: message_tx, send_listener: listener_tx },
	)
}

struct TestNetwork {
	proto: Arc<Mutex<PolkadotProtocol>>,
	gossip: GossipHandle,
}

impl NetworkService for TestNetwork {
	fn gossip_messages_for(&self, topic: Hash) -> GossipMessageStream {
		let (tx, rx) = mpsc::unbounded();
		let _  = self.gossip.send_listener.unbounded_send((topic, tx));
		GossipMessageStream::new(rx.boxed())
	}

	fn send_message(&self, _: PeerId, _: GossipMessage) {
		unimplemented!()
	}

	fn gossip_message(&self, topic: Hash, message: GossipMessage) {
		let notification = TopicNotification { message: message.encode(), sender: None };
		let _ = self.gossip.send_message.unbounded_send((topic, notification));
	}

	fn with_spec<F: Send + 'static>(&self, with: F)
		where F: FnOnce(&mut PolkadotProtocol, &mut dyn NetContext<Block>)
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
	active_parachains: Vec<(ParaId, Option<(CollatorId, Retriable)>)>,
	ingress: HashMap<ParaId, StructuredUnroutedIngress>,
}

#[derive(Default, Clone)]
struct TestApi {
	data: Arc<Mutex<ApiData>>,
}

struct RuntimeApi {
	data: Arc<Mutex<ApiData>>,
}

impl ProvideRuntimeApi<Block> for TestApi {
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

impl ApiErrorExt for RuntimeApi {
	type Error = sp_blockchain::Error;
}

impl ApiExt<Block> for RuntimeApi {
	type StateBackend = sp_state_machine::InMemoryBackend<sp_api::HasherFor<Block>>;

	fn map_api_result<F: FnOnce(&Self) -> Result<R, E>, R, E>(
		&self,
		_: F
	) -> Result<R, E> {
		unimplemented!("Not required for testing!")
	}

	fn runtime_version_at(&self, _: &BlockId) -> ClientResult<RuntimeVersion> {
		unimplemented!("Not required for testing!")
	}

	fn record_proof(&mut self) { }

	fn extract_proof(&mut self) -> Option<StorageProof> {
		None
	}

	fn into_storage_changes<
		T: sp_api::ChangesTrieStorage<sp_api::HasherFor<Block>, sp_api::NumberFor<Block>>
	>(
		&self,
		_: &Self::StateBackend,
		_: Option<&T>,
		_: <Block as sp_api::BlockT>::Hash,
	) -> std::result::Result<sp_api::StorageChanges<Self::StateBackend, Block>, String>
		where Self: Sized
	{
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
	) -> ClientResult<NativeOrEncoded<Vec<(ParaId, Option<(CollatorId, Retriable)>)>>> {
		Ok(NativeOrEncoded::Native(self.data.lock().active_parachains.clone()))
	}

	fn ParachainHost_parachain_status_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<ParaId>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<Status>>> {
		Ok(NativeOrEncoded::Native(Some(Status {
			head_data: HeadData(Vec::new()),
			balance: 0,
			fee_schedule: FeeSchedule {
				base: 0,
				per_byte: 0,
			}
		})))
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
		id: Option<(ParaId, Option<BlockNumber>)>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<StructuredUnroutedIngress>>> {
		let (id, _) = id.unwrap();
		Ok(NativeOrEncoded::Native(self.data.lock().ingress.get(&id).cloned()))
	}

	fn ParachainHost_get_heads_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_extrinsics: Option<Vec<<Block as BlockT>::Extrinsic>>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<Vec<CandidateReceipt>>>> {
		Ok(NativeOrEncoded::Native(Some(Vec::new())))
	}
}

type TestValidationNetwork = crate::validation::ValidationNetwork<
	TestApi,
	NeverExit,
	TaskExecutor,
>;

struct Built {
	gossip: Pin<Box<dyn Future<Output = ()>>>,
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
			TestChainContext::default(),
			Box::new(|_, _| {}),
		);

		TestValidationNetwork::new(
			message_val,
			NeverExit,
			runtime_api.clone(),
			executor.clone(),
		)
	});

	let networks: Vec<_> = networks.collect();

	Built {
		gossip: gossip_router.boxed(),
		api_handle,
		networks,
	}
}

#[derive(Default)]
struct IngressBuilder {
	egress: HashMap<(ParaId, ParaId), Vec<Vec<u8>>>,
}

impl IngressBuilder {
	fn add_messages(&mut self, source: ParaId, messages: &[TargetedMessage]) {
		for message in messages {
			let target = message.target;
			self.egress.entry((source, target)).or_insert_with(Vec::new).push(message.data.clone());
		}
	}

	fn build(self) -> HashMap<ParaId, BlockIngressRoots> {
		let mut map = HashMap::new();
		for ((source, target), messages) in self.egress {
			map.entry(target).or_insert_with(Vec::new)
				.push((source, polkadot_validation::message_queue_root(&messages)));
		}

		for roots in map.values_mut() {
			roots.sort_by_key(|&(para_id, _)| para_id);
		}

		map.into_iter().map(|(k, v)| (k, BlockIngressRoots(v))).collect()
	}
}

#[derive(Clone)]
struct DummyGossipMessages;

use futures::stream;
impl av_store::ProvideGossipMessages for DummyGossipMessages {
	fn gossip_messages_for(
		&self,
		_topic: Hash
	) -> Pin<Box<dyn futures::Stream<Item = (Hash, Hash, ErasureChunk)> + Send>> {
		stream::empty().boxed()
	}

	fn gossip_erasure_chunk(
		&self,
		_relay_parent: Hash,
		_candidate_hash: Hash,
		_erasure_root: Hash,
		_chunk: ErasureChunk,
	) {}
}

fn make_table(data: &ApiData, local_key: &Sr25519Keyring, parent_hash: Hash) -> Arc<SharedTable> {
	use av_store::Store;
	use sp_core::crypto::Pair;

	let sr_pair = local_key.pair();
	let local_key = polkadot_primitives::parachain::ValidatorPair::from(local_key.pair());
	let store = Store::new_in_memory(DummyGossipMessages);
	let (group_info, _) = ::polkadot_validation::make_group_info(
		DutyRoster { validator_duty: data.duties.clone() },
		&data.validators, // only possible as long as parachain crypto === aura crypto
		Some(sr_pair.public().into()),
	).unwrap();

	Arc::new(SharedTable::new(
		data.validators.clone(),
		group_info,
		Some(Arc::new(local_key)),
		parent_hash,
		store,
		None,
	))
}
