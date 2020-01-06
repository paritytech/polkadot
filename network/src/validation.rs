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

//! The "validation leaf work" networking code built on top of the base network service.
//!
//! This fulfills the `polkadot_validation::Network` trait, providing a hook to be called
//! each time validation leaf work begins on a new chain head.

use sp_runtime::traits::ProvideRuntimeApi;
use sc_network::PeerId;
use polkadot_validation::{
	Network as ParachainNetwork, SharedTable, Collators, Statement, GenericStatement, SignedStatement,
};
use polkadot_primitives::{Block, BlockId, Hash};
use polkadot_primitives::parachain::{
	Id as ParaId, Collation, OutgoingMessages, ParachainHost, CandidateReceipt, CollatorId,
	ValidatorId, PoVBlock,
};

use futures::prelude::*;
use futures::task::SpawnExt;
pub use futures::task::Spawn as Executor;
use futures::channel::oneshot;
use futures::future::{ready, select};

use std::collections::hash_map::{HashMap, Entry};
use std::io;
use std::sync::Arc;
use std::pin::Pin;

use arrayvec::ArrayVec;
use parking_lot::Mutex;

use crate::router::Router;
use crate::gossip::{RegisteredMessageValidator, MessageValidationData};

use super::NetworkService;

pub use polkadot_validation::Incoming;

/// Params to instantiate validation work on a block-DAG leaf.
pub struct LeafWorkParams {
	/// The local session key.
	pub local_session_key: Option<ValidatorId>,
	/// The parent hash.
	pub parent_hash: Hash,
	/// The authorities.
	pub authorities: Vec<ValidatorId>,
}

/// Wrapper around the network service
pub struct ValidationNetwork<P, E, T> {
	api: Arc<P>,
	executor: T,
	network: RegisteredMessageValidator,
	exit: E,
}

impl<P, E, T> ValidationNetwork<P, E, T> {
	/// Create a new consensus networking object.
	pub fn new(
		network: RegisteredMessageValidator,
		exit: E,
		api: Arc<P>,
		executor: T,
	) -> Self {
		ValidationNetwork { network, exit, api, executor }
	}
}

impl<P, E: Clone, T: Clone> Clone for ValidationNetwork<P, E, T> {
	fn clone(&self) -> Self {
		ValidationNetwork {
			network: self.network.clone(),
			exit: self.exit.clone(),
			api: self.api.clone(),
			executor: self.executor.clone(),
		}
	}
}

impl<P, E, T> ValidationNetwork<P, E, T> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	E: Clone + Future<Output=()> + Send + Sync + 'static,
	T: Clone + Executor + Send + Sync + 'static,
{
	/// Instantiate block-DAG leaf work
	/// (i.e. the work we want to be done by validators at some chain-head)
	/// at a parent hash.
	///
	/// If the used session key is new, it will be broadcast to peers.
	/// If any validation leaf-work was already instantiated at this parent hash,
	/// the underlying instance will be shared.
	///
	/// If there was already validation leaf-work instantiated and a different
	/// session key was set, then the new key will be ignored.
	///
	/// This implies that there can be multiple services intantiating validation
	/// leaf-work instances safely, but they should all be coordinated on which session keys
	/// are being used.
	pub fn instantiate_leaf_work(&self, params: LeafWorkParams)
		-> oneshot::Receiver<LeafWorkDataFetcher<P, E, T>>
	{
		let parent_hash = params.parent_hash;
		let network = self.network.clone();
		let api = self.api.clone();
		let task_executor = self.executor.clone();
		let exit = self.exit.clone();
		let authorities = params.authorities.clone();

		let (tx, rx) = oneshot::channel();

		self.network.with_spec(move |spec, ctx| {
			let actions = network.new_local_leaf(
				parent_hash,
				MessageValidationData { authorities },
				|queue_root| spec.availability_store.as_ref()
					.and_then(|store| store.queue_by_root(queue_root))
			);

			actions.perform(&network);

			let work = spec.new_validation_leaf_work(ctx, params);
			let _ = tx.send(LeafWorkDataFetcher {
				network,
				api,
				task_executor,
				parent_hash,
				knowledge: work.knowledge().clone(),
				exit,
			});
		});

		rx
	}
}

impl<P, E, T> ValidationNetwork<P, E, T> {
	/// Convert the given `CollatorId` to a `PeerId`.
	pub fn collator_id_to_peer_id(&self, collator_id: CollatorId) ->
		impl Future<Output=Option<PeerId>> + Send
	{
		let network = self.network.clone();

		async move {
			let (send, recv) = oneshot::channel();
			network.with_spec(move |spec, _| {
				let _ = send.send(spec.collator_id_to_peer_id(&collator_id).cloned());
			});
			recv.await.ok().and_then(|opt| opt)
		}
	}

	/// Create a `Stream` of checked statements for the given `relay_parent`.
	///
	/// The returned stream will not terminate, so it is required to make sure that the stream is
	/// dropped when it is not required anymore. Otherwise, it will stick around in memory
	/// infinitely.
	pub fn checked_statements(&self, relay_parent: Hash) -> impl Stream<Item=SignedStatement> {
		crate::router::checked_statements(&self.network, crate::router::attestation_topic(relay_parent))
	}
}

/// A long-lived network which can create parachain statement  routing processes on demand.
impl<P, E, T> ParachainNetwork for ValidationNetwork<P, E, T> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	E: Clone + Future<Output=()> + Send + Sync + Unpin + 'static,
	T: Clone + Executor + Send + Sync + 'static,
{
	type Error = String;
	type TableRouter = Router<P, E, T>;
	type BuildTableRouter = Box<dyn Future<Output=Result<Self::TableRouter, String>> + Send + Unpin>;

	fn communication_for(
		&self,
		table: Arc<SharedTable>,
		authorities: &[ValidatorId],
		exit: exit_future::Exit,
	) -> Self::BuildTableRouter {
		let parent_hash = *table.consensus_parent_hash();
		let local_session_key = table.session_key();

		let build_fetcher = self.instantiate_leaf_work(LeafWorkParams {
			local_session_key,
			parent_hash,
			authorities: authorities.to_vec(),
		});

		let executor = self.executor.clone();
		let network = self.network.clone();
		let work = build_fetcher
			.map_err(|e| format!("{:?}", e))
			.map_ok(move |fetcher| {
				let table_router = Router::new(
					table,
					fetcher,
					network,
				);

				let table_router_clone = table_router.clone();
				let work = table_router.checked_statements()
					.for_each(move |msg| {
						table_router_clone.import_statement(msg);
						ready(())
					});

				let work = select(work, exit).map(drop);

				let _ = executor.spawn(work);

				table_router
			});

		Box::new(work)
	}
}

/// Error when the network appears to be down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkDown;

impl<P, E: Clone, N: Clone> Collators for ValidationNetwork<P, E, N> where
	P: ProvideRuntimeApi + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
{
	type Error = NetworkDown;
	type Collation = Pin<Box<dyn Future<Output = Result<Collation, NetworkDown>> + Send>>;

	fn collate(&self, parachain: ParaId, relay_parent: Hash) -> Self::Collation {
		let (tx, rx) = oneshot::channel();
		let network = self.network.clone();

		// A future that resolves when a collation is received.
		async move {
			network.with_spec(move |spec, _| {
				let collation = spec.await_collation(relay_parent, parachain);
				let _ = tx.send(collation);
			});

			rx.await
				.map_err(|_| NetworkDown)?
				.await
				.map_err(|_| NetworkDown)
		}.boxed()
	}


	fn note_bad_collator(&self, collator: CollatorId) {
		self.network.with_spec(move |spec, ctx| spec.disconnect_bad_collator(ctx, collator));
	}
}

#[derive(Default)]
struct KnowledgeEntry {
	knows_block_data: Vec<ValidatorId>,
	knows_outgoing: Vec<ValidatorId>,
	pov: Option<PoVBlock>,
	outgoing_messages: Option<OutgoingMessages>,
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
		// those claiming it invalid do not have the outgoing messages data as it is
		// generated by valid execution.
		match *statement {
			GenericStatement::Candidate(ref c) => {
				let entry = self.candidates.entry(c.hash()).or_insert_with(Default::default);
				entry.knows_block_data.push(from.clone());
				entry.knows_outgoing.push(from);
			}
			GenericStatement::Valid(ref hash) => {
				let entry = self.candidates.entry(*hash).or_insert_with(Default::default);
				entry.knows_block_data.push(from.clone());
				entry.knows_outgoing.push(from);
			}
			GenericStatement::Invalid(ref hash) => self.candidates.entry(*hash)
				.or_insert_with(Default::default)
				.knows_block_data
				.push(from),
		}
	}

	/// Note a candidate collated or seen locally.
	pub(crate) fn note_candidate(
		&mut self,
		hash: Hash,
		pov: Option<PoVBlock>,
		outgoing_messages: Option<OutgoingMessages>,
	) {
		let entry = self.candidates.entry(hash).or_insert_with(Default::default);
		entry.pov = entry.pov.take().or(pov);
		entry.outgoing_messages = entry.outgoing_messages.take().or(outgoing_messages);
	}
}

/// A current validation leaf-work instance
#[derive(Clone)]
pub(crate) struct LiveValidationLeaf {
	parent_hash: Hash,
	knowledge: Arc<Mutex<Knowledge>>,
	local_session_key: Option<ValidatorId>,
}

impl LiveValidationLeaf {
	/// Create a new validation leaf-work instance. Needs to be attached to the
	/// network.
	pub(crate) fn new(params: LeafWorkParams) -> Self {
		LiveValidationLeaf {
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

/// Manages requests and keys for live validation leaf-work instances.
pub(crate) struct LiveValidationLeaves {
	// recent local session keys.
	recent: RecentValidatorIds,
	// live validation leaf-work instances, on `parent_hash`. refcount retained alongside.
	live_instances: HashMap<Hash, (usize, LiveValidationLeaf)>,
}

impl LiveValidationLeaves {
	/// Create a new `LiveValidationLeaves`
	pub(crate) fn new() -> Self {
		LiveValidationLeaves {
			recent: Default::default(),
			live_instances: HashMap::new(),
		}
	}

	/// Note new leaf for validation work. If the used session key is new,
	/// it returns it to be broadcasted to peers.
	///
	/// If there was already work instantiated at this leaf and a different
	/// session key was set, then the new key will be ignored.
	pub(crate) fn new_validation_leaf(
		&mut self,
		params: LeafWorkParams,
	) -> (LiveValidationLeaf, Option<ValidatorId>) {
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

		let leaf_work = LiveValidationLeaf::new(params);
		self.live_instances.insert(parent_hash, (1, leaf_work.clone()));

		(leaf_work, check_new_key())
	}

	/// Remove validation leaf-work. true indicates that it was actually removed.
	pub(crate) fn remove(&mut self, parent_hash: Hash) -> bool {
		let maybe_removed = if let Entry::Occupied(mut entry) = self.live_instances.entry(parent_hash) {
			entry.get_mut().0 -= 1;
			if entry.get().0 == 0 {
				let (_, leaf_work) = entry.remove();
				Some(leaf_work)
			} else {
				None
			}
		} else {
			None
		};

		let leaf_work = match maybe_removed {
			None => return false,
			Some(s) => s,
		};

		if let Some(ref key) = leaf_work.local_session_key {
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

	/// Call a closure with pov-data from validation leaf-work at parent hash for a given
	/// candidate-receipt hash.
	///
	/// This calls the closure with `Some(data)` where the leaf-work and data are live,
	/// `Err(Some(keys))` when the leaf-work is live but the data unknown, with a list of keys
	/// who have the data, and `Err(None)` where the leaf-work is unknown.
	pub(crate) fn with_pov_block<F, U>(&self, parent_hash: &Hash, c_hash: &Hash, f: F) -> U
		where F: FnOnce(Result<&PoVBlock, Option<&[ValidatorId]>>) -> U
	{
		match self.live_instances.get(parent_hash) {
			Some(c) => c.1.with_pov_block(c_hash, |res| f(res.map_err(Some))),
			None => f(Err(None))
		}
	}
}

/// Can fetch data for a given validation leaf-work instance.
pub struct LeafWorkDataFetcher<P, E, T> {
	network: RegisteredMessageValidator,
	api: Arc<P>,
	exit: E,
	task_executor: T,
	knowledge: Arc<Mutex<Knowledge>>,
	parent_hash: Hash,
}

impl<P, E, T> LeafWorkDataFetcher<P, E, T> {
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
	pub(crate) fn network(&self) -> &RegisteredMessageValidator {
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

impl<P, E: Clone, T: Clone> Clone for LeafWorkDataFetcher<P, E, T> {
	fn clone(&self) -> Self {
		LeafWorkDataFetcher {
			network: self.network.clone(),
			api: self.api.clone(),
			task_executor: self.task_executor.clone(),
			parent_hash: self.parent_hash,
			knowledge: self.knowledge.clone(),
			exit: self.exit.clone(),
		}
	}
}

impl<P: ProvideRuntimeApi + Send, E, T> LeafWorkDataFetcher<P, E, T> where
	P::Api: ParachainHost<Block>,
	T: Clone + Executor + Send + 'static,
	E: Future<Output=()> + Clone + Send + 'static,
{
	/// Fetch PoV block for the given candidate receipt.
	pub fn fetch_pov_block(&self, candidate: &CandidateReceipt)
		-> Pin<Box<dyn Future<Output = Result<PoVBlock, io::Error>> + Send>> {

		let parachain = candidate.parachain_index;
		let parent_hash = self.parent_hash;
		let network = self.network.clone();
		let candidate = candidate.clone();
		let (tx, rx) = oneshot::channel();

		let canon_roots = self.api.runtime_api().ingress(
			&BlockId::hash(parent_hash),
			parachain,
			None,
		)
			.map_err(|e|
				format!(
					"Cannot fetch ingress for parachain {:?} at {:?}: {:?}",
					parachain,
					parent_hash,
					e,
				)
			);

		async move {
			network.with_spec(move |spec, ctx| {
				if let Ok(Some(canon_roots)) = canon_roots {
					let inner_rx = spec.fetch_pov_block(ctx, &candidate, parent_hash, canon_roots);
					let _ = tx.send(inner_rx);
				}
			});

			let map_err = |_| io::Error::new(
				io::ErrorKind::Other,
				"Sending end of channel hung up",
			);

			rx.await
				.map_err(map_err)?
				.await
				.map_err(map_err)
		}.boxed()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_core::crypto::UncheckedInto;

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
	fn add_new_leaf_work_works() {
		let mut live_leaves = LiveValidationLeaves::new();
		let key_a: ValidatorId = [0; 32].unchecked_into();
		let key_b: ValidatorId = [1; 32].unchecked_into();
		let parent_hash = [0xff; 32].into();

		let (leaf_work, new_key) = live_leaves.new_validation_leaf(LeafWorkParams {
			parent_hash,
			local_session_key: None,
			authorities: Vec::new(),
		});

		let knowledge = leaf_work.knowledge().clone();

		assert!(new_key.is_none());

		let (leaf_work, new_key) = live_leaves.new_validation_leaf(LeafWorkParams {
			parent_hash,
			local_session_key: Some(key_a.clone()),
			authorities: Vec::new(),
		});

		// check that knowledge points to the same place.
		assert_eq!(&**leaf_work.knowledge() as *const _, &*knowledge as *const _);
		assert_eq!(new_key, Some(key_a.clone()));

		let (leaf_work, new_key) = live_leaves.new_validation_leaf(LeafWorkParams {
			parent_hash,
			local_session_key: Some(key_b.clone()),
			authorities: Vec::new(),
		});

		assert_eq!(&**leaf_work.knowledge() as *const _, &*knowledge as *const _);
		assert!(new_key.is_none());
	}
}
