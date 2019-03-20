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

//! The "consensus" networking code built on top of the base network service.
//!
//! This fulfills the `polkadot_consensus::Network` trait, providing a hook to be called
//! each time consensus begins on a new chain head.

use sr_primitives::traits::{BlakeTwo256, Hash as HashT, ProvideRuntimeApi};
use polkadot_consensus::{
	Network, SharedTable, Collators, Statement, GenericStatement,
};
use polkadot_primitives::{Block, Hash, SessionKey};
use polkadot_primitives::parachain::{
	Id as ParaId, Collation, CollatorId, Extrinsic, ParachainHost, BlockData
};
use codec::Decode;

use futures::prelude::*;
use futures::sync::mpsc;
use futures::sync::oneshot;

use std::collections::HashMap;
use std::sync::Arc;

use arrayvec::ArrayVec;
use tokio::runtime::TaskExecutor;
use parking_lot::Mutex;

use super::NetworkService;
use router::Router;
use gossip::{POLKADOT_ENGINE_ID, GossipMessage, RegisteredMessageValidator, MessageValidationData};

/// Compute the gossip topic used for attestations on this relay parent hash.
pub(crate) fn attestation_topic(parent_hash: Hash) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	v.extend(b"attestations");

	BlakeTwo256::hash(&v[..])
}

// task that processes all gossipped consensus messages,
// checking signatures
struct MessageProcessTask<P, E> {
	inner_stream: mpsc::UnboundedReceiver<Vec<u8>>,
	table_router: Router<P>,
	exit: E,
}

impl<P, E> MessageProcessTask<P, E> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	E: Future<Item=(),Error=()> + Clone + Send + 'static,
{
	fn process_message(&self, msg: Vec<u8>) -> Option<Async<()>> {
		debug!(target: "consensus", "Processing consensus statement for live consensus");

		// statements are already checked by gossip validator.
		if let Some(message) = GossipMessage::decode(&mut &msg[..]) {
			self.table_router.import_statement(message.statement, self.exit.clone());
		}

		None
	}
}

impl<P, E> Future for MessageProcessTask<P, E> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	E: Future<Item=(),Error=()> + Clone + Send + 'static,
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
pub struct ConsensusNetwork<P, E> {
	network: Arc<NetworkService>,
	api: Arc<P>,
	message_validator: RegisteredMessageValidator,
	exit: E,
}

impl<P, E> ConsensusNetwork<P, E> {
	/// Create a new consensus networking object.
	pub fn new(
		network: Arc<NetworkService>,
		exit: E,
		message_validator: RegisteredMessageValidator,
		api: Arc<P>,
	) -> Self {
		ConsensusNetwork { network, exit, message_validator, api }
	}
}

impl<P, E: Clone> Clone for ConsensusNetwork<P, E> {
	fn clone(&self) -> Self {
		ConsensusNetwork {
			network: self.network.clone(),
			exit: self.exit.clone(),
			api: self.api.clone(),
			message_validator: self.message_validator.clone(),
		}
	}
}

/// A long-lived network which can create parachain statement  routing processes on demand.
impl<P, E> Network for ConsensusNetwork<P,E> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	E: Clone + Future<Item=(),Error=()> + Send + 'static,
{
	type TableRouter = Router<P>;

	/// Instantiate a table router using the given shared table.
	fn communication_for(
		&self,
		authorities: &[SessionKey],
		table: Arc<SharedTable>,
		task_executor: TaskExecutor,
	) -> Self::TableRouter {
		let parent_hash = table.consensus_parent_hash().clone();

		let knowledge = Arc::new(Mutex::new(Knowledge::new()));

		let local_session_key = table.session_key();
		let table_router = Router::new(
			table,
			self.network.clone(),
			self.api.clone(),
			task_executor.clone(),
			parent_hash,
			knowledge.clone(),
			self.message_validator.clone(),
		);

		let attestation_topic = table_router.gossip_topic();
		let exit = self.exit.clone();

		// before requesting messages, note live consensus session.
		self.message_validator.note_consensus(
			parent_hash,
			MessageValidationData { authorities: authorities.to_vec() },
		);

		let (tx, rx) = std::sync::mpsc::channel();
		self.network.with_gossip(move |gossip, _| {
			let inner_rx = gossip.messages_for(POLKADOT_ENGINE_ID, attestation_topic);
			let _ = tx.send(inner_rx);
		});

		let table_router_clone = table_router.clone();
		let executor = task_executor.clone();

		self.network.with_spec(move |spec, ctx| {
			spec.new_consensus(ctx, parent_hash, CurrentConsensus {
				knowledge,
				local_session_key,
			});
			let inner_stream = rx.try_recv().expect("1. The with_gossip closure executed first, 2. the reply should be available");
			let process_task = MessageProcessTask {
				inner_stream,
				table_router: table_router_clone,
				exit: exit.clone(),
			};
			executor.spawn(process_task.select(exit).then(|_| Ok(())));
		});

		table_router
	}
}

/// Error when the network appears to be down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkDown;

/// A future that resolves when a collation is received.
pub struct AwaitingCollation {
	outer: oneshot::Receiver<::futures::sync::oneshot::Receiver<Collation>>,
	inner: Option<oneshot::Receiver<Collation>>
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

impl<P: ProvideRuntimeApi + Send + Sync + 'static, E: Clone> Collators for ConsensusNetwork<P, E>
	where P::Api: ParachainHost<Block>,
{
	type Error = NetworkDown;
	type Collation = AwaitingCollation;

	fn collate(&self, parachain: ParaId, relay_parent: Hash) -> Self::Collation {
		let (tx, rx) = oneshot::channel();
		self.network.with_spec(move |spec, _| {
			let collation = spec.await_collation(relay_parent, parachain);
			let _ = tx.send(collation);
		});
		AwaitingCollation { outer: rx, inner: None }
	}

	fn note_bad_collator(&self, collator: CollatorId) {
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
		match *statement {
			GenericStatement::Candidate(ref c) => {
				let mut entry = self.candidates.entry(c.hash()).or_insert_with(Default::default);
				entry.knows_block_data.push(from.clone());
				entry.knows_extrinsic.push(from);
			}
			GenericStatement::Available(ref hash) => {
				let mut entry = self.candidates.entry(*hash).or_insert_with(Default::default);
				entry.knows_block_data.push(from.clone());
				entry.knows_extrinsic.push(from);
			}
			GenericStatement::Valid(ref hash) | GenericStatement::Invalid(ref hash) => self.candidates.entry(*hash)
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

/// A current consensus instance.
pub(crate) struct CurrentConsensus {
	knowledge: Arc<Mutex<Knowledge>>,
	local_session_key: SessionKey,
}

impl CurrentConsensus {
	#[cfg(test)]
	pub(crate) fn new(knowledge: Arc<Mutex<Knowledge>>, local_session_key: SessionKey) -> Self {
		CurrentConsensus {
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

/// Wrapper for managing recent .
#[derive(Default)]
pub(crate) struct RecentValidatorIds {
	inner: ArrayVec<[SessionKey; RECENT_SESSIONS]>,
}

impl RecentValidatorIds {
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

/// Manages requests and session keys for live consensus instances.
pub(crate) struct LiveConsensusInstances {
	// recent local session keys.
	recent: RecentValidatorIds,
	// live consensus instances, on `parent_hash`.
	live_instances: HashMap<Hash, CurrentConsensus>,
}

impl LiveConsensusInstances {
	/// Create a new `LiveConsensusInstances`
	pub(crate) fn new() -> Self {
		LiveConsensusInstances {
			recent: Default::default(),
			live_instances: HashMap::new(),
		}
	}

	/// Note new consensus session. If the used session key is new,
	/// it returns it to be broadcasted to peers.
	pub(crate) fn new_consensus(
		&mut self,
		parent_hash: Hash,
		consensus: CurrentConsensus,
	) -> Option<SessionKey> {
		let inserted_key = self.recent.insert(consensus.local_session_key.clone());
		let maybe_new = if let InsertedRecentKey::New(_) = inserted_key {
			Some(consensus.local_session_key.clone())
		} else {
			None
		};

		self.live_instances.insert(parent_hash, consensus);

		maybe_new
	}

	/// Remove consensus session.
	pub(crate) fn remove(&mut self, parent_hash: &Hash) {
		if let Some(consensus) = self.live_instances.remove(parent_hash) {
			let key_still_used = self.live_instances.values()
				.any(|c| c.local_session_key == consensus.local_session_key);

			if !key_still_used {
				self.recent.remove(&consensus.local_session_key)
			}
		}
	}

	/// Recent session keys as a slice.
	pub(crate) fn recent_keys(&self) -> &[SessionKey] {
		self.recent.as_slice()
	}

	/// Call a closure with block data from consensus session at parent hash.
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
	use substrate_primitives::crypto::UncheckedInto;

	#[test]
	fn last_keys_works() {
		let a: SessionKey = [1; 32].unchecked_into();
		let b: SessionKey = [2; 32].unchecked_into();
		let c: SessionKey = [3; 32].unchecked_into();
		let d: SessionKey = [4; 32].unchecked_into();

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

		match recent.insert(b.clone()) {
			InsertedRecentKey::AlreadyKnown => {},
			_ => panic!("not new"),
		}

		match recent.insert(c.clone()) {
			InsertedRecentKey::New(None) => {},
			_ => panic!("is new, not at capacity"),
		}

		match recent.insert(c.clone()) {
			InsertedRecentKey::AlreadyKnown => {},
			_ => panic!("not new"),
		}

		match recent.insert(d.clone()) {
			InsertedRecentKey::New(Some(old)) => assert_eq!(old, a),
			_ => panic!("is new, and at capacity"),
		}

		match recent.insert(d.clone()) {
			InsertedRecentKey::AlreadyKnown => {},
			_ => panic!("not new"),
		}
	}
}
