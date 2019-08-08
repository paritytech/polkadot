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

//! Statement routing and validation statement table router implementation.
//!
//! During the attestation process, validators exchange statements on validity and availability
//! of parachain candidates.
//!
//! The `Router` in this file hooks into the underlying network to fulfill
//! the `TableRouter` trait from `polkadot-validation`, which is expected to call into a shared statement table
//! and dispatch evaluation work as necessary when new statements come in.

use sr_primitives::traits::{ProvideRuntimeApi, BlakeTwo256, Hash as HashT};
use polkadot_validation::{
	SharedTable, TableRouter, SignedStatement, GenericStatement, ParachainWork, Validated
};
use polkadot_primitives::{Block, Hash};
use polkadot_primitives::parachain::{
	Extrinsic, CandidateReceipt, ParachainHost, ValidatorIndex, Collation, PoVBlock, ErasureChunk,
};
use crate::gossip::{RegisteredMessageValidator, GossipMessage, GossipStatement, ErasureChunkMessage};

use futures::prelude::*;
use parking_lot::Mutex;
use log::{debug, trace};

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;

use crate::validation::{self, SessionDataFetcher, NetworkService, Executor};

/// Compute the gossip topic for attestations on the given parent hash.
pub(crate) fn attestation_topic(parent_hash: Hash) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	v.extend(b"attestations");

	BlakeTwo256::hash(&v[..])
}

/// Compute the gossip topic for erasure chunks of the given block hash.
pub(crate) fn erasure_chunks_topic(parent_hash: Hash) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	v.extend(b"erasure_encoding");

	BlakeTwo256::hash(&v[..])
}

/// Create a `Stream` of checked messages.
///
/// The returned stream will not terminate, so it is required to make sure that the stream is
/// dropped when it is not required anymore. Otherwise, it will stick around in memory
/// infinitely.
pub(crate) fn checked_statements<N: NetworkService>(network: &N, topic: Hash) ->
	impl Stream<Item=SignedStatement, Error=()> {
	// spin up a task in the background that processes all incoming statements.
	// validation has been done already by the gossip validator.
	// this will block internally until the gossip messages stream is obtained.
	network.gossip_messages_for(topic)
		.filter_map(|msg| match msg.0 {
			GossipMessage::Statement(s) => Some(s.signed_statement),
			_ => None
		})
}

/// Create a `Stream` of erasure chunk messages.
///
/// The returned stream will not terminate, so it is required to make sure that the stream is
/// dropped when it is not required anymore. Otherwise, it will stick around in memory
/// infinitely.
pub(crate) fn erasure_chunks<N: NetworkService>(network: &N, topic: Hash) ->
	impl Stream<Item=ErasureChunk, Error=()> {

	network.gossip_messages_for(topic)
		.filter_map(|msg| match msg.0 {
			GossipMessage::ErasureChunk(msg) => Some(msg.chunk),
			_ => None
		})
}

/// Table routing implementation.
pub struct Router<P, E, N: NetworkService, T> {
	table: Arc<SharedTable>,
	attestation_topic: Hash,
	erasure_chunks_topic: Hash,
	fetcher: SessionDataFetcher<P, E, N, T>,
	deferred_statements: Arc<Mutex<DeferredStatements>>,
	deferred_chunks: Arc<Mutex<DeferredChunks>>,
	message_validator: RegisteredMessageValidator,
}

impl<P, E, N: NetworkService, T> Router<P, E, N, T> {
	pub(crate) fn new(
		table: Arc<SharedTable>,
		fetcher: SessionDataFetcher<P, E, N, T>,
		message_validator: RegisteredMessageValidator,
	) -> Self {
		let parent_hash = fetcher.parent_hash();
		Router {
			table,
			fetcher,
			attestation_topic: attestation_topic(parent_hash),
			erasure_chunks_topic: erasure_chunks_topic(parent_hash),
			deferred_statements: Arc::new(Mutex::new(DeferredStatements::new())),
			deferred_chunks: Arc::new(Mutex::new(DeferredChunks::new())),
			message_validator,
		}
	}

	/// Return a `Stream` of checked messages. These should be imported into the router
	/// with `import_statement`.
	///
	/// The returned stream will not terminate, so it is required to make sure that the stream is
	/// dropped when it is not required anymore. Otherwise, it will stick around in memory
	/// infinitely.
	pub(crate) fn checked_statements(&self) -> impl Stream<Item=SignedStatement, Error=()> {
		checked_statements(&**self.network(), self.attestation_topic)
	}

	/// Return a `Stream` of erasure chunk messages. These should be imported into the router
	/// with `import_erasure_chunk`.
	///
	/// The returned stream will not terminate, so it is required to make sure that the stream is
	/// dropped when it is not required anymore. Otherwise, it will stick around in memory
	/// infinitely.
	pub(crate) fn erasure_chunks(&self) -> impl Stream<Item=ErasureChunk, Error=()> {
		erasure_chunks(&**self.network(), self.attestation_topic)
	}

	fn parent_hash(&self) -> Hash {
		self.fetcher.parent_hash()
	}

	fn network(&self) -> &Arc<N> {
		self.fetcher.network()
	}
}

impl<P, E: Clone, N: NetworkService, T: Clone> Clone for Router<P, E, N, T> {
	fn clone(&self) -> Self {
		Router {
			table: self.table.clone(),
			fetcher: self.fetcher.clone(),
			attestation_topic: self.attestation_topic,
			erasure_chunks_topic: self.erasure_chunks_topic,
			deferred_statements: self.deferred_statements.clone(),
			deferred_chunks: self.deferred_chunks.clone(),
			message_validator: self.message_validator.clone(),
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
		trace!(target: "p_net", "importing validation statement {:?}", statement.statement);

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
		debug!(target: "validation", "Importing statements about candidate {:?}", c_hash);
		statements.insert(0, statement);
		let producers: Vec<_> = self.table.import_remote_statements(
			self,
			statements.iter().cloned(),
		);
		// dispatch future work as necessary.
		for (producer, statement) in producers.into_iter().zip(statements) {
			if let Some(sender) = self.table.index_to_id(statement.sender) {
				self.fetcher.knowledge().lock().note_statement(sender, &statement.statement);

				if let Some(work) = producer.map(|p| self.create_work(c_hash, p)) {
					trace!(target: "validation", "driving statement work to completion");
					let work = work.select2(self.fetcher.exit().clone()).then(|_| Ok(()));
					self.fetcher.executor().spawn(work);
				}
			}
		}
	}
	/// Import a given erasure chunk.
	pub(crate) fn import_erasure_chunk(&self, chunk: ErasureChunk) {
		trace!(target: "p_net", "importing erasure chunk {:?}", chunk);

		let table = self.table.clone();
		let hash = &chunk.candidate_hash;

		match self.table.with_candidate(hash, |c| c.map(|_| *hash)) {
			Some(_) => (),
			None => {
				self.deferred_chunks.lock().push(chunk);
				return;
			}
		}

		table.import_erasure_chunk(chunk);
	}

	fn create_work<D>(&self, candidate_hash: Hash, producer: ParachainWork<D>)
		-> impl Future<Item=(),Error=()> + Send + 'static
		where
		D: Future<Item=PoVBlock,Error=io::Error> + Send + 'static,
	{
		let table = self.table.clone();
		let network = self.network().clone();
		let knowledge = self.fetcher.knowledge().clone();
		let attestation_topic = self.attestation_topic;
		let parent_hash = self.parent_hash();
		let deferred_chunks = self.deferred_chunks.clone();

		producer.prime(self.fetcher.api().clone())
			.map(move |validated| {
				// store the data before broadcasting statements, so other peers can fetch.
				knowledge.lock().note_candidate(
					candidate_hash,
					Some(validated.pov_block().clone()),
					validated.extrinsic().cloned(),
				);


				// propagate the statement.
				// consider something more targeted than gossip in the future.
				let statement = GossipStatement::new(
					parent_hash,
					table.import_validated(validated),
				);

				// regardless of the validation result remove all deferred chunks.
				// if the validation was successful, import the deferred chunks
				let chunks = deferred_chunks.lock().get_deferred(&candidate_hash);
				if let GenericStatement::Valid(_) = statement.signed_statement.statement {
					for chunk in chunks {
						table.import_erasure_chunk(chunk);
					}
				}
				network.gossip_message(attestation_topic, statement.into());
			})
			.map_err(|e| debug!(target: "p_net", "Failed to produce statements: {:?}", e))
	}
}

impl<P: ProvideRuntimeApi + Send, E, N, T> TableRouter for Router<P, E, N, T> where
	P::Api: ParachainHost<Block>,
	N: NetworkService,
	T: Clone + Executor + Send + 'static,
	E: Future<Item=(),Error=()> + Clone + Send + 'static,
{
	type Error = io::Error;
	type FetchValidationProof = validation::PoVReceiver;
	type FetchBlockChunk = validation::ChunkReceiver;

	fn local_collation(&self, collation: Collation, extrinsic: Extrinsic, chunks: (ValidatorIndex, &Vec<ErasureChunk>)) {
		// produce a signed statement
		let hash = collation.receipt.hash();
		let validated = Validated::collated_local(collation.receipt, collation.pov.clone(), extrinsic.clone());
		let statement = GossipStatement::new(
			self.parent_hash(),
			self.table.import_validated(validated),
		);

		// give to network to make available.
		self.fetcher.knowledge().lock().note_candidate(hash, Some(collation.pov), Some(extrinsic));
		self.network().gossip_message(self.attestation_topic, statement.into());

		for chunk in chunks.1 {
			let signature = self.table.sign_chunk(chunk, chunks.0);
			let message = ErasureChunkMessage {
				chunk: chunk.clone(),
				sender: chunks.0,
				signature
			};

			self.network().gossip_message(self.erasure_chunks_topic, message.into());
		}
	}

	fn fetch_pov_block(&self, candidate: &CandidateReceipt) -> Self::FetchValidationProof {
		self.fetcher.fetch_pov_block(candidate)
	}

	fn fetch_erasure_chunk(
		&self,
		relay_parent: Hash,
		candidate_hash: Hash,
		chunk_id: u64
	) -> Self::FetchBlockChunk {
		self.fetcher.fetch_erasure_chunk(relay_parent, candidate_hash, chunk_id)
	}
}

impl<P, E, N: NetworkService, T> Drop for Router<P, E, N, T> {
	fn drop(&mut self) {
		let parent_hash = self.parent_hash();
		self.network().with_spec(move |spec, _| { spec.remove_validation_session(parent_hash); });
	}
}

// A unique trace for valid statements issued by a validator.
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
enum StatementTrace {
	Valid(ValidatorIndex, Hash),
	Invalid(ValidatorIndex, Hash),
}

/// Helper for deferring erasure-encoded chunks whose associated candidate block is unknown.
struct DeferredChunks {
	deferred: HashMap<Hash, Vec<ErasureChunk>>,
}

impl DeferredChunks {
	fn new() -> Self {
		DeferredChunks {
			deferred: HashMap::new(),
		}
	}

	fn push(&mut self, chunk: ErasureChunk) {
		self.deferred.entry(chunk.candidate_hash).or_insert_with(Vec::new).push(chunk);
	}

	fn get_deferred(&mut self, hash: &Hash) -> Vec<ErasureChunk> {
		match self.deferred.remove(hash) {
			None => Vec::new(),
			Some(deferred) => deferred,
		}
	}
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
			GenericStatement::Valid(hash) => (hash, StatementTrace::Valid(statement.sender.clone(), hash)),
			GenericStatement::Invalid(hash) => (hash, StatementTrace::Invalid(statement.sender.clone(), hash)),
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
						GenericStatement::Valid(hash) => StatementTrace::Valid(statement.sender.clone(), hash),
						GenericStatement::Invalid(hash) => StatementTrace::Invalid(statement.sender.clone(), hash),
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

	#[test]
	fn deferred_statements_works() {
		let mut deferred = DeferredStatements::new();
		let hash = [1; 32].into();
		let sig = Default::default();
		let sender_index = 0;

		let statement = SignedStatement {
			statement: GenericStatement::Valid(hash),
			sender: sender_index,
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
			assert_eq!(traces[0].clone(), StatementTrace::Valid(sender_index, hash));
		}

		// after draining
		{
			let (signed, traces) = deferred.get_deferred(&hash);
			assert!(signed.is_empty());
			assert!(traces.is_empty());
		}
	}
}
