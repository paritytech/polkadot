// Copyright 2020-2021 Parity Technologies (UK) Ltd.
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

#![deny(unused_crate_dependencies)]

use std::{
	collections::{HashMap, HashSet},
	pin::Pin,
	sync::Arc,
};

use bitvec::vec::BitVec;
use futures::{
	channel::{mpsc, oneshot},
	Future, FutureExt, SinkExt, StreamExt,
};

use polkadot_node_primitives::{
	AvailableData, PoV, SignedDisputeStatement, SignedFullStatement, Statement, ValidationResult,
};
use polkadot_node_subsystem_util::{
	self as util,
	metrics::{self, prometheus},
	request_from_runtime, request_session_index_for_child, request_validator_groups,
	request_validators, FromJobCommand, JobSender, Validator,
};
use polkadot_primitives::v1::{
	BackedCandidate, CandidateCommitments, CandidateDescriptor, CandidateHash, CandidateReceipt,
	CollatorId, CommittedCandidateReceipt, CoreIndex, CoreState, Hash, Id as ParaId, SessionIndex,
	SigningContext, ValidatorId, ValidatorIndex, ValidatorSignature, ValidityAttestation,
};
use polkadot_subsystem::{
	jaeger,
	messages::{
		AllMessages, AvailabilityDistributionMessage, AvailabilityStoreMessage,
		CandidateBackingMessage, CandidateValidationMessage, CollatorProtocolMessage,
		DisputeCoordinatorMessage, ImportStatementsResult, ProvisionableData, ProvisionerMessage,
		RuntimeApiRequest, StatementDistributionMessage, ValidationFailed,
	},
	overseer, PerLeafSpan, Stage, SubsystemSender,
};
use sp_keystore::SyncCryptoStorePtr;
use statement_table::{
	generic::AttestedCandidate as TableAttestedCandidate,
	v1::{
		SignedStatement as TableSignedStatement, Statement as TableStatement,
		Summary as TableSummary,
	},
	Context as TableContextTrait, Table,
};
use thiserror::Error;

#[cfg(test)]
mod tests;

const LOG_TARGET: &str = "parachain::candidate-backing";

/// Errors that can occur in candidate backing.
#[derive(Debug, Error)]
pub enum Error {
	#[error("Candidate is not found")]
	CandidateNotFound,
	#[error("Signature is invalid")]
	InvalidSignature,
	#[error("Failed to send candidates {0:?}")]
	Send(Vec<BackedCandidate>),
	#[error("FetchPoV failed")]
	FetchPoV,
	#[error("ValidateFromChainState channel closed before receipt")]
	ValidateFromChainState(#[source] oneshot::Canceled),
	#[error("StoreAvailableData channel closed before receipt")]
	StoreAvailableData(#[source] oneshot::Canceled),
	#[error("a channel was closed before receipt in try_join!")]
	JoinMultiple(#[source] oneshot::Canceled),
	#[error("Obtaining erasure chunks failed")]
	ObtainErasureChunks(#[from] erasure_coding::Error),
	#[error(transparent)]
	ValidationFailed(#[from] ValidationFailed),
	#[error(transparent)]
	Mpsc(#[from] mpsc::SendError),
	#[error(transparent)]
	UtilError(#[from] util::Error),
}

/// PoV data to validate.
enum PoVData {
	/// Already available (from candidate selection).
	Ready(Arc<PoV>),
	/// Needs to be fetched from validator (we are checking a signed statement).
	FetchFromValidator {
		from_validator: ValidatorIndex,
		candidate_hash: CandidateHash,
		pov_hash: Hash,
	},
}

enum ValidatedCandidateCommand {
	// We were instructed to second the candidate that has been already validated.
	Second(BackgroundValidationResult),
	// We were instructed to validate the candidate.
	Attest(BackgroundValidationResult),
	// We were not able to `Attest` because backing validator did not send us the PoV.
	AttestNoPoV(CandidateHash),
}

impl std::fmt::Debug for ValidatedCandidateCommand {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		let candidate_hash = self.candidate_hash();
		match *self {
			ValidatedCandidateCommand::Second(_) => write!(f, "Second({})", candidate_hash),
			ValidatedCandidateCommand::Attest(_) => write!(f, "Attest({})", candidate_hash),
			ValidatedCandidateCommand::AttestNoPoV(_) => write!(f, "Attest({})", candidate_hash),
		}
	}
}

impl ValidatedCandidateCommand {
	fn candidate_hash(&self) -> CandidateHash {
		match *self {
			ValidatedCandidateCommand::Second(Ok((ref candidate, _, _))) => candidate.hash(),
			ValidatedCandidateCommand::Second(Err(ref candidate)) => candidate.hash(),
			ValidatedCandidateCommand::Attest(Ok((ref candidate, _, _))) => candidate.hash(),
			ValidatedCandidateCommand::Attest(Err(ref candidate)) => candidate.hash(),
			ValidatedCandidateCommand::AttestNoPoV(candidate_hash) => candidate_hash,
		}
	}
}

/// Holds all data needed for candidate backing job operation.
pub struct CandidateBackingJob {
	/// The hash of the relay parent on top of which this job is doing it's work.
	parent: Hash,
	/// The session index this corresponds to.
	session_index: SessionIndex,
	/// The `ParaId` assigned to this validator
	assignment: Option<ParaId>,
	/// The collator required to author the candidate, if any.
	required_collator: Option<CollatorId>,
	/// Spans for all candidates that are not yet backable.
	unbacked_candidates: HashMap<CandidateHash, jaeger::Span>,
	/// We issued `Seconded`, `Valid` or `Invalid` statements on about these candidates.
	issued_statements: HashSet<CandidateHash>,
	/// These candidates are undergoing validation in the background.
	awaiting_validation: HashSet<CandidateHash>,
	/// Data needed for retrying in case of `ValidatedCandidateCommand::AttestNoPoV`.
	fallbacks: HashMap<CandidateHash, (AttestingData, Option<jaeger::Span>)>,
	/// `Some(h)` if this job has already issued `Seconded` statement for some candidate with `h` hash.
	seconded: Option<CandidateHash>,
	/// The candidates that are includable, by hash. Each entry here indicates
	/// that we've sent the provisioner the backed candidate.
	backed: HashSet<CandidateHash>,
	keystore: SyncCryptoStorePtr,
	table: Table<TableContext>,
	table_context: TableContext,
	background_validation: mpsc::Receiver<ValidatedCandidateCommand>,
	background_validation_tx: mpsc::Sender<ValidatedCandidateCommand>,
	metrics: Metrics,
}

/// In case a backing validator does not provide a PoV, we need to retry with other backing
/// validators.
///
/// This is the data needed to accomplish this. Basically all the data needed for spawning a
/// validation job and a list of backing validators, we can try.
#[derive(Clone)]
struct AttestingData {
	/// The candidate to attest.
	candidate: CandidateReceipt,
	/// Hash of the PoV we need to fetch.
	pov_hash: Hash,
	/// Validator we are currently trying to get the PoV from.
	from_validator: ValidatorIndex,
	/// Other backing validators we can try in case `from_validator` failed.
	backing: Vec<ValidatorIndex>,
}

const fn group_quorum(n_validators: usize) -> usize {
	(n_validators / 2) + 1
}

#[derive(Default)]
struct TableContext {
	validator: Option<Validator>,
	groups: HashMap<ParaId, Vec<ValidatorIndex>>,
	validators: Vec<ValidatorId>,
}

impl TableContextTrait for TableContext {
	type AuthorityId = ValidatorIndex;
	type Digest = CandidateHash;
	type GroupId = ParaId;
	type Signature = ValidatorSignature;
	type Candidate = CommittedCandidateReceipt;

	fn candidate_digest(candidate: &CommittedCandidateReceipt) -> CandidateHash {
		candidate.hash()
	}

	fn candidate_group(candidate: &CommittedCandidateReceipt) -> ParaId {
		candidate.descriptor().para_id
	}

	fn is_member_of(&self, authority: &ValidatorIndex, group: &ParaId) -> bool {
		self.groups
			.get(group)
			.map_or(false, |g| g.iter().position(|a| a == authority).is_some())
	}

	fn requisite_votes(&self, group: &ParaId) -> usize {
		self.groups.get(group).map_or(usize::MAX, |g| group_quorum(g.len()))
	}
}

struct InvalidErasureRoot;

// It looks like it's not possible to do an `impl From` given the current state of
// the code. So this does the necessary conversion.
fn primitive_statement_to_table(s: &SignedFullStatement) -> TableSignedStatement {
	let statement = match s.payload() {
		Statement::Seconded(c) => TableStatement::Seconded(c.clone()),
		Statement::Valid(h) => TableStatement::Valid(h.clone()),
	};

	TableSignedStatement {
		statement,
		signature: s.signature().clone(),
		sender: s.validator_index(),
	}
}

fn table_attested_to_backed(
	attested: TableAttestedCandidate<
		ParaId,
		CommittedCandidateReceipt,
		ValidatorIndex,
		ValidatorSignature,
	>,
	table_context: &TableContext,
) -> Option<BackedCandidate> {
	let TableAttestedCandidate { candidate, validity_votes, group_id: para_id } = attested;

	let (ids, validity_votes): (Vec<_>, Vec<ValidityAttestation>) =
		validity_votes.into_iter().map(|(id, vote)| (id, vote.into())).unzip();

	let group = table_context.groups.get(&para_id)?;

	let mut validator_indices = BitVec::with_capacity(group.len());

	validator_indices.resize(group.len(), false);

	// The order of the validity votes in the backed candidate must match
	// the order of bits set in the bitfield, which is not necessarily
	// the order of the `validity_votes` we got from the table.
	let mut vote_positions = Vec::with_capacity(validity_votes.len());
	for (orig_idx, id) in ids.iter().enumerate() {
		if let Some(position) = group.iter().position(|x| x == id) {
			validator_indices.set(position, true);
			vote_positions.push((orig_idx, position));
		} else {
			tracing::warn!(
				target: LOG_TARGET,
				"Logic error: Validity vote from table does not correspond to group",
			);

			return None
		}
	}
	vote_positions.sort_by_key(|(_orig, pos_in_group)| *pos_in_group);

	Some(BackedCandidate {
		candidate,
		validity_votes: vote_positions
			.into_iter()
			.map(|(pos_in_votes, _pos_in_group)| validity_votes[pos_in_votes].clone())
			.collect(),
		validator_indices,
	})
}

async fn store_available_data(
	sender: &mut JobSender<impl SubsystemSender>,
	id: Option<ValidatorIndex>,
	n_validators: u32,
	candidate_hash: CandidateHash,
	available_data: AvailableData,
) -> Result<(), Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(AvailabilityStoreMessage::StoreAvailableData(
			candidate_hash,
			id,
			n_validators,
			available_data,
			tx,
		))
		.await;

	let _ = rx.await.map_err(Error::StoreAvailableData)?;

	Ok(())
}

// Make a `PoV` available.
//
// This will compute the erasure root internally and compare it to the expected erasure root.
// This returns `Err()` iff there is an internal error. Otherwise, it returns either `Ok(Ok(()))` or `Ok(Err(_))`.
async fn make_pov_available(
	sender: &mut JobSender<impl SubsystemSender>,
	validator_index: Option<ValidatorIndex>,
	n_validators: usize,
	pov: Arc<PoV>,
	candidate_hash: CandidateHash,
	validation_data: polkadot_primitives::v1::PersistedValidationData,
	expected_erasure_root: Hash,
	span: Option<&jaeger::Span>,
) -> Result<Result<(), InvalidErasureRoot>, Error> {
	let available_data = AvailableData { pov, validation_data };

	{
		let _span = span.as_ref().map(|s| s.child("erasure-coding").with_candidate(candidate_hash));

		let chunks = erasure_coding::obtain_chunks_v1(n_validators, &available_data)?;

		let branches = erasure_coding::branches(chunks.as_ref());
		let erasure_root = branches.root();

		if erasure_root != expected_erasure_root {
			return Ok(Err(InvalidErasureRoot))
		}
	}

	{
		let _span = span.as_ref().map(|s| s.child("store-data").with_candidate(candidate_hash));

		store_available_data(
			sender,
			validator_index,
			n_validators as u32,
			candidate_hash,
			available_data,
		)
		.await?;
	}

	Ok(Ok(()))
}

async fn request_pov(
	sender: &mut JobSender<impl SubsystemSender>,
	relay_parent: Hash,
	from_validator: ValidatorIndex,
	candidate_hash: CandidateHash,
	pov_hash: Hash,
) -> Result<Arc<PoV>, Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(AvailabilityDistributionMessage::FetchPoV {
			relay_parent,
			from_validator,
			candidate_hash,
			pov_hash,
			tx,
		})
		.await;

	let pov = rx.await.map_err(|_| Error::FetchPoV)?;
	Ok(Arc::new(pov))
}

async fn request_candidate_validation(
	sender: &mut JobSender<impl SubsystemSender>,
	candidate: CandidateDescriptor,
	pov: Arc<PoV>,
) -> Result<ValidationResult, Error> {
	let (tx, rx) = oneshot::channel();

	sender
		.send_message(CandidateValidationMessage::ValidateFromChainState(candidate, pov, tx))
		.await;

	match rx.await {
		Ok(Ok(validation_result)) => Ok(validation_result),
		Ok(Err(err)) => Err(Error::ValidationFailed(err)),
		Err(err) => Err(Error::ValidateFromChainState(err)),
	}
}

type BackgroundValidationResult =
	Result<(CandidateReceipt, CandidateCommitments, Arc<PoV>), CandidateReceipt>;

struct BackgroundValidationParams<S: overseer::SubsystemSender<AllMessages>, F> {
	sender: JobSender<S>,
	tx_command: mpsc::Sender<ValidatedCandidateCommand>,
	candidate: CandidateReceipt,
	relay_parent: Hash,
	pov: PoVData,
	validator_index: Option<ValidatorIndex>,
	n_validators: usize,
	span: Option<jaeger::Span>,
	make_command: F,
}

async fn validate_and_make_available(
	params: BackgroundValidationParams<
		impl SubsystemSender,
		impl Fn(BackgroundValidationResult) -> ValidatedCandidateCommand + Sync,
	>,
) -> Result<(), Error> {
	let BackgroundValidationParams {
		mut sender,
		mut tx_command,
		candidate,
		relay_parent,
		pov,
		validator_index,
		n_validators,
		span,
		make_command,
	} = params;

	let pov = match pov {
		PoVData::Ready(pov) => pov,
		PoVData::FetchFromValidator { from_validator, candidate_hash, pov_hash } => {
			let _span = span.as_ref().map(|s| s.child("request-pov"));
			match request_pov(&mut sender, relay_parent, from_validator, candidate_hash, pov_hash)
				.await
			{
				Err(Error::FetchPoV) => {
					tx_command
						.send(ValidatedCandidateCommand::AttestNoPoV(candidate.hash()))
						.await
						.map_err(Error::Mpsc)?;
					return Ok(())
				},
				Err(err) => return Err(err),
				Ok(pov) => pov,
			}
		},
	};

	let v = {
		let _span = span.as_ref().map(|s| {
			s.child("request-validation")
				.with_pov(&pov)
				.with_para_id(candidate.descriptor().para_id)
		});
		request_candidate_validation(&mut sender, candidate.descriptor.clone(), pov.clone()).await?
	};

	let expected_commitments_hash = candidate.commitments_hash;

	let res = match v {
		ValidationResult::Valid(commitments, validation_data) => {
			tracing::debug!(
				target: LOG_TARGET,
				candidate_hash = ?candidate.hash(),
				"Validation successful",
			);

			// If validation produces a new set of commitments, we vote the candidate as invalid.
			if commitments.hash() != expected_commitments_hash {
				tracing::debug!(
					target: LOG_TARGET,
					candidate_hash = ?candidate.hash(),
					actual_commitments = ?commitments,
					"Commitments obtained with validation don't match the announced by the candidate receipt",
				);
				Err(candidate)
			} else {
				let erasure_valid = make_pov_available(
					&mut sender,
					validator_index,
					n_validators,
					pov.clone(),
					candidate.hash(),
					validation_data,
					candidate.descriptor.erasure_root,
					span.as_ref(),
				)
				.await?;

				match erasure_valid {
					Ok(()) => Ok((candidate, commitments, pov.clone())),
					Err(InvalidErasureRoot) => {
						tracing::debug!(
							target: LOG_TARGET,
							candidate_hash = ?candidate.hash(),
							actual_commitments = ?commitments,
							"Erasure root doesn't match the announced by the candidate receipt",
						);
						Err(candidate)
					},
				}
			}
		},
		ValidationResult::Invalid(reason) => {
			tracing::debug!(
				target: LOG_TARGET,
				candidate_hash = ?candidate.hash(),
				reason = ?reason,
				"Validation yielded an invalid candidate",
			);
			Err(candidate)
		},
	};

	tx_command.send(make_command(res)).await.map_err(Into::into)
}

struct ValidatorIndexOutOfBounds;

impl CandidateBackingJob {
	/// Run asynchronously.
	async fn run_loop(
		mut self,
		mut sender: JobSender<impl SubsystemSender>,
		mut rx_to: mpsc::Receiver<CandidateBackingMessage>,
		span: PerLeafSpan,
	) -> Result<(), Error> {
		loop {
			futures::select! {
				validated_command = self.background_validation.next() => {
					let _span = span.child("process-validation-result");
					if let Some(c) = validated_command {
						self.handle_validated_candidate_command(&span, &mut sender, c).await?;
					} else {
						panic!("`self` hasn't dropped and `self` holds a reference to this sender; qed");
					}
				}
				to_job = rx_to.next() => match to_job {
					None => break,
					Some(msg) => {
						// we intentionally want spans created in `process_msg` to descend from the
						// `span ` which is longer-lived than this ephemeral timing span.
						let _timing_span = span.child("process-message");
						self.process_msg(&span, &mut sender, msg).await?;
					}
				}
			}
		}

		Ok(())
	}

	async fn handle_validated_candidate_command(
		&mut self,
		root_span: &jaeger::Span,
		sender: &mut JobSender<impl SubsystemSender>,
		command: ValidatedCandidateCommand,
	) -> Result<(), Error> {
		let candidate_hash = command.candidate_hash();
		self.awaiting_validation.remove(&candidate_hash);

		match command {
			ValidatedCandidateCommand::Second(res) => {
				match res {
					Ok((candidate, commitments, _)) => {
						// sanity check.
						if self.seconded.is_none() &&
							!self.issued_statements.contains(&candidate_hash)
						{
							self.seconded = Some(candidate_hash);
							self.issued_statements.insert(candidate_hash);
							self.metrics.on_candidate_seconded();

							let statement = Statement::Seconded(CommittedCandidateReceipt {
								descriptor: candidate.descriptor.clone(),
								commitments,
							});
							if let Some(stmt) = self
								.sign_import_and_distribute_statement(sender, statement, root_span)
								.await?
							{
								sender
									.send_message(CollatorProtocolMessage::Seconded(
										self.parent,
										stmt,
									))
									.await;
							}
						}
					},
					Err(candidate) => {
						sender
							.send_message(CollatorProtocolMessage::Invalid(self.parent, candidate))
							.await;
					},
				}
			},
			ValidatedCandidateCommand::Attest(res) => {
				// We are done - avoid new validation spawns:
				self.fallbacks.remove(&candidate_hash);
				// sanity check.
				if !self.issued_statements.contains(&candidate_hash) {
					if res.is_ok() {
						let statement = Statement::Valid(candidate_hash);
						self.sign_import_and_distribute_statement(sender, statement, &root_span)
							.await?;
					}
					self.issued_statements.insert(candidate_hash);
				}
			},
			ValidatedCandidateCommand::AttestNoPoV(candidate_hash) => {
				if let Some((attesting, span)) = self.fallbacks.get_mut(&candidate_hash) {
					if let Some(index) = attesting.backing.pop() {
						attesting.from_validator = index;
						// Ok, another try:
						let c_span = span.as_ref().map(|s| s.child("try"));
						let attesting = attesting.clone();
						self.kick_off_validation_work(sender, attesting, c_span).await?
					}
				} else {
					tracing::warn!(
						target: LOG_TARGET,
						"AttestNoPoV was triggered without fallback being available."
					);
					debug_assert!(false);
				}
			},
		}

		Ok(())
	}

	async fn background_validate_and_make_available(
		&mut self,
		sender: &mut JobSender<impl SubsystemSender>,
		params: BackgroundValidationParams<
			impl SubsystemSender,
			impl Fn(BackgroundValidationResult) -> ValidatedCandidateCommand + Send + 'static + Sync,
		>,
	) -> Result<(), Error> {
		let candidate_hash = params.candidate.hash();
		if self.awaiting_validation.insert(candidate_hash) {
			// spawn background task.
			let bg = async move {
				if let Err(e) = validate_and_make_available(params).await {
					tracing::error!(
						target: LOG_TARGET,
						"Failed to validate and make available: {:?}",
						e
					);
				}
			};
			sender
				.send_command(FromJobCommand::Spawn("Backing Validation", bg.boxed()))
				.await?;
		}

		Ok(())
	}

	/// Kick off background validation with intent to second.
	async fn validate_and_second(
		&mut self,
		parent_span: &jaeger::Span,
		root_span: &jaeger::Span,
		sender: &mut JobSender<impl SubsystemSender>,
		candidate: &CandidateReceipt,
		pov: Arc<PoV>,
	) -> Result<(), Error> {
		// Check that candidate is collated by the right collator.
		if self
			.required_collator
			.as_ref()
			.map_or(false, |c| c != &candidate.descriptor().collator)
		{
			sender
				.send_message(CollatorProtocolMessage::Invalid(self.parent, candidate.clone()))
				.await;
			return Ok(())
		}

		let candidate_hash = candidate.hash();
		let mut span = self.get_unbacked_validation_child(
			root_span,
			candidate_hash,
			candidate.descriptor().para_id,
		);

		span.as_mut().map(|span| span.add_follows_from(parent_span));

		tracing::debug!(
			target: LOG_TARGET,
			candidate_hash = ?candidate_hash,
			candidate_receipt = ?candidate,
			"Validate and second candidate",
		);

		let bg_sender = sender.clone();
		self.background_validate_and_make_available(
			sender,
			BackgroundValidationParams {
				sender: bg_sender,
				tx_command: self.background_validation_tx.clone(),
				candidate: candidate.clone(),
				relay_parent: self.parent,
				pov: PoVData::Ready(pov),
				validator_index: self.table_context.validator.as_ref().map(|v| v.index()),
				n_validators: self.table_context.validators.len(),
				span,
				make_command: ValidatedCandidateCommand::Second,
			},
		)
		.await?;

		Ok(())
	}

	async fn sign_import_and_distribute_statement(
		&mut self,
		sender: &mut JobSender<impl SubsystemSender>,
		statement: Statement,
		root_span: &jaeger::Span,
	) -> Result<Option<SignedFullStatement>, Error> {
		if let Some(signed_statement) = self.sign_statement(statement).await {
			self.import_statement(sender, &signed_statement, root_span).await?;
			let smsg = StatementDistributionMessage::Share(self.parent, signed_statement.clone());
			sender.send_unbounded_message(smsg);

			Ok(Some(signed_statement))
		} else {
			Ok(None)
		}
	}

	/// Check if there have happened any new misbehaviors and issue necessary messages.
	async fn issue_new_misbehaviors(&mut self, sender: &mut JobSender<impl SubsystemSender>) {
		// collect the misbehaviors to avoid double mutable self borrow issues
		let misbehaviors: Vec<_> = self.table.drain_misbehaviors().collect();
		for (validator_id, report) in misbehaviors {
			sender
				.send_message(ProvisionerMessage::ProvisionableData(
					self.parent,
					ProvisionableData::MisbehaviorReport(self.parent, validator_id, report),
				))
				.await;
		}
	}

	/// Import a statement into the statement table and return the summary of the import.
	async fn import_statement(
		&mut self,
		sender: &mut JobSender<impl SubsystemSender>,
		statement: &SignedFullStatement,
		root_span: &jaeger::Span,
	) -> Result<Option<TableSummary>, Error> {
		tracing::debug!(
			target: LOG_TARGET,
			statement = ?statement.payload().to_compact(),
			validator_index = statement.validator_index().0,
			"Importing statement",
		);

		let candidate_hash = statement.payload().candidate_hash();
		let import_statement_span = {
			// create a span only for candidates we're already aware of.
			self.get_unbacked_statement_child(
				root_span,
				candidate_hash,
				statement.validator_index(),
			)
		};

		if let Err(ValidatorIndexOutOfBounds) = self
			.dispatch_new_statement_to_dispute_coordinator(sender, candidate_hash, &statement)
			.await
		{
			tracing::warn!(
				target: LOG_TARGET,
				session_index = ?self.session_index,
				relay_parent = ?self.parent,
				validator_index = statement.validator_index().0,
				"Supposedly 'Signed' statement has validator index out of bounds."
			);

			return Ok(None)
		}

		let stmt = primitive_statement_to_table(statement);

		let summary = self.table.import_statement(&self.table_context, stmt);

		let unbacked_span = if let Some(attested) = summary
			.as_ref()
			.and_then(|s| self.table.attested_candidate(&s.candidate, &self.table_context))
		{
			let candidate_hash = attested.candidate.hash();
			// `HashSet::insert` returns true if the thing wasn't in there already.
			if self.backed.insert(candidate_hash) {
				let span = self.remove_unbacked_span(&candidate_hash);

				if let Some(backed) = table_attested_to_backed(attested, &self.table_context) {
					tracing::debug!(
						target: LOG_TARGET,
						candidate_hash = ?candidate_hash,
						relay_parent = ?self.parent,
						para_id = %backed.candidate.descriptor.para_id,
						"Candidate backed",
					);

					let message = ProvisionerMessage::ProvisionableData(
						self.parent,
						ProvisionableData::BackedCandidate(backed.receipt()),
					);
					sender.send_message(message).await;

					span.as_ref().map(|s| s.child("backed"));
					span
				} else {
					None
				}
			} else {
				None
			}
		} else {
			None
		};

		self.issue_new_misbehaviors(sender).await;

		// It is important that the child span is dropped before its parent span (`unbacked_span`)
		drop(import_statement_span);
		drop(unbacked_span);

		Ok(summary)
	}

	/// The dispute coordinator keeps track of all statements by validators about every recent
	/// candidate.
	///
	/// When importing a statement, this should be called access the candidate receipt either
	/// from the statement itself or from the underlying statement table in order to craft
	/// and dispatch the notification to the dispute coordinator.
	///
	/// This also does bounds-checking on the validator index and will return an error if the
	/// validator index is out of bounds for the current validator set. It's expected that
	/// this should never happen due to the interface of the candidate backing subsystem -
	/// the networking component responsible for feeding statements to the backing subsystem
	/// is meant to check the signature and provenance of all statements before submission.
	async fn dispatch_new_statement_to_dispute_coordinator(
		&self,
		sender: &mut JobSender<impl SubsystemSender>,
		candidate_hash: CandidateHash,
		statement: &SignedFullStatement,
	) -> Result<(), ValidatorIndexOutOfBounds> {
		// Dispatch the statement to the dispute coordinator.
		let validator_index = statement.validator_index();
		let signing_context =
			SigningContext { parent_hash: self.parent, session_index: self.session_index };

		let validator_public = match self.table_context.validators.get(validator_index.0 as usize) {
			None => {
				return Err(ValidatorIndexOutOfBounds)
			},
			Some(v) => v,
		};

		let maybe_candidate_receipt = match statement.payload() {
			Statement::Seconded(receipt) => Some(receipt.to_plain()),
			Statement::Valid(candidate_hash) => {
				// Valid statements are only supposed to be imported
				// once we've seen at least one `Seconded` statement.
				self.table.get_candidate(&candidate_hash).map(|c| c.to_plain())
			},
		};

		let maybe_signed_dispute_statement = SignedDisputeStatement::from_backing_statement(
			statement.as_unchecked(),
			signing_context,
			validator_public.clone(),
		)
		.ok();

		if let (Some(candidate_receipt), Some(dispute_statement)) =
			(maybe_candidate_receipt, maybe_signed_dispute_statement)
		{
			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			sender
				.send_message(DisputeCoordinatorMessage::ImportStatements {
					candidate_hash,
					candidate_receipt,
					session: self.session_index,
					statements: vec![(dispute_statement, validator_index)],
					pending_confirmation,
				})
				.await;

			match confirmation_rx.await {
				Err(oneshot::Canceled) =>
					tracing::warn!(target: LOG_TARGET, "Dispute coordinator confirmation lost",),
				Ok(ImportStatementsResult::ValidImport) => {},
				Ok(ImportStatementsResult::InvalidImport) =>
					tracing::warn!(target: LOG_TARGET, "Failed to import statements of validity",),
			}
		}

		Ok(())
	}

	async fn process_msg(
		&mut self,
		root_span: &jaeger::Span,
		sender: &mut JobSender<impl SubsystemSender>,
		msg: CandidateBackingMessage,
	) -> Result<(), Error> {
		match msg {
			CandidateBackingMessage::Second(relay_parent, candidate, pov) => {
				let _timer = self.metrics.time_process_second();

				let span = root_span
					.child("second")
					.with_stage(jaeger::Stage::CandidateBacking)
					.with_pov(&pov)
					.with_candidate(candidate.hash())
					.with_relay_parent(relay_parent);

				// Sanity check that candidate is from our assignment.
				if Some(candidate.descriptor().para_id) != self.assignment {
					return Ok(())
				}

				// If the message is a `CandidateBackingMessage::Second`, sign and dispatch a
				// Seconded statement only if we have not seconded any other candidate and
				// have not signed a Valid statement for the requested candidate.
				if self.seconded.is_none() {
					// This job has not seconded a candidate yet.
					let candidate_hash = candidate.hash();

					if !self.issued_statements.contains(&candidate_hash) {
						let pov = Arc::new(pov);
						self.validate_and_second(&span, &root_span, sender, &candidate, pov)
							.await?;
					}
				}
			},
			CandidateBackingMessage::Statement(_relay_parent, statement) => {
				let _timer = self.metrics.time_process_statement();
				let _span = root_span
					.child("statement")
					.with_stage(jaeger::Stage::CandidateBacking)
					.with_candidate(statement.payload().candidate_hash())
					.with_relay_parent(_relay_parent);

				match self.maybe_validate_and_import(&root_span, sender, statement).await {
					Err(Error::ValidationFailed(_)) => return Ok(()),
					Err(e) => return Err(e),
					Ok(()) => (),
				}
			},
			CandidateBackingMessage::GetBackedCandidates(_, requested_candidates, tx) => {
				let _timer = self.metrics.time_get_backed_candidates();

				let backed = requested_candidates
					.into_iter()
					.filter_map(|hash| {
						self.table.attested_candidate(&hash, &self.table_context).and_then(
							|attested| table_attested_to_backed(attested, &self.table_context),
						)
					})
					.collect();

				tx.send(backed).map_err(|data| Error::Send(data))?;
			},
		}

		Ok(())
	}

	/// Kick off validation work and distribute the result as a signed statement.
	async fn kick_off_validation_work(
		&mut self,
		sender: &mut JobSender<impl SubsystemSender>,
		attesting: AttestingData,
		span: Option<jaeger::Span>,
	) -> Result<(), Error> {
		let candidate_hash = attesting.candidate.hash();
		if self.issued_statements.contains(&candidate_hash) {
			return Ok(())
		}

		let descriptor = attesting.candidate.descriptor().clone();

		tracing::debug!(
			target: LOG_TARGET,
			candidate_hash = ?candidate_hash,
			candidate_receipt = ?attesting.candidate,
			"Kicking off validation",
		);

		// Check that candidate is collated by the right collator.
		if self.required_collator.as_ref().map_or(false, |c| c != &descriptor.collator) {
			// If not, we've got the statement in the table but we will
			// not issue validation work for it.
			//
			// Act as though we've issued a statement.
			self.issued_statements.insert(candidate_hash);
			return Ok(())
		}

		let bg_sender = sender.clone();
		let pov = PoVData::FetchFromValidator {
			from_validator: attesting.from_validator,
			candidate_hash,
			pov_hash: attesting.pov_hash,
		};
		self.background_validate_and_make_available(
			sender,
			BackgroundValidationParams {
				sender: bg_sender,
				tx_command: self.background_validation_tx.clone(),
				candidate: attesting.candidate,
				relay_parent: self.parent,
				pov,
				validator_index: self.table_context.validator.as_ref().map(|v| v.index()),
				n_validators: self.table_context.validators.len(),
				span,
				make_command: ValidatedCandidateCommand::Attest,
			},
		)
		.await
	}

	/// Import the statement and kick off validation work if it is a part of our assignment.
	async fn maybe_validate_and_import(
		&mut self,
		root_span: &jaeger::Span,
		sender: &mut JobSender<impl SubsystemSender>,
		statement: SignedFullStatement,
	) -> Result<(), Error> {
		if let Some(summary) = self.import_statement(sender, &statement, root_span).await? {
			if Some(summary.group_id) != self.assignment {
				return Ok(())
			}
			let (attesting, span) = match statement.payload() {
				Statement::Seconded(receipt) => {
					let candidate_hash = summary.candidate;

					let span = self.get_unbacked_validation_child(
						root_span,
						summary.candidate,
						summary.group_id,
					);

					let attesting = AttestingData {
						candidate: self
							.table
							.get_candidate(&candidate_hash)
							.ok_or(Error::CandidateNotFound)?
							.to_plain(),
						pov_hash: receipt.descriptor.pov_hash,
						from_validator: statement.validator_index(),
						backing: Vec::new(),
					};
					let child = span.as_ref().map(|s| s.child("try"));
					self.fallbacks.insert(summary.candidate, (attesting.clone(), span));
					(attesting, child)
				},
				Statement::Valid(candidate_hash) => {
					if let Some((attesting, span)) = self.fallbacks.get_mut(candidate_hash) {
						let our_index = self.table_context.validator.as_ref().map(|v| v.index());
						if our_index == Some(statement.validator_index()) {
							return Ok(())
						}

						if self.awaiting_validation.contains(candidate_hash) {
							// Job already running:
							attesting.backing.push(statement.validator_index());
							return Ok(())
						} else {
							// No job, so start another with current validator:
							attesting.from_validator = statement.validator_index();
							(attesting.clone(), span.as_ref().map(|s| s.child("try")))
						}
					} else {
						return Ok(())
					}
				},
			};

			self.kick_off_validation_work(sender, attesting, span).await?;
		}
		Ok(())
	}

	async fn sign_statement(&self, statement: Statement) -> Option<SignedFullStatement> {
		let signed = self
			.table_context
			.validator
			.as_ref()?
			.sign(self.keystore.clone(), statement)
			.await
			.ok()
			.flatten()?;
		self.metrics.on_statement_signed();
		Some(signed)
	}

	/// Insert or get the unbacked-span for the given candidate hash.
	fn insert_or_get_unbacked_span(
		&mut self,
		parent_span: &jaeger::Span,
		hash: CandidateHash,
		para_id: Option<ParaId>,
	) -> Option<&jaeger::Span> {
		if !self.backed.contains(&hash) {
			// only add if we don't consider this backed.
			let span = self.unbacked_candidates.entry(hash).or_insert_with(|| {
				let s = parent_span.child("unbacked-candidate").with_candidate(hash);
				if let Some(para_id) = para_id {
					s.with_para_id(para_id)
				} else {
					s
				}
			});
			Some(span)
		} else {
			None
		}
	}

	fn get_unbacked_validation_child(
		&mut self,
		parent_span: &jaeger::Span,
		hash: CandidateHash,
		para_id: ParaId,
	) -> Option<jaeger::Span> {
		self.insert_or_get_unbacked_span(parent_span, hash, Some(para_id)).map(|span| {
			span.child("validation")
				.with_candidate(hash)
				.with_stage(Stage::CandidateBacking)
		})
	}

	fn get_unbacked_statement_child(
		&mut self,
		parent_span: &jaeger::Span,
		hash: CandidateHash,
		validator: ValidatorIndex,
	) -> Option<jaeger::Span> {
		self.insert_or_get_unbacked_span(parent_span, hash, None).map(|span| {
			span.child("import-statement")
				.with_candidate(hash)
				.with_validator_index(validator)
		})
	}

	fn remove_unbacked_span(&mut self, hash: &CandidateHash) -> Option<jaeger::Span> {
		self.unbacked_candidates.remove(hash)
	}
}

impl util::JobTrait for CandidateBackingJob {
	type ToJob = CandidateBackingMessage;
	type Error = Error;
	type RunArgs = SyncCryptoStorePtr;
	type Metrics = Metrics;

	const NAME: &'static str = "CandidateBackingJob";

	fn run<S: SubsystemSender>(
		parent: Hash,
		span: Arc<jaeger::Span>,
		keystore: SyncCryptoStorePtr,
		metrics: Metrics,
		rx_to: mpsc::Receiver<Self::ToJob>,
		mut sender: JobSender<S>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
		async move {
			macro_rules! try_runtime_api {
				($x: expr) => {
					match $x {
						Ok(x) => x,
						Err(e) => {
							tracing::warn!(
								target: LOG_TARGET,
								err = ?e,
								"Failed to fetch runtime API data for job",
							);

							// We can't do candidate validation work if we don't have the
							// requisite runtime API data. But these errors should not take
							// down the node.
							return Ok(());
						}
					}
				}
			}

			let span = PerLeafSpan::new(span, "backing");
			let _span = span.child("runtime-apis");

			let (validators, groups, session_index, cores) = futures::try_join!(
				request_validators(parent, &mut sender).await,
				request_validator_groups(parent, &mut sender).await,
				request_session_index_for_child(parent, &mut sender).await,
				request_from_runtime(parent, &mut sender, |tx| {
					RuntimeApiRequest::AvailabilityCores(tx)
				},)
				.await,
			)
			.map_err(Error::JoinMultiple)?;

			let validators = try_runtime_api!(validators);
			let (validator_groups, group_rotation_info) = try_runtime_api!(groups);
			let session_index = try_runtime_api!(session_index);
			let cores = try_runtime_api!(cores);

			drop(_span);
			let _span = span.child("validator-construction");

			let signing_context = SigningContext { parent_hash: parent, session_index };
			let validator =
				match Validator::construct(&validators, signing_context.clone(), keystore.clone())
					.await
				{
					Ok(v) => Some(v),
					Err(util::Error::NotAValidator) => None,
					Err(e) => {
						tracing::warn!(
							target: LOG_TARGET,
							err = ?e,
							"Cannot participate in candidate backing",
						);

						return Ok(())
					},
				};

			drop(_span);
			let mut assignments_span = span.child("compute-assignments");

			let mut groups = HashMap::new();

			let n_cores = cores.len();

			let mut assignment = None;

			for (idx, core) in cores.into_iter().enumerate() {
				// Ignore prospective assignments on occupied cores for the time being.
				if let CoreState::Scheduled(scheduled) = core {
					let core_index = CoreIndex(idx as _);
					let group_index = group_rotation_info.group_for_core(core_index, n_cores);
					if let Some(g) = validator_groups.get(group_index.0 as usize) {
						if validator.as_ref().map_or(false, |v| g.contains(&v.index())) {
							assignment = Some((scheduled.para_id, scheduled.collator));
						}
						groups.insert(scheduled.para_id, g.clone());
					}
				}
			}

			let table_context = TableContext { groups, validators, validator };

			let (assignment, required_collator) = match assignment {
				None => {
					assignments_span.add_string_tag("assigned", "false");
					(None, None)
				},
				Some((assignment, required_collator)) => {
					assignments_span.add_string_tag("assigned", "true");
					assignments_span.add_para_id(assignment);
					(Some(assignment), required_collator)
				},
			};

			drop(assignments_span);
			let _span = span.child("wait-for-job");

			let (background_tx, background_rx) = mpsc::channel(16);
			let job = CandidateBackingJob {
				parent,
				session_index,
				assignment,
				required_collator,
				issued_statements: HashSet::new(),
				awaiting_validation: HashSet::new(),
				fallbacks: HashMap::new(),
				seconded: None,
				unbacked_candidates: HashMap::new(),
				backed: HashSet::new(),
				keystore,
				table: Table::default(),
				table_context,
				background_validation: background_rx,
				background_validation_tx: background_tx,
				metrics,
			};
			drop(_span);

			job.run_loop(sender, rx_to, span).await
		}
		.boxed()
	}
}

#[derive(Clone)]
struct MetricsInner {
	signed_statements_total: prometheus::Counter<prometheus::U64>,
	candidates_seconded_total: prometheus::Counter<prometheus::U64>,
	process_second: prometheus::Histogram,
	process_statement: prometheus::Histogram,
	get_backed_candidates: prometheus::Histogram,
}

/// Candidate backing metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_statement_signed(&self) {
		if let Some(metrics) = &self.0 {
			metrics.signed_statements_total.inc();
		}
	}

	fn on_candidate_seconded(&self) {
		if let Some(metrics) = &self.0 {
			metrics.candidates_seconded_total.inc();
		}
	}

	/// Provide a timer for handling `CandidateBackingMessage:Second` which observes on drop.
	fn time_process_second(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.process_second.start_timer())
	}

	/// Provide a timer for handling `CandidateBackingMessage::Statement` which observes on drop.
	fn time_process_statement(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.process_statement.start_timer())
	}

	/// Provide a timer for handling `CandidateBackingMessage::GetBackedCandidates` which observes on drop.
	fn time_get_backed_candidates(
		&self,
	) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.get_backed_candidates.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			signed_statements_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_candidate_backing_signed_statements_total",
					"Number of statements signed.",
				)?,
				registry,
			)?,
			candidates_seconded_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_candidate_backing_candidates_seconded_total",
					"Number of candidates seconded.",
				)?,
				registry,
			)?,
			process_second: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"parachain_candidate_backing_process_second",
					"Time spent within `candidate_backing::process_second`",
				))?,
				registry,
			)?,
			process_statement: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"parachain_candidate_backing_process_statement",
					"Time spent within `candidate_backing::process_statement`",
				))?,
				registry,
			)?,
			get_backed_candidates: prometheus::register(
				prometheus::Histogram::with_opts(prometheus::HistogramOpts::new(
					"parachain_candidate_backing_get_backed_candidates",
					"Time spent within `candidate_backing::get_backed_candidates`",
				))?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

/// The candidate backing subsystem.
pub type CandidateBackingSubsystem<Spawner> =
	polkadot_node_subsystem_util::JobSubsystem<CandidateBackingJob, Spawner>;
