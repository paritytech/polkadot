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
//! the `TableRouter` trait from `polkadot-validation`, which is expected to call into a shared statement table
//! and dispatch evaluation work as necessary when new statements come in.

use sr_primitives::traits::{ProvideRuntimeApi, BlakeTwo256, Hash as HashT};
use polkadot_validation::{
	SharedTable, TableRouter, SignedStatement, GenericStatement, ParachainWork, Incoming,
	Validated, Outgoing,
};
use polkadot_primitives::{Block, Hash, SessionKey};
use polkadot_primitives::parachain::{
	BlockData, Extrinsic, CandidateReceipt, ParachainHost, Id as ParaId, Message
};

use codec::{Encode, Decode};
use futures::{future, prelude::*};
use futures::sync::oneshot::{self, Receiver};
use parking_lot::Mutex;

use std::collections::{hash_map::{Entry, HashMap}, HashSet};
use std::{io, mem};
use std::sync::Arc;

use validation::{NetworkService, Knowledge, Executor};

type IngressPair = (ParaId, Vec<Message>);
type IngressPairRef<'a> = (ParaId, &'a [Message]);

fn attestation_topic(parent_hash: Hash) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	v.extend(b"attestations");

	BlakeTwo256::hash(&v[..])
}

fn incoming_message_topic(parent_hash: Hash, parachain: ParaId) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	parachain.using_encoded(|s| v.extend(s));
	v.extend(b"incoming");

	BlakeTwo256::hash(&v[..])
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
pub struct Router<P, E, N: NetworkService, T> {
	table: Arc<SharedTable>,
	network: Arc<N>,
	api: Arc<P>,
	exit: E,
	task_executor: T,
	parent_hash: Hash,
	attestation_topic: Hash,
	knowledge: Arc<Mutex<Knowledge>>,
	fetch_incoming: Arc<Mutex<HashMap<ParaId, IncomingReceiver>>>,
	deferred_statements: Arc<Mutex<DeferredStatements>>,
}

impl<P, E, N: NetworkService, T> Router<P, E, N, T> {
	pub(crate) fn new(
		table: Arc<SharedTable>,
		network: Arc<N>,
		api: Arc<P>,
		task_executor: T,
		parent_hash: Hash,
		knowledge: Arc<Mutex<Knowledge>>,
		exit: E,
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
			exit,
		}
	}

	/// Get the attestation topic for gossip.
	pub(crate) fn gossip_topic(&self) -> Hash {
		self.attestation_topic
	}
}

impl<P, E: Clone, N: NetworkService, T: Clone> Clone for Router<P, E, N, T> {
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
			exit: self.exit.clone(),
		}
	}
}

impl<P: ProvideRuntimeApi + Send + Sync + 'static, E, N, T> Router<P, E, N, T> where
	P::Api: ParachainHost<Block>,
	N: NetworkService,
	T: Clone + Executor + Send + 'static,
	E: Future<Item=(),Error=()> + Clone + Send + 'static,
{
	/// Import a statement whose signature has been checked already.
	pub(crate) fn import_statement(&self, statement: SignedStatement) {
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
				let work = work.select2(self.exit.clone()).then(|_| Ok(()));
				self.task_executor.spawn(work);
			}
		}
	}

	/// Broadcast outgoing messages to peers.
	pub(crate) fn broadcast_egress(&self, outgoing: Outgoing) {
		use slice_group_by::LinearGroupBy;

		let mut group_messages = Vec::new();
		for egress in outgoing {
			let source = egress.from;
			let messages = egress.messages.outgoing_messages;

			let groups = LinearGroupBy::new(&messages, |a, b| a.target == b.target);
			for group in groups {
				let target = match group.get(0) {
					Some(msg) => msg.target,
					None => continue, // skip empty.
				};

				group_messages.clear(); // reuse allocation from previous iterations.
				group_messages.extend(group.iter().map(|msg| msg.data.clone()).map(Message));

				debug!(target: "consensus", "Circulating messages from {:?} to {:?} at {}",
					source, target, self.parent_hash);

				// this is the ingress from source to target, with given messages.
				let target_incoming = incoming_message_topic(self.parent_hash, target);
				let ingress_for: IngressPairRef = (source, &group_messages[..]);

				self.network.gossip_message(target_incoming, ingress_for.encode());
			}
		}
	}

	fn create_work<D>(&self, candidate_hash: Hash, producer: ParachainWork<D>)
		-> impl Future<Item=(),Error=()> + Send + 'static
		where
		D: Future<Item=(BlockData, Incoming),Error=io::Error> + Send + 'static,
	{
		let table = self.table.clone();
		let network = self.network.clone();
		let knowledge = self.knowledge.clone();
		let attestation_topic = self.attestation_topic.clone();

		producer.prime(self.api.clone())
			.map(move |validated| {
				// store the data before broadcasting statements, so other peers can fetch.
				knowledge.lock().note_candidate(
					candidate_hash,
					Some(validated.block_data().clone()),
					validated.extrinsic().cloned(),
				);

				// propagate the statement.
				// consider something more targeted than gossip in the future.
				let signed = table.import_validated(validated);
				network.gossip_message(attestation_topic, signed.encode());
			})
			.map_err(|e| debug!(target: "p_net", "Failed to produce statements: {:?}", e))
	}

}

impl<P: ProvideRuntimeApi, E, N, T> Router<P, E, N, T> where
	P::Api: ParachainHost<Block>,
	N: NetworkService,
	T: Executor,
	E: Future<Item=(),Error=()> + Clone + Send + 'static,
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

impl<P: ProvideRuntimeApi + Send, E, N, T> TableRouter for Router<P, E, N, T> where
	P::Api: ParachainHost<Block>,
	N: NetworkService,
	T: Clone + Executor + Send + 'static,
	E: Future<Item=(),Error=()> + Clone + Send + 'static,
{
	type Error = io::Error;
	type FetchCandidate = BlockDataReceiver;
	type FetchIncoming = IncomingReceiver;

	fn local_candidate(&self, receipt: CandidateReceipt, block_data: BlockData, extrinsic: Extrinsic) {
		// produce a signed statement
		let hash = receipt.hash();
		let validated = Validated::collated_local(receipt, block_data.clone(), extrinsic.clone());
		let statement = self.table.import_validated(validated);

		// give to network to make available.
		self.knowledge.lock().note_candidate(hash, Some(block_data), Some(extrinsic));
		self.network.gossip_message(self.attestation_topic, statement.encode());
	}

	fn fetch_block_data(&self, candidate: &CandidateReceipt) -> BlockDataReceiver {
		let parent_hash = self.parent_hash.clone();
		let candidate = candidate.clone();
		let (tx, rx) = ::futures::sync::oneshot::channel();
		self.network.with_spec(move |spec, ctx| {
			let inner_rx = spec.fetch_block_data(ctx, &candidate, parent_hash);
			let _ = tx.send(inner_rx);
		});
		BlockDataReceiver { outer: rx, inner: None }
	}

	fn fetch_incoming(&self, parachain: ParaId) -> Self::FetchIncoming {
		self.do_fetch_incoming(parachain)
	}
}

impl<P, E, N: NetworkService, T> Drop for Router<P, E, N, T> {
	fn drop(&mut self) {
		let parent_hash = self.parent_hash.clone();
		self.network.with_spec(move |spec, _| spec.remove_validation_session(&parent_hash));
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
					Some(mem::replace(&mut self.incoming, Vec::new()))
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
	use substrate_primitives::H512;
	use futures::stream;

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
}
