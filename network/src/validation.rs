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

//! The "validation session" networking code built on top of the base network service.
//!
//! This fulfills the `polkadot_validation::Network` trait, providing a hook to be called
//! each time a validation session begins on a new chain head.

use crate::gossip::GossipMessage;
use sr_primitives::traits::ProvideRuntimeApi;
use substrate_network::{PeerId, Context as NetContext};
use substrate_network::consensus_gossip::{
	self, TopicNotification, MessageRecipient as GossipMessageRecipient, ConsensusMessage,
};
use polkadot_validation::{
	Network as ParachainNetwork, SharedTable, Collators, Statement, GenericStatement, SignedStatement,
};
use polkadot_primitives::{Block, BlockId, Hash, SessionKey};
use polkadot_primitives::parachain::{
	Id as ParaId, Collation, Extrinsic, ParachainHost, CandidateReceipt, CollatorId,
	ValidatorId, PoVBlock, ValidatorIndex,
};

use futures::prelude::*;
use futures::future::{self, Executor as FutureExecutor};
use futures::sync::mpsc;
use futures::sync::oneshot::{self, Receiver};

use std::collections::hash_map::{HashMap, Entry};
use std::io;
use std::sync::Arc;

use arrayvec::ArrayVec;
use parking_lot::Mutex;
use log::{debug, warn};

use crate::router::Router;
use crate::gossip::{POLKADOT_ENGINE_ID, RegisteredMessageValidator, MessageValidationData};

use super::PolkadotProtocol;

pub use polkadot_validation::Incoming;

use parity_codec::{Encode, Decode};

/// An executor suitable for dispatching async consensus tasks.
pub trait Executor {
	fn spawn<F: Future<Item=(),Error=()> + Send + 'static>(&self, f: F);
}

/// A wrapped futures::future::Executor.
#[derive(Clone)]
pub struct WrappedExecutor<T>(pub T);

impl<T> Executor for WrappedExecutor<T>
	where T: FutureExecutor<Box<dyn Future<Item=(),Error=()> + Send + 'static>>
{
	fn spawn<F: Future<Item=(),Error=()> + Send + 'static>(&self, f: F) {
		if let Err(e) = self.0.execute(Box::new(f)) {
			warn!(target: "validation", "could not spawn consensus task: {:?}", e);
		}
	}
}

impl Executor for Arc<
	dyn futures::future::Executor<Box<dyn Future<Item = (), Error = ()> + Send>> + Send + Sync
> {
	fn spawn<F: Future<Item=(),Error=()> + Send + 'static>(&self, f: F) {
		let _ = FutureExecutor::execute(&**self, Box::new(f));
	}
}

/// A gossip network subservice.
pub trait GossipService {
	fn send_message(&mut self, ctx: &mut dyn NetContext<Block>, who: &PeerId, message: ConsensusMessage);
}

impl GossipService for consensus_gossip::ConsensusGossip<Block> {
	fn send_message(&mut self, ctx: &mut dyn NetContext<Block>, who: &PeerId, message: ConsensusMessage) {
		consensus_gossip::ConsensusGossip::send_message(self, ctx, who, message)
	}
}

/// A stream of gossip messages and an optional sender for a topic.
pub struct GossipMessageStream {
	topic_stream: mpsc::UnboundedReceiver<TopicNotification>,
}

impl GossipMessageStream {
	/// Create a new instance with the given topic stream.
	pub fn new(topic_stream: mpsc::UnboundedReceiver<TopicNotification>) -> Self {
		Self {
			topic_stream
		}
	}
}

impl Stream for GossipMessageStream {
	type Item = (GossipMessage, Option<PeerId>);
	type Error = ();

	fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
		loop {
			let msg = match futures::try_ready!(self.topic_stream.poll()) {
				Some(msg) => msg,
				None => return Ok(Async::Ready(None)),
			};

			debug!(target: "validation", "Processing statement for live validation session");
			if let Some(gmsg) = GossipMessage::decode(&mut &msg.message[..]) {
				return Ok(Async::Ready(Some((gmsg, msg.sender))))
			}
		}
	}
}

/// Basic functionality that a network has to fulfill.
pub trait NetworkService: Send + Sync + 'static {
	/// Get a stream of gossip messages for a given hash.
	fn gossip_messages_for(&self, topic: Hash) -> GossipMessageStream;

	/// Gossip a message on given topic.
	fn gossip_message(&self, topic: Hash, message: GossipMessage);

	/// Execute a closure with the gossip service.
	fn with_gossip<F: Send + 'static>(&self, with: F)
		where F: FnOnce(&mut dyn GossipService, &mut dyn NetContext<Block>);

	/// Execute a closure with the polkadot protocol.
	fn with_spec<F: Send + 'static>(&self, with: F)
		where F: FnOnce(&mut PolkadotProtocol, &mut dyn NetContext<Block>);
}

impl NetworkService for super::NetworkService {
	fn gossip_messages_for(&self, topic: Hash) -> GossipMessageStream {
		let (tx, rx) = std::sync::mpsc::channel();

		super::NetworkService::with_gossip(self, move |gossip, _| {
			let inner_rx = gossip.messages_for(POLKADOT_ENGINE_ID, topic);
			let _ = tx.send(inner_rx);
		});

		let topic_stream = match rx.recv() {
			Ok(rx) => rx,
			Err(_) => mpsc::unbounded().1, // return empty channel.
		};

		GossipMessageStream::new(topic_stream)
	}

	fn gossip_message(&self, topic: Hash, message: GossipMessage) {
		self.gossip_consensus_message(
			topic,
			POLKADOT_ENGINE_ID,
			message.encode(),
			GossipMessageRecipient::BroadcastToAll,
		);
	}

	fn with_gossip<F: Send + 'static>(&self, with: F)
		where F: FnOnce(&mut dyn GossipService, &mut dyn NetContext<Block>)
	{
		super::NetworkService::with_gossip(self, move |gossip, ctx| with(gossip, ctx))
	}

	fn with_spec<F: Send + 'static>(&self, with: F)
		where F: FnOnce(&mut PolkadotProtocol, &mut dyn NetContext<Block>)
	{
		super::NetworkService::with_spec(self, with)
	}
}

/// Params to a current validation session.
pub struct SessionParams {
	/// The local session key.
	pub local_session_key: Option<SessionKey>,
	/// The parent hash.
	pub parent_hash: Hash,
	/// The authorities.
	pub authorities: Vec<SessionKey>,
}

/// Wrapper around the network service
pub struct ValidationNetwork<P, E, N, T> {
	network: Arc<N>,
	api: Arc<P>,
	executor: T,
	message_validator: RegisteredMessageValidator,
	exit: E,
}

impl<P, E, N, T> ValidationNetwork<P, E, N, T> {
	/// Create a new consensus networking object.
	pub fn new(
		network: Arc<N>,
		exit: E,
		message_validator: RegisteredMessageValidator,
		api: Arc<P>,
		executor: T,
	) -> Self {
		ValidationNetwork { network, exit, message_validator, api, executor }
	}
}

impl<P, E: Clone, N, T: Clone> Clone for ValidationNetwork<P, E, N, T> {
	fn clone(&self) -> Self {
		ValidationNetwork {
			network: self.network.clone(),
			exit: self.exit.clone(),
			api: self.api.clone(),
			executor: self.executor.clone(),
			message_validator: self.message_validator.clone(),
		}
	}
}

impl<P, E, N, T> ValidationNetwork<P, E, N, T> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	E: Clone + Future<Item=(),Error=()> + Send + Sync + 'static,
	N: NetworkService,
	T: Clone + Executor + Send + Sync + 'static,
{
	/// Instantiate session data fetcher at a parent hash.
	///
	/// If the used session key is new, it will be broadcast to peers.
	/// If a validation session was already instantiated at this parent hash,
	/// the underlying instance will be shared.
	///
	/// If there was already a validation session instantiated and a different
	/// session key was set, then the new key will be ignored.
	///
	/// This implies that there can be multiple services intantiating validation
	/// session instances safely, but they should all be coordinated on which session keys
	/// are being used.
	pub fn instantiate_session(&self, params: SessionParams)
		-> oneshot::Receiver<SessionDataFetcher<P, E, N, T>>
	{
		let parent_hash = params.parent_hash;
		let network = self.network.clone();
		let api = self.api.clone();
		let task_executor = self.executor.clone();
		let exit = self.exit.clone();
		let message_validator = self.message_validator.clone();
		let index_mapping = params.authorities
			.iter()
			.enumerate()
			.map(|(i, k)| (i as ValidatorIndex, k.clone()))
			.collect();

		let (tx, rx) = oneshot::channel();

		{
			let message_validator = self.message_validator.clone();
			let authorities = params.authorities.clone();
			self.network.with_gossip(move |gossip, ctx| {
				message_validator.note_session(
					parent_hash,
					MessageValidationData { authorities, index_mapping },
					|peer_id, message| gossip.send_message(ctx, peer_id, message),
				);
			});
		}

		self.network.with_spec(move |spec, ctx| {
			let session = spec.new_validation_session(ctx, params);
			let _ = tx.send(SessionDataFetcher {
				network,
				api,
				task_executor,
				parent_hash,
				knowledge: session.knowledge().clone(),
				exit,
				message_validator,
			});
		});

		rx
	}
}

impl<P, E, N, T> ValidationNetwork<P, E, N, T> where N: NetworkService {
	/// Convert the given `CollatorId` to a `PeerId`.
	pub fn collator_id_to_peer_id(&self, collator_id: CollatorId) ->
		impl Future<Item=Option<PeerId>, Error=()> + Send
	{
		let (send, recv) = oneshot::channel();
		self.network.with_spec(move |spec, _| {
			let _ = send.send(spec.collator_id_to_peer_id(&collator_id).cloned());
		});
		recv.map_err(|_| ())
	}

	/// Create a `Stream` of checked statements for the given `relay_parent`.
	///
	/// The returned stream will not terminate, so it is required to make sure that the stream is
	/// dropped when it is not required anymore. Otherwise, it will stick around in memory
	/// infinitely.
	pub fn checked_statements(&self, relay_parent: Hash) -> impl Stream<Item=SignedStatement, Error=()> {
		crate::router::checked_statements(&*self.network, crate::router::attestation_topic(relay_parent))
	}
}

/// A long-lived network which can create parachain statement  routing processes on demand.
impl<P, E, N, T> ParachainNetwork for ValidationNetwork<P, E, N, T> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	E: Clone + Future<Item=(),Error=()> + Send + Sync + 'static,
	N: NetworkService,
	T: Clone + Executor + Send + Sync + 'static,
{
	type Error = String;
	type TableRouter = Router<P, E, N, T>;
	type BuildTableRouter = Box<dyn Future<Item=Self::TableRouter, Error=String> + Send>;

	fn communication_for(
		&self,
		table: Arc<SharedTable>,
		authorities: &[ValidatorId],
	) -> Self::BuildTableRouter {
		let parent_hash = *table.consensus_parent_hash();
		let local_session_key = table.session_key();

		let build_fetcher = self.instantiate_session(SessionParams {
			local_session_key: Some(local_session_key),
			parent_hash,
			authorities: authorities.to_vec(),
		});
		let message_validator = self.message_validator.clone();

		let executor = self.executor.clone();
		let work = build_fetcher
			.map_err(|e| format!("{:?}", e))
			.map(move |fetcher| {
				let table_router = Router::new(
					table,
					fetcher,
					message_validator,
				);

				let table_router_clone = table_router.clone();
				let work = table_router.checked_statements()
					.for_each(move |msg| { table_router_clone.import_statement(msg); Ok(()) });
				executor.spawn(work);

				table_router
			});

		Box::new(work)
	}
}

/// Error when the network appears to be down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkDown;

/// A future that resolves when a collation is received.
pub struct AwaitingCollation {
	outer: futures::sync::oneshot::Receiver<::futures::sync::oneshot::Receiver<Collation>>,
	inner: Option<::futures::sync::oneshot::Receiver<Collation>>
}

impl Future for AwaitingCollation {
	type Item = Collation;
	type Error = NetworkDown;

	fn poll(&mut self) -> Poll<Collation, NetworkDown> {
		if let Some(ref mut inner) = self.inner {
			return inner
				.poll()
				.map_err(|_| NetworkDown)
		}
		match self.outer.poll() {
			Ok(futures::Async::Ready(inner)) => {
				self.inner = Some(inner);
				self.poll()
			},
			Ok(futures::Async::NotReady) => Ok(futures::Async::NotReady),
			Err(_) => Err(NetworkDown)
		}
	}
}

impl<P, E: Clone, N, T: Clone> Collators for ValidationNetwork<P, E, N, T> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	N: NetworkService,
{
	type Error = NetworkDown;
	type Collation = AwaitingCollation;

	fn collate(&self, parachain: ParaId, relay_parent: Hash) -> Self::Collation {
		let (tx, rx) = ::futures::sync::oneshot::channel();
		self.network.with_spec(move |spec, _| {
			let collation = spec.await_collation(relay_parent, parachain);
			let _ = tx.send(collation);
		});
		AwaitingCollation{outer: rx, inner: None}
	}


	fn note_bad_collator(&self, collator: CollatorId) {
		self.network.with_spec(move |spec, ctx| spec.disconnect_bad_collator(ctx, collator));
	}
}

#[derive(Default)]
struct KnowledgeEntry {
	knows_block_data: Vec<ValidatorId>,
	knows_extrinsic: Vec<ValidatorId>,
	pov: Option<PoVBlock>,
	extrinsic: Option<Extrinsic>,
}

/// Tracks knowledge of peers.
pub(crate) struct Knowledge {
	candidates: HashMap<Hash, KnowledgeEntry>,
}

impl Knowledge {
	/// Create a new knowledge instance.
	pub(crate) fn new() -> Self {
		Knowledge {
			candidates: HashMap::new(),
		}
	}

	/// Note a statement seen from another validator.
	pub(crate) fn note_statement(&mut self, from: ValidatorId, statement: &Statement) {
		// those proposing the candidate or declaring it valid know everything.
		// those claiming it invalid do not have the extrinsic data as it is
		// generated by valid execution.
		match *statement {
			GenericStatement::Candidate(ref c) => {
				let entry = self.candidates.entry(c.hash()).or_insert_with(Default::default);
				entry.knows_block_data.push(from.clone());
				entry.knows_extrinsic.push(from);
			}
			GenericStatement::Valid(ref hash) => {
				let entry = self.candidates.entry(*hash).or_insert_with(Default::default);
				entry.knows_block_data.push(from.clone());
				entry.knows_extrinsic.push(from);
			}
			GenericStatement::Invalid(ref hash) => self.candidates.entry(*hash)
				.or_insert_with(Default::default)
				.knows_block_data
				.push(from),
		}
	}

	/// Note a candidate collated or seen locally.
	pub(crate) fn note_candidate(&mut self, hash: Hash, pov: Option<PoVBlock>, extrinsic: Option<Extrinsic>) {
		let entry = self.candidates.entry(hash).or_insert_with(Default::default);
		entry.pov = entry.pov.take().or(pov);
		entry.extrinsic = entry.extrinsic.take().or(extrinsic);
	}
}

/// receiver for incoming data.
#[derive(Clone)]
pub struct IncomingReceiver {
	inner: future::Shared<Receiver<Incoming>>
}

impl Future for IncomingReceiver {
	type Item = Incoming;
	type Error = io::Error;

	fn poll(&mut self) -> Poll<Incoming, io::Error> {
		match self.inner.poll() {
			Ok(Async::NotReady) => Ok(Async::NotReady),
			Ok(Async::Ready(i)) => Ok(Async::Ready(Incoming::clone(&*i))),
			Err(_) => Err(io::Error::new(
				io::ErrorKind::Other,
				"Sending end of channel hung up",
			)),
		}
	}
}

/// A current validation session instance.
#[derive(Clone)]
pub(crate) struct ValidationSession {
	parent_hash: Hash,
	knowledge: Arc<Mutex<Knowledge>>,
	local_session_key: Option<ValidatorId>,
}

impl ValidationSession {
	/// Create a new validation session instance. Needs to be attached to the
	/// network.
	pub(crate) fn new(params: SessionParams) -> Self {
		ValidationSession {
			parent_hash: params.parent_hash,
			knowledge: Arc::new(Mutex::new(Knowledge::new())),
			local_session_key: params.local_session_key,
		}
	}

	/// Get a handle to the shared knowledge relative to this consensus
	/// instance.
	pub(crate) fn knowledge(&self) -> &Arc<Mutex<Knowledge>> {
		&self.knowledge
	}

	// execute a closure with locally stored proof-of-validation for a candidate, or a slice of session identities
	// we believe should have the data.
	fn with_pov_block<F, U>(&self, hash: &Hash, f: F) -> U
		where F: FnOnce(Result<&PoVBlock, &[ValidatorId]>) -> U
	{
		let knowledge = self.knowledge.lock();
		let res = knowledge.candidates.get(hash)
			.ok_or(&[] as &_)
			.and_then(|entry| entry.pov.as_ref().ok_or(&entry.knows_block_data[..]));

		f(res)
	}
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
pub(crate) struct RecentValidatorIds {
	inner: ArrayVec<[ValidatorId; RECENT_SESSIONS]>,
}

impl RecentValidatorIds {
	/// Insert a new session key. This returns one to be pushed out if the
	/// set is full.
	pub(crate) fn insert(&mut self, key: ValidatorId) -> InsertedRecentKey {
		if self.inner.contains(&key) { return InsertedRecentKey::AlreadyKnown }

		let old = if self.inner.len() == RECENT_SESSIONS {
			Some(self.inner.remove(0))
		} else {
			None
		};

		self.inner.push(key);
		InsertedRecentKey::New(old)
	}

	/// As a slice.
	pub(crate) fn as_slice(&self) -> &[ValidatorId] {
		&*self.inner
	}

	fn remove(&mut self, key: &ValidatorId) {
		self.inner.retain(|k| k != key)
	}
}

/// Manages requests and keys for live validation session instances.
pub(crate) struct LiveValidationSessions {
	// recent local session keys.
	recent: RecentValidatorIds,
	// live validation session instances, on `parent_hash`. refcount retained alongside.
	live_instances: HashMap<Hash, (usize, ValidationSession)>,
}

impl LiveValidationSessions {
	/// Create a new `LiveValidationSessions`
	pub(crate) fn new() -> Self {
		LiveValidationSessions {
			recent: Default::default(),
			live_instances: HashMap::new(),
		}
	}

	/// Note new validation session. If the used session key is new,
	/// it returns it to be broadcasted to peers.
	///
	/// If there was already a validation session instantiated and a different
	/// session key was set, then the new key will be ignored.
	pub(crate) fn new_validation_session(
		&mut self,
		params: SessionParams,
	) -> (ValidationSession, Option<ValidatorId>) {
		let parent_hash = params.parent_hash;

		let key = params.local_session_key.clone();
		let recent = &mut self.recent;

		let mut check_new_key = || {
			let inserted_key = key.clone().map(|key| recent.insert(key));
			if let Some(InsertedRecentKey::New(_)) = inserted_key {
				key.clone()
			} else {
				None
			}
		};

		if let Some(&mut (ref mut rc, ref mut prev)) = self.live_instances.get_mut(&parent_hash) {
			let maybe_new = if prev.local_session_key.is_none() {
				prev.local_session_key = key.clone();
				check_new_key()
			} else {
				None
			};

			*rc += 1;
			return (prev.clone(), maybe_new)
		}

		let session = ValidationSession::new(params);
		self.live_instances.insert(parent_hash, (1, session.clone()));

		(session, check_new_key())
	}

	/// Remove validation session. true indicates that it was actually removed.
	pub(crate) fn remove(&mut self, parent_hash: Hash) -> bool {
		let maybe_removed = if let Entry::Occupied(mut entry) = self.live_instances.entry(parent_hash) {
			entry.get_mut().0 -= 1;
			if entry.get().0 == 0 {
				let (_, session) = entry.remove();
				Some(session)
			} else {
				None
			}
		} else {
			None
		};

		let session = match maybe_removed {
			None => return false,
			Some(s) => s,
		};

		if let Some(ref key) = session.local_session_key {
			let key_still_used = self.live_instances.values()
				.any(|c| c.1.local_session_key.as_ref() == Some(key));

			if !key_still_used {
				self.recent.remove(key)
			}
		}

		true
	}

	/// Recent session keys as a slice.
	pub(crate) fn recent_keys(&self) -> &[ValidatorId] {
		self.recent.as_slice()
	}

	/// Call a closure with pov-data from validation session at parent hash for a given
	/// candidate-receipt hash.
	///
	/// This calls the closure with `Some(data)` where the session and data are live,
	/// `Err(Some(keys))` when the session is live but the data unknown, with a list of keys
	/// who have the data, and `Err(None)` where the session is unknown.
	pub(crate) fn with_pov_block<F, U>(&self, parent_hash: &Hash, c_hash: &Hash, f: F) -> U
		where F: FnOnce(Result<&PoVBlock, Option<&[ValidatorId]>>) -> U
	{
		match self.live_instances.get(parent_hash) {
			Some(c) => c.1.with_pov_block(c_hash, |res| f(res.map_err(Some))),
			None => f(Err(None))
		}
	}
}

/// Receiver for block data.
pub struct PoVReceiver {
	outer: Receiver<Receiver<PoVBlock>>,
	inner: Option<Receiver<PoVBlock>>
}

impl Future for PoVReceiver {
	type Item = PoVBlock;
	type Error = io::Error;

	fn poll(&mut self) -> Poll<PoVBlock, io::Error> {
		let map_err = |_| io::Error::new(
			io::ErrorKind::Other,
			"Sending end of channel hung up",
		);

		if let Some(ref mut inner) = self.inner {
			return inner.poll().map_err(map_err);
		}
		match self.outer.poll().map_err(map_err)? {
			Async::Ready(inner) => {
				self.inner = Some(inner);
				self.poll()
			}
			Async::NotReady => Ok(Async::NotReady),
		}
	}
}

/// Can fetch data for a given validation session
pub struct SessionDataFetcher<P, E, N: NetworkService, T> {
	network: Arc<N>,
	api: Arc<P>,
	exit: E,
	task_executor: T,
	knowledge: Arc<Mutex<Knowledge>>,
	parent_hash: Hash,
	message_validator: RegisteredMessageValidator,
}

impl<P, E, N: NetworkService, T> SessionDataFetcher<P, E, N, T> {
	/// Get the parent hash.
	pub(crate) fn parent_hash(&self) -> Hash {
		self.parent_hash
	}

	/// Get the shared knowledge.
	pub(crate) fn knowledge(&self) -> &Arc<Mutex<Knowledge>> {
		&self.knowledge
	}

	/// Get the exit future.
	pub(crate) fn exit(&self) -> &E {
		&self.exit
	}

	/// Get the network service.
	pub(crate) fn network(&self) -> &Arc<N> {
		&self.network
	}

	/// Get the executor.
	pub(crate) fn executor(&self) -> &T {
		&self.task_executor
	}

	/// Get the runtime API.
	pub(crate) fn api(&self) -> &Arc<P> {
		&self.api
	}
}

impl<P, E: Clone, N: NetworkService, T: Clone> Clone for SessionDataFetcher<P, E, N, T> {
	fn clone(&self) -> Self {
		SessionDataFetcher {
			network: self.network.clone(),
			api: self.api.clone(),
			task_executor: self.task_executor.clone(),
			parent_hash: self.parent_hash,
			knowledge: self.knowledge.clone(),
			exit: self.exit.clone(),
			message_validator: self.message_validator.clone(),
		}
	}
}

impl<P: ProvideRuntimeApi + Send, E, N, T> SessionDataFetcher<P, E, N, T> where
	P::Api: ParachainHost<Block>,
	N: NetworkService,
	T: Clone + Executor + Send + 'static,
	E: Future<Item=(),Error=()> + Clone + Send + 'static,
{
	/// Fetch PoV block for the given candidate receipt.
	pub fn fetch_pov_block(&self, candidate: &CandidateReceipt) -> PoVReceiver {
		let parachain = candidate.parachain_index;
		let parent_hash = self.parent_hash;

		let canon_roots = self.api.runtime_api().ingress(&BlockId::hash(parent_hash), parachain)
			.map_err(|e|
				format!(
					"Cannot fetch ingress for parachain {:?} at {:?}: {:?}",
					parachain,
					parent_hash,
					e,
				)
			);

		let candidate = candidate.clone();
		let (tx, rx) = ::futures::sync::oneshot::channel();
		self.network.with_spec(move |spec, ctx| {
			if let Ok(Some(canon_roots)) = canon_roots {
				let inner_rx = spec.fetch_pov_block(ctx, &candidate, parent_hash, canon_roots);
				let _ = tx.send(inner_rx);
			}
		});
		PoVReceiver { outer: rx, inner: None }
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use substrate_primitives::crypto::UncheckedInto;

	#[test]
	fn last_keys_works() {
		let a: ValidatorId = [1; 32].unchecked_into();
		let b: ValidatorId = [2; 32].unchecked_into();
		let c: ValidatorId = [3; 32].unchecked_into();
		let d: ValidatorId = [4; 32].unchecked_into();

		let mut recent = RecentValidatorIds::default();

		match recent.insert(a.clone()) {
			InsertedRecentKey::New(None) => {},
			_ => panic!("is new, not at capacity"),
		}

		match recent.insert(a.clone()) {
			InsertedRecentKey::AlreadyKnown => {},
			_ => panic!("not new"),
		}

		match recent.insert(b.clone()) {
			InsertedRecentKey::New(None) => {},
			_ => panic!("is new, not at capacity"),
		}

		match recent.insert(b) {
			InsertedRecentKey::AlreadyKnown => {},
			_ => panic!("not new"),
		}

		match recent.insert(c.clone()) {
			InsertedRecentKey::New(None) => {},
			_ => panic!("is new, not at capacity"),
		}

		match recent.insert(c) {
			InsertedRecentKey::AlreadyKnown => {},
			_ => panic!("not new"),
		}

		match recent.insert(d.clone()) {
			InsertedRecentKey::New(Some(old)) => assert_eq!(old, a),
			_ => panic!("is new, and at capacity"),
		}

		match recent.insert(d) {
			InsertedRecentKey::AlreadyKnown => {},
			_ => panic!("not new"),
		}
	}

	#[test]
	fn add_new_sessions_works() {
		let mut live_sessions = LiveValidationSessions::new();
		let key_a: ValidatorId = [0; 32].unchecked_into();
		let key_b: ValidatorId = [1; 32].unchecked_into();
		let parent_hash = [0xff; 32].into();

		let (session, new_key) = live_sessions.new_validation_session(SessionParams {
			parent_hash,
			local_session_key: None,
			authorities: Vec::new(),
		});

		let knowledge = session.knowledge().clone();

		assert!(new_key.is_none());

		let (session, new_key) = live_sessions.new_validation_session(SessionParams {
			parent_hash,
			local_session_key: Some(key_a.clone()),
			authorities: Vec::new(),
		});

		// check that knowledge points to the same place.
		assert_eq!(&**session.knowledge() as *const _, &*knowledge as *const _);
		assert_eq!(new_key, Some(key_a.clone()));

		let (session, new_key) = live_sessions.new_validation_session(SessionParams {
			parent_hash,
			local_session_key: Some(key_b.clone()),
			authorities: Vec::new(),
		});

		assert_eq!(&**session.knowledge() as *const _, &*knowledge as *const _);
		assert!(new_key.is_none());
	}
}
