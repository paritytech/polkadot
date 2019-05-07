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

//! Polkadot-specific network implementation.
//!
//! This manages routing for parachain statements, parachain block and extrinsic data fetching,
//! communication between collators and validators, and more.

extern crate parity_codec as codec;
extern crate substrate_network;
extern crate substrate_primitives;
extern crate sr_primitives;

extern crate polkadot_validation;
extern crate polkadot_availability_store as av_store;
extern crate polkadot_primitives;

extern crate arrayvec;
extern crate parking_lot;
extern crate tokio;
extern crate slice_group_by;
extern crate exit_future;

#[macro_use]
extern crate futures;
#[macro_use]
extern crate log;
#[macro_use]
extern crate parity_codec_derive;

#[cfg(test)]
extern crate substrate_client;

#[cfg(test)]
extern crate substrate_keyring;

mod collator_pool;
mod local_collations;
mod router;
pub mod validation;
pub mod gossip;

use codec::{Decode, Encode};
use futures::sync::oneshot;
use polkadot_primitives::{Block, SessionKey, Hash, Header};
use polkadot_primitives::parachain::{
	Id as ParaId, BlockData, CollatorId, CandidateReceipt, Collation, PoVBlock,
	ConsolidatedIngressRoots,
};
use substrate_network::{PeerId, RequestId, Context};
use substrate_network::{message, generic_message};
use substrate_network::specialization::NetworkSpecialization as Specialization;
use substrate_network::StatusMessage as GenericFullStatus;
use self::validation::{LiveValidationSessions, RecentValidatorIds, InsertedRecentKey};
use self::collator_pool::{CollatorPool, Role, Action};
use self::local_collations::LocalCollations;

use std::collections::{HashMap, HashSet};

#[cfg(test)]
mod tests;

mod cost {
	pub(super) const UNEXPECTED_MESSAGE: i32 = -200;
	pub(super) const INVALID_FORMAT: i32 = -200;

	pub(super) const UNKNOWN_PEER: i32 = -50;
	pub(super) const COLLATOR_ALREADY_KNOWN: i32 = -100;
	pub(super) const BAD_COLLATION: i32 = -1000;
}

type FullStatus = GenericFullStatus<Block>;

/// Specialization of the network service for the polkadot protocol.
pub type NetworkService = ::substrate_network::Service<Block, PolkadotProtocol>;

/// Status of a Polkadot node.
#[derive(Debug, PartialEq, Eq, Clone, Encode, Decode)]
pub struct Status {
	collating_for: Option<(CollatorId, ParaId)>,
}

struct PoVBlockRequest {
	attempted_peers: HashSet<SessionKey>,
	validation_session_parent: Hash,
	candidate_hash: Hash,
	block_data_hash: Hash,
	sender: oneshot::Sender<PoVBlock>,
	canon_roots: ConsolidatedIngressRoots,
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
	fn send_key<F: FnMut(Message)>(&mut self, key: SessionKey, mut f: F) {
		f(Message::SessionKey(key));
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
	SessionKey(SessionKey),
	/// Requesting parachain proof-of-validation block (relay_parent, candidate_hash).
	RequestPovBlock(RequestId, Hash, Hash),
	/// Provide requested proof-of-validation block data by candidate hash or nothing if unknown.
	PovBlock(RequestId, Option<PoVBlock>),
	/// Request block data (relay_parent, candidate_hash)
	RequestBlockData(RequestId, Hash, Hash),
	/// Provide requested block data by candidate hash or nothing.
	BlockData(RequestId, Option<BlockData>),
	/// Tell a collator their role.
	CollatorRole(Role),
	/// A collation provided by a peer. Relay parent and collation.
	Collation(Hash, Collation),
}

fn send_polkadot_message(ctx: &mut Context<Block>, to: PeerId, message: Message) {
	trace!(target: "p_net", "Sending polkadot message to {}: {:?}", to, message);
	let encoded = message.encode();
	ctx.send_chain_specific(to, encoded)
}

/// Polkadot protocol attachment for substrate.
pub struct PolkadotProtocol {
	peers: HashMap<PeerId, PeerInfo>,
	collating_for: Option<(CollatorId, ParaId)>,
	collators: CollatorPool,
	validators: HashMap<SessionKey, PeerId>,
	local_collations: LocalCollations<Collation>,
	live_validation_sessions: LiveValidationSessions,
	in_flight: HashMap<(RequestId, PeerId), PoVBlockRequest>,
	pending: Vec<PoVBlockRequest>,
	extrinsic_store: Option<::av_store::Store>,
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
			live_validation_sessions: LiveValidationSessions::new(),
			in_flight: HashMap::new(),
			pending: Vec::new(),
			extrinsic_store: None,
			next_req_id: 1,
		}
	}

	/// Fetch block data by candidate receipt.
	fn fetch_pov_block(
		&mut self,
		ctx: &mut Context<Block>,
		candidate: &CandidateReceipt,
		relay_parent: Hash,
		canon_roots: ConsolidatedIngressRoots,
	) -> oneshot::Receiver<PoVBlock> {
		let (tx, rx) = oneshot::channel();

		self.pending.push(PoVBlockRequest {
			attempted_peers: Default::default(),
			validation_session_parent: relay_parent,
			candidate_hash: candidate.hash(),
			block_data_hash: candidate.block_data_hash,
			sender: tx,
			canon_roots,
		});

		self.dispatch_pending_requests(ctx);
		rx
	}

	/// Note new validation session.
	fn new_validation_session(
		&mut self,
		ctx: &mut Context<Block>,
		params: validation::SessionParams,
	) -> validation::ValidationSession {

		let (session, new_local) = self.live_validation_sessions
			.new_validation_session(params);

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

		session
	}

	// true indicates that it was removed actually.
	fn remove_validation_session(&mut self, parent_hash: Hash) -> bool {
		self.live_validation_sessions.remove(parent_hash)
	}

	fn dispatch_pending_requests(&mut self, ctx: &mut Context<Block>) {
		let mut new_pending = Vec::new();
		let validator_keys = &mut self.validators;
		let next_req_id = &mut self.next_req_id;
		let in_flight = &mut self.in_flight;

		for mut pending in ::std::mem::replace(&mut self.pending, Vec::new()) {
			let parent = pending.validation_session_parent;
			let c_hash = pending.candidate_hash;

			let still_pending = self.live_validation_sessions.with_pov_block(&parent, &c_hash, |x| match x {
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
				Err(None) => None, // no such known validation session. prune out.
			});

			if let Some(pending) = still_pending {
				new_pending.push(pending);
			}
		}

		self.pending = new_pending;
	}

	fn on_polkadot_message(&mut self, ctx: &mut Context<Block>, who: PeerId, msg: Message) {
		trace!(target: "p_net", "Polkadot message from {}: {:?}", who, msg);
		match msg {
			Message::SessionKey(key) => self.on_session_key(ctx, who, key),
			Message::RequestPovBlock(req_id, relay_parent, candidate_hash) => {
				let pov_block = self.live_validation_sessions.with_pov_block(
					&relay_parent,
					&candidate_hash,
					|res| res.ok().map(|b| b.clone()),
				);

				send_polkadot_message(ctx, who, Message::PovBlock(req_id, pov_block));
			}
			Message::RequestBlockData(req_id, relay_parent, candidate_hash) => {
				let block_data = self.live_validation_sessions
					.with_pov_block(
						&relay_parent,
						&candidate_hash,
						|res| res.ok().map(|b| b.block_data.clone()),
					)
					.or_else(|| self.extrinsic_store.as_ref()
						.and_then(|s| s.block_data(relay_parent, candidate_hash))
					);

				send_polkadot_message(ctx, who, Message::BlockData(req_id, block_data));
			}
			Message::PovBlock(req_id, data) => self.on_pov_block(ctx, who, req_id, data),
			Message::BlockData(_req_id, _data) => {
				// current block data is never requested bare by the node.
				ctx.report_peer(who, cost::UNEXPECTED_MESSAGE);
			}
			Message::Collation(relay_parent, collation) => self.on_collation(ctx, who, relay_parent, collation),
			Message::CollatorRole(role) => self.on_new_role(ctx, who, role),
		}
	}

	fn on_session_key(&mut self, ctx: &mut Context<Block>, who: PeerId, key: SessionKey) {
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
		ctx: &mut Context<Block>,
		who: PeerId,
		req_id: RequestId,
		pov_block: Option<PoVBlock>,
	) {
		match self.in_flight.remove(&(req_id, who.clone())) {
			Some(mut req) => {
				if let Some(pov_block) = pov_block {
					match req.process_response(pov_block) {
						Ok(()) => return,
						Err(r) => { req = r; }
					}
				}

				self.pending.push(req);
				self.dispatch_pending_requests(ctx);
			}
			None => ctx.report_peer(who, cost::UNEXPECTED_MESSAGE),
		}
	}

	// when a validator sends us (a collator) a new role.
	fn on_new_role(&mut self, ctx: &mut Context<Block>, who: PeerId, role: Role) {
		let info = match self.peers.get_mut(&who) {
			Some(peer) => peer,
			None => {
				trace!(target: "p_net", "Network inconsistency: message received from unconnected peer {}", who);
				return
			}
		};

		debug!(target: "p_net", "New collator role {:?} from {}", role, who);

		if info.validator_keys.as_slice().is_empty() {
			ctx.report_peer(who, cost::UNEXPECTED_MESSAGE);
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
}

impl Specialization<Block> for PolkadotProtocol {
	fn status(&self) -> Vec<u8> {
		Status { collating_for: self.collating_for.clone() }.encode()
	}

	fn on_connect(&mut self, ctx: &mut Context<Block>, who: PeerId, status: FullStatus) {
		let local_status = match Status::decode(&mut &status.chain_status[..]) {
			Some(status) => status,
			None => {
				Status { collating_for: None }
			}
		};

		let validator = status.roles.contains(substrate_network::config::Roles::AUTHORITY);

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

			let collator_role = self.collators.on_new_collator(acc_id.clone(), para_id.clone());

			peer_info.collator_state.set_role(collator_role, |msg| send_polkadot_message(
				ctx,
				who.clone(),
				msg,
			));
		}

		// send session keys.
		if peer_info.should_send_key() {
			for local_session_key in self.live_validation_sessions.recent_keys() {
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

	fn on_disconnect(&mut self, ctx: &mut Context<Block>, who: PeerId) {
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
							validation_session_parent: Default::default(),
							candidate_hash: Default::default(),
							block_data_hash: Default::default(),
							canon_roots: ConsolidatedIngressRoots(Vec::new()),
							sender,
						}));
					}

					retain
				});
			}
			self.dispatch_pending_requests(ctx);
		}
	}

	fn on_message(&mut self, ctx: &mut Context<Block>, who: PeerId, message: &mut Option<message::Message<Block>>) {
		match message.take() {
			Some(generic_message::Message::ChainSpecific(raw)) => {
				match Message::decode(&mut raw.as_slice()) {
					Some(msg) => self.on_polkadot_message(ctx, who, msg),
					None => {
						trace!(target: "p_net", "Bad message from {}", who);
						ctx.report_peer(who, cost::INVALID_FORMAT);
						*message = Some(generic_message::Message::ChainSpecific(raw));
					}
				}
			}
			Some(other) => *message = Some(other),
			_ => {}
		}
	}

	fn on_abort(&mut self) { }

	fn maintain_peers(&mut self, ctx: &mut Context<Block>) {
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

	fn on_block_imported(&mut self, _ctx: &mut Context<Block>, hash: Hash, header: &Header) {
		self.collators.collect_garbage(Some(&hash));
		self.local_collations.collect_garbage(Some(&header.parent_hash));
	}
}

impl PolkadotProtocol {
	// we received a collation from a peer
	fn on_collation(&mut self, ctx: &mut Context<Block>, from: PeerId, relay_parent: Hash, collation: Collation) {
		let collation_para = collation.receipt.parachain_index;
		let collated_acc = collation.receipt.collator.clone();

		match self.peers.get(&from) {
			None => ctx.report_peer(from, cost::UNKNOWN_PEER),
			Some(peer_info) => match peer_info.collating_for {
				None => ctx.report_peer(from, cost::UNEXPECTED_MESSAGE),
				Some((ref acc_id, ref para_id)) => {
					let structurally_valid = para_id == &collation_para && acc_id == &collated_acc;
					if structurally_valid && collation.receipt.check_signature().is_ok() {
						debug!(target: "p_net", "Received collation for parachain {:?} from peer {}", para_id, from);
						self.collators.on_collation(acc_id.clone(), relay_parent, collation)
					} else {
						ctx.report_peer(from, cost::INVALID_FORMAT)
					};
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
	fn disconnect_bad_collator(&mut self, ctx: &mut Context<Block>, collator_id: CollatorId) {
		if let Some((who, _)) = self.collator_peer(collator_id) {
			ctx.report_peer(who, cost::BAD_COLLATION)
		}
	}
}

impl PolkadotProtocol {
	/// Add a local collation and broadcast it to the necessary peers.
	pub fn add_local_collation(
		&mut self,
		ctx: &mut Context<Block>,
		relay_parent: Hash,
		targets: HashSet<SessionKey>,
		collation: Collation,
	) {
		debug!(target: "p_net", "Importing local collation on relay parent {:?} and parachain {:?}",
			relay_parent, collation.receipt.parachain_index);

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
	}

	/// register availability store.
	pub fn register_availability_store(&mut self, extrinsic_store: ::av_store::Store) {
		self.extrinsic_store = Some(extrinsic_store);
	}
}
