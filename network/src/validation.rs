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

use sr_primitives::traits::ProvideRuntimeApi;
use substrate_network::{consensus_gossip::ConsensusMessage, Context as NetContext};
use polkadot_validation::{Network as ParachainNetwork, SharedTable, Collators, Statement, GenericStatement};
use polkadot_primitives::{AccountId, Block, Hash, SessionKey};
use polkadot_primitives::parachain::{Id as ParaId, Collation, Extrinsic, ParachainHost, BlockData};
use codec::Decode;

use futures::prelude::*;
use futures::future::Executor as FutureExecutor;
use futures::sync::mpsc;

use std::collections::HashMap;
use std::sync::Arc;

use arrayvec::ArrayVec;
use tokio::runtime::TaskExecutor;
use parking_lot::Mutex;

use router::Router;
use super::PolkadotProtocol;

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

// task that processes all gossipped consensus messages,
// checking signatures
struct MessageProcessTask<P, E, N: NetworkService, T> {
	inner_stream: mpsc::UnboundedReceiver<ConsensusMessage>,
	parent_hash: Hash,
	table_router: Router<P, E, N, T>,
}

impl<P, E, N, T> MessageProcessTask<P, E, N, T> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	E: Future<Item=(),Error=()> + Clone + Send + 'static,
	N: NetworkService,
	T: Clone + Executor + Send + 'static,
{
	fn process_message(&self, msg: ConsensusMessage) -> Option<Async<()>> {
		use polkadot_validation::SignedStatement;

		debug!(target: "validation", "Processing validation statement for live session");
		if let Some(statement) = SignedStatement::decode(&mut msg.as_slice()) {
			if ::polkadot_validation::check_statement(
				&statement.statement,
				&statement.signature,
				statement.sender,
				&self.parent_hash
			) {
				self.table_router.import_statement(statement);
			}
		}

		None
	}
}

impl<P, E, N, T> Future for MessageProcessTask<P, E, N, T> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	E: Future<Item=(),Error=()> + Clone + Send + 'static,
	N: NetworkService,
	T: Clone + Executor + Send + 'static,
{
	type Item = ();
	type Error = ();

	fn poll(&mut self) -> Poll<(), ()> {
		loop {
			match self.inner_stream.poll() {
				Ok(Async::Ready(Some(val))) => if let Some(async) = self.process_message(val) {
					return Ok(async);
				},
				Ok(Async::Ready(None)) => return Ok(Async::Ready(())),
				Ok(Async::NotReady) => return Ok(Async::NotReady),
				Err(e) => debug!(target: "p_net", "Error getting consensus message: {:?}", e),
			}
		}
	}
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

/// A long-lived network which can create parachain statement  routing processes on demand.
impl<P, E, N, T> ParachainNetwork for ValidationNetwork<P, E, N, T> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	E: Clone + Future<Item=(),Error=()> + Send + 'static,
	N: NetworkService,
	T: Clone + Executor + Send + 'static,
{
	type TableRouter = Router<P, E, N, T>;

	fn communication_for(
		&self,
		table: Arc<SharedTable>,
		outgoing: polkadot_validation::Outgoing,
	) -> Self::TableRouter {
		let parent_hash = table.consensus_parent_hash().clone();

		let knowledge = Arc::new(Mutex::new(Knowledge::new()));

		let local_session_key = table.session_key();
		let table_router = Router::new(
			table,
			self.network.clone(),
			self.api.clone(),
			self.executor.clone(),
			parent_hash,
			knowledge.clone(),
			self.exit.clone(),
		);

		table_router.broadcast_egress(outgoing);

		let attestation_topic = table_router.gossip_topic();

		let table_router_clone = table_router.clone();
		let executor = self.executor.clone();

		// spin up a task in the background that processes all incoming statements
		// TODO: propagate statements on a timer?
		let inner_stream = self.network.gossip_messages_for(attestation_topic);
		self.network
			.with_spec(move |spec, ctx| {
				spec.new_validation_session(ctx, parent_hash, ValidationSession {
					knowledge,
					local_session_key,
				});
				let process_task = MessageProcessTask {
					inner_stream,
					parent_hash,
					table_router: table_router_clone,
				};

				executor.spawn(process_task);
		});

		table_router
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

/// A current validation session instance.
pub(crate) struct ValidationSession {
	knowledge: Arc<Mutex<Knowledge>>,
	local_session_key: SessionKey,
}

impl ValidationSession {
	#[cfg(test)]
	pub(crate) fn new(knowledge: Arc<Mutex<Knowledge>>, local_session_key: SessionKey) -> Self {
		ValidationSession {
			knowledge,
			local_session_key
		}
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
	pub(crate) fn new_validation_session(
		&mut self,
		parent_hash: Hash,
		session: ValidationSession,
	) -> Option<SessionKey> {
		let inserted_key = self.recent.insert(session.local_session_key);
		let maybe_new = if let InsertedRecentKey::New(_) = inserted_key {
			Some(session.local_session_key)
		} else {
			None
		};

		self.live_instances.insert(parent_hash, session);

		maybe_new
	}

	/// Remove validation session.
	pub(crate) fn remove(&mut self, parent_hash: &Hash) {
		if let Some(validation_session) = self.live_instances.remove(parent_hash) {
			let key_still_used = self.live_instances.values()
				.any(|c| c.local_session_key == validation_session.local_session_key);

			if !key_still_used {
				self.recent.remove(&validation_session.local_session_key)
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

#[cfg(test)]
mod tests {
	use super::*;

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
}
