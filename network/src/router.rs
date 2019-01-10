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
use polkadot_consensus::{SharedTable, TableRouter, SignedStatement, GenericStatement, StatementProducer};
use polkadot_primitives::{Block, Hash, BlockId, SessionKey};
use polkadot_primitives::parachain::{BlockData, Extrinsic, CandidateReceipt, ParachainHost};

use codec::Encode;
use futures::prelude::*;
use tokio::runtime::TaskExecutor;
use parking_lot::Mutex;

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;

use consensus::Knowledge;
use super::NetworkService;

fn attestation_topic(parent_hash: Hash) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	v.extend(b"attestations");

	BlakeTwo256::hash(&v[..])
}

/// Table routing implementation.
pub struct Router<P> {
	table: Arc<SharedTable>,
	network: Arc<NetworkService>,
	api: Arc<P>,
	task_executor: TaskExecutor,
	parent_hash: Hash,
	attestation_topic: Hash,
	knowledge: Arc<Mutex<Knowledge>>,
	deferred_statements: Arc<Mutex<DeferredStatements>>,
}

impl<P> Router<P> {
	pub(crate) fn new(
		table: Arc<SharedTable>,
		network: Arc<NetworkService>,
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
			deferred_statements: Arc::new(Mutex::new(DeferredStatements::new())),
		}
	}

	/// Get the attestation topic for gossip.
	pub(crate) fn gossip_topic(&self) -> Hash {
		self.attestation_topic
	}
}

impl<P> Clone for Router<P> {
	fn clone(&self) -> Self {
		Router {
			table: self.table.clone(),
			network: self.network.clone(),
			api: self.api.clone(),
			task_executor: self.task_executor.clone(),
			parent_hash: self.parent_hash.clone(),
			attestation_topic: self.attestation_topic.clone(),
			deferred_statements: self.deferred_statements.clone(),
			knowledge: self.knowledge.clone(),
		}
	}
}

impl<P: ProvideRuntimeApi + Send + Sync + 'static> Router<P>
	where P::Api: ParachainHost<Block>
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
					| GenericStatement::Available(ref hash)
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

	fn create_work<D, E>(&self, candidate_hash: Hash, producer: StatementProducer<D, E>)
		-> impl Future<Item=(),Error=()>
		where
		D: Future<Item=BlockData,Error=io::Error> + Send + 'static,
		E: Future<Item=Extrinsic,Error=io::Error> + Send + 'static,
	{
		let parent_hash = self.parent_hash.clone();

		let api = self.api.clone();
		let validate = move |collation| -> Option<bool> {
			let id = BlockId::hash(parent_hash);
			match ::polkadot_consensus::validate_collation(&*api, &id, &collation) {
				Ok(()) => Some(true),
				Err(e) => {
					debug!(target: "p_net", "Encountered bad collation: {}", e);
					Some(false)
				}
			}
		};

		let table = self.table.clone();
		let network = self.network.clone();
		let knowledge = self.knowledge.clone();
		let attestation_topic = self.attestation_topic.clone();

		producer.prime(validate)
			.map(move |produced| {
				// store the data before broadcasting statements, so other peers can fetch.
				knowledge.lock().note_candidate(
					candidate_hash,
					produced.block_data,
					produced.extrinsic,
				);

				if produced.validity.is_none() && produced.availability.is_none() {
					return
				}

				let mut gossip = network.consensus_gossip().write();

				// propagate the statements
				// consider something more targeted than gossip in the future.
				if let Some(validity) = produced.validity {
					let signed = table.sign_and_import(validity.clone()).0;
					network.with_spec(|_, ctx|
						gossip.multicast(ctx, attestation_topic, signed.encode(), false)
					);
				}

				if let Some(availability) = produced.availability {
					let signed = table.sign_and_import(availability).0;
					network.with_spec(|_, ctx|
						gossip.multicast(ctx, attestation_topic, signed.encode(), false)
					);
				}
			})
			.map_err(|e| debug!(target: "p_net", "Failed to produce statements: {:?}", e))
	}
}

impl<P: ProvideRuntimeApi + Send> TableRouter for Router<P>
	where P::Api: ParachainHost<Block>
{
	type Error = io::Error;
	type FetchCandidate = BlockDataReceiver;
	type FetchExtrinsic = Result<Extrinsic, Self::Error>;

	fn local_candidate(&self, receipt: CandidateReceipt, block_data: BlockData, extrinsic: Extrinsic) {
		// give to network to make available.
		let hash = receipt.hash();
		let (candidate, availability) = self.table.sign_and_import(GenericStatement::Candidate(receipt));

		self.knowledge.lock().note_candidate(hash, Some(block_data), Some(extrinsic));
		let mut gossip = self.network.consensus_gossip().write();
		self.network.with_spec(|_spec, ctx| {
			gossip.multicast(ctx, self.attestation_topic, candidate.encode(), false);
			if let Some(availability) = availability {
				gossip.multicast(ctx, self.attestation_topic, availability.encode(), false);
			}
		});
	}

	fn fetch_block_data(&self, candidate: &CandidateReceipt) -> BlockDataReceiver {
		let parent_hash = self.parent_hash;
		let rx = self.network.with_spec(|spec, ctx| { spec.fetch_block_data(ctx, candidate, parent_hash) });
		BlockDataReceiver { inner: rx }
	}

	fn fetch_extrinsic_data(&self, _candidate: &CandidateReceipt) -> Self::FetchExtrinsic {
		Ok(Extrinsic)
	}
}

impl<P> Drop for Router<P> {
	fn drop(&mut self) {
		let parent_hash = &self.parent_hash;
		self.network.with_spec(|spec, _| spec.remove_consensus(parent_hash));
	}
}

/// Receiver for block data.
pub struct BlockDataReceiver {
	inner: ::futures::sync::oneshot::Receiver<BlockData>,
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

// A unique trace for valid statements issued by a validator.
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
enum StatementTrace {
	Valid(SessionKey, Hash),
	Invalid(SessionKey, Hash),
	Available(SessionKey, Hash),
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
			GenericStatement::Available(hash) => (hash, StatementTrace::Available(statement.sender, hash)),
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
						GenericStatement::Available(hash) => StatementTrace::Available(statement.sender, hash),
					};

					self.known_traces.remove(&trace);
					traces.push(trace);
				}

				(deferred, traces)
			}
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
