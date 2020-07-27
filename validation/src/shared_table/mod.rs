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
use table::{v0 as table_v0, Table, Context as TableContextTrait};
use polkadot_primitives::v0::{
	Block, Hash,
	Id as ParaId, AbridgedCandidateReceipt, ValidatorPair, ValidatorId,
	AttestedCandidate, ParachainHost, PoVBlock, ValidatorIndex, SigningContext,
	ValidatorSignature,
};

use parking_lot::Mutex;
use futures::prelude::*;
use futures::channel::oneshot;
use log::{warn, debug};
use bitvec::bitvec;

use super::GroupInfo;
use self::includable::IncludabilitySender;
use primitives::Pair;
use sp_api::ProvideRuntimeApi;

use crate::pipeline::{FullOutput, ValidationPool};
use crate::Error;

mod includable;

pub use table_v0::{SignedStatement, Statement};
pub use table::generic::Statement as GenericStatement;

struct TableContext {
	signing_context: SigningContext,
	key: Option<Arc<ValidatorPair>>,
	groups: HashMap<ParaId, GroupInfo>,
	validators: Vec<ValidatorId>,
}

impl TableContextTrait for TableContext {
	type AuthorityId = ValidatorIndex;
	type Digest = Hash;
	type GroupId = ParaId;
	type Signature = ValidatorSignature;
	type Candidate = AbridgedCandidateReceipt;

	fn candidate_digest(candidate: &AbridgedCandidateReceipt) -> Hash {
		candidate.hash()
	}

	fn candidate_group(candidate: &AbridgedCandidateReceipt) -> ParaId {
		candidate.parachain_index
	}

	fn is_member_of(&self, authority: &ValidatorIndex, group: &ParaId) -> bool {
		let key = match self.validators.get(*authority as usize) {
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

	fn sign_statement(&self, statement: table_v0::Statement) -> Option<table_v0::SignedStatement> {
		self.local_index().and_then(move |sender|
			self.key.as_ref()
				.map(|key| crate::sign_table_statement(
						&statement,
						key,
						&self.signing_context,
					).into()
				)
				.map(move |signature| table_v0::SignedStatement { statement, signature, sender })
		)
	}
}

pub(crate) enum Validation {
	Valid(PoVBlock),
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
	validation_pool: Option<ValidationPool>,
}

impl SharedTableInner {
	// Import a single statement. Provide a handle to a table router and a function
	// used to determine if a referenced candidate is valid.
	//
	// the statement producer, if any, will produce only statements concerning the same candidate
	// as the one just imported
	fn import_remote_statement<Fetch>(
		&mut self,
		context: &TableContext,
		fetch_pov_block: impl Fn(&AbridgedCandidateReceipt) -> Fetch,
		statement: table_v0::SignedStatement,
		max_block_data_size: Option<u64>,
	) -> Option<ParachainWork<
		Fetch,
	>> {
		let summary = self.table.import_statement(context, statement)?;
		self.update_trackers(&summary.candidate, context);

		let local_index = context.local_index()?;
		let para_member = context.is_member_of(&local_index, &summary.group_id);
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
					let fetch = fetch_pov_block(candidate);

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
			validation_pool: self.validation_pool.clone(),
			availability_store: self.availability_store.clone(),
			relay_parent: context.signing_context.parent_hash.clone(),
			work,
			max_block_data_size,
			n_validators: context.validators.len(),
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
	statement: table_v0::Statement,
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
	pub fn known_good(hash: Hash, collation: PoVBlock) -> Self {
		Validated {
			statement: GenericStatement::Valid(hash),
			result: Validation::Valid(collation),
		}
	}

	/// Note that we've collated a candidate.
	/// outgoing message required.
	pub fn collated_local(
		receipt: AbridgedCandidateReceipt,
		collation: PoVBlock,
	) -> Self {
		Validated {
			statement: GenericStatement::Candidate(receipt),
			result: Validation::Valid(collation),
		}
	}

	/// Get a reference to the proof-of-validation block.
	pub fn pov_block(&self) -> &PoVBlock {
		match self.result {
			Validation::Valid(ref b) | Validation::Invalid(ref b) => b,
		}
	}
}

/// Future that performs parachain validation work.
pub struct ParachainWork<Fetch> {
	validation_pool: Option<ValidationPool>,
	work: Work<Fetch>,
	relay_parent: Hash,
	availability_store: AvailabilityStore,
	max_block_data_size: Option<u64>,
	n_validators: usize,
}

impl<Fetch: Future + Unpin> ParachainWork<Fetch> {
	/// Prime the parachain work with an API reference for extracting
	/// chain information.
	pub fn prime<P: ProvideRuntimeApi<Block>>(self, api: Arc<P>)
		-> PrimedParachainWork<
			Fetch,
			impl Send + FnMut(&PoVBlock, &AbridgedCandidateReceipt)
				-> Result<FullOutput, Error> + Unpin,
		>
		where
			P: Send + Sync + 'static,
			P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	{
		let max_block_data_size = self.max_block_data_size;
		let n_validators = self.n_validators;
		let expected_relay_parent = self.relay_parent;

		let pool = self.validation_pool.clone();
		let validate = move |pov_block: &PoVBlock, candidate: &AbridgedCandidateReceipt| {
			let collation_info = candidate.to_collation_info();
			let full_output = crate::pipeline::full_output_validation_with_api(
				pool.as_ref(),
				&*api,
				&collation_info,
				pov_block,
				&expected_relay_parent,
				max_block_data_size,
				n_validators,
				primitives::testing::TaskExecutor::new(),
			)?;

			full_output.check_consistency(candidate)?;
			Ok(full_output)
		};

		PrimedParachainWork { inner: self, validate }
	}

	/// Prime the parachain work with a custom validation function.
	#[cfg(test)]
	pub fn prime_with<F>(self, validate: F) -> PrimedParachainWork<Fetch, F>
		where F: FnMut(&PoVBlock, &AbridgedCandidateReceipt)
			-> Result<FullOutput, Error>
	{
		PrimedParachainWork { inner: self, validate }
	}
}

struct Work<Fetch> {
	candidate_receipt: AbridgedCandidateReceipt,
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
		F: FnMut(&PoVBlock, &AbridgedCandidateReceipt) -> Result<FullOutput, Error> + Unpin,
		Err: From<std::io::Error>,
{
	pub async fn validate(self) -> Result<Validated, Err> {
		let candidate = self.inner.work.candidate_receipt;
		let pov_block = self.inner.work.fetch.await?;

		let mut validate = self.validate;
		let relay_parent = self.inner.relay_parent;

		// create a wrapper around the custom validation function that does
		// some more general pre and post checks.
		let mut validate = move |pov_block: &_, candidate: &AbridgedCandidateReceipt| {
			if candidate.relay_parent != relay_parent {
				return Err(Error::DisallowedRelayParent(candidate.relay_parent));
			}

			let full_output = validate(pov_block, candidate)?;

			if full_output.commitments != candidate.commitments {
				return Err(Error::CommitmentsMismatch);
			}

			Ok(full_output)
		};

		let validation_res = (validate)(&pov_block, &candidate);
		let candidate_hash = candidate.hash();

		debug!(target: "validation", "Making validity statement about candidate {}: is_good? {:?}",
			candidate_hash, validation_res.is_ok());

		match validation_res {
			Err(err) => {
				debug!(target: "validation", "candidate is invalid: {}", err);
				Ok(Validated {
					statement: GenericStatement::Invalid(candidate_hash),
					result: Validation::Invalid(pov_block),
				})
			}
			Ok(full_output) => {
				// make data and all erasure chunks available. The chunk
				// must be fully available before we add the chunks.
				self.inner.availability_store.make_available(
					candidate_hash,
					full_output.available_data,
				).await?;
				self.inner.availability_store.add_erasure_chunks(
					candidate,
					full_output.n_validators as _,
					full_output.erasure_chunks,
				).await?;

				Ok(Validated {
					statement: GenericStatement::Valid(candidate_hash),
					result: Validation::Valid(pov_block),
				})
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
		signing_context: SigningContext,
		availability_store: AvailabilityStore,
		max_block_data_size: Option<u64>,
		validation_pool: Option<ValidationPool>,
	) -> Self {
		SharedTable {
			context: Arc::new(TableContext { groups, key, signing_context, validators: validators.clone(), }),
			max_block_data_size,
			inner: Arc::new(Mutex::new(SharedTableInner {
				table: Table::default(),
				validated: HashMap::new(),
				trackers: Vec::new(),
				availability_store,
				validation_pool,
			}))
		}
	}

	/// Get the parent hash this table should hold statements localized to.
	pub fn signing_context(&self) -> &SigningContext {
		&self.context.signing_context
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
	pub fn import_remote_statement<Fetch>(
		&self,
		fetch_pov_block: impl Fn(&AbridgedCandidateReceipt) -> Fetch,
		statement: table_v0::SignedStatement,
	) -> Option<ParachainWork<
		Fetch,
	>> {
		self.inner.lock().import_remote_statement(
			&*self.context,
			fetch_pov_block,
			statement,
			self.max_block_data_size,
		)
	}

	/// Import many statements at once.
	///
	/// Provide an iterator yielding remote, pre-checked statements.
	/// Validity and invalidity statements are only valid if the corresponding
	/// candidate has already been imported.
	///
	/// The ParachainWork, if any, will produce only statements concerning the same candidate
	/// as the one just imported
	pub fn import_remote_statements<Fetch, I, U>(
		&self,
		fetch_pov_block: impl Fn(&AbridgedCandidateReceipt) -> Fetch,
		iterable: I,
	) -> U
		where
			I: IntoIterator<Item=table_v0::SignedStatement>,
			U: ::std::iter::FromIterator<Option<ParachainWork<
				Fetch,
			>>>,
	{
		let mut inner = self.inner.lock();

		iterable.into_iter().map(move |statement| {
			inner.import_remote_statement(
				&*self.context,
				&fetch_pov_block,
				statement,
				self.max_block_data_size,
			)
		}).collect()
	}

	/// Sign and import the result of candidate validation. Returns `None` if the table
	/// was instantiated without a local key. Otherwise, returns a copy of the signed
	/// statement.
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
		where F: FnOnce(Option<&AbridgedCandidateReceipt>) -> U
	{
		let inner = self.inner.lock();
		f(inner.table.get_candidate(digest))
	}

	/// Get a set of candidates that can be proposed.
	pub fn proposed_set(&self) -> Vec<AttestedCandidate> {
		use table::generic::{ValidityAttestation as GAttestation};
		use polkadot_primitives::v0::ValidityAttestation;

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

				let mut validator_indices = bitvec![
					bitvec::order::Lsb0, u8;
					0;
					validity_votes.last().map(|(i, _)| i + 1).unwrap_or_default()
				];
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
	pub fn get_misbehavior(&self) -> HashMap<ValidatorIndex, table_v0::Misbehavior> {
		self.inner.lock().table.get_misbehavior().clone()
	}

	/// Track includability of a given set of candidate hashes.
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
	use polkadot_primitives::v0::{
		BlockData, ErasureChunk, AvailableData,
	};
	use polkadot_erasure_coding::{self as erasure};
	use availability_store::ErasureNetworking;
	use futures::future;
	use futures::executor::block_on;
	use std::pin::Pin;

	fn pov_block_with_data(data: Vec<u8>) -> PoVBlock {
		PoVBlock {
			block_data: BlockData(data),
		}
	}

	#[derive(Clone)]
	struct DummyErasureNetworking;

	impl ErasureNetworking for DummyErasureNetworking {
		type Error = String;

		fn fetch_erasure_chunk(
			&self,
			_candidate_hash: &Hash,
			_index: u32,
		) -> Pin<Box<dyn Future<Output = Result<ErasureChunk, Self::Error>> + Send>> {
			future::pending().boxed()
		}

		fn distribute_erasure_chunk(
			&self,
			_candidate_hash: Hash,
			_chunk: ErasureChunk,
		) {}
	}

	fn lazy_fetch_pov()
		-> Box<
			dyn Fn(&AbridgedCandidateReceipt) -> future::Ready<
				Result<PoVBlock, std::io::Error>
			>
		>
	{
		Box::new(|_| future::ok(pov_block_with_data(vec![1, 2, 3, 4, 5])))
	}

	#[test]
	fn statement_triggers_fetch_and_evaluate() {
		let mut groups = HashMap::new();

		let para_id = ParaId::from(1);
		let parent_hash = Hash::default();
		let signing_context = SigningContext {
			session_index: Default::default(),
			parent_hash: parent_hash.clone(),
		};

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
			signing_context.clone(),
			AvailabilityStore::new_in_memory(DummyErasureNetworking),
			None,
			None,
		);

		let mut candidate = AbridgedCandidateReceipt::default();
		candidate.parachain_index = para_id;
		candidate.relay_parent = parent_hash;

		let candidate_statement = GenericStatement::Candidate(candidate);

		let signature = crate::sign_table_statement(
			&candidate_statement,
			&validity_other_key.into(),
			&signing_context,
		);
		let signed_statement = table::generic::SignedStatement {
			statement: candidate_statement,
			signature: signature.into(),
			sender: validity_other_index,
		};

		shared_table.import_remote_statement(
			lazy_fetch_pov(),
			signed_statement,
		).expect("candidate and local validity group are same");
	}

	#[test]
	fn statement_triggers_fetch_and_validity() {
		let mut groups = HashMap::new();

		let para_id = ParaId::from(1);
		let parent_hash = Hash::default();
		let signing_context = SigningContext {
			session_index: Default::default(),
			parent_hash: parent_hash.clone(),
		};

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
			signing_context.clone(),
			AvailabilityStore::new_in_memory(DummyErasureNetworking),
			None,
			None,
		);

		let mut candidate = AbridgedCandidateReceipt::default();
		candidate.parachain_index = para_id;
		candidate.relay_parent = parent_hash;

		let candidate_statement = GenericStatement::Candidate(candidate);

		let signature = crate::sign_table_statement(
			&candidate_statement,
			&validity_other_key.into(),
			&signing_context,
		);
		let signed_statement = table::generic::SignedStatement {
			statement: candidate_statement,
			signature: signature.into(),
			sender: validity_other_index,
		};

		shared_table.import_remote_statement(
			lazy_fetch_pov(),
			signed_statement,
		).expect("should produce work");
	}

	#[test]
	fn evaluate_makes_block_data_available() {
		let store = AvailabilityStore::new_in_memory(DummyErasureNetworking);
		let relay_parent = [0; 32].into();
		let para_id = 5.into();
		let pov_block = pov_block_with_data(vec![1, 2, 3]);
		let pov_block_hash = pov_block.hash();
		let local_index = 0;
		let n_validators = 2;

		let mut candidate = AbridgedCandidateReceipt::default();
		candidate.parachain_index = para_id;
		candidate.relay_parent = relay_parent;
		candidate.pov_block_hash = pov_block_hash;

		let candidate_hash = candidate.hash();

		store.note_validator_index_and_n_validators(
			&relay_parent,
			local_index as u32,
			n_validators as u32,
		).unwrap();

		let producer: ParachainWork<future::Ready<Result<_, ::std::io::Error>>> = ParachainWork {
			work: Work {
				candidate_receipt: candidate,
				fetch: future::ok(pov_block.clone()),
			},
			relay_parent,
			availability_store: store.clone(),
			max_block_data_size: None,
			n_validators,
			validation_pool: None,
		};

		for i in 0..n_validators {
			assert!(store.get_erasure_chunk(&candidate_hash, i).is_none());
		}

		let validated = block_on(producer.prime_with(|_, _| Ok(
			FullOutput {
				available_data: AvailableData {
					pov_block: pov_block.clone(),
					omitted_validation: Default::default(),
				},
				erasure_chunks: (0..n_validators).map(|i| ErasureChunk {
					chunk: vec![1, 2, 3],
					index: i as u32,
					proof: vec![],
				}).collect(),
				commitments: Default::default(),
				n_validators,
			}
		)).validate()).unwrap();

		assert_eq!(validated.pov_block(), &pov_block);
		assert_eq!(validated.statement, GenericStatement::Valid(candidate_hash));

		for i in 0..n_validators {
			assert!(store.get_erasure_chunk(&candidate_hash, i).is_some());
		}
	}

	#[test]
	fn full_availability() {
		let store = AvailabilityStore::new_in_memory(DummyErasureNetworking);
		let relay_parent = [0; 32].into();
		let para_id = 5.into();
		let pov_block = pov_block_with_data(vec![1, 2, 3]);
		let pov_block_hash = pov_block.hash();
		let local_index = 0;
		let n_validators = 2;

		let mut candidate = AbridgedCandidateReceipt::default();
		candidate.parachain_index = para_id;
		candidate.pov_block_hash = pov_block_hash;
		candidate.relay_parent = relay_parent;

		let candidate_hash = candidate.hash();

		let available_data = AvailableData {
			pov_block: pov_block.clone(),
			omitted_validation: Default::default(),
		};

		let chunks = erasure::obtain_chunks_v0(n_validators, &available_data).unwrap();

		store.note_validator_index_and_n_validators(
			&relay_parent,
			local_index as u32,
			n_validators as u32,
		).unwrap();

		let producer = ParachainWork {
			work: Work {
				candidate_receipt: candidate,
				fetch: future::ok::<_, ::std::io::Error>(pov_block.clone()),
			},
			relay_parent,
			availability_store: store.clone(),
			max_block_data_size: None,
			n_validators,
			validation_pool: None,
		};

		let validated = block_on(producer.prime_with(|_, _| Ok(
			FullOutput {
				available_data: AvailableData {
					pov_block: pov_block.clone(),
					omitted_validation: Default::default(),
				},
				erasure_chunks: (0..n_validators).map(|i| ErasureChunk {
					chunk: chunks[i].clone(),
					index: i as u32,
					proof: vec![],
				}).collect(),
				commitments: Default::default(),
				n_validators,
			}
		)).validate()).unwrap();

		assert_eq!(validated.pov_block(), &pov_block);

		assert_eq!(store.execution_data(&candidate_hash).unwrap().pov_block, pov_block);
	}

	#[test]
	fn does_not_dispatch_work_after_starting_validation() {
		let mut groups = HashMap::new();

		let para_id = ParaId::from(1);
		let parent_hash = Hash::default();
		let signing_context = SigningContext {
			session_index: Default::default(),
			parent_hash: parent_hash.clone(),
		};

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
			signing_context.clone(),
			AvailabilityStore::new_in_memory(DummyErasureNetworking),
			None,
			None,
		);

		let mut candidate = AbridgedCandidateReceipt::default();
		candidate.parachain_index = para_id;
		candidate.relay_parent = parent_hash;

		let candidate_hash = candidate.hash();
		let candidate_statement = GenericStatement::Candidate(candidate);

		let signature = crate::sign_table_statement(
			&candidate_statement,
			&validity_other_key.into(),
			&signing_context,
		);
		let signed_statement = table::generic::SignedStatement {
			statement: candidate_statement,
			signature: signature.into(),
			sender: validity_other_index,
		};

		let _a = shared_table.import_remote_statement(
			lazy_fetch_pov(),
			signed_statement.clone(),
		).expect("should produce work");

		assert!(shared_table.inner.lock().validated.get(&candidate_hash)
			.expect("validation has started").is_in_progress());

		let b = shared_table.import_remote_statement(
			lazy_fetch_pov(),
			signed_statement.clone(),
		);

		assert!(b.is_none(), "cannot work when validation has started");
	}

	#[test]
	fn does_not_dispatch_after_local_candidate() {
		let mut groups = HashMap::new();

		let para_id = ParaId::from(1);
		let pov_block = pov_block_with_data(vec![1, 2, 3]);
		let parent_hash = Hash::default();
		let signing_context = SigningContext {
			session_index: Default::default(),
			parent_hash: parent_hash.clone(),
		};

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
			signing_context.clone(),
			AvailabilityStore::new_in_memory(DummyErasureNetworking),
			None,
			None,
		);

		let mut candidate = AbridgedCandidateReceipt::default();
		candidate.parachain_index = para_id;
		candidate.relay_parent = parent_hash;

		let candidate_hash = candidate.hash();
		let signed_statement = shared_table.import_validated(Validated::collated_local(
			candidate,
			pov_block,
		)).unwrap();

		assert!(shared_table.inner.lock().validated.get(&candidate_hash)
			.expect("validation has started").is_done());

		let a = shared_table.import_remote_statement(
			lazy_fetch_pov(),
			signed_statement,
		);

		assert!(a.is_none());
	}
}
