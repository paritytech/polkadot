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

//! Polkadot-specific base networking protocol.
//!
//! This is implemented using the `sc-network` APIs for futures-based
//! notifications protocols. In some cases, we emulate request/response on top
//! of the notifications machinery, which is slightly less efficient but not
//! meaningfully so.
//!
//! We handle events from `sc-network` in a thin wrapper that forwards to a
//! background worker, which also handles commands from other parts of the node.

use arrayvec::ArrayVec;
use codec::{Decode, Encode};
use futures::channel::{mpsc, oneshot};
use futures::future::Either;
use futures::prelude::*;
use futures::task::{Spawn, SpawnExt};
use log::{debug, trace};

use polkadot_primitives::{
	Hash, Block,
	parachain::{
		PoVBlock, ValidatorId, ValidatorIndex, Collation, AbridgedCandidateReceipt,
		ErasureChunk, ParachainHost, Id as ParaId, CollatorId,
	},
};
use polkadot_validation::{
	SharedTable, TableRouter, Network as ParachainNetwork, Validated, GenericStatement, Collators,
	SignedStatement,
};
use sc_network::{config::Roles, Event, PeerId};
use sp_api::ProvideRuntimeApi;

use std::collections::{hash_map::{Entry, HashMap}, HashSet};
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::Duration;

use super::{cost, benefit, PolkadotNetworkService};
use crate::legacy::collator_pool::Role as CollatorRole;
use crate::legacy::gossip::{GossipMessage, ErasureChunkMessage, RegisteredMessageValidator};

/// The current protocol version.
pub const VERSION: u32 = 1;
/// The minimum supported protocol version.
pub const MIN_SUPPORTED_VERSION: u32 = 1;

/// The engine ID of the polkadot network protocol.
pub const POLKADOT_ENGINE_ID: sp_runtime::ConsensusEngineId = *b"dot2";
/// The protocol name.
pub const POLKADOT_PROTOCOL_NAME: &[u8] = b"/polkadot/1";

pub use crate::legacy::gossip::ChainContext;

// Messages from the service API or network adapter.
enum ServiceToWorkerMsg {
	// basic peer messages.
	PeerConnected(PeerId, Roles),
	PeerMessage(PeerId, Vec<bytes::Bytes>),
	PeerDisconnected(PeerId),

	// service messages.
	BuildConsensusNetworking(Arc<SharedTable>, Vec<ValidatorId>, oneshot::Sender<Router>),
	DropConsensusNetworking(Hash),
	SubmitValidatedCollation(
		Hash, // relay-parent
		AbridgedCandidateReceipt,
		PoVBlock,
		(ValidatorIndex, Vec<ErasureChunk>),
	),
	FetchPoVBlock(
		Hash, // relay-parent
		AbridgedCandidateReceipt,
		oneshot::Sender<PoVBlock>,
	),
	FetchErasureChunk(
		Hash, // candidate-hash.
		u32, // validator index.
		oneshot::Sender<ErasureChunk>,
	),
	DistributeErasureChunk(
		Hash, // candidate-hash,
		ErasureChunk,
	),
	AwaitCollation(
		Hash, // relay-parent,
		ParaId,
		oneshot::Sender<Collation>,
	),
	NoteBadCollator(
		CollatorId,
	),
	RegisterAvailabilityStore(
		av_store::Store,
	),
	OurCollation(
		HashSet<ValidatorId>,
		Collation,
	),
	ListenCheckedStatements(
		Hash, // relay-parent,
		oneshot::Sender<Pin<Box<dyn Stream<Item = SignedStatement> + Send>>>,
	),
}

/// An async handle to the network service.
#[derive(Clone)]
pub struct Service {
	sender: mpsc::Sender<ServiceToWorkerMsg>,
	network_service: Arc<PolkadotNetworkService>,
}

/// Registers the protocol.
///
/// You are very strongly encouraged to call this method very early on. Any connection open
/// will retain the protocols that were registered then, and not any new one.
pub fn start<C, Api, SP>(
	service: Arc<PolkadotNetworkService>,
	config: Config,
	chain_context: C,
	api: Arc<Api>,
	executor: SP,
) -> Result<Service, futures::task::SpawnError> where
	C: ChainContext + 'static,
	Api: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	Api::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	SP: Spawn + Clone + Send + 'static,
{
	const SERVICE_TO_WORKER_BUF: usize = 256;

	let mut event_stream = service.event_stream();
	service.register_notifications_protocol(POLKADOT_ENGINE_ID, POLKADOT_PROTOCOL_NAME);
	let (mut worker_sender, worker_receiver) = mpsc::channel(SERVICE_TO_WORKER_BUF);

	let gossip_validator = crate::legacy::gossip::register_validator(
		service.clone(),
		chain_context,
		&executor,
	);
	executor.spawn(worker_loop(
		config,
		service.clone(),
		gossip_validator,
		worker_sender.clone(),
		api,
		worker_receiver,
		executor.clone(),
	))?;

	let polkadot_service = Service {
		sender: worker_sender.clone(),
		network_service: service.clone(),
	};

	executor.spawn(async move {
		while let Some(event) = event_stream.next().await {
			let res = match event {
				Event::Dht(_) => continue,
				Event::NotificationStreamOpened {
					remote,
					engine_id,
					roles,
				} => {
					if engine_id != POLKADOT_ENGINE_ID { continue }

					worker_sender.send(ServiceToWorkerMsg::PeerConnected(remote, roles)).await
				},
				Event::NotificationStreamClosed {
					remote,
					engine_id,
				} => {
					if engine_id != POLKADOT_ENGINE_ID { continue }

					worker_sender.send(ServiceToWorkerMsg::PeerDisconnected(remote)).await
				},
				Event::NotificationsReceived {
					remote,
					messages,
				} => {
					let our_notifications = messages.into_iter()
						.filter_map(|(engine, message)| if engine == POLKADOT_ENGINE_ID {
							Some(message)
						} else {
							None
						})
						.collect();

					worker_sender.send(
						ServiceToWorkerMsg::PeerMessage(remote, our_notifications)
					).await
				}
			};

			if let Err(e) = res {
				// full is impossible here, as we've `await`ed the value being sent.
				if e.is_disconnected() {
					break
				}
			}
		}
	})?;

	Ok(polkadot_service)
}

/// The Polkadot protocol status message.
#[derive(Debug, Encode, Decode)]
pub struct Status {
	version: u32, // protocol version.
	collating_for: Option<(CollatorId, ParaId)>,
}

/// Polkadot-specific messages from peer to peer.
#[derive(Debug, Encode, Decode)]
pub enum Message {
	/// Exchange status with a peer. This should be the first message sent.
	#[codec(index = "0")]
	Status(Status),
	/// Inform a peer of their role as a collator. May only be sent after
	/// validator ID.
	#[codec(index = "1")]
	CollatorRole(CollatorRole),
	/// Send a collation.
	#[codec(index = "2")]
	Collation(Hash, Collation),
	/// Inform a peer of a new validator public key.
	#[codec(index = "3")]
	ValidatorId(ValidatorId),
}

// ensures collator-protocol messages are sent in correct order.
// session key must be sent before collator role.
enum CollatorState {
	Fresh,
	RolePending(CollatorRole),
	Primed(Option<CollatorRole>),
}

impl CollatorState {
	fn send_key<F: FnMut(Message)>(&mut self, key: ValidatorId, mut f: F) {
		f(Message::ValidatorId(key));
		if let CollatorState::RolePending(role) = *self {
			f(Message::CollatorRole(role));
			*self = CollatorState::Primed(Some(role));
		}
	}

	fn set_role<F: FnMut(Message)>(&mut self, role: CollatorRole, mut f: F) {
		if let CollatorState::Primed(ref mut r) = *self {
			f(Message::CollatorRole(role));
			*r = Some(role);
		} else {
			*self = CollatorState::RolePending(role);
		}
	}
}

enum ProtocolState {
	Fresh,
	Ready(Status, CollatorState),
}

struct PeerData {
	claimed_validator: bool,
	protocol_state: ProtocolState,
	session_keys: RecentValidatorIds,
}

impl PeerData {
	fn ready_and_collating_for(&self) -> Option<(CollatorId, ParaId)> {
		match self.protocol_state {
			ProtocolState::Ready(ref status, _) => status.collating_for.clone(),
			_ => None,
		}
	}

	fn collator_state_mut(&mut self) -> Option<&mut CollatorState> {
		match self.protocol_state {
			ProtocolState::Ready(_, ref mut c_state) => Some(c_state),
			_ => None,
		}
	}

	fn should_send_key(&self) -> bool {
		self.claimed_validator || self.ready_and_collating_for().is_some()
	}
}

struct ConsensusNetworkingInstance {
	statement_table: Arc<SharedTable>,
	relay_parent: Hash,
	attestation_topic: Hash,
	_drop_signal: exit_future::Signal,
}

/// Protocol configuration.
#[derive(Default)]
pub struct Config {
	/// Which collator-id to use when collating, and on which parachain.
	/// `None` if not collating.
	pub collating_for: Option<(CollatorId, ParaId)>,
}

// 3 is chosen because sessions change infrequently and usually
// only the last 2 (current session and "last" session) are relevant.
// the extra is an error boundary.
const RECENT_SESSIONS: usize = 3;

/// Result when inserting recent session key.
#[derive(PartialEq, Eq)]
pub(crate) enum InsertedRecentKey {
	/// Key was already known.
	AlreadyKnown,
	/// Key was new and pushed out optional old item.
	New(Option<ValidatorId>),
}

/// Wrapper for managing recent session keys.
#[derive(Default)]
struct RecentValidatorIds {
	inner: ArrayVec<[ValidatorId; RECENT_SESSIONS]>,
}

impl RecentValidatorIds {
	/// Insert a new session key. This returns one to be pushed out if the
	/// set is full.
	fn insert(&mut self, key: ValidatorId) -> InsertedRecentKey {
		if self.inner.contains(&key) { return InsertedRecentKey::AlreadyKnown }

		let old = if self.inner.len() == RECENT_SESSIONS {
			Some(self.inner.remove(0))
		} else {
			None
		};

		self.inner.push(key);
		InsertedRecentKey::New(old)
	}

	/// As a slice. Most recent is last.
	fn as_slice(&self) -> &[ValidatorId] {
		&*self.inner
	}
}

struct ProtocolHandler {
	service: Arc<PolkadotNetworkService>,
	peers: HashMap<PeerId, PeerData>,
	// reverse mapping from validator-ID to PeerID. Multiple peers can represent
	// the same validator because of sentry nodes.
	connected_validators: HashMap<ValidatorId, HashSet<PeerId>>,
	collators: crate::legacy::collator_pool::CollatorPool,
	local_collations: crate::legacy::local_collations::LocalCollations<Collation>,
	config: Config,
}

impl ProtocolHandler {
	fn new(
		service: Arc<PolkadotNetworkService>,
		config: Config,
	) -> Self {
		ProtocolHandler {
			service,
			peers: HashMap::new(),
			connected_validators: HashMap::new(),
			collators: Default::default(),
			local_collations: Default::default(),
			config,
		}
	}

	fn on_connect(&mut self, peer: PeerId, roles: Roles) {
		let claimed_validator = roles.contains(sc_network::config::Roles::AUTHORITY);

		self.peers.insert(peer.clone(), PeerData {
			claimed_validator,
			protocol_state: ProtocolState::Fresh,
			session_keys: Default::default(),
		});

		let status = Message::Status(Status {
			version: VERSION,
			collating_for: self.config.collating_for.clone(),
		}).encode();

		self.service.write_notification(peer, POLKADOT_ENGINE_ID, status);
	}

	fn on_disconnect(&mut self, peer: PeerId) {
		let mut new_primary = None;
		if let Some(data) = self.peers.remove(&peer) {
			// replace collator.
			if let Some((collator_id, _)) = data.ready_and_collating_for() {
				if self.collators.collator_id_to_peer_id(&collator_id) == Some(&peer) {
					new_primary = self.collators.on_disconnect(collator_id);
				}
			}

			// clean up stated validator IDs.
			for validator_id in data.session_keys.as_slice().iter().cloned() {
				self.validator_representative_removed(validator_id, &peer);
			}
		}

		let service = &self.service;
		let peers = &mut self.peers;
		if let Some(new_primary) = new_primary {
			let new_primary_peer_id = match self.collators.collator_id_to_peer_id(&new_primary) {
				None => return,
				Some(p) => p.clone(),
			};
			if let Some(c_state) = peers.get_mut(&new_primary_peer_id)
				.and_then(|p| p.collator_state_mut())
			{
				c_state.set_role(
					CollatorRole::Primary,
					|msg| service.write_notification(
						new_primary_peer_id.clone(),
						POLKADOT_ENGINE_ID,
						msg.encode(),
					),
				);
			}
		}
	}

	fn on_raw_messages(&mut self, remote: PeerId, messages: Vec<bytes::Bytes>) {
		for raw_message in messages {
			match Message::decode(&mut raw_message.as_ref()) {
				Ok(message) => {
					self.service.report_peer(remote.clone(), benefit::VALID_FORMAT);
					match message {
						Message::Status(status) => {
							self.on_status(remote.clone(), status);
						}
						Message::CollatorRole(role) => {
							self.on_collator_role(remote.clone(), role)
						}
						Message::Collation(relay_parent, collation) => {
							self.on_remote_collation(remote.clone(), relay_parent, collation);
						}
						Message::ValidatorId(session_key) => {
							self.on_validator_id(remote.clone(), session_key)
						}
					}
				},
				Err(_) => self.service.report_peer(remote.clone(), cost::INVALID_FORMAT),
			}
		}
	}

	fn on_status(&mut self, remote: PeerId, status: Status) {
		let peer = match self.peers.get_mut(&remote) {
			None => { self.service.report_peer(remote, cost::UNKNOWN_PEER); return }
			Some(p) => p,
		};

		match peer.protocol_state {
			ProtocolState::Fresh => {
				peer.protocol_state = ProtocolState::Ready(status, CollatorState::Fresh);
				if let Some((collator_id, para_id)) = peer.ready_and_collating_for() {
					let collator_attached = self.collators
						.collator_id_to_peer_id(&collator_id)
						.map_or(false, |id| id != &remote);

					// we only care about the first connection from this collator.
					if !collator_attached {
						let role = self.collators
							.on_new_collator(collator_id, para_id, remote.clone());
						let service = &self.service;
						if let Some(c_state) = peer.collator_state_mut() {
							c_state.set_role(role, |msg| service.write_notification(
								remote.clone(),
								POLKADOT_ENGINE_ID,
								msg.encode(),
							));
						}
					}
				}
			}
			ProtocolState::Ready(_, _) => {
				self.service.report_peer(remote, cost::UNEXPECTED_MESSAGE);
			}
		}
	}

	fn on_remote_collation(&mut self, remote: PeerId, relay_parent: Hash, collation: Collation) {
		let peer = match self.peers.get_mut(&remote) {
			None => { self.service.report_peer(remote, cost::UNKNOWN_PEER); return }
			Some(p) => p,
		};

		let (collator_id, para_id) = match peer.ready_and_collating_for() {
			None => {
				self.service.report_peer(remote, cost::UNEXPECTED_MESSAGE);
				return
			}
			Some(x) => x,
		};

		let collation_para = collation.info.parachain_index;
		let collated_acc = collation.info.collator.clone();

		let structurally_valid = para_id == collation_para && collator_id == collated_acc;
		if structurally_valid && collation.info.check_signature().is_ok() {
			debug!(target: "p_net", "Received collation for parachain {:?} from peer {}",
				para_id, remote);

			if self.collators.collator_id_to_peer_id(&collator_id) == Some(&remote) {
				self.collators.on_collation(collator_id, relay_parent, collation);
				self.service.report_peer(remote, benefit::GOOD_COLLATION);
			}
		} else {
			self.service.report_peer(remote, cost::INVALID_FORMAT);
		}
	}

	fn on_collator_role(&mut self, remote: PeerId, role: CollatorRole) {
		let collations_to_send;

		{
			let peer = match self.peers.get_mut(&remote) {
				None => { self.service.report_peer(remote, cost::UNKNOWN_PEER); return }
				Some(p) => p,
			};

			match peer.protocol_state {
				ProtocolState::Fresh => {
					self.service.report_peer(remote, cost::UNEXPECTED_MESSAGE);
					return;
				}
				ProtocolState::Ready(_, _) => {
					let last_key = match peer.session_keys.as_slice().last() {
						None => {
							self.service.report_peer(remote, cost::UNEXPECTED_MESSAGE);
							return;
						}
						Some(k) => k,
					};

					collations_to_send = self.local_collations
						.note_validator_role(last_key.clone(), role);
				}
			}
		}

		send_peer_collations(&*self.service, remote, collations_to_send);
	}

	fn on_validator_id(&mut self, remote: PeerId, key: ValidatorId) {
		let mut collations_to_send = Vec::new();
		let mut invalidated_key = None;

		{
			let peer = match self.peers.get_mut(&remote) {
				None => { self.service.report_peer(remote, cost::UNKNOWN_PEER); return }
				Some(p) => p,
			};

			match peer.protocol_state {
				ProtocolState::Fresh => {
					self.service.report_peer(remote, cost::UNEXPECTED_MESSAGE);
					return
				}
				ProtocolState::Ready(_, _) => {
					if let InsertedRecentKey::New(Some(last)) = peer.session_keys.insert(key.clone()) {
						collations_to_send = self.local_collations.fresh_key(&last, &key);
						invalidated_key = Some(last);
					}
				}
			}
		}

		if let Some(invalidated) = invalidated_key {
			self.validator_representative_removed(invalidated, &remote);
		}

		send_peer_collations(&*self.service, remote, collations_to_send);
	}

	// call when the given peer no longer represents the given validator key.
	//
	// this can occur when the peer advertises a new key, invalidating an old one,
	// or when the peer disconnects.
	fn validator_representative_removed(&mut self, validator_id: ValidatorId, peer_id: &PeerId) {
		if let Entry::Occupied(mut entry) = self.connected_validators.entry(validator_id) {
			entry.get_mut().remove(peer_id);
			if entry.get().is_empty() {
				let _ = entry.remove_entry();
			}
		}
	}

	fn await_collation(
		&mut self,
		relay_parent: Hash,
		para_id: ParaId,
		sender: oneshot::Sender<Collation>,
	) {
		self.collators.await_collation(relay_parent, para_id, sender);
	}

	fn collect_garbage(&mut self) {
		self.collators.collect_garbage(None);
		self.local_collations.collect_garbage(None);
	}

	fn note_bad_collator(&mut self, who: CollatorId) {
		if let Some(peer) = self.collators.collator_id_to_peer_id(&who) {
			self.service.report_peer(peer.clone(), cost::BAD_COLLATION);
		}
	}

	// distribute a new session key to any relevant peers.
	fn distribute_new_session_key(&mut self, key: ValidatorId) {
		let service = &self.service;

		for (peer_id, peer) in self.peers.iter_mut() {
			if !peer.should_send_key() { continue }

			if let Some(c_state) = peer.collator_state_mut() {
				c_state.send_key(key.clone(), |msg| service.write_notification(
					peer_id.clone(),
					POLKADOT_ENGINE_ID,
					msg.encode(),
				));
			}
		}
	}

	// distribute our (as a collator node) collation to peers.
	fn distribute_our_collation(&mut self, targets: HashSet<ValidatorId>, collation: Collation) {
		let relay_parent = collation.info.relay_parent;
		let distribution = self.local_collations.add_collation(relay_parent, targets, collation);

		for (validator, collation) in distribution {
			let validator_representatives = self.connected_validators.get(&validator)
				.into_iter().flat_map(|reps| reps);

			for remote in validator_representatives {
				send_peer_collations(
					&*self.service,
					remote.clone(),
					std::iter::once((relay_parent, collation.clone())),
				);
			}
		}
	}
}

fn send_peer_collations(
	service: &PolkadotNetworkService,
	remote: PeerId,
	collations: impl IntoIterator<Item=(Hash, Collation)>,
) {
	for (relay_parent, collation) in collations {
		service.write_notification(
			remote.clone(),
			POLKADOT_ENGINE_ID,
			Message::Collation(relay_parent, collation).encode(),
		);
	}
}

async fn worker_loop<Api, Sp>(
	config: Config,
	service: Arc<PolkadotNetworkService>,
	gossip_handle: RegisteredMessageValidator,
	sender: mpsc::Sender<ServiceToWorkerMsg>,
	api: Arc<Api>,
	mut receiver: mpsc::Receiver<ServiceToWorkerMsg>,
	executor: Sp,
) where
	Api: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	Api::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	Sp: Spawn + Clone + Send + 'static,
{
	const COLLECT_GARBAGE_INTERVAL: Duration = Duration::from_secs(29);

	let mut protocol_handler = ProtocolHandler::new(service, config);
	let mut consensus_instances = HashMap::new();
	let mut local_keys = RecentValidatorIds::default();

	let mut collect_garbage = stream::unfold((), move |_| {
		futures_timer::Delay::new(COLLECT_GARBAGE_INTERVAL).map(|_| Some(((), ())))
	}).map(drop);

	loop {
		let message = match future::select(receiver.next(), collect_garbage.next()).await {
			Either::Left((None, _)) | Either::Right((None, _)) => break,
			Either::Left((Some(message), _)) => message,
			Either::Right(_) => {
				protocol_handler.collect_garbage();
				continue
			}
		};

		match message {
			ServiceToWorkerMsg::PeerConnected(remote, roles) => {
				protocol_handler.on_connect(remote, roles);
			}
			ServiceToWorkerMsg::PeerDisconnected(remote) => {
				protocol_handler.on_disconnect(remote);
			}
			ServiceToWorkerMsg::PeerMessage(remote, messages) => {
				protocol_handler.on_raw_messages(remote, messages)
			}

			ServiceToWorkerMsg::BuildConsensusNetworking(table, authorities, router_sender) => {
				// glue: let gossip know about our new local leaf.
				let relay_parent = table.consensus_parent_hash().clone();
				let (signal, exit) = exit_future::signal();

				let router = Router {
					inner: Arc::new(RouterInner { relay_parent, sender: sender.clone() }),
				};

				let key = table.session_key();
				if let Some(key) = key {
					if let InsertedRecentKey::New(_) = local_keys.insert(key.clone()) {
						protocol_handler.distribute_new_session_key(key);
					}
				}

				let new_leaf_actions = gossip_handle.new_local_leaf(
					relay_parent,
					crate::legacy::gossip::MessageValidationData { authorities },
				);

				new_leaf_actions.perform(&gossip_handle);

				consensus_instances.insert(relay_parent, ConsensusNetworkingInstance {
					statement_table: table.clone(),
					relay_parent,
					attestation_topic: crate::legacy::gossip::attestation_topic(relay_parent),
					_drop_signal: signal,
				});

				let weak_router = Arc::downgrade(&router.inner);

				// glue the incoming messages, shared table, and validation
				// work together.
				let _ = executor.spawn(statement_import_loop(
					relay_parent,
					table,
					api.clone(),
					weak_router,
					gossip_handle.clone(),
					exit,
					executor.clone(),
				));

				let _ = router_sender.send(router);
			}
			ServiceToWorkerMsg::DropConsensusNetworking(relay_parent) => {
				consensus_instances.remove(&relay_parent);
			}
			ServiceToWorkerMsg::SubmitValidatedCollation(relay_parent, receipt, pov_block, chunks) => {
				let instance = match consensus_instances.get(&relay_parent) {
					None => continue,
					Some(instance) => instance,
				};

				distribute_validated_collation(
					instance,
					receipt,
					pov_block,
					chunks,
					&gossip_handle,
				);
			}
			ServiceToWorkerMsg::FetchPoVBlock(_relay_parent, _candidate, _sender) => {
				// TODO https://github.com/paritytech/polkadot/issues/742:
				// create a filter on gossip for it and send to sender.
			}
			ServiceToWorkerMsg::FetchErasureChunk(candidate_hash, validator_index, sender) => {
				let topic = crate::erasure_coding_topic(&candidate_hash);

				// for every erasure-root, relay-parent pair, there should only be one
				// valid chunk with the given index.
				//
				// so we only care about the first item of the filtered stream.
				let get_msg = gossip_handle.gossip_messages_for(topic)
					.filter_map(move |(msg, _)| {
						future::ready(match msg {
							GossipMessage::ErasureChunk(chunk) =>
								if chunk.chunk.index == validator_index {
									Some(chunk.chunk)
								} else {
									None
								},
							_ => None,
						})
					})
					.into_future()
					.map(|(item, _)| item.expect(
						"gossip message streams do not conclude early; qed"
					));

				let _ = executor.spawn(async move {
					let chunk = get_msg.await;
					let _ = sender.send(chunk);
				});
			}
			ServiceToWorkerMsg::DistributeErasureChunk(candidate_hash, erasure_chunk) => {
				let topic = crate::erasure_coding_topic(&candidate_hash);
				gossip_handle.gossip_message(
					topic,
					GossipMessage::ErasureChunk(ErasureChunkMessage {
						chunk: erasure_chunk,
						candidate_hash,
					})
				);
			}
			ServiceToWorkerMsg::AwaitCollation(relay_parent, para_id, sender) => {
				debug!(target: "p_net", "Attempting to get collation for parachain {:?} on relay parent {:?}", para_id, relay_parent);
				protocol_handler.await_collation(relay_parent, para_id, sender)
			}
			ServiceToWorkerMsg::NoteBadCollator(collator) => {
				protocol_handler.note_bad_collator(collator);
			}
			ServiceToWorkerMsg::RegisterAvailabilityStore(store) => {
				gossip_handle.register_availability_store(store);
			}
			ServiceToWorkerMsg::OurCollation(targets, collation) => {
				protocol_handler.distribute_our_collation(targets, collation);
			}
			ServiceToWorkerMsg::ListenCheckedStatements(relay_parent, sender) => {
				let topic = crate::legacy::gossip::attestation_topic(relay_parent);
				let checked_messages = gossip_handle.gossip_messages_for(topic)
					.filter_map(|msg| match msg.0 {
						GossipMessage::Statement(s) => future::ready(Some(s.signed_statement)),
						_ => future::ready(None),
					})
					.boxed();

				let _ = sender.send(checked_messages);
			}
		}
	}
}

// A unique trace for valid statements issued by a validator.
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub(crate) enum StatementTrace {
	Valid(ValidatorIndex, Hash),
	Invalid(ValidatorIndex, Hash),
}

/// Helper for deferring statements whose associated candidate is unknown.
struct DeferredStatements {
	deferred: HashMap<Hash, Vec<SignedStatement>>,
	known_traces: HashSet<StatementTrace>,
}

impl DeferredStatements {
	/// Create a new `DeferredStatements`.
	fn new() -> Self {
		DeferredStatements {
			deferred: HashMap::new(),
			known_traces: HashSet::new(),
		}
	}

	/// Push a new statement onto the deferred pile. `Candidate` statements
	/// cannot be deferred and are ignored.
	fn push(&mut self, statement: SignedStatement) {
		let (hash, trace) = match statement.statement {
			GenericStatement::Candidate(_) => return,
			GenericStatement::Valid(hash) => (hash, StatementTrace::Valid(statement.sender.clone(), hash)),
			GenericStatement::Invalid(hash) => (hash, StatementTrace::Invalid(statement.sender.clone(), hash)),
		};

		if self.known_traces.insert(trace) {
			self.deferred.entry(hash).or_insert_with(Vec::new).push(statement);
		}
	}

	/// Take all deferred statements referencing the given candidate hash out.
	fn take_deferred(&mut self, hash: &Hash) -> (Vec<SignedStatement>, Vec<StatementTrace>) {
		match self.deferred.remove(hash) {
			None => (Vec::new(), Vec::new()),
			Some(deferred) => {
				let mut traces = Vec::new();
				for statement in deferred.iter() {
					let trace = match statement.statement {
						GenericStatement::Candidate(_) => continue,
						GenericStatement::Valid(hash) => StatementTrace::Valid(statement.sender.clone(), hash),
						GenericStatement::Invalid(hash) => StatementTrace::Invalid(statement.sender.clone(), hash),
					};

					self.known_traces.remove(&trace);
					traces.push(trace);
				}

				(deferred, traces)
			}
		}
	}
}

// the internal loop of waiting for messages and spawning validation work
// as a result of those messages. this future exits when `exit` is ready.
async fn statement_import_loop<Api>(
	relay_parent: Hash,
	table: Arc<SharedTable>,
	api: Arc<Api>,
	weak_router: Weak<RouterInner>,
	gossip_handle: RegisteredMessageValidator,
	mut exit: exit_future::Exit,
	executor: impl Spawn,
) where
	Api: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	Api::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
{
	let topic = crate::legacy::gossip::attestation_topic(relay_parent);
	let mut checked_messages = gossip_handle.gossip_messages_for(topic)
		.filter_map(|msg| match msg.0 {
			GossipMessage::Statement(s) => future::ready(Some(s.signed_statement)),
			_ => future::ready(None),
		});

	let mut deferred_statements = DeferredStatements::new();

	loop {
		let statement = match future::select(exit, checked_messages.next()).await {
			Either::Left(_) | Either::Right((None, _)) => return,
			Either::Right((Some(statement), e)) => {
				exit = e;
				statement
			}
		};

		// defer any statements for which we haven't imported the candidate yet
		let c_hash = {
			let candidate_data = match statement.statement {
				GenericStatement::Candidate(ref c) => Some(c.hash()),
				GenericStatement::Valid(ref hash)
					| GenericStatement::Invalid(ref hash)
					=> table.with_candidate(hash, |c| c.map(|_| *hash)),
			};
			match candidate_data {
				Some(x) => x,
				None => {
					deferred_statements.push(statement);
					continue;
				}
			}
		};

		// import all statements pending on this candidate
		let (mut statements, _traces) = if let GenericStatement::Candidate(_) = statement.statement {
			deferred_statements.take_deferred(&c_hash)
		} else {
			(Vec::new(), Vec::new())
		};

		// prepend the candidate statement.
		debug!(target: "validation", "Importing statements about candidate {:?}", c_hash);
		statements.insert(0, statement);

		let producers: Vec<_> = {
			// create a temporary router handle for importing all of these statements
			let temp_router = match weak_router.upgrade() {
				None => break,
				Some(inner) => Router { inner },
			};

			table.import_remote_statements(
				&temp_router,
				statements.iter().cloned(),
			)
		};

		// dispatch future work as necessary.
		for (producer, statement) in producers.into_iter().zip(statements) {
			if let Some(_sender) = table.index_to_id(statement.sender) {
				if let Some(producer) = producer {
					trace!(target: "validation", "driving statement work to completion");

					let table = table.clone();
					let gossip_handle = gossip_handle.clone();

					let work = producer.prime(api.clone()).validate().map(move |res| {
						let validated = match res {
							Err(e) => {
								debug!(target: "p_net", "Failed to act on statement: {}", e);
								return
							}
							Ok(v) => v,
						};

						// propagate the statement.
						let statement = crate::legacy::gossip::GossipStatement::new(
							relay_parent,
							match table.import_validated(validated) {
								Some(s) => s,
								None => return,
							}
						);

						gossip_handle.gossip_message(topic, statement.into());
					});

					let work = future::select(work.boxed(), exit.clone()).map(drop);
					let _ = executor.spawn(work);
				}
			}
		}
	}
}

// distribute a "local collation": this is the collation gotten by a validator
// from a collator. it needs to be distributed to other validators in the same
// group.
fn distribute_validated_collation(
	instance: &ConsensusNetworkingInstance,
	receipt: AbridgedCandidateReceipt,
	pov_block: PoVBlock,
	chunks: (ValidatorIndex, Vec<ErasureChunk>),
	gossip_handle: &RegisteredMessageValidator,
) {
	// produce a signed statement.
	let hash = receipt.hash();
	let validated = Validated::collated_local(
		receipt,
		pov_block,
	);

	let statement = crate::legacy::gossip::GossipStatement::new(
		instance.relay_parent,
		match instance.statement_table.import_validated(validated) {
			None => return,
			Some(s) => s,
		}
	);

	gossip_handle.gossip_message(instance.attestation_topic, statement.into());

	for chunk in chunks.1 {
		let message = crate::legacy::gossip::ErasureChunkMessage {
			chunk,
			candidate_hash: hash,
		};

		gossip_handle.gossip_message(
			crate::erasure_coding_topic(&hash),
			message.into(),
		);
	}
}

/// Routing logic for a particular attestation session.
#[derive(Clone)]
pub struct Router {
	inner: Arc<RouterInner>,
}

// note: do _not_ make this `Clone`: the drop implementation needs to _uniquely_
// send the `DropConsensusNetworking` message.
struct RouterInner {
	relay_parent: Hash,
	sender: mpsc::Sender<ServiceToWorkerMsg>,
}

impl Drop for RouterInner  {
	fn drop(&mut self) {
		let res = self.sender.try_send(
			ServiceToWorkerMsg::DropConsensusNetworking(self.relay_parent)
		);

		if let Err(e) = res {
			assert!(
				!e.is_full(),
				"futures 0.3 guarantees at least one free slot in the capacity \
				per sender; this is the first message sent via this sender; \
				therefore we will not have to wait for capacity; qed"
			);
			// other error variants (disconnection) are fine here.
		}
	}
}

impl Service {
	/// Register an availablility-store that the network can query.
	pub fn register_availability_store(&self, store: av_store::Store) {
		let _ = self.sender.clone()
			.try_send(ServiceToWorkerMsg::RegisterAvailabilityStore(store));
	}

	/// Submit a collation that we (as a collator) have prepared to validators.
	///
	/// Provide a set of validator-IDs we should distribute to.
	pub fn distribute_collation(&self, targets: HashSet<ValidatorId>, collation: Collation) {
		let _ = self.sender.clone()
			.try_send(ServiceToWorkerMsg::OurCollation(targets, collation));
	}

	/// Returns a stream that listens for checked statements on a particular
	/// relay chain parent hash.
	///
	/// Take care to drop the stream, as the sending side will not be cleaned
	/// up until it is.
	pub fn checked_statements(&self, relay_parent: Hash)
		-> Pin<Box<dyn Stream<Item = SignedStatement>>> {
		let (tx, rx) = oneshot::channel();
		let mut sender = self.sender.clone();

		let receive_stream = async move {
			sender.send(
				ServiceToWorkerMsg::ListenCheckedStatements(relay_parent, tx)
			).map_err(future::Either::Left).await?;

			rx.map_err(future::Either::Right).await
		};

		receive_stream
			.map(|res| match res {
				Ok(s) => s.left_stream(),
				Err(e) => {
					log::warn!(
						target: "p_net",
						"Polkadot network worker appears to be down: {:?}",
						e,
					);
					stream::pending().right_stream()
				}
			})
			.flatten_stream()
			.boxed()
	}
}

impl ParachainNetwork for Service {
	type Error = future::Either<mpsc::SendError, oneshot::Canceled>;
	type TableRouter = Router;
	type BuildTableRouter = Pin<Box<dyn Future<Output=Result<Router,Self::Error>> + Send>>;

	fn build_table_router(
		&self,
		table: Arc<SharedTable>,
		authorities: &[ValidatorId],
	) -> Self::BuildTableRouter {
		let authorities = authorities.to_vec();
		let mut sender = self.sender.clone();

		let (tx, rx) = oneshot::channel();
		Box::pin(async move {
			sender.send(
				ServiceToWorkerMsg::BuildConsensusNetworking(table, authorities, tx)
			).map_err(future::Either::Left).await?;

			rx.map_err(future::Either::Right).await
		})
	}
}

impl Collators for Service {
	type Error = future::Either<mpsc::SendError, oneshot::Canceled>;
	type Collation = Pin<Box<dyn Future<Output = Result<Collation, Self::Error>> + Send>>;

	fn collate(&self, parachain: ParaId, relay_parent: Hash) -> Self::Collation {
		let (tx, rx) = oneshot::channel();
		let mut sender = self.sender.clone();

		Box::pin(async move {
			sender.send(
				ServiceToWorkerMsg::AwaitCollation(relay_parent, parachain, tx)
			).map_err(future::Either::Left).await?;

			rx.map_err(future::Either::Right).await
		})
	}

	fn note_bad_collator(&self, collator: CollatorId) {
		let _ = self.sender.clone().try_send(ServiceToWorkerMsg::NoteBadCollator(collator));
	}
}

impl av_store::ErasureNetworking for Service {
	type Error = future::Either<mpsc::SendError, oneshot::Canceled>;

	fn fetch_erasure_chunk(&self, candidate_hash: &Hash, index: u32)
		-> Pin<Box<dyn Future<Output = Result<ErasureChunk, Self::Error>> + Send>>
	{
		let (tx, rx) = oneshot::channel();
		let mut sender = self.sender.clone();

		let candidate_hash = *candidate_hash;
		Box::pin(async move {
			sender.send(
				ServiceToWorkerMsg::FetchErasureChunk(candidate_hash, index, tx)
			).map_err(future::Either::Left).await?;

			rx.map_err(future::Either::Right).await
		})
	}

	fn distribute_erasure_chunk(
		&self,
		candidate_hash: Hash,
		chunk: ErasureChunk,
	) {
		let _ = self.sender.clone().try_send(
			ServiceToWorkerMsg::DistributeErasureChunk(candidate_hash, chunk)
		);
	}
}

/// Errors when interacting with the statement router.
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum RouterError {
	#[display(fmt = "Encountered unexpected I/O error: {}", _0)]
	Io(std::io::Error),
	#[display(fmt = "Worker hung up while answering request.")]
	Canceled(oneshot::Canceled),
	#[display(fmt = "Could not reach worker with request: {}", _0)]
	SendError(mpsc::SendError),
}

impl TableRouter for Router {
	type Error = RouterError;
	type SendLocalCollation = Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>>;
	type FetchValidationProof = Pin<Box<dyn Future<Output = Result<PoVBlock, Self::Error>> + Send>>;

	fn local_collation(
		&self,
		receipt: AbridgedCandidateReceipt,
		pov_block: PoVBlock,
		chunks: (ValidatorIndex, &[ErasureChunk]),
	) -> Self::SendLocalCollation {
		let message = ServiceToWorkerMsg::SubmitValidatedCollation(
			self.inner.relay_parent.clone(),
			receipt,
			pov_block,
			(chunks.0, chunks.1.to_vec()),
		);
		let mut sender = self.inner.sender.clone();
		Box::pin(async move {
			sender.send(message).map_err(Into::into).await
		})
	}

	fn fetch_pov_block(&self, candidate: &AbridgedCandidateReceipt) -> Self::FetchValidationProof {
		let (tx, rx) = oneshot::channel();
		let message = ServiceToWorkerMsg::FetchPoVBlock(
			self.inner.relay_parent.clone(),
			candidate.clone(),
			tx,
		);

		let mut sender = self.inner.sender.clone();
		Box::pin(async move {
			sender.send(message).await?;
			rx.map_err(Into::into).await
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

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
}
