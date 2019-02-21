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

use sr_primitives::traits::{BlakeTwo256, ProvideRuntimeApi, Hash as HashT};
use substrate_network::{consensus_gossip::ConsensusMessage, Context as NetContext};
use polkadot_validation::{Network as ParachainNetwork, SharedTable, Collators, Statement, GenericStatement};
use polkadot_primitives::{AccountId, Block, Hash, SessionKey};
use polkadot_primitives::parachain::{
	Id as ParaId, Collation, Extrinsic, ParachainHost, BlockData, Message, CandidateReceipt,
};
use codec::{Encode, Decode};

use futures::prelude::*;
use futures::future::{self, Executor as FutureExecutor};
use futures::sync::mpsc;
use futures::sync::oneshot::{self, Receiver};

use std::collections::hash_map::{HashMap, Entry};
use std::io;
use std::sync::Arc;

use arrayvec::ArrayVec;
use tokio::runtime::TaskExecutor;
use parking_lot::Mutex;

use router::Router;
use super::PolkadotProtocol;

pub use polkadot_validation::Incoming;

/// An executor suitable for dispatching async consensus tasks.
pub trait Executor {
	fn spawn<F: Future<Item=(),Error=()> + Send + 'static>(&self, f: F);
}

/// A wrapped futures::future::Executor.
pub struct WrappedExecutor<T>(pub T);

impl<T> Executor for WrappedExecutor<T>
	where T: FutureExecutor<Box<Future<Item=(),Error=()> + Send + 'static>>
{
	fn spawn<F: Future<Item=(),Error=()> + Send + 'static>(&self, f: F) {
		if let Err(e) = self.0.execute(Box::new(f)) {
			warn!(target: "validation", "could not spawn consensus task: {:?}", e);
		}
	}
}

impl Executor for TaskExecutor {
	fn spawn<F: Future<Item=(),Error=()> + Send + 'static>(&self, f: F) {
		TaskExecutor::spawn(self, f)
	}
}

/// Basic functionality that a network has to fulfill.
pub trait NetworkService: Send + Sync + 'static {
	/// Get a stream of gossip messages for a given hash.
	fn gossip_messages_for(&self, topic: Hash) -> mpsc::UnboundedReceiver<ConsensusMessage>;

	/// Gossip a message on given topic.
	fn gossip_message(&self, topic: Hash, message: Vec<u8>);

	/// Drop a gossip topic.
	fn drop_gossip(&self, topic: Hash);

	/// Execute a closure with the polkadot protocol.
	fn with_spec<F: Send + 'static>(&self, with: F)
		where F: FnOnce(&mut PolkadotProtocol, &mut NetContext<Block>);
}

impl NetworkService for super::NetworkService {
	fn gossip_messages_for(&self, topic: Hash) -> mpsc::UnboundedReceiver<ConsensusMessage> {
		let (tx, rx) = std::sync::mpsc::channel();

		self.with_gossip(move |gossip, _| {
			let inner_rx = gossip.messages_for(topic);
			let _ = tx.send(inner_rx);
		});

		match rx.recv() {
			Ok(rx) => rx,
			Err(_) => mpsc::unbounded().1, // return empty channel.
		}
	}

	fn gossip_message(&self, topic: Hash, message: Vec<u8>) {
		self.gossip_consensus_message(topic, message, false);
	}

	fn drop_gossip(&self, topic: Hash) {
		self.with_gossip(move |gossip, _| {
			gossip.collect_garbage_for_topic(topic);
		})
	}

	fn with_spec<F: Send + 'static>(&self, with: F)
		where F: FnOnce(&mut PolkadotProtocol, &mut NetContext<Block>)
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
}

/// Wrapper around the network service
pub struct ValidationNetwork<P, E, N, T> {
	network: Arc<N>,
	api: Arc<P>,
	executor: T,
	exit: E,
}

impl<P, E, N, T> ValidationNetwork<P, E, N, T> {
	/// Create a new validation session networking object.
	pub fn new(network: Arc<N>, exit: E, api: Arc<P>, executor: T) -> Self {
		ValidationNetwork { network, exit, api, executor }
	}
}

impl<P, E: Clone, N, T: Clone> Clone for ValidationNetwork<P, E, N, T> {
	fn clone(&self) -> Self {
		ValidationNetwork {
			network: self.network.clone(),
			exit: self.exit.clone(),
			api: self.api.clone(),
			executor: self.executor.clone(),
		}
	}
}

impl<P, E, N, T> ValidationNetwork<P, E, N, T> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	E: Clone + Future<Item=(),Error=()> + Send + 'static,
	N: NetworkService,
	T: Clone + Executor + Send + 'static,
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

		let (tx, rx) = oneshot::channel();
		self.network.with_spec(move |spec, ctx| {
			let session = spec.new_validation_session(ctx, params);
			let _ = tx.send(SessionDataFetcher {
				network,
				api,
				task_executor,
				parent_hash,
				knowledge: session.knowledge().clone(),
				exit,
				fetch_incoming: session.fetched_incoming().clone(),
			});
		});

		rx
	}
}

/// A long-lived network which can create parachain statement  routing processes on demand.
impl<P, E, N, T> ParachainNetwork for ValidationNetwork<P, E, N, T> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	E: Clone + Future<Item=(),Error=()> + Send + 'static,
	N: NetworkService,
	T: Clone + Executor + Send + 'static,
{
	type Error = String;
	type TableRouter = Router<P, E, N, T>;
	type BuildTableRouter = Box<Future<Item=Self::TableRouter,Error=String> + Send>;

	fn communication_for(
		&self,
		table: Arc<SharedTable>,
		outgoing: polkadot_validation::Outgoing,
	) -> Self::BuildTableRouter {
		let parent_hash = table.consensus_parent_hash().clone();
		let local_session_key = table.session_key();

		let build_fetcher = self.instantiate_session(SessionParams {
			local_session_key: Some(local_session_key),
			parent_hash,
		});

		let executor = self.executor.clone();
		let work = build_fetcher
			.map_err(|e| format!("{:?}", e))
			.map(move |fetcher| {
				let table_router = Router::new(
					table,
					fetcher,
				);

				table_router.broadcast_egress(outgoing);

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
	outer: ::futures::sync::oneshot::Receiver<::futures::sync::oneshot::Receiver<Collation>>,
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
		if let Ok(futures::Async::Ready(mut inner)) = self.outer.poll() {
			let poll_result = inner.poll();
			self.inner = Some(inner);
			return poll_result.map_err(|_| NetworkDown)
		}
		Ok(futures::Async::NotReady)
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


	fn note_bad_collator(&self, collator: AccountId) {
		self.network.with_spec(move |spec, ctx| spec.disconnect_bad_collator(ctx, collator));
	}
}

#[derive(Default)]
struct KnowledgeEntry {
	knows_block_data: Vec<SessionKey>,
	knows_extrinsic: Vec<SessionKey>,
	block_data: Option<BlockData>,
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
	pub(crate) fn note_statement(&mut self, from: SessionKey, statement: &Statement) {
		// those proposing the candidate or declaring it valid know everything.
		// those claiming it invalid do not have the extrinsic data as it is
		// generated by valid execution.
		match *statement {
			GenericStatement::Candidate(ref c) => {
				let mut entry = self.candidates.entry(c.hash()).or_insert_with(Default::default);
				entry.knows_block_data.push(from);
				entry.knows_extrinsic.push(from);
			}
			GenericStatement::Valid(ref hash) => {
				let mut entry = self.candidates.entry(*hash).or_insert_with(Default::default);
				entry.knows_block_data.push(from);
				entry.knows_extrinsic.push(from);
			}
			GenericStatement::Invalid(ref hash) => self.candidates.entry(*hash)
				.or_insert_with(Default::default)
				.knows_block_data
				.push(from),
		}
	}

	/// Note a candidate collated or seen locally.
	pub(crate) fn note_candidate(&mut self, hash: Hash, block_data: Option<BlockData>, extrinsic: Option<Extrinsic>) {
		let entry = self.candidates.entry(hash).or_insert_with(Default::default);
		entry.block_data = entry.block_data.take().or(block_data);
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

/// Incoming message gossip topic for a parachain at a given block hash.
pub(crate) fn incoming_message_topic(parent_hash: Hash, parachain: ParaId) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	parachain.using_encoded(|s| v.extend(s));
	v.extend(b"incoming");

	BlakeTwo256::hash(&v[..])
}

/// A current validation session instance.
#[derive(Clone)]
pub(crate) struct ValidationSession {
	parent_hash: Hash,
	knowledge: Arc<Mutex<Knowledge>>,
	local_session_key: Option<SessionKey>,
	fetch_incoming: Arc<Mutex<HashMap<ParaId, IncomingReceiver>>>,
}

impl ValidationSession {
	/// Create a new validation session instance. Needs to be attached to the
	/// nework.
	pub(crate) fn new(params: SessionParams) -> Self {
		ValidationSession {
			parent_hash: params.parent_hash,
			knowledge: Arc::new(Mutex::new(Knowledge::new())),
			local_session_key: params.local_session_key,
			fetch_incoming: Arc::new(Mutex::new(HashMap::new())),
		}
	}

	/// Get a handle to the shared knowledge relative to this consensus
	/// instance.
	pub(crate) fn knowledge(&self) -> &Arc<Mutex<Knowledge>> {
		&self.knowledge
	}

	/// Get a handle to the shared list of parachains' incoming data fetch.
	pub(crate) fn fetched_incoming(&self) -> &Arc<Mutex<HashMap<ParaId, IncomingReceiver>>> {
		&self.fetch_incoming
	}

	// execute a closure with locally stored block data for a candidate, or a slice of session identities
	// we believe should have the data.
	fn with_block_data<F, U>(&self, hash: &Hash, f: F) -> U
		where F: FnOnce(Result<&BlockData, &[SessionKey]>) -> U
	{
		let knowledge = self.knowledge.lock();
		let res = knowledge.candidates.get(hash)
			.ok_or(&[] as &_)
			.and_then(|entry| entry.block_data.as_ref().ok_or(&entry.knows_block_data[..]));

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
	New(Option<SessionKey>),
}

/// Wrapper for managing recent session keys.
#[derive(Default)]
pub(crate) struct RecentSessionKeys {
	inner: ArrayVec<[SessionKey; RECENT_SESSIONS]>,
}

impl RecentSessionKeys {
	/// Insert a new session key. This returns one to be pushed out if the
	/// set is full.
	pub(crate) fn insert(&mut self, key: SessionKey) -> InsertedRecentKey {
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
	pub(crate) fn as_slice(&self) -> &[SessionKey] {
		&*self.inner
	}

	fn remove(&mut self, key: &SessionKey) {
		self.inner.retain(|k| k != key)
	}
}

/// Manages requests and keys for live validation session instances.
pub(crate) struct LiveValidationSessions {
	// recent local session keys.
	recent: RecentSessionKeys,
	// live validation session instances, on `parent_hash`.
	live_instances: HashMap<Hash, ValidationSession>,
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
	) -> (ValidationSession, Option<SessionKey>) {
		let parent_hash = params.parent_hash.clone();

		let key = params.local_session_key;
		let recent = &mut self.recent;

		let mut check_new_key = move || {
			let inserted_key = key.map(|key|recent.insert(key));
			if let Some(InsertedRecentKey::New(_)) = inserted_key {
				key
			} else {
				None
			}
		};

		if let Some(prev) = self.live_instances.get_mut(&parent_hash) {
			let maybe_new = if prev.local_session_key.is_none() {
				prev.local_session_key = key;
				check_new_key()
			} else {
				None
			};
			return (prev.clone(), maybe_new)
		}

		let session = ValidationSession::new(params);
		self.live_instances.insert(parent_hash, session.clone());

		(session, check_new_key())
	}

	/// Remove validation session.
	pub(crate) fn remove(&mut self, parent_hash: &Hash) {
		if let Some(session) = self.live_instances.remove(parent_hash) {
			if let Some(ref key) = session.local_session_key {
				let key_still_used = self.live_instances.values()
					.any(|c| c.local_session_key.as_ref() == Some(key));

				if !key_still_used {
					self.recent.remove(key)
				}
			}
		}
	}

	/// Recent session keys as a slice.
	pub(crate) fn recent_keys(&self) -> &[SessionKey] {
		self.recent.as_slice()
	}

	/// Call a closure with block data from validation session at parent hash.
	///
	/// This calls the closure with `Some(data)` where the session and data are live,
	/// `Err(Some(keys))` when the session is live but the data unknown, with a list of keys
	/// who have the data, and `Err(None)` where the session is unknown.
	pub(crate) fn with_block_data<F, U>(&self, parent_hash: &Hash, c_hash: &Hash, f: F) -> U
		where F: FnOnce(Result<&BlockData, Option<&[SessionKey]>>) -> U
	{
		match self.live_instances.get(parent_hash) {
			Some(c) => c.with_block_data(c_hash, |res| f(res.map_err(Some))),
			None => f(Err(None))
		}
	}
}

/// Receiver for block data.
pub struct BlockDataReceiver {
	outer: Receiver<Receiver<BlockData>>,
	inner: Option<Receiver<BlockData>>
}

impl Future for BlockDataReceiver {
	type Item = BlockData;
	type Error = io::Error;

	fn poll(&mut self) -> Poll<BlockData, io::Error> {
		let map_err = |_| io::Error::new(
			io::ErrorKind::Other,
			"Sending end of channel hung up",
		);

		if let Some(ref mut inner) = self.inner {
			return inner.poll().map_err(map_err);
		}
		match self.outer.poll().map_err(map_err)? {
			Async::Ready(mut inner) => {
				let poll_result = inner.poll();
				self.inner = Some(inner);
				poll_result.map_err(map_err)
			}
			Async::NotReady => Ok(Async::NotReady),
		}
	}
}

/// Can fetch data for a given validation session
pub struct SessionDataFetcher<P, E, N: NetworkService, T> {
	network: Arc<N>,
	api: Arc<P>,
	fetch_incoming: Arc<Mutex<HashMap<ParaId, IncomingReceiver>>>,
	exit: E,
	task_executor: T,
	knowledge: Arc<Mutex<Knowledge>>,
	parent_hash: Hash,
}

impl<P, E, N: NetworkService, T> SessionDataFetcher<P, E, N, T> {
	/// Get the parent hash.
	pub(crate) fn parent_hash(&self) -> Hash {
		self.parent_hash.clone()
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
			parent_hash: self.parent_hash.clone(),
			fetch_incoming: self.fetch_incoming.clone(),
			knowledge: self.knowledge.clone(),
			exit: self.exit.clone(),
		}
	}
}

impl<P: ProvideRuntimeApi + Send, E, N, T> SessionDataFetcher<P, E, N, T> where
	P::Api: ParachainHost<Block>,
	N: NetworkService,
	T: Clone + Executor + Send + 'static,
	E: Future<Item=(),Error=()> + Clone + Send + 'static,
{
	/// Fetch block data for the given candidate receipt.
	pub fn fetch_block_data(&self, candidate: &CandidateReceipt) -> BlockDataReceiver {
		let parent_hash = self.parent_hash;
		let candidate = candidate.clone();
		let (tx, rx) = ::futures::sync::oneshot::channel();
		self.network.with_spec(move |spec, ctx| {
			let inner_rx = spec.fetch_block_data(ctx, &candidate, parent_hash);
			let _ = tx.send(inner_rx);
		});
		BlockDataReceiver { outer: rx, inner: None }
	}

	/// Fetch incoming messages for a parachain.
	pub fn fetch_incoming(&self, parachain: ParaId) -> IncomingReceiver {
		use polkadot_primitives::BlockId;
		let (tx, rx) = {
			let mut fetching = self.fetch_incoming.lock();
			match fetching.entry(parachain) {
				Entry::Occupied(entry) => return entry.get().clone(),
				Entry::Vacant(entry) => {
					// has not been requested yet.
					let (tx, rx) = oneshot::channel();
					let rx = IncomingReceiver { inner: rx.shared() };
					entry.insert(rx.clone());

					(tx, rx)
				}
			}
		};

		let parent_hash = self.parent_hash();
		let topic = incoming_message_topic(parent_hash, parachain);
		let gossip_messages = self.network().gossip_messages_for(topic)
			.map_err(|()| panic!("unbounded receivers do not throw errors; qed"))
			.filter_map(|msg| IngressPair::decode(&mut msg.as_slice()));

		let canon_roots = self.api.runtime_api().ingress(&BlockId::hash(parent_hash), parachain)
			.map_err(|e| format!("Cannot fetch ingress for parachain {:?} at {:?}: {:?}",
				parachain, parent_hash, e)
			);

		let work = canon_roots.into_future()
			.and_then(move |ingress_roots| match ingress_roots {
				None => Err(format!("No parachain {:?} registered at {}", parachain, parent_hash)),
				Some(roots) => Ok(roots.into_iter().collect())
			})
			.and_then(move |ingress_roots| ComputeIngress {
				inner: gossip_messages,
				ingress_roots,
				incoming: Vec::new(),
			})
			.map(move |incoming| if let Some(i) = incoming { let _ = tx.send(i); })
			.select2(self.exit.clone())
			.then(|_| Ok(()));

		self.task_executor.spawn(work);

		rx
	}
}

impl<P, E, N: NetworkService, T> Drop for SessionDataFetcher<P, E, N, T> {
	fn drop(&mut self) {
		let parent_hash = self.parent_hash();
		self.network.with_spec(move |spec, _| spec.remove_validation_session(&parent_hash));

		{
			let mut incoming_fetched = self.fetch_incoming.lock();
			for (para_id, _) in incoming_fetched.drain() {
				self.network.drop_gossip(incoming_message_topic(
					self.parent_hash,
					para_id,
				));
			}
		}
	}
}

type IngressPair = (ParaId, Vec<Message>);

// computes ingress from incoming stream of messages.
// returns `None` if the stream concludes too early.
#[must_use = "futures do nothing unless polled"]
struct ComputeIngress<S> {
	ingress_roots: HashMap<ParaId, Hash>,
	incoming: Vec<IngressPair>,
	inner: S,
}

impl<S> Future for ComputeIngress<S> where S: Stream<Item=IngressPair> {
	type Item = Option<Incoming>;
	type Error = S::Error;

	fn poll(&mut self) -> Poll<Option<Incoming>, Self::Error> {
		loop {
			if self.ingress_roots.is_empty() {
				return Ok(Async::Ready(
					Some(::std::mem::replace(&mut self.incoming, Vec::new()))
				))
			}

			let (para_id, messages) = match try_ready!(self.inner.poll()) {
				None => return Ok(Async::Ready(None)),
				Some(next) => next,
			};

			match self.ingress_roots.entry(para_id) {
				Entry::Vacant(_) => continue,
				Entry::Occupied(occupied) => {
					let canon_root = occupied.get().clone();
					let messages = messages.iter().map(|m| &m.0[..]);
					if ::polkadot_validation::message_queue_root(messages) != canon_root {
						continue;
					}

					occupied.remove();
				}
			}

			let pos = self.incoming.binary_search_by_key(
				&para_id,
				|&(id, _)| id,
			)
				.err()
				.expect("incoming starts empty and only inserted when \
					para_id not inserted before; qed");

			self.incoming.insert(pos, (para_id, messages));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures::stream;

	#[test]
	fn last_keys_works() {
		let a = [1; 32].into();
		let b = [2; 32].into();
		let c = [3; 32].into();
		let d = [4; 32].into();

		let mut recent = RecentSessionKeys::default();

		match recent.insert(a) {
			InsertedRecentKey::New(None) => {},
			_ => panic!("is new, not at capacity"),
		}

		match recent.insert(a) {
			InsertedRecentKey::AlreadyKnown => {},
			_ => panic!("not new"),
		}

		match recent.insert(b) {
			InsertedRecentKey::New(None) => {},
			_ => panic!("is new, not at capacity"),
		}

		match recent.insert(b) {
			InsertedRecentKey::AlreadyKnown => {},
			_ => panic!("not new"),
		}

		match recent.insert(c) {
			InsertedRecentKey::New(None) => {},
			_ => panic!("is new, not at capacity"),
		}

		match recent.insert(c) {
			InsertedRecentKey::AlreadyKnown => {},
			_ => panic!("not new"),
		}

		match recent.insert(d) {
			InsertedRecentKey::New(Some(old)) => assert_eq!(old, a),
			_ => panic!("is new, and at capacity"),
		}

		match recent.insert(d) {
			InsertedRecentKey::AlreadyKnown => {},
			_ => panic!("not new"),
		}
	}

	#[test]
	fn compute_ingress_works() {
		let actual_messages = [
			(
				ParaId::from(1),
				vec![Message(vec![1, 3, 5, 6]), Message(vec![4, 4, 4, 4])],
			),
			(
				ParaId::from(2),
				vec![
					Message(vec![1, 3, 7, 9, 1, 2, 3, 4, 5, 6]),
					Message(b"hello world".to_vec()),
				],
			),
			(
				ParaId::from(5),
				vec![Message(vec![1, 2, 3, 4, 5]), Message(vec![6, 9, 6, 9])],
			),
		];

		let roots: HashMap<_, _> = actual_messages.iter()
			.map(|&(para_id, ref messages)| (
				para_id,
				::polkadot_validation::message_queue_root(messages.iter().map(|m| &m.0)),
			))
			.collect();

		let inputs = [
			(
				ParaId::from(1), // wrong message.
				vec![Message(vec![1, 1, 2, 2]), Message(vec![3, 3, 4, 4])],
			),
			(
				ParaId::from(1),
				vec![Message(vec![1, 3, 5, 6]), Message(vec![4, 4, 4, 4])],
			),
			(
				ParaId::from(1), // duplicate
				vec![Message(vec![1, 3, 5, 6]), Message(vec![4, 4, 4, 4])],
			),

			(
				ParaId::from(5), // out of order
				vec![Message(vec![1, 2, 3, 4, 5]), Message(vec![6, 9, 6, 9])],
			),
			(
				ParaId::from(1234), // un-routed parachain.
				vec![Message(vec![9, 9, 9, 9])],
			),
			(
				ParaId::from(2),
				vec![
					Message(vec![1, 3, 7, 9, 1, 2, 3, 4, 5, 6]),
					Message(b"hello world".to_vec()),
				],
			),
		];
		let ingress = ComputeIngress {
			ingress_roots: roots,
			incoming: Vec::new(),
			inner: stream::iter_ok::<_, ()>(inputs.iter().cloned()),
		};

		assert_eq!(ingress.wait().unwrap().unwrap(), actual_messages);
	}

	#[test]
	fn add_new_sessions_works() {
		let mut live_sessions = LiveValidationSessions::new();
		let key_a = [0; 32].into();
		let key_b = [1; 32].into();
		let parent_hash = [0xff; 32].into();

		let (session, new_key) = live_sessions.new_validation_session(SessionParams {
			parent_hash,
			local_session_key: None,
		});

		let knowledge = session.knowledge().clone();

		assert!(new_key.is_none());

		let (session, new_key) = live_sessions.new_validation_session(SessionParams {
			parent_hash,
			local_session_key: Some(key_a),
		});

		// check that knowledge points to the same place.
		assert_eq!(&**session.knowledge() as *const _, &*knowledge as *const _);
		assert_eq!(new_key, Some(key_a));

		let (session, new_key) = live_sessions.new_validation_session(SessionParams {
			parent_hash,
			local_session_key: Some(key_b),
		});

		assert_eq!(&**session.knowledge() as *const _, &*knowledge as *const _);
		assert!(new_key.is_none());
	}
}
