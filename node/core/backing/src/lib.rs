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

use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::pin::Pin;
use std::sync::Arc;

use bitvec::vec::BitVec;
use futures::{
	channel::{mpsc, oneshot},
	Future, FutureExt, SinkExt, StreamExt,
};

use keystore::KeyStorePtr;
use polkadot_primitives::v1::{
	CommittedCandidateReceipt, BackedCandidate, Id as ParaId, ValidatorId,
	ValidatorIndex, SigningContext, PoV, OmittedValidationData,
	CandidateDescriptor, AvailableData, ValidatorSignature, Hash, CandidateReceipt,
	CandidateCommitments,
};
use polkadot_node_primitives::{
	FromTableMisbehavior, Statement, SignedFullStatement, MisbehaviorReport,
	ValidationOutputs, ValidationResult, SpawnNamed,
};
use polkadot_subsystem::{
	Subsystem, SubsystemContext, SpawnedSubsystem,
	messages::{
		AllMessages, AvailabilityStoreMessage, CandidateBackingMessage, CandidateSelectionMessage,
		CandidateValidationMessage, NewBackedCandidate, PoVDistributionMessage, ProvisionableData,
		ProvisionerMessage, RuntimeApiMessage, StatementDistributionMessage, ValidationFailed,
	},
	util::{
		self,
		request_signing_context,
		request_validator_groups,
		request_validators,
		Validator,
	},
};
use statement_table::{
	generic::AttestedCandidate as TableAttestedCandidate,
	Context as TableContextTrait,
	Table,
	v1::{
		Statement as TableStatement,
		SignedStatement as TableSignedStatement, Summary as TableSummary,
	},
};

#[derive(Debug, derive_more::From)]
enum Error {
	CandidateNotFound,
	InvalidSignature,
	StoreFailed,
	#[from]
	Erasure(erasure_coding::Error),
	#[from]
	ValidationFailed(ValidationFailed),
	#[from]
	Oneshot(oneshot::Canceled),
	#[from]
	Mpsc(mpsc::SendError),
	#[from]
	UtilError(util::Error),
}

/// Holds all data needed for candidate backing job operation.
struct CandidateBackingJob {
	/// The hash of the relay parent on top of which this job is doing it's work.
	parent: Hash,
	/// Inbound message channel receiving part.
	rx_to: mpsc::Receiver<ToJob>,
	/// Outbound message channel sending part.
	tx_from: mpsc::Sender<FromJob>,
	/// The `ParaId`s assigned to this validator.
	assignment: ParaId,
	/// We issued `Valid` or `Invalid` statements on about these candidates.
	issued_statements: HashSet<Hash>,
	/// `Some(h)` if this job has already issues `Seconded` statemt for some candidate with `h` hash.
	seconded: Option<Hash>,
	/// We have already reported misbehaviors for these validators.
	reported_misbehavior_for: HashSet<ValidatorIndex>,
	table: Table<TableContext>,
	table_context: TableContext,
}

const fn group_quorum(n_validators: usize) -> usize {
	(n_validators / 2) + 1
}

#[derive(Default)]
struct TableContext {
	signing_context: SigningContext,
	validator: Option<Validator>,
	groups: HashMap<ParaId, Vec<ValidatorIndex>>,
	validators: Vec<ValidatorId>,
}

impl TableContextTrait for TableContext {
	type AuthorityId = ValidatorIndex;
	type Digest = Hash;
	type GroupId = ParaId;
	type Signature = ValidatorSignature;
	type Candidate = CommittedCandidateReceipt;

	fn candidate_digest(candidate: &CommittedCandidateReceipt) -> Hash {
		candidate.hash()
	}

	fn candidate_group(candidate: &CommittedCandidateReceipt) -> ParaId {
		candidate.descriptor().para_id
	}

	fn is_member_of(&self, authority: &ValidatorIndex, group: &ParaId) -> bool {
		self.groups.get(group).map_or(false, |g| g.iter().position(|a| a == authority).is_some())
	}

	fn requisite_votes(&self, group: &ParaId) -> usize {
		self.groups.get(group).map_or(usize::max_value(), |g| group_quorum(g.len()))
	}
}

/// A message type that is sent from `CandidateBackingSubsystem` to `CandidateBackingJob`.
pub enum ToJob {
	/// A `CandidateBackingMessage`.
	CandidateBacking(CandidateBackingMessage),
	/// Stop working.
	Stop,
}

impl TryFrom<AllMessages> for ToJob {
	type Error = ();

	fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
		match msg {
			AllMessages::CandidateBacking(msg) => Ok(ToJob::CandidateBacking(msg)),
			_ => Err(()),
		}
	}
}

impl From<CandidateBackingMessage> for ToJob {
	fn from(msg: CandidateBackingMessage) -> Self {
		Self::CandidateBacking(msg)
	}
}

impl util::ToJobTrait for ToJob {
	const STOP: Self = ToJob::Stop;

	fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::CandidateBacking(cb) => cb.relay_parent(),
			Self::Stop => None,
		}
	}
}

/// A message type that is sent from `CandidateBackingJob` to `CandidateBackingSubsystem`.
enum FromJob {
	AvailabilityStore(AvailabilityStoreMessage),
	RuntimeApiMessage(RuntimeApiMessage),
	CandidateValidation(CandidateValidationMessage),
	CandidateSelection(CandidateSelectionMessage),
	Provisioner(ProvisionerMessage),
	PoVDistribution(PoVDistributionMessage),
	StatementDistribution(StatementDistributionMessage),
}

impl From<FromJob> for AllMessages {
	fn from(f: FromJob) -> Self {
		match f {
			FromJob::AvailabilityStore(msg) => AllMessages::AvailabilityStore(msg),
			FromJob::RuntimeApiMessage(msg) => AllMessages::RuntimeApi(msg),
			FromJob::CandidateValidation(msg) => AllMessages::CandidateValidation(msg),
			FromJob::CandidateSelection(msg) => AllMessages::CandidateSelection(msg),
			FromJob::StatementDistribution(msg) => AllMessages::StatementDistribution(msg),
			FromJob::PoVDistribution(msg) => AllMessages::PoVDistribution(msg),
			FromJob::Provisioner(msg) => AllMessages::Provisioner(msg),
		}
	}
}

impl TryFrom<AllMessages> for FromJob {
	type Error = &'static str;

	fn try_from(f: AllMessages) -> Result<Self, Self::Error> {
		match f {
			AllMessages::AvailabilityStore(msg) => Ok(FromJob::AvailabilityStore(msg)),
			AllMessages::RuntimeApi(msg) => Ok(FromJob::RuntimeApiMessage(msg)),
			AllMessages::CandidateValidation(msg) => Ok(FromJob::CandidateValidation(msg)),
			AllMessages::CandidateSelection(msg) => Ok(FromJob::CandidateSelection(msg)),
			AllMessages::StatementDistribution(msg) => Ok(FromJob::StatementDistribution(msg)),
			AllMessages::PoVDistribution(msg) => Ok(FromJob::PoVDistribution(msg)),
			AllMessages::Provisioner(msg) => Ok(FromJob::Provisioner(msg)),
			_ => Err("can't convert this AllMessages variant to FromJob"),
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

impl CandidateBackingJob {
	/// Run asynchronously.
	async fn run_loop(mut self) -> Result<(), Error> {
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

	async fn issue_candidate_invalid_message(
		&mut self,
		candidate: CandidateReceipt,
	) -> Result<(), Error> {
		self.tx_from.send(FromJob::CandidateSelection(
			CandidateSelectionMessage::Invalid(self.parent, candidate)
		)).await?;

		Ok(())
	}

	/// Validate the candidate that is requested to be `Second`ed and distribute validation result.
	///
	/// Returns `Ok(true)` if we issued a `Seconded` statement about this candidate.
	async fn validate_and_second(
		&mut self,
		candidate: &CandidateReceipt,
		pov: PoV,
	) -> Result<bool, Error> {
		let valid = self.request_candidate_validation(
			candidate.descriptor().clone(),
			Arc::new(pov.clone()),
		).await?;

		let candidate_hash = candidate.hash();

		let statement = match valid {
			ValidationResult::Valid(outputs) => {
				// make PoV available for later distribution. Send data to the availability
				// store to keep. Sign and dispatch `valid` statement to network if we
				// have not seconded the given candidate.
				//
				// If the commitments hash produced by validation is not the same as given by
				// the collator, do not make available and report the collator.
				let commitments_check = self.make_pov_available(
					pov,
					outputs,
					|commitments| if commitments.hash() == candidate.commitments_hash {
						Ok(CommittedCandidateReceipt {
							descriptor: candidate.descriptor().clone(),
							commitments,
						})
					} else {
						Err(())
					},
				).await?;

				match commitments_check {
					Ok(candidate) => {
						self.issued_statements.insert(candidate_hash);
						Some(Statement::Seconded(candidate))
					}
					Err(()) => {
						self.issue_candidate_invalid_message(candidate.clone()).await?;
						None
					}
				}
			}
			ValidationResult::Invalid => {
				// no need to issue a statement about this if we aren't seconding it.
				//
				// there's an infinite amount of garbage out there. no need to acknowledge
				// all of it.
				self.issue_candidate_invalid_message(candidate.clone()).await?;
				None
			}
		};

		let issued_statement = statement.is_some();
		if let Some(signed_statement) = statement.and_then(|s| self.sign_statement(s)) {
			self.import_statement(&signed_statement).await?;
			self.distribute_signed_statement(signed_statement).await?;
		}

		Ok(issued_statement)
	}

	fn get_backed(&self) -> Vec<NewBackedCandidate> {
		let proposed = self.table.proposed_candidates(&self.table_context);
		let mut res = Vec::with_capacity(proposed.len());

		for p in proposed.into_iter() {
			let TableAttestedCandidate { candidate, validity_votes, .. } = p;

			let (ids, validity_votes): (Vec<_>, Vec<_>) = validity_votes
						.into_iter()
						.map(|(id, vote)| (id, vote.into()))
						.unzip();

			let group = match self.table_context.groups.get(&self.assignment) {
				Some(group) => group,
				None => continue,
			};

			let mut validator_indices = BitVec::with_capacity(group.len());

			validator_indices.resize(group.len(), false);

			for id in ids.iter() {
				if let Some(position) = group.iter().position(|x| x == id) {
					validator_indices.set(position, true);
				}
			}

			let backed = BackedCandidate {
				candidate,
				validity_votes,
				validator_indices,
			};

			res.push(NewBackedCandidate(backed.clone()));
		}

		res
	}

	/// Check if there have happened any new misbehaviors and issue necessary messages.
	///
	/// TODO: Report multiple misbehaviors (https://github.com/paritytech/polkadot/issues/1387)
	async fn issue_new_misbehaviors(&mut self) -> Result<(), Error> {
		let mut reports = Vec::new();

		for (k, v) in self.table.get_misbehavior().iter() {
			if !self.reported_misbehavior_for.contains(k) {
				self.reported_misbehavior_for.insert(*k);

				let f = FromTableMisbehavior {
					id: *k,
					report: v.clone(),
					signing_context: self.table_context.signing_context.clone(),
					key: self.table_context.validators[*k as usize].clone(),
				};

				if let Ok(report) = MisbehaviorReport::try_from(f) {
					let message = ProvisionerMessage::ProvisionableData(
						ProvisionableData::MisbehaviorReport(self.parent, report),
					);

					reports.push(message);
				}
			}
		}

		for report in reports.drain(..) {
			self.send_to_provisioner(report).await?
		}

		Ok(())
	}

	/// Import a statement into the statement table and return the summary of the import.
	async fn import_statement(
		&mut self,
		statement: &SignedFullStatement,
	) -> Result<Option<TableSummary>, Error> {
		let stmt = primitive_statement_to_table(statement);

		let summary = self.table.import_statement(&self.table_context, stmt);

		self.issue_new_misbehaviors().await?;

		return Ok(summary);
	}

	async fn process_msg(&mut self, msg: CandidateBackingMessage) -> Result<(), Error> {
		match msg {
			CandidateBackingMessage::Second(_, candidate, pov) => {
				// Sanity check that candidate is from our assignment.
				if candidate.descriptor().para_id != self.assignment {
					return Ok(());
				}

				// If the message is a `CandidateBackingMessage::Second`, sign and dispatch a
				// Seconded statement only if we have not seconded any other candidate and
				// have not signed a Valid statement for the requested candidate.
				match self.seconded {
					// This job has not seconded a candidate yet.
					None => {
						let candidate_hash = candidate.hash();

						if !self.issued_statements.contains(&candidate_hash) {
							if let Ok(true) = self.validate_and_second(
								&candidate,
								pov,
							).await {
								self.seconded = Some(candidate_hash);
							}
						}
					}
					// This job has already seconded a candidate.
					Some(_) => {}
				}
			}
			CandidateBackingMessage::Statement(_, statement) => {
				self.check_statement_signature(&statement)?;
				match self.maybe_validate_and_import(statement).await {
					Err(Error::ValidationFailed(_)) => return Ok(()),
					Err(e) => return Err(e),
					Ok(()) => (),
				}
			}
			CandidateBackingMessage::GetBackedCandidates(_, tx) => {
				let backed = self.get_backed();

				tx.send(backed).map_err(|_| oneshot::Canceled)?;
			}
		}

		Ok(())
	}

	/// Kick off validation work and distribute the result as a signed statement.
	async fn kick_off_validation_work(
		&mut self,
		summary: TableSummary,
	) -> Result<(), Error> {
		let candidate_hash = summary.candidate.clone();

		if self.issued_statements.contains(&candidate_hash) {
			return Ok(())
		}

		// We clone the commitments here because there are borrowck
		// errors relating to this being a struct and methods borrowing the entirety of self
		// and not just those things that the function uses.
		let candidate = self.table.get_candidate(&candidate_hash).ok_or(Error::CandidateNotFound)?;
		let expected_commitments = candidate.commitments.clone();

		let descriptor = candidate.descriptor().clone();
		let pov = self.request_pov_from_distribution(descriptor.clone()).await?;
		let v = self.request_candidate_validation(descriptor, pov.clone()).await?;

		let statement = match v {
			ValidationResult::Valid(outputs) => {
				// If validation produces a new set of commitments, we vote the candidate as invalid.
				let commitments_check = self.make_pov_available(
					(&*pov).clone(),
					outputs,
					|commitments| if commitments == expected_commitments {
						Ok(())
					} else {
						Err(())
					}
				).await?;

				match commitments_check {
					Ok(()) => Statement::Valid(candidate_hash),
					Err(()) => Statement::Invalid(candidate_hash),
				}
			}
			ValidationResult::Invalid => {
				Statement::Invalid(candidate_hash)
			}
		};

		self.issued_statements.insert(candidate_hash);

		if let Some(signed_statement) = self.sign_statement(statement) {
			self.distribute_signed_statement(signed_statement).await?;
		}

		Ok(())
	}

	/// Import the statement and kick off validation work if it is a part of our assignment.
	async fn maybe_validate_and_import(
		&mut self,
		statement: SignedFullStatement,
	) -> Result<(), Error> {
		if let Some(summary) = self.import_statement(&statement).await? {
			if let Statement::Seconded(_) = statement.payload() {
				if summary.group_id == self.assignment {
					self.kick_off_validation_work(summary).await?;
				}
			}
		}

		Ok(())
	}

	fn sign_statement(&self, statement: Statement) -> Option<SignedFullStatement> {
		Some(self.table_context.validator.as_ref()?.sign(statement))
	}

	fn check_statement_signature(&self, statement: &SignedFullStatement) -> Result<(), Error> {
		let idx = statement.validator_index() as usize;

		if self.table_context.validators.len() > idx {
			statement.check_signature(
				&self.table_context.signing_context,
				&self.table_context.validators[idx],
			).map_err(|_| Error::InvalidSignature)?;
		} else {
			return Err(Error::InvalidSignature);
		}

		Ok(())
	}

	async fn send_to_provisioner(&mut self, msg: ProvisionerMessage) -> Result<(), Error> {
		self.tx_from.send(FromJob::Provisioner(msg)).await?;

		Ok(())
	}

	async fn request_pov_from_distribution(
		&mut self,
		descriptor: CandidateDescriptor,
	) -> Result<Arc<PoV>, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::PoVDistribution(
			PoVDistributionMessage::FetchPoV(self.parent, descriptor, tx)
		)).await?;

		Ok(rx.await?)
	}

	async fn request_candidate_validation(
		&mut self,
		candidate: CandidateDescriptor,
		pov: Arc<PoV>,
	) -> Result<ValidationResult, Error> {
		let (tx, rx) = oneshot::channel();

		self.tx_from.send(FromJob::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					candidate,
					pov,
					tx,
				)
			)
		).await?;

		Ok(rx.await??)
	}

	async fn store_available_data(
		&mut self,
		id: Option<ValidatorIndex>,
		n_validators: u32,
		available_data: AvailableData,
	) -> Result<(), Error> {
		let (tx, rx) = oneshot::channel();
		self.tx_from.send(FromJob::AvailabilityStore(
				AvailabilityStoreMessage::StoreAvailableData(
					self.parent,
					id,
					n_validators,
					available_data,
					tx,
				)
			)
		).await?;

		rx.await?.map_err(|_| Error::StoreFailed)?;

		Ok(())
	}

	// Make a `PoV` available.
	//
	// This calls an inspection function before making the PoV available for any last checks
	// that need to be done. If the inspection function returns an error, this function returns
	// early without making the PoV available.
	async fn make_pov_available<T, E>(
		&mut self,
		pov: PoV,
		outputs: ValidationOutputs,
		with_commitments: impl FnOnce(CandidateCommitments) -> Result<T, E>,
	) -> Result<Result<T, E>, Error> {
		let omitted_validation = OmittedValidationData {
			global_validation: outputs.global_validation_data,
			local_validation: outputs.local_validation_data,
		};

		let available_data = AvailableData {
			pov,
			omitted_validation,
		};

		let chunks = erasure_coding::obtain_chunks_v1(
			self.table_context.validators.len(),
			&available_data,
		)?;

		let branches = erasure_coding::branches(chunks.as_ref());
		let erasure_root = branches.root();

		let commitments = CandidateCommitments {
			fees: outputs.fees,
			upward_messages: outputs.upward_messages,
			erasure_root,
			new_validation_code: outputs.new_validation_code,
			head_data: outputs.head_data,
		};

		let res = match with_commitments(commitments) {
			Ok(x) => x,
			Err(e) => return Ok(Err(e)),
		};

		self.store_available_data(
			self.table_context.validator.as_ref().map(|v| v.index()),
			self.table_context.validators.len() as u32,
			available_data,
		).await?;

		Ok(Ok(res))
	}

	async fn distribute_signed_statement(&mut self, s: SignedFullStatement) -> Result<(), Error> {
		let smsg = StatementDistributionMessage::Share(self.parent, s);

		self.tx_from.send(FromJob::StatementDistribution(smsg)).await?;

		Ok(())
	}
}

impl util::JobTrait for CandidateBackingJob {
	type ToJob = ToJob;
	type FromJob = FromJob;
	type Error = Error;
	type RunArgs = KeyStorePtr;

	const NAME: &'static str = "CandidateBackingJob";

	fn run(
		parent: Hash,
		keystore: KeyStorePtr,
		rx_to: mpsc::Receiver<Self::ToJob>,
		mut tx_from: mpsc::Sender<Self::FromJob>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		async move {
			let (validators, roster, signing_context) = futures::try_join!(
				request_validators(parent, &mut tx_from).await?,
				request_validator_groups(parent, &mut tx_from).await?,
				request_signing_context(parent, &mut tx_from).await?,
			)?;

			let validator = Validator::construct(&validators, signing_context, keystore.clone())?;

			let mut groups = HashMap::new();

			for assignment in roster.scheduled {
				if let Some(g) = roster.validator_groups.get(assignment.group_idx.0 as usize) {
					groups.insert(assignment.para_id, g.clone());
				}
			}

			let mut assignment = Default::default();

			if let Some(idx) = validators.iter().position(|k| *k == validator.id()) {
				let idx = idx as u32;
				for (para_id, group) in groups.iter() {
					if group.contains(&idx) {
						assignment = *para_id;
						break;
					}
				}
			}

			let table_context = TableContext {
				groups,
				validators,
				signing_context: validator.signing_context().clone(),
				validator: Some(validator),
			};

			let job = CandidateBackingJob {
				parent,
				rx_to,
				tx_from,
				assignment,
				issued_statements: HashSet::new(),
				seconded: None,
				reported_misbehavior_for: HashSet::new(),
				table: Table::default(),
				table_context,
			};

			job.run_loop().await
		}
		.boxed()
	}
}

/// Manager type for the CandidateBackingSubsystem
type Manager<Spawner, Context> = util::JobManager<Spawner, Context, CandidateBackingJob>;

/// An implementation of the Candidate Backing subsystem.
pub struct CandidateBackingSubsystem<Spawner, Context> {
	manager: Manager<Spawner, Context>,
}

impl<Spawner, Context> CandidateBackingSubsystem<Spawner, Context>
where
	Spawner: Clone + SpawnNamed + Send + Unpin,
	Context: SubsystemContext,
	ToJob: From<<Context as SubsystemContext>::Message>,
{
	/// Creates a new `CandidateBackingSubsystem`.
	pub fn new(spawner: Spawner, keystore: KeyStorePtr) -> Self {
		CandidateBackingSubsystem {
			manager: util::JobManager::new(spawner, keystore)
		}
	}

	/// Run this subsystem
	pub async fn run(ctx: Context, keystore: KeyStorePtr, spawner: Spawner) {
		<Manager<Spawner, Context>>::run(ctx, keystore, spawner, None).await
	}
}

impl<Spawner, Context> Subsystem<Context> for CandidateBackingSubsystem<Spawner, Context>
where
	Spawner: SpawnNamed + Send + Clone + Unpin + 'static,
	Context: SubsystemContext,
	<Context as SubsystemContext>::Message: Into<ToJob>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		self.manager.start(ctx)
	}
}



#[cfg(test)]
mod tests {
	use super::*;
	use assert_matches::assert_matches;
	use futures::{executor, future, Future};
	use polkadot_primitives::v1::{
		AssignmentKind, BlockData, CandidateCommitments, CollatorId, CoreAssignment, CoreIndex,
		LocalValidationData, GlobalValidationData, GroupIndex, HeadData,
		ValidatorPair, ValidityAttestation,
	};
	use polkadot_subsystem::{
		messages::{RuntimeApiRequest, SchedulerRoster},
		ActiveLeavesUpdate, FromOverseer, OverseerSignal,
	};
	use sp_keyring::Sr25519Keyring;
	use std::collections::HashMap;

	fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
		val_ids.iter().map(|v| v.public().into()).collect()
	}

	struct TestState {
		chain_ids: Vec<ParaId>,
		keystore: KeyStorePtr,
		validators: Vec<Sr25519Keyring>,
		validator_public: Vec<ValidatorId>,
		global_validation_data: GlobalValidationData,
		local_validation_data: LocalValidationData,
		roster: SchedulerRoster,
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

			let validator_groups = vec![vec![2, 0, 3], vec![1], vec![4]];

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

			let relay_parent = Hash::from([5; 32]);

			let local_validation_data = LocalValidationData {
				parent_head: HeadData(vec![7, 8, 9]),
				balance: Default::default(),
				code_upgrade_allowed: None,
				validation_code_hash: Default::default(),
			};

			let global_validation_data = GlobalValidationData {
				max_code_size: 1000,
				max_head_data_size: 1000,
				block_number: Default::default(),
			};

			Self {
				chain_ids,
				keystore,
				validators,
				validator_public,
				roster,
				head_data,
				local_validation_data,
				global_validation_data,
				signing_context,
				relay_parent,
			}
		}
	}

	struct TestHarness {
		virtual_overseer: polkadot_subsystem::test_helpers::TestSubsystemContextHandle<CandidateBackingMessage>,
	}

	fn test_harness<T: Future<Output=()>>(keystore: KeyStorePtr, test: impl FnOnce(TestHarness) -> T) {
		let pool = sp_core::testing::TaskExecutor::new();

		let (context, virtual_overseer) = polkadot_subsystem::test_helpers::make_subsystem_context(pool.clone());

		let subsystem = CandidateBackingSubsystem::run(context, keystore, pool.clone());

		let test_fut = test(TestHarness {
			virtual_overseer,
		});

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}

	fn make_erasure_root(test: &TestState, pov: PoV) -> Hash {
		let omitted_validation = OmittedValidationData {
			global_validation: test.global_validation_data.clone(),
			local_validation: test.local_validation_data.clone(),
		};

		let available_data = AvailableData {
			omitted_validation,
			pov,
		};

		let chunks = erasure_coding::obtain_chunks_v1(test.validators.len(), &available_data).unwrap();
		erasure_coding::branches(&chunks).root()
	}

	#[derive(Default)]
	struct TestCandidateBuilder {
		para_id: ParaId,
		head_data: HeadData,
		pov_hash: Hash,
		relay_parent: Hash,
		erasure_root: Hash,
	}

	impl TestCandidateBuilder {
		fn build(self) -> CommittedCandidateReceipt {
			CommittedCandidateReceipt {
				descriptor: CandidateDescriptor {
					para_id: self.para_id,
					pov_hash: self.pov_hash,
					relay_parent: self.relay_parent,
					..Default::default()
				},
				commitments: CandidateCommitments {
					head_data: self.head_data,
					erasure_root: self.erasure_root,
					..Default::default()
				},
			}
		}
	}

	// Tests that the subsystem performs actions that are requied on startup.
	async fn test_startup(
		virtual_overseer: &mut polkadot_subsystem::test_helpers::TestSubsystemContextHandle<CandidateBackingMessage>,
		test_state: &TestState,
	) {
		// Start work on some new parent.
		virtual_overseer.send(FromOverseer::Signal(
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(test_state.relay_parent)))
		).await;

		// Check that subsystem job issues a request for a validator set.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::Validators(tx))
			) if parent == test_state.relay_parent => {
				tx.send(test_state.validator_public.clone()).unwrap();
			}
		);

		// Check that subsystem job issues a request for the validator groups.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidatorGroups(tx))
			) if parent == test_state.relay_parent => {
				tx.send(test_state.roster.clone()).unwrap();
			}
		);

		// Check that subsystem job issues a request for the signing context.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::SigningContext(tx))
			) if parent == test_state.relay_parent => {
				tx.send(test_state.signing_context.clone()).unwrap();
			}
		);
	}

	// Test that a `CandidateBackingMessage::Second` issues validation work
	// and in case validation is successful issues a `StatementDistributionMessage`.
	#[test]
	fn backing_second_works() {
		let test_state = TestState::default();
		test_harness(test_state.keystore.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;

			test_startup(&mut virtual_overseer, &test_state).await;

			let pov = PoV {
				block_data: BlockData(vec![42, 43, 44]),
			};

			let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

			let pov_hash = pov.hash();
			let candidate = TestCandidateBuilder {
				para_id: test_state.chain_ids[0],
				relay_parent: test_state.relay_parent,
				pov_hash,
				head_data: expected_head_data.clone(),
				erasure_root: make_erasure_root(&test_state, pov.clone()),
				..Default::default()
			}.build();

			let second = CandidateBackingMessage::Second(
				test_state.relay_parent,
				candidate.to_plain(),
				pov.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: second }).await;


			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromChainState(
						c,
						pov,
						tx,
					)
				) if pov == pov && &c == candidate.descriptor() => {
					tx.send(Ok(
						ValidationResult::Valid(ValidationOutputs {
							global_validation_data: test_state.global_validation_data,
							local_validation_data: test_state.local_validation_data,
							head_data: expected_head_data.clone(),
							upward_messages: Vec::new(),
							fees: Default::default(),
							new_validation_code: None,
						}),
					)).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::AvailabilityStore(
					AvailabilityStoreMessage::StoreAvailableData(parent_hash, _, _, _, tx)
				) if parent_hash == test_state.relay_parent => {
					tx.send(Ok(())).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
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
			);

			virtual_overseer.send(FromOverseer::Signal(
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::stop_work(test_state.relay_parent)))
			).await;
		});
	}

	// Test that the candidate reaches quorum succesfully.
	#[test]
	fn backing_works() {
		let test_state = TestState::default();
		test_harness(test_state.keystore.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;

			test_startup(&mut virtual_overseer, &test_state).await;

			let pov = PoV {
				block_data: BlockData(vec![1, 2, 3]),
			};

			let pov_hash = pov.hash();

			let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

			let candidate_a = TestCandidateBuilder {
				para_id: test_state.chain_ids[0],
				relay_parent: test_state.relay_parent,
				pov_hash,
				head_data: expected_head_data.clone(),
				erasure_root: make_erasure_root(&test_state, pov.clone()),
				..Default::default()
			}.build();

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

			// Sending a `Statement::Seconded` for our assignment will start
			// validation process. The first thing requested is PoV from the
			// `PoVDistribution`.
			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::PoVDistribution(
					PoVDistributionMessage::FetchPoV(relay_parent, _, tx)
				) if relay_parent == test_state.relay_parent => {
					tx.send(Arc::new(pov.clone())).unwrap();
				}
			);

			// The next step is the actual request to Validation subsystem
			// to validate the `Seconded` candidate.
			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromChainState(
						c,
						pov,
						tx,
					)
				) if pov == pov && &c == candidate_a.descriptor() => {
					tx.send(Ok(
						ValidationResult::Valid(ValidationOutputs {
							global_validation_data: test_state.global_validation_data,
							local_validation_data: test_state.local_validation_data,
							head_data: expected_head_data.clone(),
							upward_messages: Vec::new(),
							fees: Default::default(),
							new_validation_code: None,
						}),
					)).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::AvailabilityStore(
					AvailabilityStoreMessage::StoreAvailableData(parent_hash, _, _, _, tx)
				) if parent_hash == test_state.relay_parent => {
					tx.send(Ok(())).unwrap();
				}
			);

			let statement = CandidateBackingMessage::Statement(
				test_state.relay_parent,
				signed_b.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			let (tx, rx) = oneshot::channel();

			// The backed candidats set should be not empty at this point.
			virtual_overseer.send(FromOverseer::Communication{
				msg: CandidateBackingMessage::GetBackedCandidates(
					test_state.relay_parent,
					tx,
				)
			}).await;

			let backed = rx.await.unwrap();

			// `validity_votes` may be in any order so we can't do this in a single assert.
			assert_eq!(backed[0].0.candidate, candidate_a);
			assert_eq!(backed[0].0.validity_votes.len(), 2);
			assert!(backed[0].0.validity_votes.contains(
				&ValidityAttestation::Explicit(signed_b.signature().clone())
			));
			assert!(backed[0].0.validity_votes.contains(
				&ValidityAttestation::Implicit(signed_a.signature().clone())
			));
			assert_eq!(backed[0].0.validator_indices, bitvec::bitvec![Lsb0, u8; 1, 1, 0]);

			virtual_overseer.send(FromOverseer::Signal(
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::stop_work(test_state.relay_parent)))
			).await;
		});
	}

	// Issuing conflicting statements on the same candidate should
	// be a misbehavior.
	#[test]
	fn backing_misbehavior_works() {
		let test_state = TestState::default();
		test_harness(test_state.keystore.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;

			test_startup(&mut virtual_overseer, &test_state).await;

			let pov = PoV {
				block_data: BlockData(vec![1, 2, 3]),
			};

			let pov_hash = pov.hash();

			let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

			let candidate_a = TestCandidateBuilder {
				para_id: test_state.chain_ids[0],
				relay_parent: test_state.relay_parent,
				pov_hash,
				erasure_root: make_erasure_root(&test_state, pov.clone()),
				head_data: expected_head_data.clone(),
				..Default::default()
			}.build();

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

			let signed_c = SignedFullStatement::sign(
				Statement::Invalid(candidate_a_hash),
				&test_state.signing_context,
				0,
				&test_state.validators[0].pair().into(),
			);

			let statement = CandidateBackingMessage::Statement(test_state.relay_parent, signed_a.clone());

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::PoVDistribution(
					PoVDistributionMessage::FetchPoV(relay_parent, _, tx)
				) if relay_parent == test_state.relay_parent => {
					tx.send(Arc::new(pov.clone())).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromChainState(
						c,
						pov,
						tx,
					)
				) if pov == pov && &c == candidate_a.descriptor() => {
					tx.send(Ok(
						ValidationResult::Valid(ValidationOutputs {
							global_validation_data: test_state.global_validation_data,
							local_validation_data: test_state.local_validation_data,
							head_data: expected_head_data.clone(),
							upward_messages: Vec::new(),
							fees: Default::default(),
							new_validation_code: None,
						}),
					)).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::AvailabilityStore(
					AvailabilityStoreMessage::StoreAvailableData(parent_hash, _, _, _, tx)
					) if parent_hash == test_state.relay_parent => {
						tx.send(Ok(())).unwrap();
					}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::Share(
						relay_parent,
						signed_statement,
					)
				) if relay_parent == test_state.relay_parent => {
					signed_statement.check_signature(
						&test_state.signing_context,
						&test_state.validator_public[0],
					).unwrap();

					assert_eq!(*signed_statement.payload(), Statement::Valid(candidate_a_hash));
				}
			);

			let statement = CandidateBackingMessage::Statement(test_state.relay_parent, signed_b.clone());

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			let statement = CandidateBackingMessage::Statement(test_state.relay_parent, signed_c.clone());

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::Provisioner(
					ProvisionerMessage::ProvisionableData(
						ProvisionableData::MisbehaviorReport(
							relay_parent,
							MisbehaviorReport::SelfContradiction(_, s1, s2),
						)
					)
				) if relay_parent == test_state.relay_parent => {
					s1.check_signature(
						&test_state.signing_context,
						&test_state.validator_public[s1.validator_index() as usize],
					).unwrap();

					s2.check_signature(
						&test_state.signing_context,
						&test_state.validator_public[s2.validator_index() as usize],
					).unwrap();
				}
			);
		});
	}

	// Test that if we are asked to second an invalid candidate we
	// can still second a valid one afterwards.
	#[test]
	fn backing_dont_second_invalid() {
		let test_state = TestState::default();
		test_harness(test_state.keystore.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;

			test_startup(&mut virtual_overseer, &test_state).await;

			let pov_block_a = PoV {
				block_data: BlockData(vec![42, 43, 44]),
			};

			let pov_block_b = PoV {
				block_data: BlockData(vec![45, 46, 47]),
			};

			let pov_hash_a = pov_block_a.hash();
			let pov_hash_b = pov_block_b.hash();

			let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

			let candidate_a = TestCandidateBuilder {
				para_id: test_state.chain_ids[0],
				relay_parent: test_state.relay_parent,
				pov_hash: pov_hash_a,
				erasure_root: make_erasure_root(&test_state, pov_block_a.clone()),
				..Default::default()
			}.build();

			let candidate_b = TestCandidateBuilder {
				para_id: test_state.chain_ids[0],
				relay_parent: test_state.relay_parent,
				pov_hash: pov_hash_b,
				erasure_root: make_erasure_root(&test_state, pov_block_b.clone()),
				head_data: expected_head_data.clone(),
				..Default::default()
			}.build();

			let second = CandidateBackingMessage::Second(
				test_state.relay_parent,
				candidate_a.to_plain(),
				pov_block_a.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: second }).await;


			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromChainState(
						c,
						pov,
						tx,
					)
				) if pov == pov && &c == candidate_a.descriptor() => {
					tx.send(Ok(ValidationResult::Invalid)).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateSelection(
					CandidateSelectionMessage::Invalid(parent_hash, c)
				) if parent_hash == test_state.relay_parent && c == candidate_a.to_plain()
			);

			let second = CandidateBackingMessage::Second(
				test_state.relay_parent,
				candidate_b.to_plain(),
				pov_block_b.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: second }).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromChainState(
						c,
						pov,
						tx,
					)
				) if pov == pov && &c == candidate_b.descriptor() => {
					tx.send(Ok(
						ValidationResult::Valid(ValidationOutputs {
							global_validation_data: test_state.global_validation_data,
							local_validation_data: test_state.local_validation_data,
							head_data: expected_head_data.clone(),
							upward_messages: Vec::new(),
							fees: Default::default(),
							new_validation_code: None,
						}),
					)).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::AvailabilityStore(
					AvailabilityStoreMessage::StoreAvailableData(parent_hash, _, _, _, tx)
				) if parent_hash == test_state.relay_parent => {
					tx.send(Ok(())).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
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

					assert_eq!(*signed_statement.payload(), Statement::Seconded(candidate_b));
				}
			);

			virtual_overseer.send(FromOverseer::Signal(
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::stop_work(test_state.relay_parent)))
			).await;
		});
	}

	// Test that if we have already issued a statement (in this case `Invalid`) about a
	// candidate we will not be issuing a `Seconded` statement on it.
	#[test]
	fn backing_multiple_statements_work() {
		let test_state = TestState::default();
		test_harness(test_state.keystore.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;

			test_startup(&mut virtual_overseer, &test_state).await;

			let pov = PoV {
				block_data: BlockData(vec![42, 43, 44]),
			};

			let pov_hash = pov.hash();

			let candidate = TestCandidateBuilder {
				para_id: test_state.chain_ids[0],
				relay_parent: test_state.relay_parent,
				pov_hash,
				erasure_root: make_erasure_root(&test_state, pov.clone()),
				..Default::default()
			}.build();

			let candidate_hash = candidate.hash();

			let signed_a = SignedFullStatement::sign(
				Statement::Seconded(candidate.clone()),
				&test_state.signing_context,
				2,
				&test_state.validators[2].pair().into(),
			);

			// Send in a `Statement` with a candidate.
			let statement = CandidateBackingMessage::Statement(
				test_state.relay_parent,
				signed_a.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			// Subsystem requests PoV and requests validation.
			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::PoVDistribution(
					PoVDistributionMessage::FetchPoV(relay_parent, _, tx)
				) => {
					assert_eq!(relay_parent, test_state.relay_parent);
					tx.send(Arc::new(pov.clone())).unwrap();
				}
			);


			// Tell subsystem that this candidate is invalid.
			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromChainState(
						c,
						pov,
						tx,
					)
				) if pov == pov && &c == candidate.descriptor() => {
					tx.send(Ok(ValidationResult::Invalid)).unwrap();
				}
			);

			// The invalid message is shared.
			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::Share(
						relay_parent,
						signed_statement,
					)
				) => {
					assert_eq!(relay_parent, test_state.relay_parent);
					signed_statement.check_signature(
						&test_state.signing_context,
						&test_state.validator_public[0],
					).unwrap();
					assert_eq!(*signed_statement.payload(), Statement::Invalid(candidate_hash));
				}
			);

			// Ask subsystem to `Second` a candidate that already has a statement issued about.
			// This should emit no actions from subsystem.
			let second = CandidateBackingMessage::Second(
				test_state.relay_parent,
				candidate.to_plain(),
				pov.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: second }).await;

			let pov_to_second = PoV {
				block_data: BlockData(vec![3, 2, 1]),
			};

			let pov_hash = pov_to_second.hash();

			let candidate_to_second = TestCandidateBuilder {
				para_id: test_state.chain_ids[0],
				relay_parent: test_state.relay_parent,
				pov_hash,
				erasure_root: make_erasure_root(&test_state, pov_to_second.clone()),
				..Default::default()
			}.build();

			let second = CandidateBackingMessage::Second(
				test_state.relay_parent,
				candidate_to_second.to_plain(),
				pov_to_second.clone(),
			);

			// In order to trigger _some_ actions from subsystem ask it to second another
			// candidate. The only reason to do so is to make sure that no actions were
			// triggered on the prev step.
			virtual_overseer.send(FromOverseer::Communication{ msg: second }).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromChainState(
						_,
						pov,
						_,
					)
				) => {
					assert_eq!(&*pov, &pov_to_second);
				}
			);
		});
	}

	// That that if the validation of the candidate has failed this does not stop
	// the work of this subsystem and so it is not fatal to the node.
	#[test]
	fn backing_works_after_failed_validation() {
		let test_state = TestState::default();
		test_harness(test_state.keystore.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;

			test_startup(&mut virtual_overseer, &test_state).await;

			let pov = PoV {
				block_data: BlockData(vec![42, 43, 44]),
			};

			let pov_hash = pov.hash();

			let candidate = TestCandidateBuilder {
				para_id: test_state.chain_ids[0],
				relay_parent: test_state.relay_parent,
				pov_hash,
				erasure_root: make_erasure_root(&test_state, pov.clone()),
				..Default::default()
			}.build();

			let signed_a = SignedFullStatement::sign(
				Statement::Seconded(candidate.clone()),
				&test_state.signing_context,
				2,
				&test_state.validators[2].pair().into(),
			);

			// Send in a `Statement` with a candidate.
			let statement = CandidateBackingMessage::Statement(
				test_state.relay_parent,
				signed_a.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			// Subsystem requests PoV and requests validation.
			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::PoVDistribution(
					PoVDistributionMessage::FetchPoV(relay_parent, _, tx)
				) => {
					assert_eq!(relay_parent, test_state.relay_parent);
					tx.send(Arc::new(pov.clone())).unwrap();
				}
			);

			// Tell subsystem that this candidate is invalid.
			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateValidation(
					CandidateValidationMessage::ValidateFromChainState(
						c,
						pov,
						tx,
					)
				) if pov == pov && &c == candidate.descriptor() => {
					tx.send(Err(ValidationFailed)).unwrap();
				}
			);

			// Try to get a set of backable candidates to trigger _some_ action in the subsystem
			// and check that it is still alive.
			let (tx, rx) = oneshot::channel();
			let msg = CandidateBackingMessage::GetBackedCandidates(
				test_state.relay_parent,
				tx,
			);

			virtual_overseer.send(FromOverseer::Communication{ msg }).await;
			assert_eq!(rx.await.unwrap().len(), 0);
		});
	}
}
