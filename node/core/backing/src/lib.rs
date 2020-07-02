// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Implements a `CandidateBackingSubsystem`.

#![recursion_limit="256"]

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bitvec::vec::BitVec;
use log;
use futures::{
	select, FutureExt, SinkExt, StreamExt,
	channel::{oneshot, mpsc},
	future::{self, Either},
	task::{Spawn, SpawnExt},
};
use futures_timer::Delay;
use streamunordered::{StreamUnordered, StreamYield};

use primitives::Pair;
use keystore::KeyStorePtr;
use polkadot_primitives::{
	Hash, BlockNumber,
	parachain::{
		AbridgedCandidateReceipt, BackedCandidate, Id as ParaId, ValidatorPair, ValidatorId,
		ValidatorIndex, HeadData, ValidationCode, SigningContext, PoVBlock,
	},
};
use polkadot_node_primitives::{Statement, SignedFullStatement};
use polkadot_subsystem::{
	FromOverseer, OverseerSignal, Subsystem, SubsystemContext, SpawnedSubsystem,
};
use polkadot_subsystem::messages::{
	AllMessages, AvailabilityStoreMessage, CandidateBackingMessage, SchedulerRoster,
	RuntimeApiMessage, RuntimeApiRequest, CandidateValidationMessage, ValidationFailed,
	StatementDistributionMessage, NewBackedCandidate,
};
use statement_table::{
	generic::AttestedCandidate as TableAttestedCandidate,
	self, Table, Context as TableContextTrait,
	Statement as TableStatement, SignedStatement as TableSignedStatement,
};

#[derive(Debug, derive_more::From)]
enum Error {
	#[from]
	ValidationFailed(ValidationFailed),
	#[from]
	Oneshot(oneshot::Canceled),
	#[from]
	Mpsc(mpsc::SendError),
}

/// Holds all data needed for candidate backing job operation.
struct CandidateBackingJob {
	/// The keystore which holds the signing keys.
	keystore: KeyStorePtr,
	/// The hash of the relay parent on top of which this job is doing it's work.
	parent: Hash,
	/// Inbound message channel receiving part.
	rx_to: mpsc::Receiver<ToJob>,
	/// Inbound message sending part. Handed out to whom it may concern.
	tx_to: mpsc::Sender<ToJob>,
	/// Outbound message channel sending part.
	tx_from: mpsc::Sender<FromJob>,

	/// The validation codes of the `ParaId`s we are assigned as validators to.
	validation_code: ValidationCode,
	/// `HeadData`s of the parachains that this validator is assigned to.
	head_data: HeadData,
	/// The `ParaId`s assigned to this validator.
	assignment: ParaId,
	/// We issued `Valid` statements on about these candidates.
	issued_validity: HashSet<Hash>,
	/// We sent `NewBackedCandidate` on these candidates.
	backed_candidates: HashSet<Hash>,
	/// `Some(h)` if this job has already issues `Seconded` statemt for some candidate with `h` hash.
	seconded: Option<Hash>,
	/// Registered backing watchers.
	backing_watchers: Vec<mpsc::Sender<NewBackedCandidate>>,

	table: Table<TableContext>,
	table_context: TableContext,
}

const fn needed_votes(n_validators: usize) -> usize {
	let mut threshold = (n_validators * 2) / 3;
	threshold += (n_validators * 2) % 3;
	threshold
}

#[derive(Default)]
struct TableContext {
	signing_context: SigningContext,
	key: Option<Arc<ValidatorPair>>,
	groups: HashMap<ParaId, HashSet<ValidatorId>>,
	validators: Vec<ValidatorId>,
}

impl TableContextTrait for TableContext {
	fn is_member_of(&self, authority: ValidatorIndex, group: &ParaId) -> bool {
		let key = match self.validators.get(authority as usize) {
			Some(val) => val,
			None => return false,
		};

		self.groups.get(group).map_or(false, |g| g.get(&key).is_some())
	}

	fn requisite_votes(&self, group: &ParaId) -> usize {
		self.groups.get(group).map_or(usize::max_value(), |g| needed_votes(g.len()))
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
}

const CHANNEL_CAPACITY: usize = 64;

/// A message type that is sent from `CandidateBackingSubsystem` to `CandidateBackingJob`.
enum ToJob {
	/// A `CandidateBackingMessage`.
	CandidateBacking(CandidateBackingMessage),
	/// Stop working.
	Stop,
}

/// A message type that is sent from `CandidateBackingJob` to `CandidateBackingSubsystem`.
enum FromJob {
	RuntimeApiMessage(RuntimeApiMessage),
	AvailabilityStoreMessage(AvailabilityStoreMessage),
	CandidateValidation(CandidateValidationMessage),
	StatementDistribution(StatementDistributionMessage),
}

impl From<FromJob> for AllMessages {
	fn from(f: FromJob) -> Self {
		match f {
			FromJob::RuntimeApiMessage(msg) => AllMessages::RuntimeApi(msg),
			FromJob::AvailabilityStoreMessage(msg) => AllMessages::AvailabilityStore(msg),
			FromJob::CandidateValidation(msg) => AllMessages::CandidateValidation(msg),
			FromJob::StatementDistribution(msg) => AllMessages::StatementDistribution(msg),
		}
	}
}

// It looks like it's not possible to do an `impl From` given the current state of
// the code. So this does the necessary conversion.
fn primitive_statement_to_table(s: &SignedFullStatement) -> TableSignedStatement {
	let statement = match s.payload() {
		Statement::Seconded(c) => TableStatement::Candidate(c.clone()),
		Statement::Valid(h) => TableStatement::Valid(h.clone()),
		Statement::Invalid(h) => TableStatement::Invalid(h.clone()),
	};

	TableSignedStatement {
		statement,
		signature: s.signature().clone(),
		sender: s.validator_index(),
	}
}

// finds the first key we are capable of signing with out of the given set of validators,
// if any.
fn signing_key(validators: &[ValidatorId], keystore: &KeyStorePtr) -> Option<Arc<ValidatorPair>> {
	let keystore = keystore.read();
	validators.iter()
		.find_map(|v| {
			keystore.key_pair::<ValidatorPair>(&v).ok()
		})
		.map(|pair| Arc::new(pair))
}

impl CandidateBackingJob {
	/// Create a new `CandidateBackingJob` working on top of some parent and with a
	/// keystore holding keys.
	fn new(parent: Hash, keystore: KeyStorePtr) -> (Self, mpsc::Receiver<FromJob>) {
		let (tx_to, rx_to) = mpsc::channel(CHANNEL_CAPACITY);
		let (tx_from, rx_from) = mpsc::channel(CHANNEL_CAPACITY);

		(
			Self {
				keystore,
				parent,
				rx_to,
				tx_to,
				tx_from,

				validation_code: ValidationCode::default(),
				head_data: HeadData::default(),
				assignment: ParaId::default(),

				issued_validity: HashSet::default(),
				backed_candidates: HashSet::default(),
				seconded: None,
				backing_watchers: Vec::new(),

				table: Table::default(),
				table_context: TableContext::default(),
			},
			rx_from,
		)
	}

	/// Hand out the sending part of the channel to communicate with this job.
	fn tx(&mut self) -> mpsc::Sender<ToJob> {
		self.tx_to.clone()
	}

	/// Request a validator set from the `RuntimeApi`.
	async fn request_validators(&mut self) -> Result<Vec<ValidatorId>, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::RuntimeApiMessage(RuntimeApiMessage::Request(
				self.parent.clone(),
				RuntimeApiRequest::Validators(tx),
			)
		)).await?;

		Ok(rx.await?)
	}

	/// Request the scheduler roster from `RuntimeApi`.
	async fn request_validator_groups(&mut self) -> Result<SchedulerRoster, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::RuntimeApiMessage(RuntimeApiMessage::Request(
				self.parent.clone(),
				RuntimeApiRequest::ValidatorGroups(tx),
			)
		)).await?;

		Ok(rx.await?)
	}

	/// Request the validation code for some `ParaId` at given block
	/// from `RuntimeApi`.
	async fn request_validation_code(
		&mut self,
		id: ParaId,
		parent: BlockNumber,
		parablock: Option<BlockNumber>,
	) -> Result<ValidationCode, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::RuntimeApiMessage(RuntimeApiMessage::Request(
				self.parent.clone(),
				RuntimeApiRequest::ValidationCode(id, parent, parablock, tx),
			)
		)).await?;

		Ok(rx.await?)
	}

	/// Request a `SigningContext` from the `RuntimeApi`.
	async fn request_signing_context(&mut self) -> Result<SigningContext, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::RuntimeApiMessage(RuntimeApiMessage::Request(
				self.parent.clone(),
				RuntimeApiRequest::SigningContext(tx),
			)
		)).await?;

		Ok(rx.await?)
	}

	/// Request `HeadData` for some `ParaId` from `RuntimeApi`.
	async fn request_head_data(&mut self, id: ParaId) -> Result<HeadData, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::RuntimeApiMessage(RuntimeApiMessage::Request(
				self.parent.clone(),
				RuntimeApiRequest::HeadData(id, tx),
			)
		)).await?;

		Ok(rx.await?)
	}

	/// Logic that runs on startup.
	async fn on_startup(&mut self) -> Result<(), Error> {
		let validators = self.request_validators().await?;
	
		let roster = self.request_validator_groups().await?; 

		self.table_context.key = signing_key(&validators[..], &self.keystore);

		for assignment in roster.scheduled {
			self.table_context.groups.insert(
				assignment.para_id,
				roster.validator_groups[assignment.group_idx.0 as usize]
				.iter()
				.map(|idx| validators[*idx as usize].clone())
				.collect()
			);
		}

		if let Some(ref key) = self.table_context.key {
			for (para_id, group) in self.table_context.groups.iter() {
				if group.contains(&key.as_ref().public()) {
					self.assignment = *para_id;
				}
			}
		}

		self.validation_code = self.request_validation_code(self.assignment, 0, None).await?;
		self.head_data = self.request_head_data(self.assignment).await?;
		self.table_context.signing_context = self.request_signing_context().await?;
		self.table_context.validators = validators;

		Ok(())
	}

	/// Run asynchronously.
	async fn run(mut self) -> Result<(), Error> {
		self.on_startup().await?;

		while let Some(msg) = self.rx_to.next().await {
			match msg {
				ToJob::CandidateBacking(msg) => {
					self.process_msg(msg).await?;
				}
				_ => break,
			}
		}

		Ok(())
	}

	/// Validate the candidate that is requested to be `Second`ed and distribute validation result.
	async fn validate_and_second(&mut self, candidate: AbridgedCandidateReceipt) -> Result<(), Error> {
		let statement = match self.spawn_validation_work(candidate.clone()).await {
			Ok(()) => {
				// make PoV available for later distribution. Send data to the availability
				// store to keep. Sign and dispatch `valid` statement to network if we
				// have not seconded the given candidate.
				Statement::Seconded(candidate)
			}

			// sign and dispatch `invalid` statement to network.
			Err(_) => Statement::Invalid(candidate.hash()),
		};

		if let Some(ref signing_key) = self.table_context.key {
			if let Some(local_index) = self.table_context.local_index() {
				let signed_statement = SignedFullStatement::sign(
					statement,
					&self.table_context.signing_context,
					local_index,
					signing_key,
				);

				self.distribute_signed_statement(signed_statement).await?;
			}
		}

		Ok(())
	}

	async fn import_statement(&mut self, statement: SignedFullStatement) -> Result<(), Error> {
		let idx = statement.validator_index() as usize;

		if self.table_context.validators.len() > idx {
			match statement.check_signature(
				&self.table_context.signing_context,
				&self.table_context.validators[idx],
			) {
				Ok(()) => {
					let stmt = primitive_statement_to_table(&statement);

					self.table.import_statement(&self.table_context, stmt);

					let proposed = self.table.proposed_candidates(&self.table_context);

					for p in proposed.into_iter() {
						if !self.backed_candidates.contains(&p.candidate.hash()) {
							let TableAttestedCandidate { candidate, validity_votes, .. } = p;

							let (ids, validity_votes): (Vec<_>, Vec<_>) = validity_votes
										.into_iter()
										.map(|(id, vote)| (id, vote.into()))
										.unzip();

							self.backed_candidates.insert(candidate.hash());

							let mut validator_indices = BitVec::with_capacity(
								self.table_context.validators.len(),
							);

							validator_indices.resize(self.table_context.validators.len(), false);

							for id in ids.iter() {
								validator_indices.set(*id as usize, true);
							}

							let backed = BackedCandidate {
								candidate,
								validity_votes,
								validator_indices,
							};

							for watcher in self.backing_watchers.iter_mut() {
								let _ = watcher.send(NewBackedCandidate(backed.clone())).await;
							}
						}
					}
				}
				Err(()) => {
					return Err(Error::ValidationFailed(ValidationFailed));
				}
			}
		};

		Ok(())
	}

	async fn process_msg(&mut self, msg: CandidateBackingMessage) -> Result<(), Error> {
		match msg {
			CandidateBackingMessage::Second(_, candidate) => {
				// If the message is a `CandidateBackingMessage::Second`, sign and dispatch a
				// Seconded statement only if we have not seconded any other candidate and
				// have not signed a Valid statement for the requested candidate.
				match self.seconded {
					// This job has not seconded a candidate yet.
					None => {
						let candidate_hash = candidate.hash();

						if !self.issued_validity.contains(&candidate_hash) {
							if let Ok(()) = self.validate_and_second(candidate).await {
								self.seconded = Some(candidate_hash);
							}
						}
					}
					// This job has already seconded a candidate.
					Some(_) => {
					},
				}
			}
			CandidateBackingMessage::Statement(_, statement) => {
				let _ = self.import_statement(statement).await;
			}
			CandidateBackingMessage::RegisterBackingWatcher(_, tx) => {
				self.backing_watchers.push(tx);
			}
		}

		Ok(())
	}

	async fn request_candidate_validation(
		&mut self,
		candidate: AbridgedCandidateReceipt,
		pov: PoVBlock,
	) -> Result<(), Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::CandidateValidation(
				CandidateValidationMessage::Validate(
					self.parent,
					candidate,
					pov,
					tx,
				)
			)
		).await?;

		rx.await??;

		Ok(())
	}

	async fn distribute_signed_statement(&mut self, s: SignedFullStatement) -> Result<(), Error> {
		let smsg = StatementDistributionMessage::Share(self.parent, s);

		self.tx_from.send(FromJob::StatementDistribution(smsg)).await?;

		Ok(())
	}

	async fn spawn_validation_work(
		&mut self,
		candidate: AbridgedCandidateReceipt,
	) -> Result<(), Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::AvailabilityStoreMessage(
				AvailabilityStoreMessage::QueryPoV(
					candidate.pov_block_hash,
					tx,
				)
			)
		).await?;

		let pov = rx.await?.unwrap();

		self.request_candidate_validation(candidate, pov).await
	}
}

struct JobHandle {
	abort_handle: future::AbortHandle,
	to_job: mpsc::Sender<ToJob>,
	finished: oneshot::Receiver<()>,
	su_handle: usize,
}

impl JobHandle {
	async fn stop(mut self) {
		let _ = self.to_job.send(ToJob::Stop).await;
		let stop_timer = Delay::new(Duration::from_secs(1));

		match future::select(stop_timer, self.finished).await {
			Either::Left((_, _)) => {
			},
			Either::Right((_, _)) => {
				self.abort_handle.abort();
			},
		}
	}

	async fn send_msg(&mut self, msg: ToJob) -> Result<(), Error> {
		Ok(self.to_job.send(msg).await?)
	}
}

struct Jobs<S> {
	spawner: S,
	running: HashMap<Hash, JobHandle>,
	outgoing_msgs: StreamUnordered<mpsc::Receiver<FromJob>>,
}

impl<S: Spawn> Jobs<S> {
	fn new(spawner: S) -> Self {
		Self {
			spawner,
			running: HashMap::default(),
			outgoing_msgs: StreamUnordered::new(),
		}
	}

	fn spawn_job(&mut self, parent_hash: Hash, keystore: KeyStorePtr) -> Result<(), ()> {
		let (mut job, from_job) = CandidateBackingJob::new(parent_hash,keystore);

		let to_job = job.tx();

		let (future, abort_handle) = future::abortable(async move {
			if let Err(_) = job.run().await {
				log::error!("Job returned an error");
			}
		});
		let (finished_tx, finished) = oneshot::channel();

		let future = async move {
			let _ = future.await;
			let _ = finished_tx.send(());
		};
		self.spawner.spawn(future).map_err(|_| ())?;

		let su_handle = self.outgoing_msgs.push(from_job);

		let handle = JobHandle {
			abort_handle,
			to_job,
			finished,
			su_handle,
		};

		self.running.insert(parent_hash, handle);

		Ok(())
	}

	async fn stop_job(&mut self, parent_hash: Hash) -> Result<(), ()> {
		match self.running.remove(&parent_hash) {
			Some(handle) => {
				Pin::new(&mut self.outgoing_msgs).remove(handle.su_handle);
				handle.stop().await;
				Ok(())
			}
			None => Err(())
		}
	}

	async fn send_msg(&mut self, parent_hash: Hash, msg: ToJob) {
		if let Some(job) = self.running.get_mut(&parent_hash) {
			let _ = job.send_msg(msg).await;
		}
	}

	async fn next(&mut self) -> Option<FromJob> {
		self.outgoing_msgs.next().await.and_then(|(e, _)| match e {
			StreamYield::Item(e) => Some(e),
			_ => None,
		})
	}
}

pub struct CandidateBackingSubsystem<S, Context> {
	spawner: S,
	keystore: KeyStorePtr,
	_context: std::marker::PhantomData<Context>,
}

impl<S, Context> CandidateBackingSubsystem<S, Context>
	where
		S: Spawn + Clone,
		Context: SubsystemContext<Message=CandidateBackingMessage>,
{
	pub fn new(keystore: KeyStorePtr, spawner: S) -> Self {
		Self {
			spawner,
			keystore,
			_context: std::marker::PhantomData,
		}
	}

	async fn run(
		mut ctx: Context,
		keystore: KeyStorePtr,
		spawner: S,
	) {
		let mut jobs = Jobs::new(spawner.clone());

		loop {
			select! {
				incoming = ctx.recv().fuse() => {
					match incoming {
						Ok(msg) => match msg {
							FromOverseer::Signal(OverseerSignal::StartWork(hash)) => {
								let _ = jobs.spawn_job(hash, keystore.clone());
							}
							FromOverseer::Signal(OverseerSignal::StopWork(hash)) => {
								let _ = jobs.stop_job(hash);
							}
							FromOverseer::Communication { msg } => {
								match msg {
									CandidateBackingMessage::Second(hash, _) |
									CandidateBackingMessage::Statement(hash, _) |
									CandidateBackingMessage::RegisterBackingWatcher(hash, _) => {
										let _ = jobs.send_msg(hash.clone(), ToJob::CandidateBacking(msg)).await;
									}
									_ => (),
								}
							}
							_ => (),
						},
						Err(_) => break,
					}
				}
				outgoing = jobs.next().fuse() => {
					match outgoing {
						Some(msg) => {
							let _ = ctx.send_message(msg.into()).await;
						}
						None => break,
					}
				}
				complete => break,
			}
		}
	}
}

impl<S, Context> Subsystem<Context> for CandidateBackingSubsystem<S, Context>
	where
		S: Spawn + Send + Clone + 'static,
		Context: SubsystemContext<Message=CandidateBackingMessage>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let keystore = self.keystore.clone();
		let spawner = self.spawner.clone();

		SpawnedSubsystem(Box::pin(async move {
			Self::run(ctx, keystore, spawner).await;
		}))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures::{
		channel::mpsc, Future,
		executor::{self, ThreadPool},
	};
	use std::collections::HashMap;
	use sp_keyring::Sr25519Keyring;
	use polkadot_primitives::parachain::{
		AssignmentKind, CollatorId, CoreAssignment, BlockData, CoreIndex, GroupIndex, ValidityAttestation,
	};

	fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
		val_ids.iter().map(|v| v.public().into()).collect()
	}

	struct TestState {
		chain_ids: Vec<ParaId>,
		keystore: KeyStorePtr,
		validators: Vec<Sr25519Keyring>,
		validator_public: Vec<ValidatorId>,
		roster: SchedulerRoster,
		validation_code: HashMap<ParaId, ValidationCode>,
		head_data: HashMap<ParaId, HeadData>,
		signing_context: SigningContext,
		relay_parent: Hash,
	}

	impl Default for TestState {
		fn default() -> Self {
			let chain_a = ParaId::from(1);
			let chain_b = ParaId::from(2);
			let thread_a = ParaId::from(3);

			let chain_ids = vec![chain_a, chain_b, thread_a];

			let validators = vec![
				Sr25519Keyring::Alice,
				Sr25519Keyring::Bob,
				Sr25519Keyring::Charlie,
				Sr25519Keyring::Dave,
				Sr25519Keyring::Ferdie,
			];

			let keystore = keystore::Store::new_in_memory();
			// Make sure `Alice` key is in the keystore, so this mocked node will be a parachain validator.
			keystore.write().insert_ephemeral_from_seed::<ValidatorPair>(&validators[0].to_seed())
				.expect("Insert key into keystore");

			let validator_public = validator_pubkeys(&validators);

			let chain_a_assignment = CoreAssignment {
				core: CoreIndex::from(0),
				para_id: chain_a,
				kind: AssignmentKind::Parachain,
				group_idx: GroupIndex::from(0),
			};

			let chain_b_assignment = CoreAssignment {
				core: CoreIndex::from(1),
				para_id: chain_b,
				kind: AssignmentKind::Parachain,
				group_idx: GroupIndex::from(1),
			};

			let thread_collator: CollatorId = Sr25519Keyring::Two.public().into();

			let thread_a_assignment = CoreAssignment {
				core: CoreIndex::from(2),
				para_id: thread_a,
				kind: AssignmentKind::Parathread(thread_collator.clone(), 0),
				group_idx: GroupIndex::from(2),
			};

			let validator_groups = vec![vec![0, 2], vec![1, 3], vec![4]];

			let parent_hash_1 = [1; 32].into();

			let roster = SchedulerRoster {
				validator_groups,
				scheduled: vec![
					chain_a_assignment,
					chain_b_assignment,
					thread_a_assignment,
				],
				upcoming: vec![],
				availability_cores: vec![],
			};
			let signing_context = SigningContext {
				session_index: 1,
				parent_hash: parent_hash_1,
			};

			let mut head_data = HashMap::new();
			head_data.insert(chain_a, HeadData(vec![4, 5, 6]));

			let mut validation_code = HashMap::new();
			validation_code.insert(chain_a, ValidationCode(vec![1, 2, 3]));

			let relay_parent = Hash::from([5; 32]);

			Self {
				chain_ids,
				keystore,
				validators,
				validator_public,
				roster,
				validation_code,
				head_data,
				signing_context,
				relay_parent,
			}
		}
	}

	struct TestHarness {
		virtual_overseer: subsystem_test::TestSubsystemContextHandle<CandidateBackingMessage>,
	}

	fn test_harness<T: Future<Output=()>>(keystore: KeyStorePtr, test: impl FnOnce(TestHarness) -> T) {
		let pool = ThreadPool::new().unwrap();

		let (context, virtual_overseer) = subsystem_test::make_subsystem_context(pool.clone());

		let subsystem = CandidateBackingSubsystem::run(context, keystore, pool.clone());

		let test_fut = test(TestHarness {
			virtual_overseer,
		});

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}

	async fn test_startup(
		virtual_overseer: &mut subsystem_test::TestSubsystemContextHandle<CandidateBackingMessage>,
		test_state: &TestState,
	) {
		// Start work on some new parent.
		virtual_overseer.send(FromOverseer::Signal(OverseerSignal::StartWork(test_state.relay_parent))).await;

		// Check that subsystem job issues a request for a validator set.
		match virtual_overseer.recv().await {
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::Validators(tx))
			) if parent == test_state.relay_parent => {
				tx.send(test_state.validator_public.clone()).unwrap();
			}
			msg => panic!("unexpected message {:?}", msg),
		}

		// Check that subsystem job issues a request for the validator groups.
		match virtual_overseer.recv().await {
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidatorGroups(tx))
			) if parent == test_state.relay_parent => {
				tx.send(test_state.roster.clone()).unwrap();
			}
			msg => panic!("unexpected message {:?}", msg),
		}

		// Check that subsystem job issues a request for the validation code.
		match virtual_overseer.recv().await {
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidationCode(id, _, _, tx))
			) if parent == test_state.relay_parent => {
				tx.send(test_state.validation_code.get(&id).unwrap().clone()).unwrap();
			}
			msg => panic!("unexpected message {:?}", msg),
		}

		// Check that subsystem job issues a request for the head data.
		match virtual_overseer.recv().await {
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::HeadData(id, tx))
			) if parent == test_state.relay_parent => {
				tx.send(test_state.head_data.get(&id).unwrap().clone()).unwrap();
			}
			msg => panic!("unexpected message {:?}", msg),
		}

		// Check that subsystem job issues a request for the signing context.
		match virtual_overseer.recv().await {
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::SigningContext(tx))
			) if parent == test_state.relay_parent => {
				tx.send(test_state.signing_context.clone()).unwrap();
			}
			msg => panic!("unexpected message {:?}", msg),
		}
	}

	#[test]
	fn backing_works() {
		let test_state = TestState::default();
		test_harness(test_state.keystore.clone(), |test_harness| async move {

			let TestHarness { mut virtual_overseer } = test_harness;
			
			test_startup(&mut virtual_overseer, &test_state).await;
	
			// Check that `Second` message issues all necessary requests.
			{
				let pov_block = PoVBlock {
					block_data: BlockData(vec![42, 43, 44]),
				};

				let pov_block_hash = pov_block.hash();
				let candidate = AbridgedCandidateReceipt {
					parachain_index: test_state.chain_ids[0],
					relay_parent: test_state.relay_parent,
					pov_block_hash,
					..Default::default()
				};

				let second = CandidateBackingMessage::Second(test_state.relay_parent, candidate.clone());

				virtual_overseer.send(FromOverseer::Communication{ msg: second }).await;

				match virtual_overseer.recv().await {
					AllMessages::AvailabilityStore(
						AvailabilityStoreMessage::QueryPoV(
							pov_hash,
							tx,
						)
					) if pov_hash == pov_block_hash => {
						tx.send(Some(pov_block.clone())).unwrap();
					}
					msg => panic!("unexpected msg {:?}", msg),
				}

				match virtual_overseer.recv().await {
					AllMessages::CandidateValidation(
						CandidateValidationMessage::Validate(
							parent_hash,
							c,
							pov,
							tx,
						)
					) if parent_hash == test_state.relay_parent && pov == pov_block && c == candidate => {
						tx.send(Ok(())).unwrap();
					}
					msg => panic!("unexpected msg {:?}", msg),
				}

				match virtual_overseer.recv().await {
					AllMessages::StatementDistribution(
						StatementDistributionMessage::Share(
							parent_hash,
							signed_statement,
						)
					) if parent_hash == test_state.relay_parent => {
						signed_statement.check_signature(
							&test_state.signing_context,
							&test_state.validator_public[0],
						).unwrap();
					}
					msg => panic!("unexpected message {:?}", msg),
				}
			}

			// Check that reaching a required quorum on a candidate issues a correct message
			// to the `BackingWatcher`.
			{
				let (backed_tx, mut backed_rx) = mpsc::channel(64);

				let register_watcher = CandidateBackingMessage::RegisterBackingWatcher(
					test_state.relay_parent,
					backed_tx,
				);

				virtual_overseer.send(FromOverseer::Communication{ msg: register_watcher}).await;

				let candidate_a = AbridgedCandidateReceipt {
					parachain_index: test_state.chain_ids[0],
					relay_parent: test_state.relay_parent,
					pov_block_hash: Hash::from([5; 32]),
					..Default::default()
				};
				let candidate_a_hash = candidate_a.hash();

				let signed_a = SignedFullStatement::sign(
					Statement::Seconded(candidate_a.clone()),
					&test_state.signing_context,
					2,
					&test_state.validators[2].pair().into(),
				);

				let signed_b = SignedFullStatement::sign(
					Statement::Valid(candidate_a_hash),
					&test_state.signing_context,
					0,
					&test_state.validators[0].pair().into(),
				);

				let statement = CandidateBackingMessage::Statement(test_state.relay_parent, signed_a.clone());

				virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

				let statement = CandidateBackingMessage::Statement(test_state.relay_parent, signed_b.clone());

				virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

				let backed = backed_rx.next().await.unwrap();

				// `validity_votes` may be in any order so we can't do this in a single assert.
				assert_eq!(backed.0.candidate, candidate_a);
				assert_eq!(backed.0.validity_votes.len(), 2);
				assert!(backed.0.validity_votes.contains(
					&ValidityAttestation::Explicit(signed_b.signature().clone())
				));
				assert!(backed.0.validity_votes.contains(
					&ValidityAttestation::Implicit(signed_a.signature().clone())
				));

				assert_eq!(backed.0.validator_indices, bitvec::bitvec![Lsb0, u8; 1, 0, 1, 0, 0]);
			}

			virtual_overseer.send(FromOverseer::Signal(OverseerSignal::StopWork(test_state.relay_parent))).await;
		});
	}
}
