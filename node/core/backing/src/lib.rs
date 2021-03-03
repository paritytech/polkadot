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

#![deny(unused_crate_dependencies)]

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;

use bitvec::vec::BitVec;
use futures::{channel::{mpsc, oneshot}, Future, FutureExt, SinkExt, StreamExt};

use sp_keystore::SyncCryptoStorePtr;
use polkadot_primitives::v1::{
	AvailableData, BackedCandidate, CandidateCommitments, CandidateDescriptor, CandidateHash,
	CandidateReceipt, CollatorId, CommittedCandidateReceipt, CoreIndex, CoreState, Hash, Id as ParaId,
	PoV, SigningContext, ValidatorId, ValidatorIndex, ValidatorSignature, ValidityAttestation,
};
use polkadot_node_primitives::{
	Statement, SignedFullStatement, ValidationResult,
};
use polkadot_subsystem::{
	PerLeafSpan, Stage,
	jaeger,
	messages::{
		AllMessages, AvailabilityStoreMessage, CandidateBackingMessage, CandidateSelectionMessage,
		CandidateValidationMessage, PoVDistributionMessage, ProvisionableData,
		ProvisionerMessage, StatementDistributionMessage, ValidationFailed, RuntimeApiRequest,
	},
};
use polkadot_node_subsystem_util::{
	self as util,
	request_session_index_for_child,
	request_validator_groups,
	request_validators,
	request_from_runtime,
	Validator,
	delegated_subsystem,
	FromJobCommand,
	metrics::{self, prometheus},
};
use statement_table::{
	generic::AttestedCandidate as TableAttestedCandidate,
	Context as TableContextTrait,
	Table,
	v1::{
		SignedStatement as TableSignedStatement,
		Statement as TableStatement,
		Summary as TableSummary,
	},
};
use thiserror::Error;

const LOG_TARGET: &str = "candidate_backing";

#[derive(Debug, Error)]
enum Error {
	#[error("Candidate is not found")]
	CandidateNotFound,
	#[error("Signature is invalid")]
	InvalidSignature,
	#[error("Failed to send candidates {0:?}")]
	Send(Vec<BackedCandidate>),
	#[error("FetchPoV channel closed before receipt")]
	FetchPoV(#[source] oneshot::Canceled),
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

enum ValidatedCandidateCommand {
	// We were instructed to second the candidate.
	Second(BackgroundValidationResult),
	// We were instructed to validate the candidate.
	Attest(BackgroundValidationResult),
}

impl std::fmt::Debug for ValidatedCandidateCommand {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		let candidate_hash = self.candidate_hash();
		match *self {
			ValidatedCandidateCommand::Second(_) =>
				write!(f, "Second({})", candidate_hash),
			ValidatedCandidateCommand::Attest(_) =>
				write!(f, "Attest({})", candidate_hash),
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
		}
	}
}

/// Holds all data needed for candidate backing job operation.
struct CandidateBackingJob {
	/// The hash of the relay parent on top of which this job is doing it's work.
	parent: Hash,
	/// Outbound message channel sending part.
	tx_from: mpsc::Sender<FromJobCommand>,
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
		self.groups.get(group).map_or(false, |g| g.iter().position(|a| a == authority).is_some())
	}

	fn requisite_votes(&self, group: &ParaId) -> usize {
		self.groups.get(group).map_or(usize::max_value(), |g| group_quorum(g.len()))
	}
}

struct InvalidErasureRoot;

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

#[tracing::instrument(level = "trace", skip(attested, table_context), fields(subsystem = LOG_TARGET))]
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

	let (ids, validity_votes): (Vec<_>, Vec<ValidityAttestation>) = validity_votes
		.into_iter()
		.map(|(id, vote)| (id, vote.into()))
		.unzip();

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

			return None;
		}
	}
	vote_positions.sort_by_key(|(_orig, pos_in_group)| *pos_in_group);

	Some(BackedCandidate {
		candidate,
		validity_votes: vote_positions.into_iter()
			.map(|(pos_in_votes, _pos_in_group)| validity_votes[pos_in_votes].clone())
			.collect(),
		validator_indices,
	})
}

async fn store_available_data(
	tx_from: &mut mpsc::Sender<FromJobCommand>,
	id: Option<ValidatorIndex>,
	n_validators: u32,
	candidate_hash: CandidateHash,
	available_data: AvailableData,
) -> Result<(), Error> {
	let (tx, rx) = oneshot::channel();
	tx_from.send(AllMessages::AvailabilityStore(
			AvailabilityStoreMessage::StoreAvailableData(
				candidate_hash,
				id,
				n_validators,
				available_data,
				tx,
			)
		).into()
	).await?;

	let _ = rx.await.map_err(Error::StoreAvailableData)?;

	Ok(())
}

// Make a `PoV` available.
//
// This will compute the erasure root internally and compare it to the expected erasure root.
// This returns `Err()` iff there is an internal error. Otherwise, it returns either `Ok(Ok(()))` or `Ok(Err(_))`.
#[tracing::instrument(level = "trace", skip(tx_from, pov, span), fields(subsystem = LOG_TARGET))]
async fn make_pov_available(
	tx_from: &mut mpsc::Sender<FromJobCommand>,
	validator_index: Option<ValidatorIndex>,
	n_validators: usize,
	pov: Arc<PoV>,
	candidate_hash: CandidateHash,
	validation_data: polkadot_primitives::v1::PersistedValidationData,
	expected_erasure_root: Hash,
	span: Option<&jaeger::Span>,
) -> Result<Result<(), InvalidErasureRoot>, Error> {
	let available_data = AvailableData {
		pov,
		validation_data,
	};

	{
		let _span = span.as_ref().map(|s| {
			s.child_with_candidate("erasure-coding", &candidate_hash)
		});

		let chunks = erasure_coding::obtain_chunks_v1(
			n_validators,
			&available_data,
		)?;

		let branches = erasure_coding::branches(chunks.as_ref());
		let erasure_root = branches.root();

		if erasure_root != expected_erasure_root {
			return Ok(Err(InvalidErasureRoot));
		}
	}

	{
		let _span = span.as_ref().map(|s|
			s.child_with_candidate("store-data", &candidate_hash)
		);

		store_available_data(
			tx_from,
			validator_index,
			n_validators as u32,
			candidate_hash,
			available_data,
		).await?;
	}

	Ok(Ok(()))
}

async fn request_pov_from_distribution(
	tx_from: &mut mpsc::Sender<FromJobCommand>,
	parent: Hash,
	descriptor: CandidateDescriptor,
) -> Result<Arc<PoV>, Error> {
	let (tx, rx) = oneshot::channel();

	tx_from.send(AllMessages::PoVDistribution(
		PoVDistributionMessage::FetchPoV(parent, descriptor, tx)
	).into()).await?;

	rx.await.map_err(Error::FetchPoV)
}

async fn request_candidate_validation(
	tx_from: &mut mpsc::Sender<FromJobCommand>,
	candidate: CandidateDescriptor,
	pov: Arc<PoV>,
) -> Result<ValidationResult, Error> {
	let (tx, rx) = oneshot::channel();

	tx_from.send(AllMessages::CandidateValidation(
			CandidateValidationMessage::ValidateFromChainState(
				candidate,
				pov,
				tx,
			)
		).into()
	).await?;

	match rx.await {
		Ok(Ok(validation_result)) => Ok(validation_result),
		Ok(Err(err)) => Err(Error::ValidationFailed(err)),
		Err(err) => Err(Error::ValidateFromChainState(err)),
	}
}

type BackgroundValidationResult = Result<(CandidateReceipt, CandidateCommitments, Arc<PoV>), CandidateReceipt>;

struct BackgroundValidationParams<F> {
	tx_from: mpsc::Sender<FromJobCommand>,
	tx_command: mpsc::Sender<ValidatedCandidateCommand>,
	candidate: CandidateReceipt,
	relay_parent: Hash,
	pov: Option<Arc<PoV>>,
	validator_index: Option<ValidatorIndex>,
	n_validators: usize,
	span: Option<jaeger::Span>,
	make_command: F,
}

async fn validate_and_make_available(
	params: BackgroundValidationParams<impl Fn(BackgroundValidationResult) -> ValidatedCandidateCommand + Sync>,
) -> Result<(), Error> {
	let BackgroundValidationParams {
		mut tx_from,
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
		Some(pov) => pov,
		None => {
			let _span = span.as_ref().map(|s| s.child("request-pov"));
			request_pov_from_distribution(
				&mut tx_from,
				relay_parent,
				candidate.descriptor.clone(),
			).await?
		}
	};

	let v = {
		let _span = span.as_ref().map(|s| {
			s.child_builder("request-validation")
			.with_pov(&pov)
			.build()
		});
		request_candidate_validation(&mut tx_from, candidate.descriptor.clone(), pov.clone()).await?
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
					&mut tx_from,
					validator_index,
					n_validators,
					pov.clone(),
					candidate.hash(),
					validation_data,
					candidate.descriptor.erasure_root,
					span.as_ref(),
				).await?;

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
		}
		ValidationResult::Invalid(reason) => {
			tracing::debug!(
				target: LOG_TARGET,
				candidate_hash = ?candidate.hash(),
				reason = ?reason,
				"Validation yielded an invalid candidate",
			);
			Err(candidate)
		}
	};

	tx_command.send(make_command(res)).await.map_err(Into::into)
}

impl CandidateBackingJob {
	/// Run asynchronously.
	async fn run_loop(
		mut self,
		mut rx_to: mpsc::Receiver<CandidateBackingMessage>,
		span: PerLeafSpan,
	) -> Result<(), Error> {
		loop {
			futures::select! {
				validated_command = self.background_validation.next() => {
					let _span = span.child("process-validation-result");
					if let Some(c) = validated_command {
						self.handle_validated_candidate_command(&span, c).await?;
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
						self.process_msg(&span, msg).await?;
					}
				}
			}
		}

		Ok(())
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	async fn handle_validated_candidate_command(
		&mut self,
		parent_span: &jaeger::Span,
		command: ValidatedCandidateCommand,
	) -> Result<(), Error> {
		let candidate_hash = command.candidate_hash();
		self.awaiting_validation.remove(&candidate_hash);

		match command {
			ValidatedCandidateCommand::Second(res) => {
				match res {
					Ok((candidate, commitments, pov)) => {
						// sanity check.
						if self.seconded.is_none() && !self.issued_statements.contains(&candidate_hash) {
							self.seconded = Some(candidate_hash);
							self.issued_statements.insert(candidate_hash);
							self.metrics.on_candidate_seconded();

							let statement = Statement::Seconded(CommittedCandidateReceipt {
								descriptor: candidate.descriptor.clone(),
								commitments,
							});
							if let Some(stmt) = self.sign_import_and_distribute_statement(
								statement,
								parent_span,
							).await? {
								self.issue_candidate_seconded_message(stmt).await?;
							}
							self.distribute_pov(candidate.descriptor, pov).await?;
						}
					}
					Err(candidate) => {
						self.issue_candidate_invalid_message(candidate).await?;
					}
				}
			}
			ValidatedCandidateCommand::Attest(res) => {
				// sanity check.
				if !self.issued_statements.contains(&candidate_hash) {
					let statement = if res.is_ok() {
						Statement::Valid(candidate_hash)
					} else {
						Statement::Invalid(candidate_hash)
					};

					self.issued_statements.insert(candidate_hash);
					self.sign_import_and_distribute_statement(statement, &parent_span).await?;
				}
			}
		}

		Ok(())
	}

	#[tracing::instrument(level = "trace", skip(self, params), fields(subsystem = LOG_TARGET))]
	async fn background_validate_and_make_available(
		&mut self,
		params: BackgroundValidationParams<
			impl Fn(BackgroundValidationResult) -> ValidatedCandidateCommand + Send + 'static + Sync
		>,
	) -> Result<(), Error> {
		let candidate_hash = params.candidate.hash();

		if self.awaiting_validation.insert(candidate_hash) {
			// spawn background task.
			let bg = async move {
				if let Err(e) = validate_and_make_available(params).await {
					tracing::error!("Failed to validate and make available: {:?}", e);
				}
			};
			self.tx_from.send(FromJobCommand::Spawn("Backing Validation", bg.boxed())).await?;
		}

		Ok(())
	}

	async fn issue_candidate_invalid_message(
		&mut self,
		candidate: CandidateReceipt,
	) -> Result<(), Error> {
		self.tx_from.send(AllMessages::from(CandidateSelectionMessage::Invalid(self.parent, candidate)).into()).await?;

		Ok(())
	}

	async fn issue_candidate_seconded_message(
		&mut self,
		statement: SignedFullStatement,
	) -> Result<(), Error> {
		self.tx_from.send(AllMessages::from(CandidateSelectionMessage::Seconded(self.parent, statement)).into()).await?;

		Ok(())
	}

	/// Kick off background validation with intent to second.
	#[tracing::instrument(level = "trace", skip(self, parent_span, pov), fields(subsystem = LOG_TARGET))]
	async fn validate_and_second(
		&mut self,
		parent_span: &jaeger::Span,
		candidate: &CandidateReceipt,
		pov: Arc<PoV>,
	) -> Result<(), Error> {
		// Check that candidate is collated by the right collator.
		if self.required_collator.as_ref()
			.map_or(false, |c| c != &candidate.descriptor().collator)
		{
			self.issue_candidate_invalid_message(candidate.clone()).await?;
			return Ok(());
		}

		let candidate_hash = candidate.hash();
		let span = self.get_unbacked_validation_child(parent_span, candidate_hash);

		tracing::debug!(
			target: LOG_TARGET,
			candidate_hash = ?candidate_hash,
			candidate_receipt = ?candidate,
			"Validate and second candidate",
		);

		self.background_validate_and_make_available(BackgroundValidationParams {
			tx_from: self.tx_from.clone(),
			tx_command: self.background_validation_tx.clone(),
			candidate: candidate.clone(),
			relay_parent: self.parent,
			pov: Some(pov),
			validator_index: self.table_context.validator.as_ref().map(|v| v.index()),
			n_validators: self.table_context.validators.len(),
			span,
			make_command: ValidatedCandidateCommand::Second,
		}).await?;

		Ok(())
	}

	async fn sign_import_and_distribute_statement(
		&mut self,
		statement: Statement,
		parent_span: &jaeger::Span,
	) -> Result<Option<SignedFullStatement>, Error> {
		if let Some(signed_statement) = self.sign_statement(statement).await {
			self.import_statement(&signed_statement, parent_span).await?;
			self.distribute_signed_statement(signed_statement.clone()).await?;
			Ok(Some(signed_statement))
		} else {
			Ok(None)
		}
	}

	/// Check if there have happened any new misbehaviors and issue necessary messages.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	async fn issue_new_misbehaviors(&mut self) -> Result<(), Error> {
		// collect the misbehaviors to avoid double mutable self borrow issues
		let misbehaviors: Vec<_> = self.table.drain_misbehaviors().collect();
		for (validator_id, report) in misbehaviors {
			self.send_to_provisioner(
				ProvisionerMessage::ProvisionableData(
					self.parent,
					ProvisionableData::MisbehaviorReport(self.parent, validator_id, report)
				)
			).await?
		}

		Ok(())
	}

	/// Import a statement into the statement table and return the summary of the import.
	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	async fn import_statement(
		&mut self,
		statement: &SignedFullStatement,
		parent_span: &jaeger::Span,
	) -> Result<Option<TableSummary>, Error> {
		tracing::debug!(
			target: LOG_TARGET,
			statement = ?statement.payload().to_compact(),
			"Importing statement",
		);

		let import_statement_span = {
			// create a span only for candidates we're already aware of.
			let candidate_hash = statement.payload().candidate_hash();
			self.get_unbacked_statement_child(parent_span, candidate_hash, statement.validator_index())
		};

		let stmt = primitive_statement_to_table(statement);

		let summary = self.table.import_statement(&self.table_context, stmt);

		let unbacked_span = if let Some(attested) = summary.as_ref()
			.and_then(|s| self.table.attested_candidate(&s.candidate, &self.table_context))
		{
			let candidate_hash = attested.candidate.hash();
			// `HashSet::insert` returns true if the thing wasn't in there already.
			if self.backed.insert(candidate_hash) {
				let span = self.remove_unbacked_span(&candidate_hash);

				if let Some(backed) =
					table_attested_to_backed(attested, &self.table_context)
				{
					tracing::debug!(
						target: LOG_TARGET,
						candidate_hash = ?candidate_hash,
						"Candidate backed",
					);

					let message = ProvisionerMessage::ProvisionableData(
						self.parent,
						ProvisionableData::BackedCandidate(backed.receipt()),
					);
					self.send_to_provisioner(message).await?;

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

		self.issue_new_misbehaviors().await?;

		// It is important that the child span is dropped before its parent span (`unbacked_span`)
		drop(import_statement_span);
		drop(unbacked_span);

		Ok(summary)
	}

	#[tracing::instrument(level = "trace", skip(self, span), fields(subsystem = LOG_TARGET))]
	async fn process_msg(&mut self, span: &jaeger::Span, msg: CandidateBackingMessage) -> Result<(), Error> {
		match msg {
			CandidateBackingMessage::Second(_relay_parent, candidate, pov) => {
				let _timer = self.metrics.time_process_second();

				let span = span.child_builder("second")
					.with_stage(jaeger::Stage::CandidateBacking)
					.with_pov(&pov)
					.with_candidate(&candidate.hash())
					.with_relay_parent(&_relay_parent)
					.build();

				// Sanity check that candidate is from our assignment.
				if Some(candidate.descriptor().para_id) != self.assignment {
					return Ok(());
				}

				// If the message is a `CandidateBackingMessage::Second`, sign and dispatch a
				// Seconded statement only if we have not seconded any other candidate and
				// have not signed a Valid statement for the requested candidate.
				if self.seconded.is_none() {
					// This job has not seconded a candidate yet.
					let candidate_hash = candidate.hash();
					let pov = Arc::new(pov);

					if !self.issued_statements.contains(&candidate_hash) {
						self.validate_and_second(&span, &candidate, pov.clone()).await?;
					}
				}
			}
			CandidateBackingMessage::Statement(_relay_parent, statement) => {
				let _timer = self.metrics.time_process_statement();
				let span = span.child_builder("statement")
					.with_stage(jaeger::Stage::CandidateBacking)
					.with_candidate(&statement.payload().candidate_hash())
					.with_relay_parent(&_relay_parent)
					.build();

				self.check_statement_signature(&statement)?;
				match self.maybe_validate_and_import(&span, statement).await {
					Err(Error::ValidationFailed(_)) => return Ok(()),
					Err(e) => return Err(e),
					Ok(()) => (),
				}
			}
			CandidateBackingMessage::GetBackedCandidates(_, requested_candidates, tx) => {
				let _timer = self.metrics.time_get_backed_candidates();

				let backed = requested_candidates
					.into_iter()
					.filter_map(|hash| {
						self.table.attested_candidate(&hash, &self.table_context)
							.and_then(|attested| table_attested_to_backed(attested, &self.table_context))
					})
					.collect();

				tx.send(backed).map_err(|data| Error::Send(data))?;
			}
		}

		Ok(())
	}

	/// Kick off validation work and distribute the result as a signed statement.
	#[tracing::instrument(level = "trace", skip(self, span), fields(subsystem = LOG_TARGET))]
	async fn kick_off_validation_work(
		&mut self,
		summary: TableSummary,
		span: Option<jaeger::Span>,
	) -> Result<(), Error> {
		let candidate_hash = summary.candidate;

		if self.issued_statements.contains(&candidate_hash) {
			return Ok(())
		}

		// We clone the commitments here because there are borrowck
		// errors relating to this being a struct and methods borrowing the entirety of self
		// and not just those things that the function uses.
		let candidate = self.table.get_candidate(&candidate_hash).ok_or(Error::CandidateNotFound)?.to_plain();
		let descriptor = candidate.descriptor().clone();

		tracing::debug!(
			target: LOG_TARGET,
			candidate_hash = ?candidate_hash,
			candidate_receipt = ?candidate,
			"Kicking off validation",
		);

		// Check that candidate is collated by the right collator.
		if self.required_collator.as_ref()
			.map_or(false, |c| c != &descriptor.collator)
		{
			// If not, we've got the statement in the table but we will
			// not issue validation work for it.
			//
			// Act as though we've issued a statement.
			self.issued_statements.insert(candidate_hash);
			return Ok(());
		}

		self.background_validate_and_make_available(BackgroundValidationParams {
			tx_from: self.tx_from.clone(),
			tx_command: self.background_validation_tx.clone(),
			candidate,
			relay_parent: self.parent,
			pov: None,
			validator_index: self.table_context.validator.as_ref().map(|v| v.index()),
			n_validators: self.table_context.validators.len(),
			span,
			make_command: ValidatedCandidateCommand::Attest,
		}).await
	}

	/// Import the statement and kick off validation work if it is a part of our assignment.
	#[tracing::instrument(level = "trace", skip(self, parent_span), fields(subsystem = LOG_TARGET))]
	async fn maybe_validate_and_import(
		&mut self,
		parent_span: &jaeger::Span,
		statement: SignedFullStatement,
	) -> Result<(), Error> {
		if let Some(summary) = self.import_statement(&statement, parent_span).await? {
			if let Statement::Seconded(_) = statement.payload() {
				if Some(summary.group_id) == self.assignment {
					let span = self.get_unbacked_validation_child(parent_span, summary.candidate);

					self.kick_off_validation_work(summary, span).await?;
				}
			}
		}

		Ok(())
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	async fn sign_statement(&self, statement: Statement) -> Option<SignedFullStatement> {
		let signed = self.table_context
			.validator
			.as_ref()?
			.sign(self.keystore.clone(), statement)
			.await
			.ok()
			.flatten()?;
		self.metrics.on_statement_signed();
		Some(signed)
	}

	#[tracing::instrument(level = "trace", skip(self), fields(subsystem = LOG_TARGET))]
	fn check_statement_signature(&self, statement: &SignedFullStatement) -> Result<(), Error> {
		let idx = statement.validator_index().0 as usize;

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

	/// Insert or get the unbacked-span for the given candidate hash.
	fn insert_or_get_unbacked_span(&mut self, parent_span: &jaeger::Span, hash: CandidateHash) -> Option<&jaeger::Span> {
		if !self.backed.contains(&hash) {
			// only add if we don't consider this backed.
			let span = self.unbacked_candidates.entry(hash).or_insert_with(|| {
				parent_span.child_with_candidate("unbacked-candidate", &hash)
			});
			Some(span)
		} else {
			None
		}
	}

	fn get_unbacked_validation_child(&mut self, parent_span: &jaeger::Span, hash: CandidateHash) -> Option<jaeger::Span> {
		self.insert_or_get_unbacked_span(parent_span, hash)
			.map(|span| {
				span.child_builder("validation")
					.with_candidate(&hash)
					.with_stage(Stage::CandidateBacking)
					.build()
			})
	}

	fn get_unbacked_statement_child(
		&mut self,
		parent_span: &jaeger::Span,
		hash: CandidateHash,
		validator: ValidatorIndex,
	) -> Option<jaeger::Span> {
		self.insert_or_get_unbacked_span(parent_span, hash).map(|span| {
			span.child_builder("import-statement")
				.with_candidate(&hash)
				.with_validator_index(validator)
				.build()
		})
	}

	fn remove_unbacked_span(&mut self, hash: &CandidateHash) -> Option<jaeger::Span> {
		self.unbacked_candidates.remove(hash)
	}

	async fn send_to_provisioner(&mut self, msg: ProvisionerMessage) -> Result<(), Error> {
		self.tx_from.send(AllMessages::from(msg).into()).await?;

		Ok(())
	}

	async fn distribute_pov(
		&mut self,
		descriptor: CandidateDescriptor,
		pov: Arc<PoV>,
	) -> Result<(), Error> {
		self.tx_from.send(AllMessages::from(
			PoVDistributionMessage::DistributePoV(self.parent, descriptor, pov),
		).into()).await.map_err(Into::into)
	}

	async fn distribute_signed_statement(&mut self, s: SignedFullStatement) -> Result<(), Error> {
		let smsg = StatementDistributionMessage::Share(self.parent, s);

		self.tx_from.send(AllMessages::from(smsg).into()).await?;

		Ok(())
	}
}

impl util::JobTrait for CandidateBackingJob {
	type ToJob = CandidateBackingMessage;
	type Error = Error;
	type RunArgs = SyncCryptoStorePtr;
	type Metrics = Metrics;

	const NAME: &'static str = "CandidateBackingJob";

	#[tracing::instrument(skip(span, keystore, metrics, rx_to, tx_from), fields(subsystem = LOG_TARGET))]
	fn run(
		parent: Hash,
		span: Arc<jaeger::Span>,
		keystore: SyncCryptoStorePtr,
		metrics: Metrics,
		rx_to: mpsc::Receiver<Self::ToJob>,
		mut tx_from: mpsc::Sender<FromJobCommand>,
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
				try_runtime_api!(request_validators(parent, &mut tx_from).await),
				try_runtime_api!(request_validator_groups(parent, &mut tx_from).await),
				try_runtime_api!(request_session_index_for_child(parent, &mut tx_from).await),
				try_runtime_api!(request_from_runtime(
					parent,
					&mut tx_from,
					|tx| RuntimeApiRequest::AvailabilityCores(tx),
				).await),
			).map_err(Error::JoinMultiple)?;

			let validators = try_runtime_api!(validators);
			let (validator_groups, group_rotation_info) = try_runtime_api!(groups);
			let session_index = try_runtime_api!(session_index);
			let cores = try_runtime_api!(cores);

			drop(_span);
			let _span = span.child("validator-construction");

			let signing_context = SigningContext { parent_hash: parent, session_index };
			let validator = match Validator::construct(
				&validators,
				signing_context,
				keystore.clone(),
			).await {
				Ok(v) => v,
				Err(util::Error::NotAValidator) => { return Ok(()) },
				Err(e) => {
					tracing::warn!(
						target: LOG_TARGET,
						err = ?e,
						"Cannot participate in candidate backing",
					);

					return Ok(())
				}
			};

			drop(_span);
			let _span = span.child("calc-validator-groups");


			let mut groups = HashMap::new();

			let n_cores = cores.len();

			let mut assignment = None;
			for (idx, core) in cores.into_iter().enumerate() {
				// Ignore prospective assignments on occupied cores for the time being.
				if let CoreState::Scheduled(scheduled) = core {
					let core_index = CoreIndex(idx as _);
					let group_index = group_rotation_info.group_for_core(core_index, n_cores);
					if let Some(g) = validator_groups.get(group_index.0 as usize) {
						if g.contains(&validator.index()) {
							assignment = Some((scheduled.para_id, scheduled.collator));
						}
						groups.insert(scheduled.para_id, g.clone());
					}
				}
			}

			let table_context = TableContext {
				groups,
				validators,
				signing_context: validator.signing_context().clone(),
				validator: Some(validator),
			};

			let (assignment, required_collator) = match assignment {
				None => (None, None),
				Some((assignment, required_collator)) => (Some(assignment), required_collator),
			};

			drop(_span);
			let _span = span.child("wait-for-job");

			let (background_tx, background_rx) = mpsc::channel(16);
			let job = CandidateBackingJob {
				parent,
				tx_from,
				assignment,
				required_collator,
				issued_statements: HashSet::new(),
				awaiting_validation: HashSet::new(),
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

			job.run_loop(rx_to, span).await
		}.boxed()
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
	fn time_get_backed_candidates(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
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
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_candidate_backing_process_second",
						"Time spent within `candidate_backing::process_second`",
					)
				)?,
				registry,
			)?,
			process_statement: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_candidate_backing_process_statement",
						"Time spent within `candidate_backing::process_statement`",
					)
				)?,
				registry,
			)?,
			get_backed_candidates: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_candidate_backing_get_backed_candidates",
						"Time spent within `candidate_backing::get_backed_candidates`",
					)
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

delegated_subsystem!(CandidateBackingJob(SyncCryptoStorePtr, Metrics) <- CandidateBackingMessage as CandidateBackingSubsystem);

#[cfg(test)]
mod tests {
	use super::*;
	use assert_matches::assert_matches;
	use futures::{future, Future};
	use polkadot_primitives::v1::{BlockData, GroupRotationInfo, HeadData, PersistedValidationData, ScheduledCore};
	use polkadot_subsystem::{
		messages::{RuntimeApiRequest, RuntimeApiMessage},
		ActiveLeavesUpdate, FromOverseer, OverseerSignal,
	};
	use polkadot_node_primitives::InvalidCandidate;
	use sp_keyring::Sr25519Keyring;
	use sp_application_crypto::AppKey;
	use sp_keystore::{CryptoStore, SyncCryptoStore};
	use statement_table::v1::Misbehavior;
	use std::collections::HashMap;

	fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
		val_ids.iter().map(|v| v.public().into()).collect()
	}

	fn table_statement_to_primitive(
		statement: TableStatement,
	) -> Statement {
		match statement {
			TableStatement::Candidate(committed_candidate_receipt) => Statement::Seconded(committed_candidate_receipt),
			TableStatement::Valid(candidate_hash) => Statement::Valid(candidate_hash),
			TableStatement::Invalid(candidate_hash) => Statement::Invalid(candidate_hash),
		}
	}

	struct TestState {
		chain_ids: Vec<ParaId>,
		keystore: SyncCryptoStorePtr,
		validators: Vec<Sr25519Keyring>,
		validator_public: Vec<ValidatorId>,
		validation_data: PersistedValidationData,
		validator_groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo),
		availability_cores: Vec<CoreState>,
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
				Sr25519Keyring::One,
			];

			let keystore = Arc::new(sc_keystore::LocalKeystore::in_memory());
			// Make sure `Alice` key is in the keystore, so this mocked node will be a parachain validator.
			SyncCryptoStore::sr25519_generate_new(&*keystore, ValidatorId::ID, Some(&validators[0].to_seed()))
				.expect("Insert key into keystore");

			let validator_public = validator_pubkeys(&validators);

			let validator_groups = vec![vec![2, 0, 3, 5], vec![1], vec![4]]
				.into_iter().map(|g| g.into_iter().map(ValidatorIndex).collect()).collect();
			let group_rotation_info = GroupRotationInfo {
				session_start_block: 0,
				group_rotation_frequency: 100,
				now: 1,
			};

			let thread_collator: CollatorId = Sr25519Keyring::Two.public().into();
			let availability_cores = vec![
				CoreState::Scheduled(ScheduledCore {
					para_id: chain_a,
					collator: None,
				}),
				CoreState::Scheduled(ScheduledCore {
					para_id: chain_b,
					collator: None,
				}),
				CoreState::Scheduled(ScheduledCore {
					para_id: thread_a,
					collator: Some(thread_collator.clone()),
				}),
			];

			let mut head_data = HashMap::new();
			head_data.insert(chain_a, HeadData(vec![4, 5, 6]));

			let relay_parent = Hash::repeat_byte(5);

			let signing_context = SigningContext {
				session_index: 1,
				parent_hash: relay_parent,
			};

			let validation_data = PersistedValidationData {
				parent_head: HeadData(vec![7, 8, 9]),
				relay_parent_number: Default::default(),
				max_pov_size: 1024,
				relay_parent_storage_root: Default::default(),
			};

			Self {
				chain_ids,
				keystore,
				validators,
				validator_public,
				validator_groups: (validator_groups, group_rotation_info),
				availability_cores,
				head_data,
				validation_data,
				signing_context,
				relay_parent,
			}
		}
	}

	struct TestHarness {
		virtual_overseer: polkadot_node_subsystem_test_helpers::TestSubsystemContextHandle<CandidateBackingMessage>,
	}

	fn test_harness<T: Future<Output=()>>(keystore: SyncCryptoStorePtr, test: impl FnOnce(TestHarness) -> T) {
		let pool = sp_core::testing::TaskExecutor::new();

		let (context, virtual_overseer) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool.clone());

		let subsystem = CandidateBackingSubsystem::run(context, keystore, Metrics(None), pool.clone());

		let test_fut = test(TestHarness {
			virtual_overseer,
		});

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);
		futures::executor::block_on(future::select(test_fut, subsystem));
	}

	fn make_erasure_root(test: &TestState, pov: PoV) -> Hash {
		let available_data = AvailableData {
			validation_data: test.validation_data.clone(),
			pov: Arc::new(pov),
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
					erasure_root: self.erasure_root,
					..Default::default()
				},
				commitments: CandidateCommitments {
					head_data: self.head_data,
					..Default::default()
				},
			}
		}
	}

	// Tests that the subsystem performs actions that are requied on startup.
	async fn test_startup(
		virtual_overseer: &mut polkadot_node_subsystem_test_helpers::TestSubsystemContextHandle<CandidateBackingMessage>,
		test_state: &TestState,
	) {
		// Start work on some new parent.
		virtual_overseer.send(FromOverseer::Signal(
			OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
				test_state.relay_parent,
				Arc::new(jaeger::Span::Disabled),
			)))
		).await;

		// Check that subsystem job issues a request for a validator set.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::Validators(tx))
			) if parent == test_state.relay_parent => {
				tx.send(Ok(test_state.validator_public.clone())).unwrap();
			}
		);

		// Check that subsystem job issues a request for the validator groups.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidatorGroups(tx))
			) if parent == test_state.relay_parent => {
				tx.send(Ok(test_state.validator_groups.clone())).unwrap();
			}
		);

		// Check that subsystem job issues a request for the session index for child.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::SessionIndexForChild(tx))
			) if parent == test_state.relay_parent => {
				tx.send(Ok(test_state.signing_context.session_index)).unwrap();
			}
		);

		// Check that subsystem job issues a request for the availability cores.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(parent, RuntimeApiRequest::AvailabilityCores(tx))
			) if parent == test_state.relay_parent => {
				tx.send(Ok(test_state.availability_cores.clone())).unwrap();
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
						ValidationResult::Valid(CandidateCommitments {
							head_data: expected_head_data.clone(),
							horizontal_messages: Vec::new(),
							upward_messages: Vec::new(),
							new_validation_code: None,
							processed_downward_messages: 0,
							hrmp_watermark: 0,
						}, test_state.validation_data),
					)).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::AvailabilityStore(
					AvailabilityStoreMessage::StoreAvailableData(candidate_hash, _, _, _, tx)
				) if candidate_hash == candidate.hash() => {
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

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::CandidateSelection(CandidateSelectionMessage::Seconded(hash, statement)) => {
					assert_eq!(test_state.relay_parent, hash);
					assert_matches!(statement.payload(), Statement::Seconded(_));
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::PoVDistribution(PoVDistributionMessage::DistributePoV(hash, descriptor, pov_received)) => {
					assert_eq!(test_state.relay_parent, hash);
					assert_eq!(candidate.descriptor, descriptor);
					assert_eq!(pov, *pov_received);
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
			let public0 = CryptoStore::sr25519_generate_new(
				&*test_state.keystore,
				ValidatorId::ID,
				Some(&test_state.validators[0].to_seed()),
			).await.expect("Insert key into keystore");
			let public1 = CryptoStore::sr25519_generate_new(
				&*test_state.keystore,
				ValidatorId::ID,
				Some(&test_state.validators[5].to_seed()),
			).await.expect("Insert key into keystore");
			let public2 = CryptoStore::sr25519_generate_new(
				&*test_state.keystore,
				ValidatorId::ID,
				Some(&test_state.validators[2].to_seed()),
			).await.expect("Insert key into keystore");

			let signed_a = SignedFullStatement::sign(
				&test_state.keystore,
				Statement::Seconded(candidate_a.clone()),
				&test_state.signing_context,
				ValidatorIndex(2),
				&public2.into(),
			).await.ok().flatten().expect("should be signed");

			let signed_b = SignedFullStatement::sign(
				&test_state.keystore,
				Statement::Valid(candidate_a_hash),
				&test_state.signing_context,
				ValidatorIndex(5),
				&public1.into(),
			).await.ok().flatten().expect("should be signed");

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
						ValidationResult::Valid(CandidateCommitments {
							head_data: expected_head_data.clone(),
							upward_messages: Vec::new(),
							horizontal_messages: Vec::new(),
							new_validation_code: None,
							processed_downward_messages: 0,
							hrmp_watermark: 0,
						}, test_state.validation_data),
					)).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::AvailabilityStore(
					AvailabilityStoreMessage::StoreAvailableData(candidate_hash, _, _, _, tx)
				) if candidate_hash == candidate_a.hash() => {
					tx.send(Ok(())).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::StatementDistribution(
					StatementDistributionMessage::Share(hash, stmt)
				) => {
					assert_eq!(test_state.relay_parent, hash);
					stmt.check_signature(&test_state.signing_context, &public0.into()).expect("Is signed correctly");
				}
			);

			let statement = CandidateBackingMessage::Statement(
				test_state.relay_parent,
				signed_b.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::Provisioner(
					ProvisionerMessage::ProvisionableData(
						_,
						ProvisionableData::BackedCandidate(candidate_receipt)
					)
				) => {
					assert_eq!(candidate_receipt, candidate_a.to_plain());
				}
			);

			virtual_overseer.send(FromOverseer::Signal(
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::stop_work(test_state.relay_parent)))
			).await;
		});
	}

	#[test]
	fn backing_works_while_validation_ongoing() {
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
			let public1 = CryptoStore::sr25519_generate_new(
				&*test_state.keystore,
				ValidatorId::ID,
				Some(&test_state.validators[5].to_seed()),
			).await.expect("Insert key into keystore");
			let public2 = CryptoStore::sr25519_generate_new(
				&*test_state.keystore,
				ValidatorId::ID,
				Some(&test_state.validators[2].to_seed()),
			).await.expect("Insert key into keystore");
			let public3 = CryptoStore::sr25519_generate_new(
				&*test_state.keystore,
				ValidatorId::ID,
				Some(&test_state.validators[3].to_seed()),
			).await.expect("Insert key into keystore");

			let signed_a = SignedFullStatement::sign(
				&test_state.keystore,
				Statement::Seconded(candidate_a.clone()),
				&test_state.signing_context,
				ValidatorIndex(2),
				&public2.into(),
			).await.ok().flatten().expect("should be signed");

			let signed_b = SignedFullStatement::sign(
				&test_state.keystore,
				Statement::Valid(candidate_a_hash),
				&test_state.signing_context,
				ValidatorIndex(5),
				&public1.into(),
			).await.ok().flatten().expect("should be signed");

			let signed_c = SignedFullStatement::sign(
				&test_state.keystore,
				Statement::Valid(candidate_a_hash),
				&test_state.signing_context,
				ValidatorIndex(3),
				&public3.into(),
			).await.ok().flatten().expect("should be signed");

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
					// we never validate the candidate. our local node
					// shouldn't issue any statements.
					std::mem::forget(tx);
				}
			);

			let statement = CandidateBackingMessage::Statement(
				test_state.relay_parent,
				signed_b.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			let statement = CandidateBackingMessage::Statement(
				test_state.relay_parent,
				signed_c.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			// Candidate gets backed entirely by other votes.
			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::Provisioner(
					ProvisionerMessage::ProvisionableData(
						_,
						ProvisionableData::BackedCandidate(CandidateReceipt {
							descriptor,
							..
						})
					)
				) if descriptor == candidate_a.descriptor
			);

			let (tx, rx) = oneshot::channel();
			let msg = CandidateBackingMessage::GetBackedCandidates(
				test_state.relay_parent,
				vec![candidate_a.hash()],
				tx,
			);

			virtual_overseer.send(FromOverseer::Communication{ msg }).await;

			let candidates = rx.await.unwrap();
			assert_eq!(1, candidates.len());
			assert_eq!(candidates[0].validity_votes.len(), 3);

			assert!(candidates[0].validity_votes.contains(
				&ValidityAttestation::Implicit(signed_a.signature().clone())
			));
			assert!(candidates[0].validity_votes.contains(
				&ValidityAttestation::Explicit(signed_b.signature().clone())
			));
			assert!(candidates[0].validity_votes.contains(
				&ValidityAttestation::Explicit(signed_c.signature().clone())
			));
			assert_eq!(
				candidates[0].validator_indices,
				bitvec::bitvec![bitvec::order::Lsb0, u8; 1, 0, 1, 1],
			);

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
			let public0 = CryptoStore::sr25519_generate_new(
				&*test_state.keystore,
				ValidatorId::ID, Some(&test_state.validators[0].to_seed())
			).await.expect("Insert key into keystore");
			let public2 = CryptoStore::sr25519_generate_new(
				&*test_state.keystore,
				ValidatorId::ID, Some(&test_state.validators[2].to_seed())
			).await.expect("Insert key into keystore");
			let signed_a = SignedFullStatement::sign(
				&test_state.keystore,
				Statement::Seconded(candidate_a.clone()),
				&test_state.signing_context,
				ValidatorIndex(2),
				&public2.into(),
			).await.ok().flatten().expect("should be signed");

			let signed_b = SignedFullStatement::sign(
				&test_state.keystore,
				Statement::Invalid(candidate_a_hash),
				&test_state.signing_context,
				ValidatorIndex(2),
				&public2.into(),
			).await.ok().flatten().expect("should be signed");

			let signed_c = SignedFullStatement::sign(
				&test_state.keystore,
				Statement::Invalid(candidate_a_hash),
				&test_state.signing_context,
				ValidatorIndex(0),
				&public0.into(),
			).await.ok().flatten().expect("should be signed");

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
						ValidationResult::Valid(CandidateCommitments {
							head_data: expected_head_data.clone(),
							upward_messages: Vec::new(),
							horizontal_messages: Vec::new(),
							new_validation_code: None,
							processed_downward_messages: 0,
							hrmp_watermark: 0,
						}, test_state.validation_data),
					)).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::AvailabilityStore(
					AvailabilityStoreMessage::StoreAvailableData(candidate_hash, _, _, _, tx)
				) if candidate_hash == candidate_a.hash() => {
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

			// This `Invalid` statement contradicts the `Candidate` statement
			// sent at first.
			let statement = CandidateBackingMessage::Statement(test_state.relay_parent, signed_b.clone());

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::Provisioner(
					ProvisionerMessage::ProvisionableData(
						_,
						ProvisionableData::MisbehaviorReport(
							relay_parent,
							validator_index,
							Misbehavior::ValidityDoubleVote(vdv),
						)
					)
				) if relay_parent == test_state.relay_parent => {
					let ((t1, s1), (t2, s2)) = vdv.deconstruct::<TableContext>();
					let t1 = table_statement_to_primitive(t1);
					let t2 = table_statement_to_primitive(t2);

					SignedFullStatement::new(
						t1,
						validator_index,
						s1,
						&test_state.signing_context,
						&test_state.validator_public[validator_index.0 as usize],
					).expect("signature must be valid");

					SignedFullStatement::new(
						t2,
						validator_index,
						s2,
						&test_state.signing_context,
						&test_state.validator_public[validator_index.0 as usize],
					).expect("signature must be valid");
				}
			);

			// This `Invalid` statement contradicts the `Valid` statement the subsystem
			// should have issued behind the scenes.
			let statement = CandidateBackingMessage::Statement(test_state.relay_parent, signed_c.clone());

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::Provisioner(
					ProvisionerMessage::ProvisionableData(
						_,
						ProvisionableData::MisbehaviorReport(
							relay_parent,
							validator_index,
							Misbehavior::ValidityDoubleVote(vdv),
						)
					)
				) if relay_parent == test_state.relay_parent => {
					let ((t1, s1), (t2, s2)) = vdv.deconstruct::<TableContext>();
					let t1 = table_statement_to_primitive(t1);
					let t2 = table_statement_to_primitive(t2);

					SignedFullStatement::new(
						t1,
						validator_index,
						s1,
						&test_state.signing_context,
						&test_state.validator_public[validator_index.0 as usize],
					).expect("signature must be valid");

					SignedFullStatement::new(
						t2,
						validator_index,
						s2,
						&test_state.signing_context,
						&test_state.validator_public[validator_index.0 as usize],
					).expect("signature must be valid");
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
					tx.send(Ok(ValidationResult::Invalid(InvalidCandidate::BadReturn))).unwrap();
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
						ValidationResult::Valid(CandidateCommitments {
							head_data: expected_head_data.clone(),
							upward_messages: Vec::new(),
							horizontal_messages: Vec::new(),
							new_validation_code: None,
							processed_downward_messages: 0,
							hrmp_watermark: 0,
						}, test_state.validation_data),
					)).unwrap();
				}
			);

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::AvailabilityStore(
					AvailabilityStoreMessage::StoreAvailableData(candidate_hash, _, _, _, tx)
				) if candidate_hash == candidate_b.hash() => {
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

			let validator2 = CryptoStore::sr25519_generate_new(
				&*test_state.keystore,
				ValidatorId::ID, Some(&test_state.validators[2].to_seed())
			).await.expect("Insert key into keystore");

			let signed_a = SignedFullStatement::sign(
				&test_state.keystore,
				Statement::Seconded(candidate.clone()),
				&test_state.signing_context,
				ValidatorIndex(2),
				&validator2.into(),
			).await.ok().flatten().expect("should be signed");

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
					tx.send(Ok(ValidationResult::Invalid(InvalidCandidate::BadReturn))).unwrap();
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

			let public2 = CryptoStore::sr25519_generate_new(
				&*test_state.keystore,
				ValidatorId::ID, Some(&test_state.validators[2].to_seed())
			).await.expect("Insert key into keystore");
			let signed_a = SignedFullStatement::sign(
				&test_state.keystore,
				Statement::Seconded(candidate.clone()),
				&test_state.signing_context,
				ValidatorIndex(2),
				&public2.into(),
			).await.ok().flatten().expect("should be signed");

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
					tx.send(Err(ValidationFailed("Internal test error".into()))).unwrap();
				}
			);

			// Try to get a set of backable candidates to trigger _some_ action in the subsystem
			// and check that it is still alive.
			let (tx, rx) = oneshot::channel();
			let msg = CandidateBackingMessage::GetBackedCandidates(
				test_state.relay_parent,
				vec![candidate.hash()],
				tx,
			);

			virtual_overseer.send(FromOverseer::Communication{ msg }).await;
			assert_eq!(rx.await.unwrap().len(), 0);
		});
	}

	// Test that a `CandidateBackingMessage::Second` issues validation work
	// and in case validation is successful issues a `StatementDistributionMessage`.
	#[test]
	fn backing_doesnt_second_wrong_collator() {
		let mut test_state = TestState::default();
		test_state.availability_cores[0] = CoreState::Scheduled(ScheduledCore {
			para_id: ParaId::from(1),
			collator: Some(Sr25519Keyring::Bob.public().into()),
		});

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
				AllMessages::CandidateSelection(
					CandidateSelectionMessage::Invalid(parent, c)
				) if parent == test_state.relay_parent && c == candidate.to_plain() => {
				}
			);

			virtual_overseer.send(FromOverseer::Signal(
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::stop_work(test_state.relay_parent)))
			).await;
		});
	}

	#[test]
	fn validation_work_ignores_wrong_collator() {
		let mut test_state = TestState::default();
		test_state.availability_cores[0] = CoreState::Scheduled(ScheduledCore {
			para_id: ParaId::from(1),
			collator: Some(Sr25519Keyring::Bob.public().into()),
		});

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

			let public2 = CryptoStore::sr25519_generate_new(
				&*test_state.keystore,
				ValidatorId::ID, Some(&test_state.validators[2].to_seed())
			).await.expect("Insert key into keystore");
			let seconding = SignedFullStatement::sign(
				&test_state.keystore,
				Statement::Seconded(candidate_a.clone()),
				&test_state.signing_context,
				ValidatorIndex(2),
				&public2.into(),
			).await.ok().flatten().expect("should be signed");

			let statement = CandidateBackingMessage::Statement(
				test_state.relay_parent,
				seconding.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: statement }).await;

			// The statement will be ignored because it has the wrong collator.
			virtual_overseer.send(FromOverseer::Signal(
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::stop_work(test_state.relay_parent)))
			).await;
		});
	}

	#[test]
	fn candidate_backing_reorders_votes() {
		use sp_core::Encode;
		use std::convert::TryFrom;

		let relay_parent = [1; 32].into();
		let para_id = ParaId::from(10);
		let session_index = 5;
		let signing_context = SigningContext { parent_hash: relay_parent, session_index };
		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
			Sr25519Keyring::One,
		];

		let validator_public = validator_pubkeys(&validators);
		let validator_groups = {
			let mut validator_groups = HashMap::new();
			validator_groups.insert(para_id, vec![0, 1, 2, 3, 4, 5].into_iter().map(ValidatorIndex).collect());
			validator_groups
		};

		let table_context = TableContext {
			signing_context,
			validator: None,
			groups: validator_groups,
			validators: validator_public.clone(),
		};

		let fake_attestation = |idx: u32| {
			let candidate: CommittedCandidateReceipt  = Default::default();
			let hash = candidate.hash();
			let mut data = vec![0; 64];
			data[0..32].copy_from_slice(hash.0.as_bytes());
			data[32..36].copy_from_slice(idx.encode().as_slice());

			let sig = ValidatorSignature::try_from(data).unwrap();
			statement_table::generic::ValidityAttestation::Implicit(sig)
		};

		let attested = TableAttestedCandidate {
			candidate: Default::default(),
			validity_votes: vec![
				(ValidatorIndex(5), fake_attestation(5)),
				(ValidatorIndex(3), fake_attestation(3)),
				(ValidatorIndex(1), fake_attestation(1)),
			],
			group_id: para_id,
		};

		let backed = table_attested_to_backed(attested, &table_context).unwrap();

		let expected_bitvec = {
			let mut validator_indices = BitVec::<bitvec::order::Lsb0, u8>::with_capacity(6);
			validator_indices.resize(6, false);

			validator_indices.set(1, true);
			validator_indices.set(3, true);
			validator_indices.set(5, true);

			validator_indices
		};

		// Should be in bitfield order, which is opposite to the order provided to the function.
		let expected_attestations = vec![
			fake_attestation(1).into(),
			fake_attestation(3).into(),
			fake_attestation(5).into(),
		];

		assert_eq!(backed.validator_indices, expected_bitvec);
		assert_eq!(backed.validity_votes, expected_attestations);
	}
}
