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

//! Parachain statement table meant to be shared with a message router
//! and a consensus proposer.

use std::collections::hash_map::{HashMap, Entry};
use std::sync::Arc;

use extrinsic_store::{Data, Store as ExtrinsicStore};
use table::{self, Table, Context as TableContextTrait};
use polkadot_primitives::{Block, BlockId, Hash, SessionKey};
use polkadot_primitives::parachain::{
	Id as ParaId, BlockData, Collation, Extrinsic, CandidateReceipt,
	AttestedCandidate, ParachainHost
};

use parking_lot::Mutex;
use futures::{future, prelude::*};

use super::{GroupInfo, Incoming, TableRouter};
use self::includable::IncludabilitySender;
use primitives::ed25519;
use runtime_primitives::{traits::ProvideRuntimeApi};

mod includable;

pub use self::includable::Includable;
pub use table::{SignedStatement, Statement};
pub use table::generic::Statement as GenericStatement;

struct TableContext {
	parent_hash: Hash,
	key: Arc<ed25519::Pair>,
	groups: HashMap<ParaId, GroupInfo>,
}

impl table::Context for TableContext {
	fn is_member_of(&self, authority: &SessionKey, group: &ParaId) -> bool {
		self.groups.get(group).map_or(false, |g| g.validity_guarantors.contains(authority))
	}

	fn requisite_votes(&self, group: &ParaId) -> usize {
		self.groups.get(group).map_or(usize::max_value(), |g| g.needed_validity)
	}
}

impl TableContext {
	fn local_id(&self) -> SessionKey {
		self.key.public().into()
	}

	fn sign_statement(&self, statement: table::Statement) -> table::SignedStatement {
		let signature = ::sign_table_statement(&statement, &self.key, &self.parent_hash).into();

		table::SignedStatement {
			statement,
			signature,
			sender: self.local_id(),
		}
	}
}

pub(crate) enum Validation {
	Valid(BlockData, Extrinsic),
	Invalid(BlockData), // should take proof.
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
	extrinsic_store: ExtrinsicStore,
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
	) -> Option<ParachainWork<future::Join<
		<R::FetchCandidate as IntoFuture>::Future,
		<R::FetchIncoming as IntoFuture>::Future,
	>>> {
		let summary = match self.table.import_statement(context, statement) {
			Some(summary) => summary,
			None => return None,
		};

		self.update_trackers(&summary.candidate, context);

		let local_id = context.local_id();

		let para_member = context.is_member_of(&local_id, &summary.group_id);

		let digest = &summary.candidate;

		// TODO: consider a strategy based on the number of candidate votes as well.
		let do_validation = para_member && match self.validated.entry(digest.clone()) {
			Entry::Occupied(_) => false,
			Entry::Vacant(entry) => {
				entry.insert(ValidationWork::InProgress);
				true
			}
		};

		let work = if do_validation {
			let fetch_incoming = router.fetch_incoming(summary.group_id);
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
					let fetch_block_data = router.fetch_block_data(candidate).into_future();

					Some(Work {
						candidate_receipt: candidate.clone(),
						fetch: fetch_block_data.join(fetch_incoming),
					})
				}
			}
		} else {
			None
		};

		work.map(|work| ParachainWork {
			extrinsic_store: self.extrinsic_store.clone(),
			relay_parent: context.parent_hash.clone(),
			work
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
	pub fn known_bad(hash: Hash, block_data: BlockData) -> Self {
		Validated {
			statement: GenericStatement::Invalid(hash),
			result: Validation::Invalid(block_data),
		}
	}

	/// Note that we've validated a candidate with given hash and it is good.
	/// Extrinsic data required.
	pub fn known_good(hash: Hash, block_data: BlockData, extrinsic: Extrinsic) -> Self {
		Validated {
			statement: GenericStatement::Valid(hash),
			result: Validation::Valid(block_data, extrinsic),
		}
	}

	/// Note that we've collated a candidate.
	/// Extrinsic data required.
	pub fn collated_local(
		receipt: CandidateReceipt,
		block_data: BlockData,
		extrinsic: Extrinsic,
	) -> Self {
		Validated {
			statement: GenericStatement::Candidate(receipt),
			result: Validation::Valid(block_data, extrinsic),
		}
	}

	/// Get a reference to the block data.
	pub fn block_data(&self) -> &BlockData {
		match self.result {
			Validation::Valid(ref b, _) | Validation::Invalid(ref b) => b,
		}
	}

	/// Get a reference to the extrinsic data, if any.
	pub fn extrinsic(&self) -> Option<&Extrinsic> {
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
	extrinsic_store: ExtrinsicStore,
}

impl<Fetch: Future> ParachainWork<Fetch> {
	/// Prime the parachain work with an API reference for extracting
	/// chain information.
	pub fn prime<P: ProvideRuntimeApi>(self, api: Arc<P>)
		-> PrimedParachainWork<
			Fetch,
			impl Send + FnMut(&BlockId, &Collation, &Incoming) -> Result<Extrinsic, ()>,
		>
		where
			P: Send + Sync + 'static,
			P::Api: ParachainHost<Block>,
	{
		let validate = move |id: &_, collation: &_, incoming: &_| {
			let res = ::collation::validate_collation(
				&*api,
				id,
				collation,
				incoming,
			);

			match res {
				Ok(e) => Ok(e),
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
		where F: FnMut(&BlockId, &Collation, &Incoming) -> Result<Extrinsic, ()>
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

impl<Fetch, F, Err> Future for PrimedParachainWork<Fetch, F>
	where
		Fetch: Future<Item=(BlockData, Incoming),Error=Err>,
		F: FnMut(&BlockId, &Collation, &Incoming) -> Result<Extrinsic, ()>,
		Err: From<::std::io::Error>,
{
	type Item = Validated;
	type Error = Err;

	fn poll(&mut self) -> Poll<Validated, Err> {
		let work = &mut self.inner.work;
		let candidate = &work.candidate_receipt;

		let (block, incoming) = try_ready!(work.fetch.poll());
		let validation_res = (self.validate)(
			&BlockId::hash(self.inner.relay_parent),
			&Collation { block_data: block.clone(), receipt: candidate.clone() },
			&incoming,
		);

		let candidate_hash = candidate.hash();

		debug!(target: "validation", "Making validity statement about candidate {}: is_good? {:?}",
			candidate_hash, validation_res.is_ok());

		let (validity_statement, result) = match validation_res {
			Err(()) => (
				GenericStatement::Invalid(candidate_hash),
				Validation::Invalid(block),
			),
			Ok(extrinsic) => {
				self.inner.extrinsic_store.make_available(Data {
					relay_parent: self.inner.relay_parent,
					parachain_id: work.candidate_receipt.parachain_index,
					candidate_hash,
					block_data: block.clone(),
					extrinsic: Some(extrinsic.clone()),
				})?;

				(
					GenericStatement::Valid(candidate_hash),
					Validation::Valid(block, extrinsic)
				)
			}
		};

		Ok(Async::Ready(Validated {
			statement: validity_statement,
			result,
		}))
	}
}

/// A shared table object.
pub struct SharedTable {
	context: Arc<TableContext>,
	inner: Arc<Mutex<SharedTableInner>>,
}

impl Clone for SharedTable {
	fn clone(&self) -> Self {
		SharedTable {
			context: self.context.clone(),
			inner: self.inner.clone(),
		}
	}
}

impl SharedTable {
	/// Create a new shared table.
	///
	/// Provide the key to sign with, and the parent hash of the relay chain
	/// block being built.
	pub fn new(
		groups: HashMap<ParaId, GroupInfo>,
		key: Arc<ed25519::Pair>,
		parent_hash: Hash,
		extrinsic_store: ExtrinsicStore,
	) -> Self {
		SharedTable {
			context: Arc::new(TableContext { groups, key, parent_hash }),
			inner: Arc::new(Mutex::new(SharedTableInner {
				table: Table::default(),
				validated: HashMap::new(),
				trackers: Vec::new(),
				extrinsic_store,
			}))
		}
	}

	/// Get the parent hash this table should hold statements localized to.
	pub fn consensus_parent_hash(&self) -> &Hash {
		&self.context.parent_hash
	}

	/// Get the local validator session key.
	pub fn session_key(&self) -> SessionKey {
		self.context.local_id()
	}

	/// Get group info.
	pub fn group_info(&self) -> &HashMap<ParaId, GroupInfo> {
		&self.context.groups
	}

	/// Get extrinsic data for candidate with given hash, if any.
	///
	/// This will return `Some` for any candidates that have been validated
	/// locally.
	pub(crate) fn extrinsic_data(&self, hash: &Hash) -> Option<Extrinsic> {
		self.inner.lock().validated.get(hash).and_then(|x| match *x {
			ValidationWork::Error(_) => None,
			ValidationWork::InProgress => None,
			ValidationWork::Done(Validation::Invalid(_)) => None,
			ValidationWork::Done(Validation::Valid(_, ref ex)) => Some(ex.clone()),
		})
	}

	/// Import a single statement with remote source, whose signature has already been checked.
	///
	/// The statement producer, if any, will produce only statements concerning the same candidate
	/// as the one just imported
	pub fn import_remote_statement<R: TableRouter>(
		&self,
		router: &R,
		statement: table::SignedStatement,
	) -> Option<ParachainWork<future::Join<
		<R::FetchCandidate as IntoFuture>::Future,
		<R::FetchIncoming as IntoFuture>::Future,
	>>> {
		self.inner.lock().import_remote_statement(&*self.context, router, statement)
	}

	/// Import many statements at once.
	///
	/// Provide an iterator yielding remote, pre-checked statements.
	///
	/// The statement producer, if any, will produce only statements concerning the same candidate
	/// as the one just imported
	pub fn import_remote_statements<R, I, U>(&self, router: &R, iterable: I) -> U
		where
			R: TableRouter,
			I: IntoIterator<Item=table::SignedStatement>,
			U: ::std::iter::FromIterator<Option<ParachainWork<future::Join<
				<R::FetchCandidate as IntoFuture>::Future,
				<R::FetchIncoming as IntoFuture>::Future,
			>>>>,
	{
		let mut inner = self.inner.lock();

		iterable.into_iter().map(move |statement| {
			inner.import_remote_statement(&*self.context, router, statement)
		}).collect()
	}

	/// Sign and import the result of candidate validation.
	pub fn import_validated(&self, validated: Validated)
		-> SignedStatement
	{
		let digest = match validated.statement {
			GenericStatement::Candidate(ref c) => c.hash(),
			GenericStatement::Valid(h) | GenericStatement::Invalid(h) => h,
		};

		let signed_statement = self.context.sign_statement(validated.statement);

		let mut inner = self.inner.lock();
		inner.table.import_statement(&*self.context, signed_statement.clone());
		inner.validated.insert(digest, ValidationWork::Done(validated.result));

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
			.map(|attested| AttestedCandidate {
				candidate: attested.candidate,
				validity_votes: attested.validity_votes.into_iter().map(|(a, v)| match v {
					GAttestation::Implicit(s) => (a, ValidityAttestation::Implicit(s)),
					GAttestation::Explicit(s) => (a, ValidityAttestation::Explicit(s)),
				}).collect(),
			})
			.collect()
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
	pub fn get_misbehavior(&self) -> HashMap<SessionKey, table::Misbehavior> {
		self.inner.lock().table.get_misbehavior().clone()
	}

	/// Track includability  of a given set of candidate hashes.
	pub fn track_includability<I>(&self, iterable: I) -> Includable
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
}

#[cfg(test)]
mod tests {
	use super::*;
	use substrate_keyring::Keyring;
	use futures::future;

	#[derive(Clone)]
	struct DummyRouter;
	impl TableRouter for DummyRouter {
		type Error = ::std::io::Error;
		type FetchCandidate = ::futures::future::FutureResult<BlockData,Self::Error>;
		type FetchIncoming = ::futures::future::FutureResult<Incoming,Self::Error>;

		fn local_candidate(&self, _candidate: CandidateReceipt, _block_data: BlockData, _extrinsic: Extrinsic) {

		}

		fn fetch_block_data(&self, _candidate: &CandidateReceipt) -> Self::FetchCandidate {
			future::ok(BlockData(vec![1, 2, 3, 4, 5]))
		}

		fn fetch_incoming(&self, _para_id: ParaId) -> Self::FetchIncoming {
			future::ok(Vec::new())
		}
	}

	#[test]
	fn statement_triggers_fetch_and_evaluate() {
		let mut groups = HashMap::new();

		let para_id = ParaId::from(1);
		let local_id = Keyring::Alice.to_raw_public().into();
		let local_key = Arc::new(Keyring::Alice.pair());

		let validity_other = Keyring::Bob.to_raw_public().into();
		let validity_other_key = Keyring::Bob.pair();
		let parent_hash = Default::default();

		groups.insert(para_id, GroupInfo {
			validity_guarantors: [local_id, validity_other].iter().cloned().collect(),
			needed_validity: 2,
		});

		let shared_table = SharedTable::new(
			groups,
			local_key.clone(),
			parent_hash,
			ExtrinsicStore::new_in_memory(),
		);

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			balance_uploads: Vec::new(),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [2; 32].into(),
		};

		let candidate_statement = GenericStatement::Candidate(candidate);

		let signature = ::sign_table_statement(&candidate_statement, &validity_other_key, &parent_hash);
		let signed_statement = ::table::generic::SignedStatement {
			statement: candidate_statement,
			signature: signature.into(),
			sender: validity_other,
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
		let local_id = Keyring::Alice.to_raw_public().into();
		let local_key = Arc::new(Keyring::Alice.pair());

		let validity_other = Keyring::Bob.to_raw_public().into();
		let validity_other_key = Keyring::Bob.pair();
		let parent_hash = Default::default();

		groups.insert(para_id, GroupInfo {
			validity_guarantors: [local_id, validity_other].iter().cloned().collect(),
			needed_validity: 1,
		});

		let shared_table = SharedTable::new(
			groups,
			local_key.clone(),
			parent_hash,
			ExtrinsicStore::new_in_memory(),
		);

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			balance_uploads: Vec::new(),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [2; 32].into(),
		};

		let candidate_statement = GenericStatement::Candidate(candidate);

		let signature = ::sign_table_statement(&candidate_statement, &validity_other_key, &parent_hash);
		let signed_statement = ::table::generic::SignedStatement {
			statement: candidate_statement,
			signature: signature.into(),
			sender: validity_other,
		};

		shared_table.import_remote_statement(
			&DummyRouter,
			signed_statement,
		).expect("should produce work");
	}

	#[test]
	fn evaluate_makes_block_data_available() {
		let store = ExtrinsicStore::new_in_memory();
		let relay_parent = [0; 32].into();
		let para_id = 5.into();
		let block_data = BlockData(vec![1, 2, 3]);

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			balance_uploads: Vec::new(),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [2; 32].into(),
		};

		let hash = candidate.hash();

		let producer: ParachainWork<future::FutureResult<_, ::std::io::Error>> = ParachainWork {
			work: Work {
				candidate_receipt: candidate,
				fetch: future::ok((block_data.clone(), Vec::new())),
			},
			relay_parent,
			extrinsic_store: store.clone(),
		};

		let validated = producer.prime_with(|_, _, _| Ok(Extrinsic { outgoing_messages: Vec::new() }))
			.wait()
			.unwrap();

		assert_eq!(validated.block_data(), &block_data);
		assert_eq!(validated.statement, GenericStatement::Valid(hash));

		assert_eq!(store.block_data(relay_parent, hash).unwrap(), block_data);
		assert!(store.extrinsic(relay_parent, hash).is_some());
	}

	#[test]
	fn full_availability() {
		let store = ExtrinsicStore::new_in_memory();
		let relay_parent = [0; 32].into();
		let para_id = 5.into();
		let block_data = BlockData(vec![1, 2, 3]);

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			balance_uploads: Vec::new(),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [2; 32].into(),
		};

		let hash = candidate.hash();

		let producer = ParachainWork {
			work: Work {
				candidate_receipt: candidate,
				fetch: future::ok::<_, ::std::io::Error>((block_data.clone(), Vec::new())),
			},
			relay_parent,
			extrinsic_store: store.clone(),
		};

		let validated = producer.prime_with(|_, _, _| Ok(Extrinsic { outgoing_messages: Vec::new() }))
			.wait()
			.unwrap();

		assert_eq!(validated.block_data(), &block_data);

		assert_eq!(store.block_data(relay_parent, hash).unwrap(), block_data);
		assert!(store.extrinsic(relay_parent, hash).is_some());
	}

	#[test]
	fn does_not_dispatch_work_after_starting_validation() {
		let mut groups = HashMap::new();

		let para_id = ParaId::from(1);
		let local_id = Keyring::Alice.to_raw_public().into();
		let local_key = Arc::new(Keyring::Alice.pair());

		let validity_other = Keyring::Bob.to_raw_public().into();
		let validity_other_key = Keyring::Bob.pair();
		let parent_hash = Default::default();

		groups.insert(para_id, GroupInfo {
			validity_guarantors: [local_id, validity_other].iter().cloned().collect(),
			needed_validity: 1,
		});

		let shared_table = SharedTable::new(
			groups,
			local_key.clone(),
			parent_hash,
			ExtrinsicStore::new_in_memory(),
		);

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			balance_uploads: Vec::new(),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [2; 32].into(),
		};

		let hash = candidate.hash();
		let candidate_statement = GenericStatement::Candidate(candidate);

		let signature = ::sign_table_statement(&candidate_statement, &validity_other_key, &parent_hash);
		let signed_statement = ::table::generic::SignedStatement {
			statement: candidate_statement,
			signature: signature.into(),
			sender: validity_other,
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
		let local_id = Keyring::Alice.to_raw_public().into();
		let local_key = Arc::new(Keyring::Alice.pair());
		let block_data = BlockData(vec![1, 2, 3]);
		let extrinsic = Extrinsic { outgoing_messages: Vec::new() };

		let validity_other = Keyring::Bob.to_raw_public().into();
		let parent_hash = Default::default();

		groups.insert(para_id, GroupInfo {
			validity_guarantors: [local_id, validity_other].iter().cloned().collect(),
			needed_validity: 1,
		});

		let shared_table = SharedTable::new(
			groups,
			local_key.clone(),
			parent_hash,
			ExtrinsicStore::new_in_memory(),
		);

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: [1; 32].into(),
			signature: Default::default(),
			head_data: ::polkadot_primitives::parachain::HeadData(vec![1, 2, 3, 4]),
			balance_uploads: Vec::new(),
			egress_queue_roots: Vec::new(),
			fees: 1_000_000,
			block_data_hash: [2; 32].into(),
		};

		let hash = candidate.hash();
		let signed_statement = shared_table.import_validated(Validated::collated_local(
			candidate,
			block_data,
			extrinsic,
		));

		assert!(shared_table.inner.lock().validated.get(&hash).expect("validation has started").is_done());

		let a = shared_table.import_remote_statement(
			&DummyRouter,
			signed_statement,
		);

		assert!(a.is_none());
	}
}
