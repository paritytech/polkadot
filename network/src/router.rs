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

//! Statement routing and consensus table router implementation.
//!
//! During the consensus process, validators exchange statements on validity and availability
//! of parachain candidates.
//! The `Router` in this file hooks into the underlying network to fulfill
//! the `TableRouter` trait from `polkadot-consensus`, which is expected to call into a shared statement table
//! and dispatch evaluation work as necessary when new statements come in.

use sr_primitives::traits::{ProvideRuntimeApi, BlakeTwo256, Hash as HashT};
use polkadot_consensus::{
	SharedTable, TableRouter, SignedStatement, GenericStatement, ParachainWork, Incoming,
};
use polkadot_primitives::{Block, Hash, SessionKey};
use polkadot_primitives::parachain::{
	BlockData, Extrinsic, CandidateReceipt, ParachainHost, Id as ParaId, Message
};

use codec::{Encode, Decode};
use futures::{future, prelude::*};
use futures::sync::oneshot::{self, Receiver};
use tokio::runtime::TaskExecutor;
use parking_lot::Mutex;

use std::collections::{hash_map::{Entry, HashMap}, HashSet};
use std::io;
use std::sync::Arc;

use consensus::{NetworkService, Knowledge};

type IngressPair = (ParaId, Vec<Message>);

fn attestation_topic(parent_hash: Hash) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	v.extend(b"attestations");

	BlakeTwo256::hash(&v[..])
}

fn incoming_message_topic(parent_hash: Hash, parachain: ParaId) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	parachain.using_encoded(|s| v.extend(s));

	BlakeTwo256::hash(&v[..])
}

/// Receiver for block data.
pub struct BlockDataReceiver {
	inner: Receiver<BlockData>,
}

impl Future for BlockDataReceiver {
	type Item = BlockData;
	type Error = io::Error;

	fn poll(&mut self) -> Poll<BlockData, io::Error> {
		self.inner.poll().map_err(|_| io::Error::new(
			io::ErrorKind::Other,
			"Sending end of channel hung up",
		))
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

/// Table routing implementation.
pub struct Router<P, N: NetworkService> {
	table: Arc<SharedTable>,
	network: Arc<N>,
	api: Arc<P>,
	task_executor: TaskExecutor,
	parent_hash: Hash,
	attestation_topic: Hash,
	knowledge: Arc<Mutex<Knowledge>>,
	fetch_incoming: Arc<Mutex<HashMap<ParaId, IncomingReceiver>>>,
	deferred_statements: Arc<Mutex<DeferredStatements>>,
}

impl<P, N: NetworkService> Router<P, N> {
	pub(crate) fn new(
		table: Arc<SharedTable>,
		network: Arc<N>,
		api: Arc<P>,
		task_executor: TaskExecutor,
		parent_hash: Hash,
		knowledge: Arc<Mutex<Knowledge>>,
	) -> Self {
		Router {
			table,
			network,
			api,
			task_executor,
			parent_hash,
			attestation_topic: attestation_topic(parent_hash),
			knowledge,
			fetch_incoming: Arc::new(Mutex::new(HashMap::new())),
			deferred_statements: Arc::new(Mutex::new(DeferredStatements::new())),
		}
	}

	/// Get the attestation topic for gossip.
	pub(crate) fn gossip_topic(&self) -> Hash {
		self.attestation_topic
	}
}

impl<P, N: NetworkService> Clone for Router<P, N> {
	fn clone(&self) -> Self {
		Router {
			table: self.table.clone(),
			network: self.network.clone(),
			api: self.api.clone(),
			task_executor: self.task_executor.clone(),
			parent_hash: self.parent_hash.clone(),
			attestation_topic: self.attestation_topic.clone(),
			deferred_statements: self.deferred_statements.clone(),
			fetch_incoming: self.fetch_incoming.clone(),
			knowledge: self.knowledge.clone(),
		}
	}
}

impl<P: ProvideRuntimeApi + Send + Sync + 'static, N> Router<P, N> where
	P::Api: ParachainHost<Block>,
	N: NetworkService,
{
	/// Import a statement whose signature has been checked already.
	pub(crate) fn import_statement<Exit>(&self, statement: SignedStatement, exit: Exit)
		where Exit: Future<Item=(),Error=()> + Clone + Send + 'static
	{
		trace!(target: "p_net", "importing consensus statement {:?}", statement.statement);

		// defer any statements for which we haven't imported the candidate yet
		let c_hash = {
			let candidate_data = match statement.statement {
				GenericStatement::Candidate(ref c) => Some(c.hash()),
				GenericStatement::Valid(ref hash)
					| GenericStatement::Invalid(ref hash)
					=> self.table.with_candidate(hash, |c| c.map(|_| *hash)),
			};
			match candidate_data {
				Some(x) => x,
				None => {
					self.deferred_statements.lock().push(statement);
					return;
				}
			}
		};

		// import all statements pending on this candidate
		let (mut statements, _traces) = if let GenericStatement::Candidate(_) = statement.statement {
			self.deferred_statements.lock().get_deferred(&c_hash)
		} else {
			(Vec::new(), Vec::new())
		};

		// prepend the candidate statement.
		debug!(target: "consensus", "Importing statements about candidate {:?}", c_hash);
		statements.insert(0, statement);
		let producers: Vec<_> = self.table.import_remote_statements(
			self,
			statements.iter().cloned(),
		);
		// dispatch future work as necessary.
		for (producer, statement) in producers.into_iter().zip(statements) {
			self.knowledge.lock().note_statement(statement.sender, &statement.statement);

			if let Some(work) = producer.map(|p| self.create_work(c_hash, p)) {
				trace!(target: "consensus", "driving statement work to completion");
				self.task_executor.spawn(work.select(exit.clone()).then(|_| Ok(())));
			}
		}
	}

	fn create_work<D>(&self, candidate_hash: Hash, producer: ParachainWork<D>)
		-> impl Future<Item=(),Error=()>
		where
		D: Future<Item=(BlockData, Incoming),Error=io::Error> + Send + 'static,
	{
		let table = self.table.clone();
		let network = self.network.clone();
		let knowledge = self.knowledge.clone();
		let attestation_topic = self.attestation_topic.clone();

		producer.prime(self.api.clone())
			.map(move |produced| {
				// store the data before broadcasting statements, so other peers can fetch.
				knowledge.lock().note_candidate(
					candidate_hash,
					Some(produced.block_data),
					produced.extrinsic,
				);

				// propagate the statement.
				// consider something more targeted than gossip in the future.
				let signed = table.sign_and_import(produced.validity);
				network.gossip_message(attestation_topic, signed.encode());
			})
			.map_err(|e| debug!(target: "p_net", "Failed to produce statements: {:?}", e))
	}

}

impl<P: ProvideRuntimeApi, N> Router<P, N> where
	P::Api: ParachainHost<Block>,
	N: NetworkService,
{
	fn do_fetch_incoming(&self, parachain: ParaId) -> IncomingReceiver {
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

		let parent_hash = self.parent_hash;
		let topic = incoming_message_topic(parent_hash, parachain);

		let gossip_messages = self.network.gossip_messages_for(topic)
			.map_err(|()| panic!("unbounded receivers do not throw errors; qed"))
			.filter_map(|msg| IngressPair::decode(&mut msg.as_slice()));

		let canon_roots = self.api.runtime_api().ingress(&BlockId::hash(parent_hash), parachain)
			.map_err(|e| format!("Cannot fetch ingress for parachain {:?} at {:?}: {:?}",
				parachain, parent_hash, e)
			);

		// TODO: select with `Exit` here.
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
			.then(|_| Ok(()));

		self.task_executor.spawn(work);

		rx
	}
}

impl<P: ProvideRuntimeApi + Send, N> TableRouter for Router<P, N> where
	P::Api: ParachainHost<Block>,
	N: NetworkService,
{
	type Error = io::Error;
	type FetchCandidate = BlockDataReceiver;
	type FetchIncoming = IncomingReceiver;

	fn local_candidate(&self, receipt: CandidateReceipt, block_data: BlockData, extrinsic: Extrinsic) {
		// give to network to make available.
		let hash = receipt.hash();
		let candidate = self.table.sign_and_import(GenericStatement::Candidate(receipt));

		self.knowledge.lock().note_candidate(hash, Some(block_data), Some(extrinsic));
		self.network.gossip_message(self.attestation_topic, candidate.encode());
	}

	fn fetch_block_data(&self, candidate: &CandidateReceipt) -> Self::FetchCandidate {
		let parent_hash = self.parent_hash;
		let rx = self.network.with_spec(|spec, ctx| { spec.fetch_block_data(ctx, candidate, parent_hash) });
		BlockDataReceiver { inner: rx }
	}

	fn fetch_incoming(&self, parachain: ParaId) -> Self::FetchIncoming {
		self.do_fetch_incoming(parachain)
	}
}

impl<P, N: NetworkService> Drop for Router<P, N> {
	fn drop(&mut self) {
		let parent_hash = &self.parent_hash;
		self.network.with_spec(|spec, _| spec.remove_consensus(parent_hash));
		self.network.drop_gossip(self.attestation_topic);

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

// A unique trace for valid statements issued by a validator.
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
enum StatementTrace {
	Valid(SessionKey, Hash),
	Invalid(SessionKey, Hash),
}

// helper for deferring statements whose associated candidate is unknown.
struct DeferredStatements {
	deferred: HashMap<Hash, Vec<SignedStatement>>,
	known_traces: HashSet<StatementTrace>,
}

impl DeferredStatements {
	fn new() -> Self {
		DeferredStatements {
			deferred: HashMap::new(),
			known_traces: HashSet::new(),
		}
	}

	fn push(&mut self, statement: SignedStatement) {
		let (hash, trace) = match statement.statement {
			GenericStatement::Candidate(_) => return,
			GenericStatement::Valid(hash) => (hash, StatementTrace::Valid(statement.sender, hash)),
			GenericStatement::Invalid(hash) => (hash, StatementTrace::Invalid(statement.sender, hash)),
		};

		if self.known_traces.insert(trace) {
			self.deferred.entry(hash).or_insert_with(Vec::new).push(statement);
		}
	}

	fn get_deferred(&mut self, hash: &Hash) -> (Vec<SignedStatement>, Vec<StatementTrace>) {
		match self.deferred.remove(hash) {
			None => (Vec::new(), Vec::new()),
			Some(deferred) => {
				let mut traces = Vec::new();
				for statement in deferred.iter() {
					let trace = match statement.statement {
						GenericStatement::Candidate(_) => continue,
						GenericStatement::Valid(hash) => StatementTrace::Valid(statement.sender, hash),
						GenericStatement::Invalid(hash) => StatementTrace::Invalid(statement.sender, hash),
					};

					self.known_traces.remove(&trace);
					traces.push(trace);
				}

				(deferred, traces)
			}
		}
	}
}

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
					if ::polkadot_consensus::egress_trie_root(messages) != canon_root {
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
	use substrate_primitives::H512;

	#[test]
	fn deferred_statements_works() {
		let mut deferred = DeferredStatements::new();
		let hash = [1; 32].into();
		let sig = H512::from([2; 64]).into();
		let sender = [255; 32].into();

		let statement = SignedStatement {
			statement: GenericStatement::Valid(hash),
			sender,
			signature: sig,
		};

		// pre-push.
		{
			let (signed, traces) = deferred.get_deferred(&hash);
			assert!(signed.is_empty());
			assert!(traces.is_empty());
		}

		deferred.push(statement.clone());
		deferred.push(statement.clone());

		// draining: second push should have been ignored.
		{
			let (signed, traces) = deferred.get_deferred(&hash);
			assert_eq!(signed.len(), 1);

			assert_eq!(traces.len(), 1);
			assert_eq!(signed[0].clone(), statement);
			assert_eq!(traces[0].clone(), StatementTrace::Valid(sender, hash));
		}

		// after draining
		{
			let (signed, traces) = deferred.get_deferred(&hash);
			assert!(signed.is_empty());
			assert!(traces.is_empty());
		}
	}
}
