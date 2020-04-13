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
use crate::legacy::gossip::GossipPoVBlock;
use parking_lot::Mutex;

use polkadot_primitives::Block;
use polkadot_primitives::parachain::{
	Id as ParaId, Chain, DutyRoster, ParachainHost, ValidatorId,
	Retriable, CollatorId, AbridgedCandidateReceipt,
	GlobalValidationSchedule, LocalValidationData, ErasureChunk, SigningContext,
	PoVBlock, BlockData, ValidationCode,
};
use polkadot_validation::{SharedTable, TableRouter};

use av_store::{Store as AvailabilityStore, ErasureNetworking};
use sc_network_gossip::TopicNotification;
use sp_api::{ApiRef, ProvideRuntimeApi};
use sp_runtime::traits::Block as BlockT;
use sp_core::crypto::Pair;
use sp_keyring::Sr25519Keyring;

use futures::executor::LocalPool;
use futures::task::LocalSpawnExt;

#[derive(Default)]
pub struct MockNetworkOps {
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
	gossip_messages: Arc<Mutex<HashMap<Hash, GossipMessage>>>,
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

	fn gossip_message(&self, topic: Hash, message: GossipMessage) {
		self.gossip_messages.lock().insert(topic, message);
	}

	fn send_message(&self, _who: PeerId, _message: GossipMessage) {

	}
}

impl GossipOps for MockGossip {
	fn new_local_leaf(&self, _: crate::legacy::gossip::MessageValidationData) -> crate::legacy::gossip::NewLeafActions {
		crate::legacy::gossip::NewLeafActions::new()
	}

	fn register_availability_store(&self, _store: av_store::Store) {}
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

sp_api::mock_impl_runtime_apis! {
	impl ParachainHost<Block> for RuntimeApi {
		type Error = sp_blockchain::Error;

		fn validators(&self) -> Vec<ValidatorId> {
			self.data.lock().validators.clone()
		}

		fn duty_roster(&self) -> DutyRoster {
			DutyRoster {
				validator_duty: self.data.lock().duties.clone(),
			}
		}

		fn active_parachains(&self) -> Vec<(ParaId, Option<(CollatorId, Retriable)>)> {
			self.data.lock().active_parachains.clone()
		}

		fn parachain_code(_: ParaId) -> Option<ValidationCode> {
			Some(ValidationCode(Vec::new()))
		}

		fn global_validation_schedule() -> GlobalValidationSchedule {
			Default::default()
		}

		fn local_validation_data(_: ParaId) -> Option<LocalValidationData> {
			Some(Default::default())
		}

		fn get_heads(_: Vec<<Block as BlockT>::Extrinsic>) -> Option<Vec<AbridgedCandidateReceipt>> {
			Some(Vec::new())
		}

		fn signing_context() -> SigningContext {
			SigningContext {
				session_index: Default::default(),
				parent_hash: Default::default(),
			}
		}
	}
}

impl super::Service<MockNetworkOps> {
	async fn connect_peer(&mut self, peer: PeerId, role: ObservedRole) {
		self.sender.send(ServiceToWorkerMsg::PeerConnected(peer, role)).await.unwrap();
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
	Service<MockNetworkOps>,
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
fn worker_task_shuts_down_when_sender_dropped() {
	let (service, _gossip, mut pool, worker_task) = test_setup(Config { collating_for: None });

	drop(service);
	let _ = pool.run_until(worker_task);
}

/// Given the async nature of `select!` that is being used in the main loop of the worker
/// and that consensus instances use their own channels, we don't know when the synchronize message
/// is handled. This helper functions checks multiple times that the given instance is dropped. Even
/// if the first round fails, the second one should be successful as the consensus instance drop
/// should be already handled this time.
fn wait_for_instance_drop(service: &mut Service<MockNetworkOps>, pool: &mut LocalPool, instance: Hash) {
	let mut try_counter = 0;
	let max_tries = 3;

	while try_counter < max_tries {
		let dropped = pool.run_until(service.synchronize(move |proto| {
			!proto.consensus_instances.contains_key(&instance)
		}));

		if dropped {
			return;
		}

		try_counter += 1;
	}

	panic!("Consensus instance `{}` wasn't dropped!", instance);
}

#[test]
fn consensus_instances_cleaned_up() {
	let (mut service, _gossip, mut pool, worker_task) = test_setup(Config { collating_for: None });
	let relay_parent = [0; 32].into();

	let signing_context = SigningContext {
		session_index: Default::default(),
		parent_hash: relay_parent,
	};
	let table = Arc::new(SharedTable::new(
		Vec::new(),
		HashMap::new(),
		None,
		signing_context,
		AvailabilityStore::new_in_memory(service.clone()),
		None,
		None,
	));

	pool.spawner().spawn_local(worker_task).unwrap();

	let router = pool.run_until(
		service.build_table_router(table, &[])
	).unwrap();

	drop(router);

	wait_for_instance_drop(&mut service, &mut pool, relay_parent);
}

#[test]
fn collation_is_received_with_dropped_router() {
	let (mut service, gossip, mut pool, worker_task) = test_setup(Config { collating_for: None });
	let relay_parent = [0; 32].into();
	let topic = crate::legacy::gossip::attestation_topic(relay_parent);

	let signing_context = SigningContext {
		session_index: Default::default(),
		parent_hash: relay_parent,
	};
	let table = Arc::new(SharedTable::new(
		vec![Sr25519Keyring::Alice.public().into()],
		HashMap::new(),
		Some(Arc::new(Sr25519Keyring::Alice.pair().into())),
		signing_context,
		AvailabilityStore::new_in_memory(service.clone()),
		None,
		None,
	));

	pool.spawner().spawn_local(worker_task).unwrap();

	let router = pool.run_until(
		service.build_table_router(table, &[])
	).unwrap();

	let receipt = AbridgedCandidateReceipt { relay_parent, ..Default::default() };
	let local_collation_future = router.local_collation(
		receipt,
		PoVBlock { block_data: BlockData(Vec::new()) },
		(0, &[]),
	);

	// Drop the router and make sure that the consensus instance is still alive
	drop(router);

	assert!(pool.run_until(service.synchronize(move |proto| {
		proto.consensus_instances.contains_key(&relay_parent)
	})));

	// The gossip message should still be unknown
	assert!(!gossip.gossip_messages.lock().contains_key(&topic));

	pool.run_until(local_collation_future).unwrap();

	// Make sure the instance is now dropped and the message was gossiped
	wait_for_instance_drop(&mut service, &mut pool, relay_parent);
	assert!(pool.run_until(service.synchronize(move |_| {
		gossip.gossip_messages.lock().contains_key(&topic)
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
		service.connect_peer(peer.clone(), ObservedRole::Authority).await;
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
		service.connect_peer(peer.clone(), ObservedRole::Authority).await;
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

#[test]
fn fetches_pov_block_from_gossip() {
	let (service, gossip, mut pool, worker_task) = test_setup(Config { collating_for: None });
	let relay_parent = [255; 32].into();

	let pov_block = PoVBlock {
		block_data: BlockData(vec![1, 2, 3]),
	};

	let mut candidate = AbridgedCandidateReceipt::default();
	candidate.relay_parent = relay_parent;
	candidate.pov_block_hash = pov_block.hash();
	let candidate_hash = candidate.hash();

	let signing_context = SigningContext {
		session_index: Default::default(),
		parent_hash: relay_parent,
	};

	let table = Arc::new(SharedTable::new(
		Vec::new(),
		HashMap::new(),
		None,
		signing_context,
		AvailabilityStore::new_in_memory(service.clone()),
		None,
		None,
	));

	let spawner = pool.spawner();

	spawner.spawn_local(worker_task).unwrap();
	let topic = crate::legacy::gossip::pov_block_topic(relay_parent);
	let (mut gossip_tx, _gossip_taken_rx) = gossip.add_gossip_stream(topic);

	let test_work = async move {
		let router = service.build_table_router(table, &[]).await.unwrap();
		let pov_block_listener = router.fetch_pov_block(&candidate);

		let message = GossipMessage::PoVBlock(GossipPoVBlock {
			relay_chain_leaf: relay_parent,
			candidate_hash,
			pov_block,
		}).encode();

		gossip_tx.send(TopicNotification { message, sender: None }).await.unwrap();
		pov_block_listener.await
	};

	pool.run_until(test_work).unwrap();
}

#[test]
fn validator_sends_key_to_collator_on_status() {
	let (service, _gossip, mut pool, worker_task) = test_setup(Config { collating_for: None });

	let peer = PeerId::random();
	let peer_clone = peer.clone();
	let validator_key = Sr25519Keyring::Alice.pair();
	let validator_id = ValidatorId::from(validator_key.public());
	let validator_id_clone = validator_id.clone();
	let collator_id = CollatorId::from(Sr25519Keyring::Bob.public());
	let para_id = ParaId::from(100);
	let mut service_clone = service.clone();

	pool.spawner().spawn_local(worker_task).unwrap();
	pool.run_until(async move {
		service_clone.synchronize(move |proto| { proto.local_keys.insert(validator_id_clone); }).await;
		service_clone.connect_peer(peer_clone.clone(), ObservedRole::Authority).await;
		service_clone.peer_message(peer_clone.clone(), Message::Status(Status {
			version: VERSION,
			collating_for: Some((collator_id, para_id)),
		})).await;
	});

	let expected_msg = Message::ValidatorId(validator_id.clone());
	assert!(service.network_service.recorded.lock().notifications.iter().any(|(p, notification)| {
		peer == *p && *notification == expected_msg
	}));
}
