use std::collections::HashMap;
use std::sync::Arc;
use parking_lot::{Mutex, RwLock};
use polkadot_gossip_primitives::*;
use polkadot_primitives::{Block, Hash};
use sp_runtime::traits::{Hash as HashT};
use codec::{Decode, Encode};
use futures::prelude::*;

struct Inner<C: ?Sized> {
	peers: HashMap<PeerId, PeerData>,
	attestation_view: AttestationView,
	availability_store: Option<crate::Store>,
	chain: C,
}

impl<C: ?Sized + ChainContext> Inner<C> {
	fn validate_neighbor_packet(&mut self, sender: &PeerId, packet: NeighborPacket)
		-> (GossipValidationResult<Hash>, ReputationChange, Vec<Hash>)
	{
		let chain_heads = packet.chain_heads;
		if chain_heads.len() > MAX_CHAIN_HEADS {
			(GossipValidationResult::Discard, cost::BAD_NEIGHBOR_PACKET, Vec::new())
		} else {
			let chain_heads: LeavesVec = chain_heads.into_iter().collect();
			let new_topics = if let Some(ref mut peer) = self.peers.get_mut(sender) {
				let new_leaves = peer.attestation.update_leaves(&chain_heads);
				let new_attestation_topics = new_leaves.iter().cloned().map(attestation_topic);

				new_attestation_topics.collect()
			} else {
				Vec::new()
			};

			(GossipValidationResult::Discard, cost::NONE, new_topics)
		}
	}

	fn validate_erasure_chunk_packet(&mut self, msg: ErasureChunkMessage)
		-> (GossipValidationResult<Hash>, ReputationChange)
	{
		if let Some(store) = &self.availability_store {
			if let Some(receipt) = store.get_candidate(&msg.candidate_hash) {
				let chunk_hash = erasure::branch_hash(
					&receipt.commitments.erasure_root,
					&msg.chunk.proof,
					msg.chunk.index as usize
				);

				if chunk_hash != Ok(BlakeTwo256::hash(&msg.chunk.chunk)) {
					(
						GossipValidationResult::Discard,
						cost::ERASURE_CHUNK_WRONG_ROOT
					)
				} else {
					if let Some(awaited_chunks) = store.awaited_chunks() {
						let frontier_entry = crate::AwaitedFrontierEntry {
							candidate_hash: msg.candidate_hash,
							relay_parent: receipt.relay_parent,
							validator_index: msg.chunk.index,
						};
						if awaited_chunks.contains(&frontier_entry) {
							let topic = erasure_coding_topic(
								&msg.candidate_hash
							);

							return (
								GossipValidationResult::ProcessAndKeep(topic),
								benefit::NEW_ERASURE_CHUNK,
							);
						}
					}
					(GossipValidationResult::Discard, cost::NONE)
				}
			} else {
				(GossipValidationResult::Discard, cost::ORPHANED_ERASURE_CHUNK)
			}
		} else {
			(GossipValidationResult::Discard, cost::NONE)
		}
	}

	fn multicast_neighbor_packet<F: FnMut(&PeerId, GossipMessage)>(
		&self,
		mut send_neighbor_packet: F,
	) {
		let neighbor_packet = GossipMessage::from(NeighborPacket {
			chain_heads: self.attestation_view.neighbor_info().collect(),
		});

		for peer in self.peers.keys() {
			send_neighbor_packet(peer, neighbor_packet.clone())
		}
	}
}

/// An unregistered message validator. Register this with `register_validator`.
pub struct MessageValidator<C: ?Sized> {
	report_handle: Box<dyn Fn(&PeerId, ReputationChange) + Send + Sync>,
	inner: RwLock<Inner<C>>,
}

impl<C: ChainContext + ?Sized> MessageValidator<C> {
	#[cfg(test)]
	fn new_test(
		chain: C,
		report_handle: Box<dyn Fn(&PeerId, ReputationChange) + Send + Sync>,
	) -> Self where C: Sized {
		MessageValidator {
			report_handle,
			inner: RwLock::new(Inner {
				peers: HashMap::new(),
				attestation_view: Default::default(),
				availability_store: None,
				chain,
			}),
		}
	}

	fn report(&self, who: &PeerId, cost_benefit: ReputationChange) {
		(self.report_handle)(who, cost_benefit)
	}
}

impl<C: ChainContext + ?Sized> Validator<Block> for MessageValidator<C> {
	fn new_peer(&self, _context: &mut dyn ValidatorContext<Block>, who: &PeerId, _roles: Roles) {
		let mut inner = self.inner.write();
		inner.peers.insert(who.clone(), PeerData::default());
	}

	fn peer_disconnected(&self, _context: &mut dyn ValidatorContext<Block>, who: &PeerId) {
		let mut inner = self.inner.write();
		inner.peers.remove(who);
	}

	fn validate(&self, context: &mut dyn ValidatorContext<Block>, sender: &PeerId, data: &[u8])
		-> GossipValidationResult<Hash>
	{
		let mut decode_data = data;
		let (res, cost_benefit) = match GossipMessage::decode(&mut decode_data) {
			Err(_) => (GossipValidationResult::Discard, cost::MALFORMED_MESSAGE),
			Ok(GossipMessage::Neighbor(VersionedNeighborPacket::V1(packet))) => {
				let (res, cb, topics) = self.inner.write().validate_neighbor_packet(sender, packet);
				for new_topic in topics {
					context.send_topic(sender, new_topic, false);
				}
				(res, cb)
			}
			Ok(GossipMessage::Statement(statement)) => {
				let (res, cb) = {
					let mut inner = self.inner.write();
					let inner = &mut *inner;
					inner.attestation_view.validate_statement_signature(statement, &inner.chain)
				};

				if let GossipValidationResult::ProcessAndKeep(ref topic) = res {
					context.broadcast_message(topic.clone(), data.to_vec(), false);
				}
				(res, cb)
			}
			Ok(GossipMessage::ErasureChunk(chunk)) => {
				self.inner.write().validate_erasure_chunk_packet(chunk)
			}
		};

		self.report(sender, cost_benefit);
		res
	}

	fn message_expired<'a>(&'a self) -> Box<dyn FnMut(Hash, &[u8]) -> bool + 'a> {
		let inner = self.inner.read();

		Box::new(move |topic, _data| {
			// check that messages from this topic are considered live by one of our protocols.
			// everything else is expired
			let live = inner.attestation_view.is_topic_live(&topic);

			!live // = expired
		})
	}

	fn message_allowed<'a>(&'a self) -> Box<dyn FnMut(&PeerId, MessageIntent, &Hash, &[u8]) -> bool + 'a> {
		let mut inner = self.inner.write();
		Box::new(move |who, intent, topic, data| {
			let &mut Inner {
				ref mut peers,
				ref mut attestation_view,
				..
			} = &mut *inner;

			match intent {
				MessageIntent::PeriodicRebroadcast => return false,
				_ => {},
			}

			let attestation_head = attestation_view.topic_block(topic).map(|x| x.clone());
			let peer = peers.get_mut(who);

			match GossipMessage::decode(&mut &data[..]) {
				Ok(GossipMessage::Statement(ref statement)) => {
					// to allow statements, we need peer knowledge.
					let peer_knowledge = peer.and_then(move |p| attestation_head.map(|r| (p, r)))
						.and_then(|(p, r)| p.attestation.knowledge_at_mut(&r).map(|k| (k, r)));

					peer_knowledge.map_or(false, |(knowledge, attestation_head)| {
						attestation_view.statement_allowed(
							statement,
							&attestation_head,
							knowledge,
						)
					})
				}
				_ => false,
			}
		})
	}
}

/// Register a gossip validator on the network service.
// NOTE: since RegisteredMessageValidator is meant to be a type-safe proof
// that we've actually done the registration, this should be the only way
// to construct it outside of tests.
pub fn register_validator<C: ChainContext + 'static>(
	service: Arc<NetworkService<Block, Hash>>,
	chain: C,
	executor: &impl futures::task::Spawn,
) -> RegisteredMessageValidator
{
	let s = service.clone();
	let report_handle = Box::new(move |peer: &PeerId, cost_benefit: ReputationChange| {
		if cost_benefit.value != 0 {
			s.report_peer(peer.clone(), cost_benefit);
		}
	});
	let validator = Arc::new(MessageValidator {
		report_handle,
		inner: RwLock::new(Inner {
			peers: HashMap::new(),
			attestation_view: Default::default(),
			availability_store: None,
			chain,
		})
	});

	let gossip_side = validator.clone();
	let gossip_engine = Arc::new(Mutex::new(GossipEngine::new(
		service.clone(),
		POLKADOT_ENGINE_ID,
		POLKADOT_PROTOCOL_NAME,
		gossip_side,
	)));

	// Spawn gossip engine.
	//
	// Ideally this would not be spawned as an orphaned task, but polled by
	// `RegisteredMessageValidator` which in turn would be polled by a `ValidationNetwork`.
	{
		let gossip_engine = gossip_engine.clone();
		let fut = futures::future::poll_fn(move |cx| {
			gossip_engine.lock().poll_unpin(cx)
		});
		let spawn_res = executor.spawn_obj(futures::task::FutureObj::from(Box::new(fut)));

		// Note: we consider the chances of an error to spawn a background task almost null.
		if spawn_res.is_err() {
			log::error!(target: "polkadot-gossip", "Failed to spawn background task");
		}
	}

	RegisteredMessageValidator {
		inner: validator as _,
		service: Some(service),
		gossip_engine: Some(gossip_engine),
	}
}

/// A registered message validator.
///
/// Create this using `register_validator`.
#[derive(Clone)]
pub struct RegisteredMessageValidator {
	inner: Arc<MessageValidator<dyn ChainContext>>,
	// Note: this is always `Some` in real code and `None` in tests.
	service: Option<Arc<NetworkService<Block, Hash>>>,
	// Note: this is always `Some` in real code and `None` in tests.
	gossip_engine: Option<Arc<Mutex<GossipEngine<Block>>>>,
}

impl RegisteredMessageValidator {
    /// Note that we perceive a new leaf of the block-DAG. We will notify our neighbors that
	/// we now accept parachain candidate attestations and incoming message queues
	/// relevant to this leaf.
	pub fn new_local_leaf(
		&self,
		validation: MessageValidationData,
	) -> NewLeafActions {
		// add an entry in attestation_view
		// prune any entries from attestation_view which are no longer leaves
		let mut inner = self.inner.inner.write();
		inner.attestation_view.new_local_leaf(validation);

		let mut actions = Vec::new();

		{
			let &mut Inner {
				ref chain,
				ref mut attestation_view,
				..
			} = &mut *inner;

			attestation_view.prune_old_leaves(|hash| match chain.is_known(hash) {
				Some(Known::Leaf) => true,
				_ => false,
			});
		}


		// send neighbor packets to peers
		inner.multicast_neighbor_packet(
			|who, message| actions.push(NewLeafAction::TargetedMessage(who.clone(), message))
		);

		NewLeafActions { actions }
	}

	pub(crate) fn gossip_messages_for(&self, topic: Hash) -> GossipMessageStream {
		let topic_stream = if let Some(gossip_engine) = self.gossip_engine.as_ref() {
			gossip_engine.lock().messages_for(topic)
		} else {
			log::error!("Called gossip_messages_for on a test engine");
			futures::channel::mpsc::unbounded().1
		};

		GossipMessageStream::new(topic_stream.boxed())
	}

	pub(crate) fn gossip_message(&self, topic: Hash, message: GossipMessage) {
		if let Some(gossip_engine) = self.gossip_engine.as_ref() {
			gossip_engine.lock().gossip_message(
				topic,
				message.encode(),
				false,
			);
		} else {
			log::error!("Called gossip_message on a test engine");
		}
	}

	pub(crate) fn send_message(&self, who: PeerId, message: GossipMessage) {
		if let Some(gossip_engine) = self.gossip_engine.as_ref() {
			gossip_engine.lock().send_message(vec![who], message.encode());
		} else {
			log::error!("Called send_message on a test engine");
		}
	}
}

impl GossipService for RegisteredMessageValidator {
	fn gossip_messages_for(&self, topic: Hash) -> GossipMessageStream {
		RegisteredMessageValidator::gossip_messages_for(self, topic)
	}

	fn gossip_message(&self, topic: Hash, message: GossipMessage) {
		RegisteredMessageValidator::gossip_message(self, topic, message)
	}

	fn send_message(&self, who: PeerId, message: GossipMessage) {
		RegisteredMessageValidator::send_message(self, who, message)
	}
}
