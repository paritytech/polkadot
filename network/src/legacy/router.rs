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

//! Statement routing and validation statement table router implementation.
//!
//! During the attestation process, validators exchange statements on validity and availability
//! of parachain candidates.
//!
//! The `Router` in this file hooks into the underlying network to fulfill
//! the `TableRouter` trait from `polkadot-validation`, which is expected to call into a shared statement table
//! and dispatch evaluation work as necessary when new statements come in.

use sp_runtime::traits::{BlakeTwo256, Hash as HashT};
use polkadot_validation::{
	SharedTable, TableRouter, SignedStatement, GenericStatement, ParachainWork, Validated
};
use polkadot_primitives::{Block, Hash};
use polkadot_primitives::parachain::{
	OutgoingMessages, CandidateReceipt, ParachainHost, ValidatorIndex, Collation, PoVBlock, ErasureChunk,
};
use sp_api::ProvideRuntimeApi;

use futures::prelude::*;
use futures::{task::SpawnExt, future::ready};
use parking_lot::Mutex;
use log::{debug, trace};

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;
use std::pin::Pin;

use crate::legacy::gossip::{RegisteredMessageValidator, GossipMessage, GossipStatement, ErasureChunkMessage};
use crate::legacy::validation::{LeafWorkDataFetcher, Executor};
use crate::legacy::{NetworkService, PolkadotProtocol};

/// Compute the gossip topic for attestations on the given parent hash.
pub(crate) fn attestation_topic(parent_hash: Hash) -> Hash {
	let mut v = parent_hash.as_ref().to_vec();
	v.extend(b"attestations");

	BlakeTwo256::hash(&v[..])
}

/// Create a `Stream` of checked messages.
///
/// The returned stream will not terminate, so it is required to make sure that the stream is
/// dropped when it is not required anymore. Otherwise, it will stick around in memory
/// infinitely.
pub(crate) fn checked_statements<N: NetworkService>(network: &N, topic: Hash) ->
	impl Stream<Item=SignedStatement> {
	// spin up a task in the background that processes all incoming statements
	// validation has been done already by the gossip validator.
	// this will block internally until the gossip messages stream is obtained.
	network.gossip_messages_for(topic)
		.filter_map(|msg| match msg.0 {
			GossipMessage::Statement(s) => ready(Some(s.signed_statement)),
			_ => ready(None)
		})
}

/// Table routing implementation.
pub struct Router<P, T> {
	table: Arc<SharedTable>,
	attestation_topic: Hash,
	fetcher: LeafWorkDataFetcher<P, T>,
	deferred_statements: Arc<Mutex<DeferredStatements>>,
	message_validator: RegisteredMessageValidator<PolkadotProtocol>,
	drop_signal: Arc<exit_future::Signal>,
}

impl<P, T> Router<P, T> {
	pub(crate) fn new(
		table: Arc<SharedTable>,
		fetcher: LeafWorkDataFetcher<P, T>,
		message_validator: RegisteredMessageValidator<PolkadotProtocol>,
		drop_signal: exit_future::Signal,
	) -> Self {
		let parent_hash = fetcher.parent_hash();
		Router {
			table,
			fetcher,
			attestation_topic: attestation_topic(parent_hash),
			deferred_statements: Arc::new(Mutex::new(DeferredStatements::new())),
			message_validator,
			drop_signal: Arc::new(drop_signal),
		}
	}

	/// Return a `Stream` of checked messages. These should be imported into the router
	/// with `import_statement`.
	///
	/// The returned stream will not terminate, so it is required to make sure that the stream is
	/// dropped when it is not required anymore. Otherwise, it will stick around in memory
	/// infinitely.
	pub(crate) fn checked_statements(&self) -> impl Stream<Item=SignedStatement> {
		checked_statements(&*self.network(), self.attestation_topic)
	}

	fn parent_hash(&self) -> Hash {
		self.fetcher.parent_hash()
	}

	fn network(&self) -> &RegisteredMessageValidator<PolkadotProtocol> {
		self.fetcher.network()
	}
}

impl<P, T: Clone> Clone for Router<P, T> {
	fn clone(&self) -> Self {
		Router {
			table: self.table.clone(),
			fetcher: self.fetcher.clone(),
			attestation_topic: self.attestation_topic,
			deferred_statements: self.deferred_statements.clone(),
			message_validator: self.message_validator.clone(),
			drop_signal: self.drop_signal.clone(),
		}
	}
}

impl<P: ProvideRuntimeApi<Block> + Send + Sync + 'static, T> Router<P, T> where
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	T: Clone + Executor + Send + 'static,
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
			self.deferred_statements.lock().take_deferred(&c_hash)
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

					let work = work.boxed().map(drop);
					let _ = self.fetcher.executor().spawn(work);
				}
			}
		}
	}

	fn create_work<D>(&self, candidate_hash: Hash, producer: ParachainWork<D>)
		-> impl Future<Output=()> + Send + 'static
		where
		D: Future<Output=Result<PoVBlock,io::Error>> + Send + Unpin + 'static,
	{
		let table = self.table.clone();
		let network = self.network().clone();
		let knowledge = self.fetcher.knowledge().clone();
		let attestation_topic = self.attestation_topic;
		let parent_hash = self.parent_hash();
		let api = self.fetcher.api().clone();

		async move {
			match producer.prime(api).validate().await {
				Ok(validated) => {
					// store the data before broadcasting statements, so other peers can fetch.
					knowledge.lock().note_candidate(
					candidate_hash,
					Some(validated.0.pov_block().clone()),
					validated.0.outgoing_messages().cloned(),
					);

					// propagate the statement.
					// consider something more targeted than gossip in the future.
					let statement = GossipStatement::new(
					parent_hash,
					match table.import_validated(validated.0) {
					None => return,
					Some(s) => s,
					}
					);

					network.gossip_message(attestation_topic, statement.into());
				},
				Err(err) => {
					debug!(target: "p_net", "Failed to produce statements: {:?}", err);
				}
			}
		}
	}
}

impl<P: ProvideRuntimeApi<Block> + Send, T> TableRouter for Router<P, T> where
	P::Api: ParachainHost<Block>,
	T: Clone + Executor + Send + 'static,
{
	type Error = io::Error;
	type SendLocalCollation = future::Ready<Result<(), Self::Error>>;
	type FetchValidationProof = Pin<Box<dyn Future<Output = Result<PoVBlock, io::Error>> + Send>>;

	// We have fetched from a collator and here the receipt should have been already formed.
	fn local_collation(
		&self,
		collation: Collation,
		receipt: CandidateReceipt,
		outgoing: OutgoingMessages,
		chunks: (ValidatorIndex, &[ErasureChunk])
	) -> Self::SendLocalCollation {
		// produce a signed statement
		let hash = receipt.hash();
		let erasure_root = receipt.erasure_root;
		let validated = Validated::collated_local(
			receipt,
			collation.pov.clone(),
			outgoing.clone(),
		);

		let statement = GossipStatement::new(
			self.parent_hash(),
			match self.table.import_validated(validated) {
				None => return future::ready(Ok(())),
				Some(s) => s,
			},
		);

		// give to network to make available.
		self.fetcher.knowledge().lock().note_candidate(hash, Some(collation.pov), Some(outgoing));
		self.network().gossip_message(self.attestation_topic, statement.into());

		for chunk in chunks.1 {
			let relay_parent = self.parent_hash();
			let message = ErasureChunkMessage {
				chunk: chunk.clone(),
				relay_parent,
				candidate_hash: hash,
			};

			self.network().gossip_message(
				av_store::erasure_coding_topic(relay_parent, erasure_root, chunk.index),
				message.into()
			);
		}

		future::ready(Ok(()))
	}

	fn fetch_pov_block(&self, candidate: &CandidateReceipt) -> Self::FetchValidationProof {
		self.fetcher.fetch_pov_block(candidate)
	}
}

impl<P, T> Drop for Router<P, T> {
	fn drop(&mut self) {
		let parent_hash = self.parent_hash();
		self.network().with_spec(move |spec, _| { spec.remove_validation_session(parent_hash); });
	}
}

// A unique trace for valid statements issued by a validator.
#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub(crate) enum StatementTrace {
	Valid(ValidatorIndex, Hash),
	Invalid(ValidatorIndex, Hash),
}

/// Helper for deferring statements whose associated candidate is unknown.
pub(crate) struct DeferredStatements {
	deferred: HashMap<Hash, Vec<SignedStatement>>,
	known_traces: HashSet<StatementTrace>,
}

impl DeferredStatements {
	/// Create a new `DeferredStatements`.
	pub(crate) fn new() -> Self {
		DeferredStatements {
			deferred: HashMap::new(),
			known_traces: HashSet::new(),
		}
	}

	/// Push a new statement onto the deferred pile. `Candidate` statements
	/// cannot be deferred and are ignored.
	pub(crate) fn push(&mut self, statement: SignedStatement) {
		let (hash, trace) = match statement.statement {
			GenericStatement::Candidate(_) => return,
			GenericStatement::Valid(hash) => (hash, StatementTrace::Valid(statement.sender.clone(), hash)),
			GenericStatement::Invalid(hash) => (hash, StatementTrace::Invalid(statement.sender.clone(), hash)),
		};

		if self.known_traces.insert(trace) {
			self.deferred.entry(hash).or_insert_with(Vec::new).push(statement);
		}
	}

	/// Take all deferred statements referencing the given candidate hash out.
	pub(crate) fn take_deferred(&mut self, hash: &Hash) -> (Vec<SignedStatement>, Vec<StatementTrace>) {
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
			let (signed, traces) = deferred.take_deferred(&hash);
			assert!(signed.is_empty());
			assert!(traces.is_empty());
		}

		deferred.push(statement.clone());
		deferred.push(statement.clone());

		// draining: second push should have been ignored.
		{
			let (signed, traces) = deferred.take_deferred(&hash);
			assert_eq!(signed.len(), 1);

			assert_eq!(traces.len(), 1);
			assert_eq!(signed[0].clone(), statement);
			assert_eq!(traces[0].clone(), StatementTrace::Valid(sender_index, hash));
		}

		// after draining
		{
			let (signed, traces) = deferred.take_deferred(&hash);
			assert!(signed.is_empty());
			assert!(traces.is_empty());
		}
	}
}
