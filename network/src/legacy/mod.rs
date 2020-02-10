// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Polkadot-specific network implementation.
//!
//! This manages routing for parachain statements, parachain block and outgoing message
//! data fetching, communication between collators and validators, and more.

pub mod collator_pool;
pub mod local_collations;
pub mod router;
pub mod validation;
pub mod gossip;

use codec::{Decode, Encode};
use futures::channel::oneshot;
use futures::prelude::*;
use polkadot_primitives::{Block, Hash, Header};
use polkadot_primitives::parachain::{
	Id as ParaId, CollatorId, CandidateReceipt, Collation, PoVBlock,
	StructuredUnroutedIngress, ValidatorId, OutgoingMessages, ErasureChunk,
};
use sc_network::{
	PeerId, RequestId, Context, StatusMessage as GenericFullStatus,
	specialization::NetworkSpecialization as Specialization,
};
use sc_network_gossip::TopicNotification;
use self::validation::{LiveValidationLeaves, RecentValidatorIds, InsertedRecentKey};
use self::collator_pool::{CollatorPool, Role, Action};
use self::local_collations::LocalCollations;
use log::{trace, debug, warn};

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::task::{Context as PollContext, Poll};

use self::gossip::{GossipMessage, ErasureChunkMessage, RegisteredMessageValidator};
use crate::{cost, benefit};

#[cfg(test)]
mod tests;

type FullStatus = GenericFullStatus<Block>;

/// Specialization of the network service for the polkadot protocol.
pub type PolkadotNetworkService = sc_network::NetworkService<Block, PolkadotProtocol, Hash>;

/// Basic gossip functionality that a network has to fulfill.
pub trait GossipService: Send + Sync + 'static {
	/// Get a stream of gossip messages for a given hash.
	fn gossip_messages_for(&self, topic: Hash) -> GossipMessageStream;

	/// Gossip a message on given topic.
	fn gossip_message(&self, topic: Hash, message: GossipMessage);

	/// Send a message to a specific peer we're connected to.
	fn send_message(&self, who: PeerId, message: GossipMessage);
}

/// Basic functionality that a network has to fulfill.
pub trait NetworkService: GossipService + Send + Sync + 'static {
	/// Execute a closure with the polkadot protocol.
	fn with_spec<F: Send + 'static>(&self, with: F)
		where Self: Sized, F: FnOnce(&mut PolkadotProtocol, &mut dyn Context<Block>);
}

/// This is a newtype that implements a [`ProvideGossipMessages`] shim trait.
///
/// For any wrapped [`NetworkService`] type it implements a [`ProvideGossipMessages`].
/// For more details see documentation of [`ProvideGossipMessages`].
///
/// [`NetworkService`]: ./trait.NetworkService.html
/// [`ProvideGossipMessages`]: ../polkadot_availability_store/trait.ProvideGossipMessages.html
#[derive(Clone)]
pub struct AvailabilityNetworkShim(pub RegisteredMessageValidator<PolkadotProtocol>);

impl av_store::ProvideGossipMessages for AvailabilityNetworkShim {
	fn gossip_messages_for(&self, topic: Hash)
		-> Pin<Box<dyn Stream<Item = (Hash, Hash, ErasureChunk)> + Send>>
	{
		self.0.gossip_messages_for(topic)
			.filter_map(|(msg, _)| async move {
				match msg {
					GossipMessage::ErasureChunk(chunk) => {
						Some((chunk.relay_parent, chunk.candidate_hash, chunk.chunk))
					},
					_ => None,
				}
			})
			.boxed()
	}

	fn gossip_erasure_chunk(
		&self,
		relay_parent: Hash,
		candidate_hash: Hash,
		erasure_root: Hash,
		chunk: ErasureChunk
	) {
		let topic = av_store::erasure_coding_topic(relay_parent, erasure_root, chunk.index);
		self.0.gossip_message(
			topic,
			GossipMessage::ErasureChunk(ErasureChunkMessage {
				chunk,
				relay_parent,
				candidate_hash,
			})
		)
	}
}

/// A stream of gossip messages and an optional sender for a topic.
pub struct GossipMessageStream {
	topic_stream: Pin<Box<dyn Stream<Item = TopicNotification> + Send>>,
}

impl GossipMessageStream {
	/// Create a new instance with the given topic stream.
	pub fn new(topic_stream: Pin<Box<dyn Stream<Item = TopicNotification> + Send>>) -> Self {
		Self {
			topic_stream,
		}
	}
}

impl Stream for GossipMessageStream {
	type Item = (GossipMessage, Option<PeerId>);

	fn poll_next(self: Pin<&mut Self>, cx: &mut PollContext) -> Poll<Option<Self::Item>> {
		let this = Pin::into_inner(self);

		loop {
			let msg = match Pin::new(&mut this.topic_stream).poll_next(cx) {
				Poll::Ready(Some(msg)) => msg,
				Poll::Ready(None) => return Poll::Ready(None),
				Poll::Pending => return Poll::Pending,
			};

			debug!(target: "validation", "Processing statement for live validation leaf-work");
			if let Ok(gmsg) = GossipMessage::decode(&mut &msg.message[..]) {
				return Poll::Ready(Some((gmsg, msg.sender)))
			}
		}
	}
}

/// Status of a Polkadot node.
#[derive(Debug, PartialEq, Eq, Clone, Encode, Decode)]
pub struct Status {
	collating_for: Option<(CollatorId, ParaId)>,
}

struct PoVBlockRequest {
	attempted_peers: HashSet<ValidatorId>,
	validation_leaf: Hash,
	candidate_hash: Hash,
	block_data_hash: Hash,
	sender: oneshot::Sender<PoVBlock>,
	canon_roots: StructuredUnroutedIngress,
}

impl PoVBlockRequest {
	// Attempt to process a response. If the provided block is invalid,
	// this returns an error result containing the unmodified request.
	//
	// If `Ok(())` is returned, that indicates that the request has been processed.
	fn process_response(self, pov_block: PoVBlock) -> Result<(), Self> {
		if pov_block.block_data.hash() != self.block_data_hash {
			return Err(self);
		}

		match polkadot_validation::validate_incoming(&self.canon_roots, &pov_block.ingress) {
			Ok(()) => {
				let _ = self.sender.send(pov_block);
				Ok(())
			}
			Err(_) => Err(self)
		}
	}
}

// ensures collator-protocol messages are sent in correct order.
// session key must be sent before collator role.
enum CollatorState {
	Fresh,
	RolePending(Role),
	Primed(Option<Role>),
}

impl CollatorState {
	fn send_key<F: FnMut(Message)>(&mut self, key: ValidatorId, mut f: F) {
		f(Message::ValidatorId(key));
		if let CollatorState::RolePending(role) = *self {
			f(Message::CollatorRole(role));
			*self = CollatorState::Primed(Some(role));
		}
	}

	fn set_role<F: FnMut(Message)>(&mut self, role: Role, mut f: F) {
		if let CollatorState::Primed(ref mut r) = *self {
			f(Message::CollatorRole(role));
			*r = Some(role);
		} else {
			*self = CollatorState::RolePending(role);
		}
	}

	fn role(&self) -> Option<Role> {
		match *self {
			CollatorState::Fresh => None,
			CollatorState::RolePending(role) => Some(role),
			CollatorState::Primed(role) => role,
		}
	}
}

struct PeerInfo {
	collating_for: Option<(CollatorId, ParaId)>,
	validator_keys: RecentValidatorIds,
	claimed_validator: bool,
	collator_state: CollatorState,
}

impl PeerInfo {
	fn should_send_key(&self) -> bool {
		self.claimed_validator || self.collating_for.is_some()
	}
}

/// Polkadot-specific messages.
#[derive(Debug, Encode, Decode)]
pub enum Message {
	/// As a validator, tell the peer your current session key.
	// TODO: do this with a cryptographic proof of some kind
	// https://github.com/paritytech/polkadot/issues/47
	ValidatorId(ValidatorId),
	/// Requesting parachain proof-of-validation block (relay_parent, candidate_hash).
	RequestPovBlock(RequestId, Hash, Hash),
	/// Provide requested proof-of-validation block data by candidate hash or nothing if unknown.
	PovBlock(RequestId, Option<PoVBlock>),
	/// Tell a collator their role.
	CollatorRole(Role),
	/// A collation provided by a peer. Relay parent and collation.
	Collation(Hash, Collation),
}

fn send_polkadot_message(ctx: &mut dyn Context<Block>, to: PeerId, message: Message) {
	trace!(target: "p_net", "Sending polkadot message to {}: {:?}", to, message);
	let encoded = message.encode();
	ctx.send_chain_specific(to, encoded)
}

/// Polkadot protocol attachment for substrate.
pub struct PolkadotProtocol {
	peers: HashMap<PeerId, PeerInfo>,
	collating_for: Option<(CollatorId, ParaId)>,
	collators: CollatorPool,
	validators: HashMap<ValidatorId, PeerId>,
	local_collations: LocalCollations<Collation>,
	live_validation_leaves: LiveValidationLeaves,
	in_flight: HashMap<(RequestId, PeerId), PoVBlockRequest>,
	pending: Vec<PoVBlockRequest>,
	availability_store: Option<av_store::Store>,
	next_req_id: u64,
}

impl PolkadotProtocol {
	/// Instantiate a polkadot protocol handler.
	pub fn new(collating_for: Option<(CollatorId, ParaId)>) -> Self {
		PolkadotProtocol {
			peers: HashMap::new(),
			collators: CollatorPool::new(),
			collating_for,
			validators: HashMap::new(),
			local_collations: LocalCollations::new(),
			live_validation_leaves: LiveValidationLeaves::new(),
			in_flight: HashMap::new(),
			pending: Vec::new(),
			availability_store: None,
			next_req_id: 1,
		}
	}

	/// Fetch block data by candidate receipt.
	fn fetch_pov_block(
		&mut self,
		ctx: &mut dyn Context<Block>,
		candidate: &CandidateReceipt,
		relay_parent: Hash,
		canon_roots: StructuredUnroutedIngress,
	) -> oneshot::Receiver<PoVBlock> {
		let (tx, rx) = oneshot::channel();

		self.pending.push(PoVBlockRequest {
			attempted_peers: Default::default(),
			validation_leaf: relay_parent,
			candidate_hash: candidate.hash(),
			block_data_hash: candidate.block_data_hash,
			sender: tx,
			canon_roots,
		});

		self.dispatch_pending_requests(ctx);
		rx
	}

	/// Note new leaf to do validation work at
	fn new_validation_leaf_work(
		&mut self,
		ctx: &mut dyn Context<Block>,
		params: validation::LeafWorkParams,
	) -> validation::LiveValidationLeaf {

		let (work, new_local) = self.live_validation_leaves
			.new_validation_leaf(params);

		if let Some(new_local) = new_local {
			for (id, peer_data) in self.peers.iter_mut()
				.filter(|&(_, ref info)| info.should_send_key())
			{
				peer_data.collator_state.send_key(new_local.clone(), |msg| send_polkadot_message(
					ctx,
					id.clone(),
					msg
				));
			}
		}

		work
	}

	// true indicates that it was removed actually.
	fn remove_validation_session(&mut self, parent_hash: Hash) -> bool {
		self.live_validation_leaves.remove(parent_hash)
	}

	fn dispatch_pending_requests(&mut self, ctx: &mut dyn Context<Block>) {
		let mut new_pending = Vec::new();
		let validator_keys = &mut self.validators;
		let next_req_id = &mut self.next_req_id;
		let in_flight = &mut self.in_flight;

		for mut pending in ::std::mem::replace(&mut self.pending, Vec::new()) {
			let parent = pending.validation_leaf;
			let c_hash = pending.candidate_hash;

			let still_pending = self.live_validation_leaves.with_pov_block(&parent, &c_hash, |x| match x {
				Ok(data @ &_) => {
					// answer locally.
					let _ = pending.sender.send(data.clone());
					None
				}
				Err(Some(known_keys)) => {
					let next_peer = known_keys.iter()
						.filter_map(|x| validator_keys.get(x).map(|id| (x.clone(), id.clone())))
						.find(|&(ref key, _)| pending.attempted_peers.insert(key.clone()))
						.map(|(_, id)| id);

					// dispatch to peer
					if let Some(who) = next_peer {
						let req_id = *next_req_id;
						*next_req_id += 1;

						send_polkadot_message(
							ctx,
							who.clone(),
							Message::RequestPovBlock(req_id, parent, c_hash),
						);

						in_flight.insert((req_id, who), pending);

						None
					} else {
						Some(pending)
					}
				}
				Err(None) => None, // no such known validation leaf-work. prune out.
			});

			if let Some(pending) = still_pending {
				new_pending.push(pending);
			}
		}

		self.pending = new_pending;
	}

	fn on_polkadot_message(&mut self, ctx: &mut dyn Context<Block>, who: PeerId, msg: Message) {
		trace!(target: "p_net", "Polkadot message from {}: {:?}", who, msg);
		match msg {
			Message::ValidatorId(key) => self.on_session_key(ctx, who, key),
			Message::RequestPovBlock(req_id, relay_parent, candidate_hash) => {
				let pov_block = self.live_validation_leaves.with_pov_block(
					&relay_parent,
					&candidate_hash,
					|res| res.ok().map(|b| b.clone()),
				);

				send_polkadot_message(ctx, who, Message::PovBlock(req_id, pov_block));
			}
			Message::PovBlock(req_id, data) => self.on_pov_block(ctx, who, req_id, data),
			Message::Collation(relay_parent, collation) => self.on_collation(ctx, who, relay_parent, collation),
			Message::CollatorRole(role) => self.on_new_role(ctx, who, role),
		}
	}

	fn on_session_key(&mut self, ctx: &mut dyn Context<Block>, who: PeerId, key: ValidatorId) {
		{
			let info = match self.peers.get_mut(&who) {
				Some(peer) => peer,
				None => {
					trace!(target: "p_net", "Network inconsistency: message received from unconnected peer {}", who);
					return
				}
			};

			if !info.claimed_validator {
				ctx.report_peer(who, cost::UNEXPECTED_MESSAGE);
				return;
			}
			ctx.report_peer(who.clone(), benefit::EXPECTED_MESSAGE);

			let local_collations = &mut self.local_collations;
			let new_collations = match info.validator_keys.insert(key.clone()) {
				InsertedRecentKey::AlreadyKnown => Vec::new(),
				InsertedRecentKey::New(Some(old_key)) => {
					self.validators.remove(&old_key);
					local_collations.fresh_key(&old_key, &key)
				}
				InsertedRecentKey::New(None) => info.collator_state.role()
					.map(|r| local_collations.note_validator_role(key.clone(), r))
					.unwrap_or_else(Vec::new),
			};

			for (relay_parent, collation) in new_collations {
				send_polkadot_message(
					ctx,
					who.clone(),
					Message::Collation(relay_parent, collation),
				)
			}

			self.validators.insert(key, who);
		}

		self.dispatch_pending_requests(ctx);
	}

	fn on_pov_block(
		&mut self,
		ctx: &mut dyn Context<Block>,
		who: PeerId,
		req_id: RequestId,
		pov_block: Option<PoVBlock>,
	) {
		match self.in_flight.remove(&(req_id, who.clone())) {
			Some(mut req) => {
				match pov_block {
					Some(pov_block) => {
						match req.process_response(pov_block) {
							Ok(()) => {
								ctx.report_peer(who, benefit::GOOD_POV_BLOCK);
								return;
							}
							Err(r) => {
								ctx.report_peer(who, cost::BAD_POV_BLOCK);
								req = r;
							}
						}
					},
					None => {
						ctx.report_peer(who, benefit::EXPECTED_MESSAGE);
					}
				}

				self.pending.push(req);
				self.dispatch_pending_requests(ctx);
			}
			None => ctx.report_peer(who, cost::UNEXPECTED_MESSAGE),
		}
	}

	// when a validator sends us (a collator) a new role.
	fn on_new_role(&mut self, ctx: &mut dyn Context<Block>, who: PeerId, role: Role) {
		let info = match self.peers.get_mut(&who) {
			Some(peer) => peer,
			None => {
				trace!(target: "p_net", "Network inconsistency: message received from unconnected peer {}", who);
				return
			}
		};

		debug!(target: "p_net", "New collator role {:?} from {}", role, who);

		if info.validator_keys.as_slice().is_empty() {
			ctx.report_peer(who, cost::UNEXPECTED_ROLE)
		} else {
			// update role for all saved session keys for this validator.
			let local_collations = &mut self.local_collations;
			for (relay_parent, collation) in info.validator_keys
				.as_slice()
				.iter()
				.cloned()
				.flat_map(|k| local_collations.note_validator_role(k, role))
			{
				debug!(target: "p_net", "Broadcasting collation on relay parent {:?}", relay_parent);
				send_polkadot_message(
					ctx,
					who.clone(),
					Message::Collation(relay_parent, collation),
				)
			}
		}
	}

	/// Convert the given `CollatorId` to a `PeerId`.
	pub fn collator_id_to_peer_id(&self, collator_id: &CollatorId) -> Option<&PeerId> {
		self.collators.collator_id_to_peer_id(collator_id)
	}
}

impl Specialization<Block> for PolkadotProtocol {
	fn status(&self) -> Vec<u8> {
		Status { collating_for: self.collating_for.clone() }.encode()
	}

	fn on_connect(&mut self, ctx: &mut dyn Context<Block>, who: PeerId, status: FullStatus) {
		let local_status = Status::decode(&mut &status.chain_status[..])
			.unwrap_or_else(|_| Status { collating_for: None });

		let validator = status.roles.contains(sc_network::config::Roles::AUTHORITY);

		let mut peer_info = PeerInfo {
			collating_for: local_status.collating_for.clone(),
			validator_keys: Default::default(),
			claimed_validator: validator,
			collator_state: CollatorState::Fresh,
		};

		if let Some((ref acc_id, ref para_id)) = local_status.collating_for {
			if self.collator_peer(acc_id.clone()).is_some() {
				ctx.report_peer(who, cost::COLLATOR_ALREADY_KNOWN);
				return
			}
			ctx.report_peer(who.clone(), benefit::NEW_COLLATOR);

			let collator_role = self.collators.on_new_collator(
				acc_id.clone(),
				para_id.clone(),
				who.clone(),
			);

			peer_info.collator_state.set_role(collator_role, |msg| send_polkadot_message(
				ctx,
				who.clone(),
				msg,
			));
		}

		// send session keys.
		if peer_info.should_send_key() {
			for local_session_key in self.live_validation_leaves.recent_keys() {
				peer_info.collator_state.send_key(local_session_key.clone(), |msg| send_polkadot_message(
					ctx,
					who.clone(),
					msg,
				));
			}
		}

		self.peers.insert(who, peer_info);
		self.dispatch_pending_requests(ctx);
	}

	fn on_disconnect(&mut self, ctx: &mut dyn Context<Block>, who: PeerId) {
		if let Some(info) = self.peers.remove(&who) {
			if let Some((acc_id, _)) = info.collating_for {
				let new_primary = self.collators.on_disconnect(acc_id)
					.and_then(|new_primary| self.collator_peer(new_primary));

				if let Some((new_primary, primary_info)) = new_primary {
					primary_info.collator_state.set_role(Role::Primary, |msg| send_polkadot_message(
						ctx,
						new_primary.clone(),
						msg,
					));
				}
			}

			for key in info.validator_keys.as_slice().iter() {
				self.validators.remove(key);
				self.local_collations.on_disconnect(key);
			}

			{
				let pending = &mut self.pending;
				self.in_flight.retain(|&(_, ref peer), val| {
					let retain = peer != &who;
					if !retain {
						// swap with a dummy value which will be dropped immediately.
						let (sender, _) = oneshot::channel();
						pending.push(::std::mem::replace(val, PoVBlockRequest {
							attempted_peers: Default::default(),
							validation_leaf: Default::default(),
							candidate_hash: Default::default(),
							block_data_hash: Default::default(),
							canon_roots: StructuredUnroutedIngress(Vec::new()),
							sender,
						}));
					}

					retain
				});
			}
			self.dispatch_pending_requests(ctx);
		}
	}

	fn on_message(
		&mut self,
		ctx: &mut dyn Context<Block>,
		who: PeerId,
		message: Vec<u8>,
	) {
		match Message::decode(&mut &message[..]) {
			Ok(msg) => {
				ctx.report_peer(who.clone(), benefit::VALID_FORMAT);
				self.on_polkadot_message(ctx, who, msg)
			},
			Err(_) => {
				trace!(target: "p_net", "Bad message from {}", who);
				ctx.report_peer(who, cost::INVALID_FORMAT);
			}
		}
	}

	fn maintain_peers(&mut self, ctx: &mut dyn Context<Block>) {
		self.collators.collect_garbage(None);
		self.local_collations.collect_garbage(None);
		self.dispatch_pending_requests(ctx);

		for collator_action in self.collators.maintain_peers() {
			match collator_action {
				Action::Disconnect(collator) => self.disconnect_bad_collator(ctx, collator),
				Action::NewRole(account_id, role) => if let Some((collator, info)) = self.collator_peer(account_id) {
					info.collator_state.set_role(role, |msg| send_polkadot_message(
						ctx,
						collator.clone(),
						msg,
					))
				},
			}
		}
	}

	fn on_block_imported(&mut self, _ctx: &mut dyn Context<Block>, hash: Hash, header: &Header) {
		self.collators.collect_garbage(Some(&hash));
		self.local_collations.collect_garbage(Some(&header.parent_hash));
	}
}

impl PolkadotProtocol {
	// we received a collation from a peer
	fn on_collation(
		&mut self,
		ctx: &mut dyn Context<Block>,
		from: PeerId,
		relay_parent: Hash,
		collation: Collation
	) {
		let collation_para = collation.info.parachain_index;
		let collated_acc = collation.info.collator.clone();

		match self.peers.get(&from) {
			None => ctx.report_peer(from, cost::UNKNOWN_PEER),
			Some(peer_info) => {
				ctx.report_peer(from.clone(), benefit::KNOWN_PEER);
				match peer_info.collating_for {
					None => ctx.report_peer(from, cost::UNEXPECTED_MESSAGE),
					Some((ref acc_id, ref para_id)) => {
						ctx.report_peer(from.clone(), benefit::EXPECTED_MESSAGE);
						let structurally_valid = para_id == &collation_para && acc_id == &collated_acc;
						if structurally_valid && collation.info.check_signature().is_ok() {
							debug!(target: "p_net", "Received collation for parachain {:?} from peer {}", para_id, from);
							ctx.report_peer(from, benefit::GOOD_COLLATION);
							self.collators.on_collation(acc_id.clone(), relay_parent, collation)
						} else {
							ctx.report_peer(from, cost::INVALID_FORMAT)
						};
					}
				}
			},
		}
	}

	fn await_collation(&mut self, relay_parent: Hash, para_id: ParaId) -> oneshot::Receiver<Collation> {
		let (tx, rx) = oneshot::channel();
		debug!(target: "p_net", "Attempting to get collation for parachain {:?} on relay parent {:?}", para_id, relay_parent);
		self.collators.await_collation(relay_parent, para_id, tx);
		rx
	}

	// get connected peer with given account ID for collation.
	fn collator_peer(&mut self, collator_id: CollatorId) -> Option<(PeerId, &mut PeerInfo)> {
		let check_info = |info: &PeerInfo| info
			.collating_for
			.as_ref()
			.map_or(false, |&(ref acc_id, _)| acc_id == &collator_id);

		self.peers
			.iter_mut()
			.filter(|&(_, ref info)| check_info(&**info))
			.map(|(who, info)| (who.clone(), info))
			.next()
	}

	// disconnect a collator by account-id.
	fn disconnect_bad_collator(&mut self, ctx: &mut dyn Context<Block>, collator_id: CollatorId) {
		if let Some((who, _)) = self.collator_peer(collator_id) {
			ctx.report_peer(who, cost::BAD_COLLATION)
		}
	}
}

impl PolkadotProtocol {
	/// Add a local collation and broadcast it to the necessary peers.
	///
	/// This should be called by a collator intending to get the locally-collated
	/// block into the hands of validators.
	/// It also places the outgoing message and block data in the local availability store.
	pub fn add_local_collation(
		&mut self,
		ctx: &mut dyn Context<Block>,
		relay_parent: Hash,
		targets: HashSet<ValidatorId>,
		collation: Collation,
		outgoing_targeted: OutgoingMessages,
	) -> impl Future<Output = ()> {
		debug!(target: "p_net", "Importing local collation on relay parent {:?} and parachain {:?}",
			relay_parent, collation.info.parachain_index);

		for (primary, cloned_collation) in self.local_collations.add_collation(relay_parent, targets, collation.clone()) {
			match self.validators.get(&primary) {
				Some(who) => {
					debug!(target: "p_net", "Sending local collation to {:?}", primary);
					send_polkadot_message(
						ctx,
						who.clone(),
						Message::Collation(relay_parent, cloned_collation),
					)
				},
				None =>
					warn!(target: "polkadot_network", "Encountered tracked but disconnected validator {:?}", primary),
			}
		}

		let availability_store = self.availability_store.clone();
		let collation_cloned = collation.clone();

		async move {
			if let Some(availability_store) = availability_store {
				let _ = availability_store.make_available(av_store::Data {
					relay_parent,
					parachain_id: collation_cloned.info.parachain_index,
					block_data: collation_cloned.pov.block_data.clone(),
					outgoing_queues: Some(outgoing_targeted.clone().into()),
				}).await;
			}
		}
	}

	/// Give the network protocol a handle to an availability store, used for
	/// circulation of parachain data required for validation.
	pub fn register_availability_store(&mut self, availability_store: ::av_store::Store) {
		self.availability_store = Some(availability_store);
	}
}
