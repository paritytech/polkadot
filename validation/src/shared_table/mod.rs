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

//! Parachain statement table meant to be shared with a message router
//! and a consensus proposer.

use std::collections::hash_map::{HashMap, Entry};
use std::sync::Arc;

use availability_store::{Store as AvailabilityStore};
use table::{self, Table, Context as TableContextTrait};
use polkadot_primitives::{Block, BlockId, Hash};
use polkadot_primitives::parachain::{
	Id as ParaId, OutgoingMessages, CandidateReceipt, ValidatorPair, ValidatorId,
	AttestedCandidate, ParachainHost, PoVBlock, ValidatorIndex, ErasureChunk,
};

use parking_lot::Mutex;
use futures::prelude::*;
use futures::channel::oneshot;
use log::{warn, debug};
use bitvec::bitvec;

use super::{GroupInfo, TableRouter};
use self::includable::IncludabilitySender;
use primitives::Pair;
use sp_api::ProvideRuntimeApi;

mod includable;

pub use table::{SignedStatement, Statement};
pub use table::generic::Statement as GenericStatement;

struct TableContext {
	parent_hash: Hash,
	key: Option<Arc<ValidatorPair>>,
	groups: HashMap<ParaId, GroupInfo>,
	validators: Vec<ValidatorId>,
}

impl table::Context for TableContext {
	fn is_member_of(&self, authority: ValidatorIndex, group: &ParaId) -> bool {
		let key = match self.validators.get(authority as usize) {
			Some(val) => val,
			None => return false,
		};

		self.groups.get(group).map_or(false, |g| g.validity_guarantors.get(&key).is_some())
	}

	fn requisite_votes(&self, group: &ParaId) -> usize {
		self.groups.get(group).map_or(usize::max_value(), |g| g.needed_validity)
	}
}

impl TableContext {
	fn local_id(&self) -> Option<ValidatorId> {
		self.key.as_ref().map(|k| k.public())
	}

	fn local_index(&self) -> Option<ValidatorIndex> {
		self.local_id().and_then(|id|
			self.validators
				.iter()
				.enumerate()
				.find(|(_, k)| k == &&id)
				.map(|(i, _)| i as ValidatorIndex)
		)
	}

	fn sign_statement(&self, statement: table::Statement) -> Option<table::SignedStatement> {
		self.local_index().and_then(move |sender|
			self.key.as_ref()
				.map(|key| crate::sign_table_statement(&statement, key, &self.parent_hash).into())
				.map(move |signature| table::SignedStatement { statement, signature, sender })
		)
	}
}

pub(crate) enum Validation {
	Valid(PoVBlock, OutgoingMessages),
	Invalid(PoVBlock), // should take proof.
}

enum ValidationWork {
	Done(Validation),
	InProgress,
	Error(String),
}

#[cfg(test)]
impl ValidationWork {
	fn is_in_progress(&self) -> bool {
		match *self {
			ValidationWork::InProgress => true,
			_ => false,
		}
	}

	fn is_done(&self) -> bool {
		match *self {
			ValidationWork::Done(_) => true,
			_ => false,
		}
	}
}

// A shared table object.
struct SharedTableInner {
	table: Table<TableContext>,
	trackers: Vec<IncludabilitySender>,
	availability_store: AvailabilityStore,
	validated: HashMap<Hash, ValidationWork>,
}

impl SharedTableInner {
	// Import a single statement. Provide a handle to a table router and a function
	// used to determine if a referenced candidate is valid.
	//
	// the statement producer, if any, will produce only statements concerning the same candidate
	// as the one just imported
	fn import_remote_statement<R: TableRouter>(
		&mut self,
		context: &TableContext,
		router: &R,
		statement: table::SignedStatement,
		max_block_data_size: Option<u64>,
	) -> Option<ParachainWork<
		R::FetchValidationProof
	>> {
		let summary = self.table.import_statement(context, statement)?;
		self.update_trackers(&summary.candidate, context);

		let local_index = context.local_index()?;
		let para_member = context.is_member_of(local_index, &summary.group_id);
		let digest = &summary.candidate;

		// TODO: consider a strategy based on the number of candidate votes as well.
		// https://github.com/paritytech/polkadot/issues/218
		let do_validation = para_member && match self.validated.entry(digest.clone()) {
			Entry::Occupied(_) => false,
			Entry::Vacant(entry) => {
				entry.insert(ValidationWork::InProgress);
				true
			}
		};

		let work = if do_validation {
			match self.table.get_candidate(&digest) {
				None => {
					let message = format!(
						"Table inconsistency detected. Summary returned for candidate {} \
						but receipt not present in table.",
						digest,
					);

					warn!(target: "validation", "{}", message);
					self.validated.insert(digest.clone(), ValidationWork::Error(message));
					None
				}
				Some(candidate) => {
					let fetch = router.fetch_pov_block(candidate);

					Some(Work {
						candidate_receipt: candidate.clone(),
						fetch,
					})
				}
			}
		} else {
			None
		};

		work.map(|work| ParachainWork {
			availability_store: self.availability_store.clone(),
			relay_parent: context.parent_hash.clone(),
			work,
			local_index: local_index as usize,
			max_block_data_size,
		})
	}

	fn update_trackers(&mut self, candidate: &Hash, context: &TableContext) {
		let includable = self.table.candidate_includable(candidate, context);
		for i in (0..self.trackers.len()).rev() {
			if self.trackers[i].update_candidate(candidate.clone(), includable) {
				self.trackers.swap_remove(i);
			}
		}
	}
}

/// Produced after validating a candidate.
pub struct Validated {
	/// A statement about the validity of the candidate.
	statement: table::Statement,
	/// The result of validation.
	result: Validation,
}

impl Validated {
	/// Note that we've validated a candidate with given hash and it is bad.
	pub fn known_bad(hash: Hash, collation: PoVBlock) -> Self {
		Validated {
			statement: GenericStatement::Invalid(hash),
			result: Validation::Invalid(collation),
		}
	}

	/// Note that we've validated a candidate with given hash and it is good.
	/// outgoing message required.
	pub fn known_good(hash: Hash, collation: PoVBlock, outgoing: OutgoingMessages) -> Self {
		Validated {
			statement: GenericStatement::Valid(hash),
			result: Validation::Valid(collation, outgoing),
		}
	}

	/// Note that we've collated a candidate.
	/// outgoing message required.
	pub fn collated_local(
		receipt: CandidateReceipt,
		collation: PoVBlock,
		outgoing: OutgoingMessages,
	) -> Self {
		Validated {
			statement: GenericStatement::Candidate(receipt),
			result: Validation::Valid(collation, outgoing),
		}
	}

	/// Get a reference to the proof-of-validation block.
	pub fn pov_block(&self) -> &PoVBlock {
		match self.result {
			Validation::Valid(ref b, _) | Validation::Invalid(ref b) => b,
		}
	}

	/// Get a reference to the outgoing messages data, if any.
	pub fn outgoing_messages(&self) -> Option<&OutgoingMessages> {
		match self.result {
			Validation::Valid(_, ref ex) => Some(ex),
			Validation::Invalid(_) => None,
		}
	}
}

/// Future that performs parachain validation work.
pub struct ParachainWork<Fetch> {
	work: Work<Fetch>,
	relay_parent: Hash,
	local_index: usize,
	availability_store: AvailabilityStore,
	max_block_data_size: Option<u64>,
}

impl<Fetch: Future + Unpin> ParachainWork<Fetch> {
	/// Prime the parachain work with an API reference for extracting
	/// chain information.
	pub fn prime<P: ProvideRuntimeApi<Block>>(self, api: Arc<P>)
		-> PrimedParachainWork<
			Fetch,
			impl Send + FnMut(&BlockId, &PoVBlock, &CandidateReceipt) -> Result<(OutgoingMessages, ErasureChunk), ()> + Unpin,
		>
		where
			P: Send + Sync + 'static,
			P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	{
		let max_block_data_size = self.max_block_data_size;
		let local_index = self.local_index;

		let validate = move |id: &_, pov_block: &_, receipt: &_| {
			let res = crate::collation::validate_receipt(
				&*api,
				id,
				pov_block,
				receipt,
				max_block_data_size,
			);

			match res {
				Ok((messages, mut chunks)) => {
					Ok((messages, chunks.swap_remove(local_index)))
				}
				Err(e) => {
					debug!(target: "validation", "Encountered bad collation: {}", e);
					Err(())
				}
			}
		};

		PrimedParachainWork { inner: self, validate }
	}

	/// Prime the parachain work with a custom validation function.
	pub fn prime_with<F>(self, validate: F) -> PrimedParachainWork<Fetch, F>
		where F: FnMut(&BlockId, &PoVBlock, &CandidateReceipt) -> Result<(OutgoingMessages, ErasureChunk), ()>
	{
		PrimedParachainWork { inner: self, validate }
	}
}

struct Work<Fetch> {
	candidate_receipt: CandidateReceipt,
	fetch: Fetch
}

/// Primed statement producer.
pub struct PrimedParachainWork<Fetch, F> {
	inner: ParachainWork<Fetch>,
	validate: F,
}

impl<Fetch, F, Err> PrimedParachainWork<Fetch, F>
	where
		Fetch: Future<Output=Result<PoVBlock,Err>> + Unpin,
		F: FnMut(&BlockId, &PoVBlock, &CandidateReceipt) -> Result<(OutgoingMessages, ErasureChunk), ()> + Unpin,
		Err: From<::std::io::Error>,
{
	pub async fn validate(mut self) -> Result<(Validated, Option<ErasureChunk>), Err> {
		let candidate = &self.inner.work.candidate_receipt;
		let pov_block = self.inner.work.fetch.await?;

		let validation_res = (self.validate)(
			&BlockId::hash(self.inner.relay_parent),
			&pov_block,
			&candidate,
		);

		let candidate_hash = candidate.hash();

		debug!(target: "validation", "Making validity statement about candidate {}: is_good? {:?}",
			candidate_hash, validation_res.is_ok());

		match validation_res {
			Err(()) => Ok((
				Validated {
					statement: GenericStatement::Invalid(candidate_hash),
					result: Validation::Invalid(pov_block),
				},
				None,
			)),
			Ok((outgoing_targeted, our_chunk)) => {
				self.inner.availability_store.add_erasure_chunk(
					self.inner.relay_parent,
					candidate.clone(),
					our_chunk.clone(),
				).await?;

				Ok((
					Validated {
						statement: GenericStatement::Valid(candidate_hash),
						result: Validation::Valid(pov_block, outgoing_targeted),
					},
					Some(our_chunk),
				))
			}
		}
	}
}

/// A shared table object.
pub struct SharedTable {
	context: Arc<TableContext>,
	inner: Arc<Mutex<SharedTableInner>>,
	max_block_data_size: Option<u64>,
}

impl Clone for SharedTable {
	fn clone(&self) -> Self {
		Self {
			context: self.context.clone(),
			inner: self.inner.clone(),
			max_block_data_size: self.max_block_data_size,
		}
	}
}

impl SharedTable {
	/// Create a new shared table.
	///
	/// Provide the key to sign with, and the parent hash of the relay chain
	/// block being built.
	pub fn new(
		validators: Vec<ValidatorId>,
		groups: HashMap<ParaId, GroupInfo>,
		key: Option<Arc<ValidatorPair>>,
		parent_hash: Hash,
		availability_store: AvailabilityStore,
		max_block_data_size: Option<u64>,
	) -> Self {
		SharedTable {
			context: Arc::new(TableContext { groups, key, parent_hash, validators: validators.clone(), }),
			max_block_data_size,
			inner: Arc::new(Mutex::new(SharedTableInner {
				table: Table::default(),
				validated: HashMap::new(),
				trackers: Vec::new(),
				availability_store,
			}))
		}
	}

	/// Get the parent hash this table should hold statements localized to.
	pub fn consensus_parent_hash(&self) -> &Hash {
		&self.context.parent_hash
	}

	/// Get the local validator session key.
	pub fn session_key(&self) -> Option<ValidatorId> {
		self.context.local_id()
	}

	/// Get group info.
	pub fn group_info(&self) -> &HashMap<ParaId, GroupInfo> {
		&self.context.groups
	}

	/// Import a single statement with remote source, whose signature has already been checked.
	///
	/// Validity and invalidity statements are only valid if the corresponding
	/// candidate has already been imported.
	///
	/// The ParachainWork, if any, will produce only statements concerning the same candidate
	/// as the one just imported
	pub fn import_remote_statement<R: TableRouter>(
		&self,
		router: &R,
		statement: table::SignedStatement,
	) -> Option<ParachainWork<
		R::FetchValidationProof,
	>> {
		self.inner.lock().import_remote_statement(&*self.context, router, statement, self.max_block_data_size)
	}

	/// Import many statements at once.
	///
	/// Provide an iterator yielding remote, pre-checked statements.
	/// Validity and invalidity statements are only valid if the corresponding
	/// candidate has already been imported.
	///
	/// The ParachainWork, if any, will produce only statements concerning the same candidate
	/// as the one just imported
	pub fn import_remote_statements<R, I, U>(&self, router: &R, iterable: I) -> U
		where
			R: TableRouter,
			I: IntoIterator<Item=table::SignedStatement>,
			U: ::std::iter::FromIterator<Option<ParachainWork<
				R::FetchValidationProof,
			>>>,
	{
		let mut inner = self.inner.lock();

		iterable.into_iter().map(move |statement| {
			inner.import_remote_statement(&*self.context, router, statement, self.max_block_data_size)
		}).collect()
	}

	/// Sign and import the result of candidate validation. Returns `None` if the table
	/// was instantiated without a local key.
	pub fn import_validated(&self, validated: Validated)
		-> Option<SignedStatement>
	{
		let digest = match validated.statement {
			GenericStatement::Candidate(ref c) => c.hash(),
			GenericStatement::Valid(h) | GenericStatement::Invalid(h) => h,
		};

		let signed_statement = self.context.sign_statement(validated.statement);

		if let Some(ref signed) = signed_statement {
			let mut inner = self.inner.lock();
			inner.table.import_statement(&*self.context, signed.clone());
			inner.validated.insert(digest, ValidationWork::Done(validated.result));
		}

		signed_statement
	}

	/// Execute a closure using a specific candidate.
	///
	/// Deadlocks if called recursively.
	pub fn with_candidate<F, U>(&self, digest: &Hash, f: F) -> U
		where F: FnOnce(Option<&CandidateReceipt>) -> U
	{
		let inner = self.inner.lock();
		f(inner.table.get_candidate(digest))
	}

	/// Get a set of candidates that can be proposed.
	pub fn proposed_set(&self) -> Vec<AttestedCandidate> {
		use table::generic::{ValidityAttestation as GAttestation};
		use polkadot_primitives::parachain::ValidityAttestation;

		// we transform the types of the attestations gathered from the table
		// into the type expected by the runtime. This may do signature
		// aggregation in the future.
		let table_attestations = self.inner.lock().table.proposed_candidates(&*self.context);
		table_attestations.into_iter()
			.map(|attested| {
				let mut validity_votes: Vec<_> = attested.validity_votes.into_iter().map(|(id, a)| {
					(id as usize, match a {
						GAttestation::Implicit(s) => ValidityAttestation::Implicit(s),
						GAttestation::Explicit(s) => ValidityAttestation::Explicit(s),
					})
				}).collect();
				validity_votes.sort_by(|(id1, _), (id2, _)| id1.cmp(id2));

				let mut validator_indices = bitvec![0; validity_votes.last().map(|(i, _)| i + 1).unwrap_or_default()];
				for (id, _) in &validity_votes {
					validator_indices.set(*id, true);
				}

				AttestedCandidate {
					candidate: attested.candidate,
					validity_votes: validity_votes.into_iter().map(|(_, a)| a).collect(),
					validator_indices,
				}
			}).collect()
	}

	/// Get the number of total parachains.
	pub fn num_parachains(&self) -> usize {
		self.group_info().len()
	}

	/// Get the number of parachains whose candidates may be included.
	pub fn includable_count(&self) -> usize {
		self.inner.lock().table.includable_count()
	}

	/// Get all witnessed misbehavior.
	pub fn get_misbehavior(&self) -> HashMap<ValidatorIndex, table::Misbehavior> {
		self.inner.lock().table.get_misbehavior().clone()
	}

	/// Track includability  of a given set of candidate hashes.
	pub fn track_includability<I>(&self, iterable: I) -> oneshot::Receiver<()>
		where I: IntoIterator<Item=Hash>
	{
		let mut inner = self.inner.lock();

		let (tx, rx) = includable::track(iterable.into_iter().map(|x| {
			let includable = inner.table.candidate_includable(&x, &*self.context);
			(x, includable)
		}));

		if !tx.is_complete() {
			inner.trackers.push(tx);
		}

		rx
	}

	/// Returns id of the validator corresponding to the given index.
	pub fn index_to_id(&self, index: ValidatorIndex) -> Option<ValidatorId> {
		self.context.validators.get(index as usize).cloned()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_keyring::Sr25519Keyring;
	use primitives::crypto::UncheckedInto;
	use polkadot_primitives::parachain::{AvailableMessages, BlockData, ConsolidatedIngress, Collation};
	use polkadot_erasure_coding::{self as erasure};
	use availability_store::ProvideGossipMessages;
	use futures::future;
	use futures::executor::block_on;
	use std::pin::Pin;

	fn pov_block_with_data(data: Vec<u8>) -> PoVBlock {
		PoVBlock {
			block_data: BlockData(data),
			ingress: ConsolidatedIngress(Vec::new()),
		}
	}

	#[derive(Clone)]
	struct DummyGossipMessages;

	impl ProvideGossipMessages for DummyGossipMessages {
		fn gossip_messages_for(
			&self,
			_topic: Hash
		) -> Pin<Box<dyn futures::Stream<Item = (Hash, Hash, ErasureChunk)> + Send>> {
			futures::stream::empty().boxed()
		}

		fn gossip_erasure_chunk(
			&self,
			_relay_parent: Hash,
			_candidate_hash: Hash,
			_erasure_root: Hash,
			_chunk: ErasureChunk,
		) {}
	}

	#[derive(Clone)]
	struct DummyRouter;
	impl TableRouter for DummyRouter {
		type Error = ::std::io::Error;
		type FetchValidationProof = future::Ready<Result<PoVBlock,Self::Error>>;

		fn local_collation(
			&self,
			_collation: Collation,
			_candidate: CandidateReceipt,
			_outgoing: OutgoingMessages,
			_chunks: (ValidatorIndex, &[ErasureChunk])
		) {}

		fn fetch_pov_block(&self, _candidate: &CandidateReceipt) -> Self::FetchValidationProof {
			future::ok(pov_block_with_data(vec![1, 2, 3, 4, 5]))
		}
	}

	#[test]
	fn statement_triggers_fetch_and_evaluate() {
		let mut groups = HashMap::new();

		let para_id = ParaId::from(1);
		let parent_hash = Default::default();

		let local_key = Sr25519Keyring::Alice.pair();
		let local_id: ValidatorId = local_key.public().into();
		let local_key: Arc<ValidatorPair> = Arc::new(local_key.into());

		let validity_other_key = Sr25519Keyring::Bob.pair();
		let validity_other: ValidatorId = validity_other_key.public().into();
		let validity_other_index = 1;

		groups.insert(para_id, GroupInfo {
			validity_guarantors: [local_id.clone(), validity_other.clone()].iter().cloned().collect(),
			needed_validity: 2,
		});

		let shared_table = SharedTable::new(
			[local_id, validity_other].to_vec(),
			groups,
			Some(local_key.clone()),
			parent_hash,
			AvailabilityStore::new_in_memory(DummyGossipMessages),
			None,
		);

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].unchecked_into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [2; 32].into(),
			upward_messages: Vec::new(),
			erasure_root: [1u8; 32].into(),
		};

		let candidate_statement = GenericStatement::Candidate(candidate);

		let signature = crate::sign_table_statement(&candidate_statement, &validity_other_key.into(), &parent_hash);
		let signed_statement = ::table::generic::SignedStatement {
			statement: candidate_statement,
			signature: signature.into(),
			sender: validity_other_index,
		};

		shared_table.import_remote_statement(
			&DummyRouter,
			signed_statement,
		).expect("candidate and local validity group are same");
	}

	#[test]
	fn statement_triggers_fetch_and_validity() {
		let mut groups = HashMap::new();

		let para_id = ParaId::from(1);
		let parent_hash = Default::default();

		let local_key = Sr25519Keyring::Alice.pair();
		let local_id: ValidatorId = local_key.public().into();
		let local_key: Arc<ValidatorPair> = Arc::new(local_key.into());

		let validity_other_key = Sr25519Keyring::Bob.pair();
		let validity_other: ValidatorId = validity_other_key.public().into();
		let validity_other_index = 1;

		groups.insert(para_id, GroupInfo {
			validity_guarantors: [local_id.clone(), validity_other.clone()].iter().cloned().collect(),
			needed_validity: 1,
		});

		let shared_table = SharedTable::new(
			[local_id, validity_other].to_vec(),
			groups,
			Some(local_key.clone()),
			parent_hash,
			AvailabilityStore::new_in_memory(DummyGossipMessages),
			None,
		);

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].unchecked_into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [2; 32].into(),
			upward_messages: Vec::new(),
			erasure_root: [1u8; 32].into(),
		};

		let candidate_statement = GenericStatement::Candidate(candidate);

		let signature = crate::sign_table_statement(&candidate_statement, &validity_other_key.into(), &parent_hash);
		let signed_statement = ::table::generic::SignedStatement {
			statement: candidate_statement,
			signature: signature.into(),
			sender: validity_other_index,
		};

		shared_table.import_remote_statement(
			&DummyRouter,
			signed_statement,
		).expect("should produce work");
	}

	#[test]
	fn evaluate_makes_block_data_available() {
		let store = AvailabilityStore::new_in_memory(DummyGossipMessages);
		let relay_parent = [0; 32].into();
		let para_id = 5.into();
		let pov_block = pov_block_with_data(vec![1, 2, 3]);
		let block_data_hash = [2; 32].into();
		let local_index = 0;
		let n_validators = 2;

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].unchecked_into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash,
			upward_messages: Vec::new(),
			erasure_root: [1u8; 32].into(),
		};

		let hash = candidate.hash();

		store.add_validator_index_and_n_validators(
			&relay_parent,
			local_index as u32,
			n_validators as u32,
		).unwrap();

		let producer: ParachainWork<future::Ready<Result<_, ::std::io::Error>>> = ParachainWork {
			work: Work {
				candidate_receipt: candidate,
				fetch: future::ok(pov_block.clone()),
			},
			local_index,
			relay_parent,
			availability_store: store.clone(),
			max_block_data_size: None,
		};

		let validated = block_on(producer.prime_with(|_, _, _| Ok((
				OutgoingMessages { outgoing_messages: Vec::new() },
				ErasureChunk {
					chunk: vec![1, 2, 3],
					index: local_index as u32,
					proof: vec![],
				},
			))).validate()).unwrap();

		assert_eq!(validated.0.pov_block(), &pov_block);
		assert_eq!(validated.0.statement, GenericStatement::Valid(hash));

		if let Some(messages) = validated.0.outgoing_messages() {
			let available_messages: AvailableMessages = messages.clone().into();
			for (root, queue) in available_messages.0 {
				assert_eq!(store.queue_by_root(&root), Some(queue));
			}
		}
		assert!(store.get_erasure_chunk(&relay_parent, block_data_hash, local_index).is_some());
		assert!(store.get_erasure_chunk(&relay_parent, block_data_hash, local_index + 1).is_none());
	}

	#[test]
	fn full_availability() {
		let store = AvailabilityStore::new_in_memory(DummyGossipMessages);
		let relay_parent = [0; 32].into();
		let para_id = 5.into();
		let pov_block = pov_block_with_data(vec![1, 2, 3]);
		let block_data_hash = pov_block.block_data.hash();
		let local_index = 0;
		let n_validators = 2;
		let ex = Some(AvailableMessages(Vec::new()));

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].unchecked_into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [2; 32].into(),
			upward_messages: Vec::new(),
			erasure_root: [1u8; 32].into(),
		};

		let chunks = erasure::obtain_chunks(n_validators, &pov_block.block_data, ex.as_ref()).unwrap();

		store.add_validator_index_and_n_validators(
			&relay_parent,
			local_index as u32,
			n_validators as u32,
		).unwrap();

		let producer = ParachainWork {
			work: Work {
				candidate_receipt: candidate,
				fetch: future::ok::<_, ::std::io::Error>(pov_block.clone()),
			},
			local_index,
			relay_parent,
			availability_store: store.clone(),
			max_block_data_size: None,
		};

		let validated = block_on(producer.prime_with(|_, _, _| Ok((
				OutgoingMessages { outgoing_messages: Vec::new() },
				ErasureChunk {
					chunk: chunks[local_index].clone(),
					index: local_index as u32,
					proof: vec![],
				},
			))).validate()).unwrap();

		assert_eq!(validated.0.pov_block(), &pov_block);

		if let Some(messages) = validated.0.outgoing_messages() {
			let available_messages: AvailableMessages = messages.clone().into();
			for (root, queue) in available_messages.0 {
				assert_eq!(store.queue_by_root(&root), Some(queue));
			}
		}
		// This works since there are only two validators and one erasure chunk should be
		// enough to reconstruct the block data.
		assert_eq!(store.block_data(relay_parent, block_data_hash).unwrap(), pov_block.block_data);
		// TODO: check that a message queue is included by root.
	}

	#[test]
	fn does_not_dispatch_work_after_starting_validation() {
		let mut groups = HashMap::new();

		let para_id = ParaId::from(1);
		let parent_hash = Default::default();

		let local_key = Sr25519Keyring::Alice.pair();
		let local_id: ValidatorId = local_key.public().into();
		let local_key: Arc<ValidatorPair> = Arc::new(local_key.into());

		let validity_other_key = Sr25519Keyring::Bob.pair();
		let validity_other: ValidatorId = validity_other_key.public().into();
		let validity_other_index = 1;

		groups.insert(para_id, GroupInfo {
			validity_guarantors: [local_id.clone(), validity_other.clone()].iter().cloned().collect(),
			needed_validity: 1,
		});

		let shared_table = SharedTable::new(
			[local_id, validity_other].to_vec(),
			groups,
			Some(local_key.clone()),
			parent_hash,
			AvailabilityStore::new_in_memory(DummyGossipMessages),
			None,
		);

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].unchecked_into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [2; 32].into(),
			upward_messages: Vec::new(),
			erasure_root: [1u8; 32].into(),
		};

		let hash = candidate.hash();
		let candidate_statement = GenericStatement::Candidate(candidate);

		let signature = crate::sign_table_statement(&candidate_statement, &validity_other_key.into(), &parent_hash);
		let signed_statement = ::table::generic::SignedStatement {
			statement: candidate_statement,
			signature: signature.into(),
			sender: validity_other_index,
		};

		let _a = shared_table.import_remote_statement(
			&DummyRouter,
			signed_statement.clone(),
		).expect("should produce work");

		assert!(shared_table.inner.lock().validated.get(&hash).expect("validation has started").is_in_progress());

		let b = shared_table.import_remote_statement(
			&DummyRouter,
			signed_statement.clone(),
		);

		assert!(b.is_none(), "cannot work when validation has started");
	}

	#[test]
	fn does_not_dispatch_after_local_candidate() {
		let mut groups = HashMap::new();

		let para_id = ParaId::from(1);
		let pov_block = pov_block_with_data(vec![1, 2, 3]);
		let outgoing_messages = OutgoingMessages { outgoing_messages: Vec::new() };
		let parent_hash = Default::default();

		let local_key = Sr25519Keyring::Alice.pair();
		let local_id: ValidatorId = local_key.public().into();
		let local_key: Arc<ValidatorPair> = Arc::new(local_key.into());

		let validity_other_key = Sr25519Keyring::Bob.pair();
		let validity_other: ValidatorId = validity_other_key.public().into();

		groups.insert(para_id, GroupInfo {
			validity_guarantors: [local_id.clone(), validity_other.clone()].iter().cloned().collect(),
			needed_validity: 1,
		});

		let shared_table = SharedTable::new(
			[local_id, validity_other].to_vec(),
			groups,
			Some(local_key.clone()),
			parent_hash,
			AvailabilityStore::new_in_memory(DummyGossipMessages),
			None,
		);

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].unchecked_into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [2; 32].into(),
			upward_messages: Vec::new(),
			erasure_root: [1u8; 32].into(),
		};

		let hash = candidate.hash();
		let signed_statement = shared_table.import_validated(Validated::collated_local(
			candidate,
			pov_block,
			outgoing_messages,
		)).unwrap();

		assert!(shared_table.inner.lock().validated.get(&hash).expect("validation has started").is_done());

		let a = shared_table.import_remote_statement(
			&DummyRouter,
			signed_statement,
		);

		assert!(a.is_none());
	}
}
