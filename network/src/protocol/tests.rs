// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

//! Tests for the protocol.

use super::*;
use parking_lot::Mutex;

use polkadot_primitives::{Block, Header, BlockId};
use polkadot_primitives::parachain::{
	Id as ParaId, Chain, DutyRoster, ParachainHost, ValidatorId,
	Retriable, CollatorId, AbridgedCandidateReceipt,
	GlobalValidationSchedule, LocalValidationData, ErasureChunk,
};
use polkadot_validation::SharedTable;

use av_store::{Store as AvailabilityStore, ErasureNetworking};
use sc_network_gossip::TopicNotification;
use sp_blockchain::Result as ClientResult;
use sp_api::{ApiRef, Core, RuntimeVersion, StorageProof, ApiErrorExt, ApiExt, ProvideRuntimeApi};
use sp_runtime::traits::{Block as BlockT, HashFor, NumberFor};
use sp_state_machine::ChangesTrieState;
use sp_core::{crypto::Pair, NativeOrEncoded, ExecutionContext};
use sp_keyring::Sr25519Keyring;

use futures::executor::LocalPool;
use futures::task::LocalSpawnExt;

#[derive(Default)]
struct MockNetworkOps {
	recorded: Mutex<Recorded>,
}

#[derive(Default)]
struct Recorded {
	peer_reputations: HashMap<PeerId, i32>,
	notifications: Vec<(PeerId, Message)>,
}

// Test setup registers receivers of gossip messages as well as signals that
// fire when they are taken.
type GossipStreamEntry = (mpsc::UnboundedReceiver<TopicNotification>, oneshot::Sender<()>);

#[derive(Default, Clone)]
struct MockGossip {
	inner: Arc<Mutex<HashMap<Hash, GossipStreamEntry>>>,
}

impl MockGossip {
	fn add_gossip_stream(&self, topic: Hash)
		-> (mpsc::UnboundedSender<TopicNotification>, oneshot::Receiver<()>)
	{
		let (tx, rx) = mpsc::unbounded();
		let (o_tx, o_rx) = oneshot::channel();
		self.inner.lock().insert(topic, (rx, o_tx));
		(tx, o_rx)
	}

	fn contains_listener(&self, topic: &Hash) -> bool {
		self.inner.lock().contains_key(topic)
	}
}

impl NetworkServiceOps for MockNetworkOps {
	fn report_peer(&self, peer: PeerId, value: sc_network::ReputationChange) {
		let mut recorded = self.recorded.lock();
		let total_rep = recorded.peer_reputations.entry(peer).or_insert(0);

		*total_rep = total_rep.saturating_add(value.value);
	}

	fn write_notification(
		&self,
		peer: PeerId,
		engine_id: ConsensusEngineId,
		notification: Vec<u8>,
	) {
		assert_eq!(engine_id, POLKADOT_ENGINE_ID);
		let message = Message::decode(&mut &notification[..]).expect("invalid notification");
		self.recorded.lock().notifications.push((peer, message));
	}
}

impl crate::legacy::GossipService for MockGossip {
	fn gossip_messages_for(&self, topic: Hash) -> crate::legacy::GossipMessageStream {
		crate::legacy::GossipMessageStream::new(match self.inner.lock().remove(&topic) {
			None => Box::pin(stream::empty()),
			Some((rx, o_rx)) => {
				let _ = o_rx.send(());
				Box::pin(rx)
			}
		})
	}

	fn gossip_message(&self, _topic: Hash, _message: GossipMessage) {

	}

	fn send_message(&self, _who: PeerId, _message: GossipMessage) {

	}
}

impl GossipOps for MockGossip {
	fn new_local_leaf(
		&self,
		_relay_parent: Hash,
		_validation_data: crate::legacy::gossip::MessageValidationData,
	) -> crate::legacy::gossip::NewLeafActions {
		crate::legacy::gossip::NewLeafActions::new()
	}

	fn register_availability_store(
		&self,
		_store: av_store::Store,
	) {

	}
}

#[derive(Default)]
struct ApiData {
	validators: Vec<ValidatorId>,
	duties: Vec<Chain>,
	active_parachains: Vec<(ParaId, Option<(CollatorId, Retriable)>)>,
}

#[derive(Default, Clone)]
struct TestApi {
	data: Arc<Mutex<ApiData>>,
}

#[derive(Default)]
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
		_: Option<Block>,
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
	type StateBackend = sp_state_machine::InMemoryBackend<sp_api::HashFor<Block>>;

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

	fn into_storage_changes(
		&self,
		_: &Self::StateBackend,
		_: Option<&ChangesTrieState<HashFor<Block>, NumberFor<Block>>>,
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

	fn ParachainHost_parachain_code_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<ParaId>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<Vec<u8>>>> {
		Ok(NativeOrEncoded::Native(Some(Vec::new())))
	}

	fn ParachainHost_global_validation_schedule_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<()>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<GlobalValidationSchedule>> {
		Ok(NativeOrEncoded::Native(Default::default()))
	}

	fn ParachainHost_local_validation_data_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_: Option<ParaId>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<LocalValidationData>>> {
		Ok(NativeOrEncoded::Native(Some(Default::default())))
	}

	fn ParachainHost_get_heads_runtime_api_impl(
		&self,
		_at: &BlockId,
		_: ExecutionContext,
		_extrinsics: Option<Vec<<Block as BlockT>::Extrinsic>>,
		_: Vec<u8>,
	) -> ClientResult<NativeOrEncoded<Option<Vec<AbridgedCandidateReceipt>>>> {
		Ok(NativeOrEncoded::Native(Some(Vec::new())))
	}
}

impl super::Service {
	async fn connect_peer(&mut self, peer: PeerId, roles: Roles) {
		self.sender.send(ServiceToWorkerMsg::PeerConnected(peer, roles)).await.unwrap();
	}

	async fn peer_message(&mut self, peer: PeerId, message: Message) {
		let bytes = message.encode().into();

		self.sender.send(ServiceToWorkerMsg::PeerMessage(peer, vec![bytes])).await.unwrap();
	}

	async fn disconnect_peer(&mut self, peer: PeerId) {
		self.sender.send(ServiceToWorkerMsg::PeerDisconnected(peer)).await.unwrap();
	}

	async fn synchronize<T: Send + 'static>(
		&mut self,
		callback: impl FnOnce(&mut ProtocolHandler) -> T + Send + 'static,
	) -> T {
		let (tx, rx) = oneshot::channel();

		let msg = ServiceToWorkerMsg::Synchronize(Box::new(move |proto| {
			let res = callback(proto);
			if let Err(_) = tx.send(res) {
				log::warn!(target: "p_net", "Failed to send synchronization result");
			}
		}));

		self.sender.send(msg).await.expect("Worker thread unexpectedly hung up");
		rx.await.expect("Worker thread failed to send back result")
	}
}

fn test_setup(config: Config) -> (
	Service,
	MockGossip,
	LocalPool,
	impl Future<Output = ()> + 'static,
) {
	let pool = LocalPool::new();

	let network_ops = Arc::new(MockNetworkOps::default());
	let mock_gossip = MockGossip::default();
	let (worker_tx, worker_rx) = mpsc::channel(0);
	let api = Arc::new(TestApi::default());

	let worker_task = worker_loop(
		config,
		network_ops.clone(),
		mock_gossip.clone(),
		api.clone(),
		worker_rx,
		pool.spawner(),
	);

	let service = Service {
		sender: worker_tx,
		network_service: network_ops,
	};

	(service, mock_gossip, pool, worker_task)
}

#[test]
fn router_inner_drop_sends_worker_message() {
	let parent = [1; 32].into();

	let (sender, mut receiver) = mpsc::channel(0);
	drop(RouterInner {
		relay_parent: parent,
		sender,
	});

	match receiver.try_next() {
		Ok(Some(ServiceToWorkerMsg::DropConsensusNetworking(x))) => assert_eq!(parent, x),
		_ => panic!("message not sent"),
	}
}

#[test]
fn worker_task_shuts_down_when_sender_dropped() {
	let (service, _gossip, mut pool, worker_task) = test_setup(Config { collating_for: None });

	drop(service);
	let _ = pool.run_until(worker_task);
}

#[test]
fn consensus_instances_cleaned_up() {
	let (mut service, _gossip, mut pool, worker_task) = test_setup(Config { collating_for: None });
	let relay_parent = [0; 32].into();
	let authorities = Vec::new();

	let table = Arc::new(SharedTable::new(
		Vec::new(),
		HashMap::new(),
		None,
		relay_parent,
		AvailabilityStore::new_in_memory(service.clone()),
		None,
	));

	pool.spawner().spawn_local(worker_task).unwrap();

	let router = pool.run_until(
		service.build_table_router(table, &authorities)
	).unwrap();

	drop(router);

	assert!(pool.run_until(service.synchronize(move |proto| {
		!proto.consensus_instances.contains_key(&relay_parent)
	})));
}

#[test]
fn validator_peer_cleaned_up() {
	let (mut service, _gossip, mut pool, worker_task) = test_setup(Config { collating_for: None });

	let peer = PeerId::random();
	let validator_key = Sr25519Keyring::Alice.pair();
	let validator_id = ValidatorId::from(validator_key.public());

	pool.spawner().spawn_local(worker_task).unwrap();
	pool.run_until(async move {
		service.connect_peer(peer.clone(), Roles::AUTHORITY).await;
		service.peer_message(peer.clone(), Message::Status(Status {
			version: VERSION,
			collating_for: None,
		})).await;
		service.peer_message(peer.clone(), Message::ValidatorId(validator_id.clone())).await;

		let p = peer.clone();
		let v = validator_id.clone();
		let (peer_has_key, reverse_lookup) = service.synchronize(move |proto| {
			let peer_has_key = proto.peers.get(&p).map_or(
				false,
				|p_data| p_data.session_keys.as_slice().contains(&v),
			);

			let reverse_lookup = proto.connected_validators.get(&v).map_or(
				false,
				|reps| reps.contains(&p),
			);

			(peer_has_key, reverse_lookup)
		}).await;

		assert!(peer_has_key);
		assert!(reverse_lookup);

		service.disconnect_peer(peer.clone()).await;

		let p = peer.clone();
		let v = validator_id.clone();
		let (peer_removed, rev_removed) = service.synchronize(move |proto| {
			let peer_removed = !proto.peers.contains_key(&p);
			let reverse_mapping_removed = !proto.connected_validators.contains_key(&v);

			(peer_removed, reverse_mapping_removed)
		}).await;

		assert!(peer_removed);
		assert!(rev_removed);
	});
}

#[test]
fn validator_key_spillover_cleaned() {
	let (mut service, _gossip, mut pool, worker_task) = test_setup(Config { collating_for: None });

	let peer = PeerId::random();
	let make_validator_id = |ring: Sr25519Keyring| ValidatorId::from(ring.public());

	// We will push 1 extra beyond what is normally kept.
	assert_eq!(RECENT_SESSIONS, 3);
	let key_a = make_validator_id(Sr25519Keyring::Alice);
	let key_b = make_validator_id(Sr25519Keyring::Bob);
	let key_c = make_validator_id(Sr25519Keyring::Charlie);
	let key_d = make_validator_id(Sr25519Keyring::Dave);

	let keys = vec![key_a, key_b, key_c, key_d];

	pool.spawner().spawn_local(worker_task).unwrap();
	pool.run_until(async move {
		service.connect_peer(peer.clone(), Roles::AUTHORITY).await;
		service.peer_message(peer.clone(), Message::Status(Status {
			version: VERSION,
			collating_for: None,
		})).await;

		for key in &keys {
			service.peer_message(peer.clone(), Message::ValidatorId(key.clone())).await;
		}

		let p = peer.clone();
		let active_keys = keys[1..].to_vec();
		let discarded_key = keys[0].clone();
		assert!(service.synchronize(move |proto| {
			let active_correct = proto.peers.get(&p).map_or(false, |p_data| {
				p_data.session_keys.as_slice() == &active_keys[..]
			});

			let active_lookup = active_keys.iter().all(|k| {
				proto.connected_validators.get(&k).map_or(false, |m| m.contains(&p))
			});

			let discarded = !proto.connected_validators.contains_key(&discarded_key);

			active_correct && active_lookup && discarded
		}).await);
	});
}

#[test]
fn erasure_fetch_drop_also_drops_gossip_sender() {
	let (service, gossip, mut pool, worker_task) = test_setup(Config { collating_for: None });
	let candidate_hash = [1; 32].into();

	let expected_index = 1;

	let spawner = pool.spawner();

	spawner.spawn_local(worker_task).unwrap();
	let topic = crate::erasure_coding_topic(&candidate_hash);
	let (mut gossip_tx, gossip_taken_rx) = gossip.add_gossip_stream(topic);

	let test_work = async move {
		let chunk_listener = service.fetch_erasure_chunk(
			&candidate_hash,
			expected_index,
		);

		// spawn an abortable handle to the chunk listener future.
		// we will wait until this future has proceeded enough to start grabbing
		// messages from gossip, and then we will abort the future.
		let (chunk_listener, abort_handle) = future::abortable(chunk_listener);
		let handle = spawner.spawn_with_handle(chunk_listener).unwrap();
		gossip_taken_rx.await.unwrap();

		// gossip listener was taken. and is active.
		assert!(!gossip.contains_listener(&topic));
		assert!(!gossip_tx.is_closed());

		abort_handle.abort();

		// we must `await` this, otherwise context may never transfer over
		// to the spawned `Abortable` future.
		assert!(handle.await.is_err());
		loop {
			// if dropping the sender leads to the gossip listener
			// being cleaned up, we will eventually be unable to send a message
			// on the sender.
			if gossip_tx.is_closed() { break }

			let fake_chunk = GossipMessage::ErasureChunk(
				crate::legacy::gossip::ErasureChunkMessage {
					chunk: ErasureChunk {
						chunk: vec![],
						index: expected_index + 1,
						proof: vec![],
					},
					candidate_hash,
				}
			).encode();

			match gossip_tx.send(TopicNotification { message: fake_chunk, sender: None }).await {
				Err(e) => { assert!(e.is_disconnected()); break },
				Ok(_) => continue,
			}
		}
	};

	pool.run_until(test_work);
}
