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

use std::{
	collections::{HashMap, HashSet},
	sync::Arc,
};

use bitvec::vec::BitVec;
use futures::{
	channel::{mpsc, oneshot},
	FutureExt, SinkExt, StreamExt,
};

use error::{Error, FatalResult};
use polkadot_node_primitives::{
	AvailableData, InvalidCandidate, PoV, SignedFullStatement, Statement, ValidationResult,
	BACKING_EXECUTION_TIMEOUT,
};
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		AvailabilityDistributionMessage, AvailabilityStoreMessage, CandidateBackingMessage,
		CandidateValidationMessage, CollatorProtocolMessage, ProvisionableData, ProvisionerMessage,
		RuntimeApiRequest, StatementDistributionMessage,
	},
	overseer, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, PerLeafSpan, SpawnedSubsystem,
	Stage, SubsystemError,
};
use polkadot_node_subsystem_util::{
	self as util, request_from_runtime, request_session_index_for_child, request_validator_groups,
	request_validators, Validator,
};
use polkadot_primitives::v2::{
	BackedCandidate, CandidateCommitments, CandidateHash, CandidateReceipt, CollatorId,
	CommittedCandidateReceipt, CoreIndex, CoreState, Hash, Id as ParaId, SigningContext,
	ValidatorId, ValidatorIndex, ValidatorSignature, ValidityAttestation,
};
use sp_keystore::SyncCryptoStorePtr;
use statement_table::{
	generic::AttestedCandidate as TableAttestedCandidate,
	v2::{
		SignedStatement as TableSignedStatement, Statement as TableStatement,
		Summary as TableSummary,
	},
	Context as TableContextTrait, Table,
};

mod error;

mod metrics;
use self::metrics::Metrics;

#[cfg(test)]
mod tests;

const LOG_TARGET: &str = "parachain::candidate-backing";

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

/// The candidate backing subsystem.
pub struct CandidateBackingSubsystem {
	keystore: SyncCryptoStorePtr,
	metrics: Metrics,
}

impl CandidateBackingSubsystem {
	/// Create a new instance of the `CandidateBackingSubsystem`.
	pub fn new(keystore: SyncCryptoStorePtr, metrics: Metrics) -> Self {
		Self { keystore, metrics }
	}
}

#[overseer::subsystem(CandidateBacking, error = SubsystemError, prefix = self::overseer)]
impl<Context> CandidateBackingSubsystem
where
	Context: Send + Sync,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = async move {
			run(ctx, self.keystore, self.metrics)
				.await
				.map_err(|e| SubsystemError::with_origin("candidate-backing", e))
		}
		.boxed();

		SpawnedSubsystem { name: "candidate-backing-subsystem", future }
	}
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn run<Context>(
	mut ctx: Context,
	keystore: SyncCryptoStorePtr,
	metrics: Metrics,
) -> FatalResult<()> {
	let (background_validation_tx, mut background_validation_rx) = mpsc::channel(16);
	let mut jobs = HashMap::new();

	loop {
		let res = run_iteration(
			&mut ctx,
			keystore.clone(),
			&metrics,
			&mut jobs,
			background_validation_tx.clone(),
			&mut background_validation_rx,
		)
		.await;

		match res {
			Ok(()) => break,
			Err(e) => crate::error::log_error(Err(e))?,
		}
	}

	Ok(())
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn run_iteration<Context>(
	ctx: &mut Context,
	keystore: SyncCryptoStorePtr,
	metrics: &Metrics,
	jobs: &mut HashMap<Hash, JobAndSpan<Context>>,
	background_validation_tx: mpsc::Sender<(Hash, ValidatedCandidateCommand)>,
	background_validation_rx: &mut mpsc::Receiver<(Hash, ValidatedCandidateCommand)>,
) -> Result<(), Error> {
	loop {
		futures::select!(
			validated_command = background_validation_rx.next().fuse() => {
				if let Some((relay_parent, command)) = validated_command {
					handle_validated_candidate_command(
						&mut *ctx,
						jobs,
						relay_parent,
						command,
					).await?;
				} else {
					panic!("background_validation_tx always alive at this point; qed");
				}
			}
			from_overseer = ctx.recv().fuse() => {
				match from_overseer? {
					FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)) => handle_active_leaves_update(
						&mut *ctx,
						update,
						jobs,
						&keystore,
						&background_validation_tx,
						&metrics,
					).await?,
					FromOrchestra::Signal(OverseerSignal::BlockFinalized(..)) => {}
					FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(()),
					FromOrchestra::Communication { msg } => handle_communication(&mut *ctx, jobs, msg).await?,
				}
			}
		)
	}
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn handle_validated_candidate_command<Context>(
	ctx: &mut Context,
	jobs: &mut HashMap<Hash, JobAndSpan<Context>>,
	relay_parent: Hash,
	command: ValidatedCandidateCommand,
) -> Result<(), Error> {
	if let Some(job) = jobs.get_mut(&relay_parent) {
		job.job.handle_validated_candidate_command(&job.span, ctx, command).await?;
	} else {
		// simple race condition; can be ignored - this relay-parent
		// is no longer relevant.
	}

	Ok(())
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn handle_communication<Context>(
	ctx: &mut Context,
	jobs: &mut HashMap<Hash, JobAndSpan<Context>>,
	message: CandidateBackingMessage,
) -> Result<(), Error> {
	match message {
		CandidateBackingMessage::Second(relay_parent, candidate, pov) => {
			if let Some(job) = jobs.get_mut(&relay_parent) {
				job.job.handle_second_msg(&job.span, ctx, candidate, pov).await?;
			}
		},
		CandidateBackingMessage::Statement(relay_parent, statement) => {
			if let Some(job) = jobs.get_mut(&relay_parent) {
				job.job.handle_statement_message(&job.span, ctx, statement).await?;
			}
		},
		CandidateBackingMessage::GetBackedCandidates(relay_parent, requested_candidates, tx) =>
			if let Some(job) = jobs.get_mut(&relay_parent) {
				job.job.handle_get_backed_candidates_message(requested_candidates, tx)?;
			},
	}

	Ok(())
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn handle_active_leaves_update<Context>(
	ctx: &mut Context,
	update: ActiveLeavesUpdate,
	jobs: &mut HashMap<Hash, JobAndSpan<Context>>,
	keystore: &SyncCryptoStorePtr,
	background_validation_tx: &mpsc::Sender<(Hash, ValidatedCandidateCommand)>,
	metrics: &Metrics,
) -> Result<(), Error> {
	for deactivated in update.deactivated {
		jobs.remove(&deactivated);
	}

	let leaf = match update.activated {
		None => return Ok(()),
		Some(a) => a,
	};

	macro_rules! try_runtime_api {
		($x: expr) => {
			match $x {
				Ok(x) => x,
				Err(e) => {
					gum::warn!(
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

	let parent = leaf.hash;
	let span = PerLeafSpan::new(leaf.span, "backing");
	let _span = span.child("runtime-apis");

	let (validators, groups, session_index, cores) = futures::try_join!(
		request_validators(parent, ctx.sender()).await,
		request_validator_groups(parent, ctx.sender()).await,
		request_session_index_for_child(parent, ctx.sender()).await,
		request_from_runtime(parent, ctx.sender(), |tx| {
			RuntimeApiRequest::AvailabilityCores(tx)
		},)
		.await,
	)
	.map_err(Error::JoinMultiple)?;

	let validators: Vec<_> = try_runtime_api!(validators);
	let (validator_groups, group_rotation_info) = try_runtime_api!(groups);
	let session_index = try_runtime_api!(session_index);
	let cores = try_runtime_api!(cores);

	drop(_span);
	let _span = span.child("validator-construction");

	let signing_context = SigningContext { parent_hash: parent, session_index };
	let validator =
		match Validator::construct(&validators, signing_context.clone(), keystore.clone()).await {
			Ok(v) => Some(v),
			Err(util::Error::NotAValidator) => None,
			Err(e) => {
				gum::warn!(
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

	let job = CandidateBackingJob {
		parent,
		assignment,
		required_collator,
		issued_statements: HashSet::new(),
		awaiting_validation: HashSet::new(),
		fallbacks: HashMap::new(),
		seconded: None,
		unbacked_candidates: HashMap::new(),
		backed: HashSet::new(),
		keystore: keystore.clone(),
		table: Table::default(),
		table_context,
		background_validation_tx: background_validation_tx.clone(),
		metrics: metrics.clone(),
		_marker: std::marker::PhantomData,
	};

	jobs.insert(parent, JobAndSpan { job, span });

	Ok(())
}

struct JobAndSpan<Context> {
	job: CandidateBackingJob<Context>,
	span: PerLeafSpan,
}

/// Holds all data needed for candidate backing job operation.
struct CandidateBackingJob<Context> {
	/// The hash of the relay parent on top of which this job is doing it's work.
	parent: Hash,
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
	background_validation_tx: mpsc::Sender<(Hash, ValidatedCandidateCommand)>,
	metrics: Metrics,
	_marker: std::marker::PhantomData<Context>,
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

/// How many votes we need to consider a candidate backed.
///
/// WARNING: This has to be kept in sync with the runtime check in the inclusion module.
fn minimum_votes(n_validators: usize) -> usize {
	std::cmp::min(2, n_validators)
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
		self.groups.get(group).map_or(false, |g| g.iter().any(|a| a == authority))
	}

	fn requisite_votes(&self, group: &ParaId) -> usize {
		self.groups.get(group).map_or(usize::MAX, |g| minimum_votes(g.len()))
	}
}

struct InvalidErasureRoot;

// It looks like it's not possible to do an `impl From` given the current state of
// the code. So this does the necessary conversion.
fn primitive_statement_to_table(s: &SignedFullStatement) -> TableSignedStatement {
	let statement = match s.payload() {
		Statement::Seconded(c) => TableStatement::Seconded(c.clone()),
		Statement::Valid(h) => TableStatement::Valid(*h),
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
			gum::warn!(
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
	sender: &mut impl overseer::CandidateBackingSenderTrait,
	n_validators: u32,
	candidate_hash: CandidateHash,
	available_data: AvailableData,
) -> Result<(), Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(AvailabilityStoreMessage::StoreAvailableData {
			candidate_hash,
			n_validators,
			available_data,
			tx,
		})
		.await;

	let _ = rx.await.map_err(Error::StoreAvailableData)?;

	Ok(())
}

// Make a `PoV` available.
//
// This will compute the erasure root internally and compare it to the expected erasure root.
// This returns `Err()` iff there is an internal error. Otherwise, it returns either `Ok(Ok(()))` or `Ok(Err(_))`.

async fn make_pov_available(
	sender: &mut impl overseer::CandidateBackingSenderTrait,
	n_validators: usize,
	pov: Arc<PoV>,
	candidate_hash: CandidateHash,
	validation_data: polkadot_primitives::v2::PersistedValidationData,
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

		store_available_data(sender, n_validators as u32, candidate_hash, available_data).await?;
	}

	Ok(Ok(()))
}

async fn request_pov(
	sender: &mut impl overseer::CandidateBackingSenderTrait,
	relay_parent: Hash,
	from_validator: ValidatorIndex,
	para_id: ParaId,
	candidate_hash: CandidateHash,
	pov_hash: Hash,
) -> Result<Arc<PoV>, Error> {
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(AvailabilityDistributionMessage::FetchPoV {
			relay_parent,
			from_validator,
			para_id,
			candidate_hash,
			pov_hash,
			tx,
		})
		.await;

	let pov = rx.await.map_err(|_| Error::FetchPoV)?;
	Ok(Arc::new(pov))
}

async fn request_candidate_validation(
	sender: &mut impl overseer::CandidateBackingSenderTrait,
	candidate_receipt: CandidateReceipt,
	pov: Arc<PoV>,
) -> Result<ValidationResult, Error> {
	let (tx, rx) = oneshot::channel();

	sender
		.send_message(CandidateValidationMessage::ValidateFromChainState(
			candidate_receipt,
			pov,
			BACKING_EXECUTION_TIMEOUT,
			tx,
		))
		.await;

	match rx.await {
		Ok(Ok(validation_result)) => Ok(validation_result),
		Ok(Err(err)) => Err(Error::ValidationFailed(err)),
		Err(err) => Err(Error::ValidateFromChainState(err)),
	}
}

type BackgroundValidationResult =
	Result<(CandidateReceipt, CandidateCommitments, Arc<PoV>), CandidateReceipt>;

struct BackgroundValidationParams<S: overseer::CandidateBackingSenderTrait, F> {
	sender: S,
	tx_command: mpsc::Sender<(Hash, ValidatedCandidateCommand)>,
	candidate: CandidateReceipt,
	relay_parent: Hash,
	pov: PoVData,
	n_validators: usize,
	span: Option<jaeger::Span>,
	make_command: F,
}

async fn validate_and_make_available(
	params: BackgroundValidationParams<
		impl overseer::CandidateBackingSenderTrait,
		impl Fn(BackgroundValidationResult) -> ValidatedCandidateCommand + Sync,
	>,
) -> Result<(), Error> {
	let BackgroundValidationParams {
		mut sender,
		mut tx_command,
		candidate,
		relay_parent,
		pov,
		n_validators,
		span,
		make_command,
	} = params;

	let pov = match pov {
		PoVData::Ready(pov) => pov,
		PoVData::FetchFromValidator { from_validator, candidate_hash, pov_hash } => {
			let _span = span.as_ref().map(|s| s.child("request-pov"));
			match request_pov(
				&mut sender,
				relay_parent,
				from_validator,
				candidate.descriptor.para_id,
				candidate_hash,
				pov_hash,
			)
			.await
			{
				Err(Error::FetchPoV) => {
					tx_command
						.send((
							relay_parent,
							ValidatedCandidateCommand::AttestNoPoV(candidate.hash()),
						))
						.await
						.map_err(Error::BackgroundValidationMpsc)?;
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
		request_candidate_validation(&mut sender, candidate.clone(), pov.clone()).await?
	};

	let res = match v {
		ValidationResult::Valid(commitments, validation_data) => {
			gum::debug!(
				target: LOG_TARGET,
				candidate_hash = ?candidate.hash(),
				"Validation successful",
			);

			let erasure_valid = make_pov_available(
				&mut sender,
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
					gum::debug!(
						target: LOG_TARGET,
						candidate_hash = ?candidate.hash(),
						actual_commitments = ?commitments,
						"Erasure root doesn't match the announced by the candidate receipt",
					);
					Err(candidate)
				},
			}
		},
		ValidationResult::Invalid(InvalidCandidate::CommitmentsHashMismatch) => {
			// If validation produces a new set of commitments, we vote the candidate as invalid.
			gum::warn!(
				target: LOG_TARGET,
				candidate_hash = ?candidate.hash(),
				"Validation yielded different commitments",
			);
			Err(candidate)
		},
		ValidationResult::Invalid(reason) => {
			gum::debug!(
				target: LOG_TARGET,
				candidate_hash = ?candidate.hash(),
				reason = ?reason,
				"Validation yielded an invalid candidate",
			);
			Err(candidate)
		},
	};

	tx_command.send((relay_parent, make_command(res))).await.map_err(Into::into)
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
impl<Context> CandidateBackingJob<Context> {
	async fn handle_validated_candidate_command(
		&mut self,
		root_span: &jaeger::Span,
		ctx: &mut Context,
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
								.sign_import_and_distribute_statement(ctx, statement, root_span)
								.await?
							{
								// Break cycle - bounded as there is only one candidate to
								// second per block.
								ctx.send_unbounded_message(CollatorProtocolMessage::Seconded(
									self.parent,
									stmt,
								));
							}
						}
					},
					Err(candidate) => {
						// Break cycle - bounded as there is only one candidate to
						// second per block.
						ctx.send_unbounded_message(CollatorProtocolMessage::Invalid(
							self.parent,
							candidate,
						));
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
						self.sign_import_and_distribute_statement(ctx, statement, &root_span)
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
						self.kick_off_validation_work(ctx, attesting, c_span).await?
					}
				} else {
					gum::warn!(
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
		ctx: &mut Context,
		params: BackgroundValidationParams<
			impl overseer::CandidateBackingSenderTrait,
			impl Fn(BackgroundValidationResult) -> ValidatedCandidateCommand + Send + 'static + Sync,
		>,
	) -> Result<(), Error> {
		let candidate_hash = params.candidate.hash();
		if self.awaiting_validation.insert(candidate_hash) {
			// spawn background task.
			let bg = async move {
				if let Err(e) = validate_and_make_available(params).await {
					if let Error::BackgroundValidationMpsc(error) = e {
						gum::debug!(
							target: LOG_TARGET,
							?error,
							"Mpsc background validation mpsc died during validation- leaf no longer active?"
						);
					} else {
						gum::error!(
							target: LOG_TARGET,
							"Failed to validate and make available: {:?}",
							e
						);
					}
				}
			};

			ctx.spawn("backing-validation", bg.boxed())
				.map_err(|_| Error::FailedToSpawnBackgroundTask)?;
		}

		Ok(())
	}

	/// Kick off background validation with intent to second.
	async fn validate_and_second(
		&mut self,
		parent_span: &jaeger::Span,
		root_span: &jaeger::Span,
		ctx: &mut Context,
		candidate: &CandidateReceipt,
		pov: Arc<PoV>,
	) -> Result<(), Error> {
		// Check that candidate is collated by the right collator.
		if self
			.required_collator
			.as_ref()
			.map_or(false, |c| c != &candidate.descriptor().collator)
		{
			// Break cycle - bounded as there is only one candidate to
			// second per block.
			ctx.send_unbounded_message(CollatorProtocolMessage::Invalid(
				self.parent,
				candidate.clone(),
			));
			return Ok(())
		}

		let candidate_hash = candidate.hash();
		let mut span = self.get_unbacked_validation_child(
			root_span,
			candidate_hash,
			candidate.descriptor().para_id,
		);

		span.as_mut().map(|span| span.add_follows_from(parent_span));

		gum::debug!(
			target: LOG_TARGET,
			candidate_hash = ?candidate_hash,
			candidate_receipt = ?candidate,
			"Validate and second candidate",
		);

		let bg_sender = ctx.sender().clone();
		self.background_validate_and_make_available(
			ctx,
			BackgroundValidationParams {
				sender: bg_sender,
				tx_command: self.background_validation_tx.clone(),
				candidate: candidate.clone(),
				relay_parent: self.parent,
				pov: PoVData::Ready(pov),
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
		ctx: &mut Context,
		statement: Statement,
		root_span: &jaeger::Span,
	) -> Result<Option<SignedFullStatement>, Error> {
		if let Some(signed_statement) = self.sign_statement(statement).await {
			self.import_statement(ctx, &signed_statement, root_span).await?;
			let smsg = StatementDistributionMessage::Share(self.parent, signed_statement.clone());
			ctx.send_unbounded_message(smsg);

			Ok(Some(signed_statement))
		} else {
			Ok(None)
		}
	}

	/// Check if there have happened any new misbehaviors and issue necessary messages.
	fn issue_new_misbehaviors(&mut self, sender: &mut impl overseer::CandidateBackingSenderTrait) {
		// collect the misbehaviors to avoid double mutable self borrow issues
		let misbehaviors: Vec<_> = self.table.drain_misbehaviors().collect();
		for (validator_id, report) in misbehaviors {
			// The provisioner waits on candidate-backing, which means
			// that we need to send unbounded messages to avoid cycles.
			//
			// Misbehaviors are bounded by the number of validators and
			// the block production protocol.
			sender.send_unbounded_message(ProvisionerMessage::ProvisionableData(
				self.parent,
				ProvisionableData::MisbehaviorReport(self.parent, validator_id, report),
			));
		}
	}

	/// Import a statement into the statement table and return the summary of the import.
	async fn import_statement(
		&mut self,
		ctx: &mut Context,
		statement: &SignedFullStatement,
		root_span: &jaeger::Span,
	) -> Result<Option<TableSummary>, Error> {
		gum::debug!(
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
					gum::debug!(
						target: LOG_TARGET,
						candidate_hash = ?candidate_hash,
						relay_parent = ?self.parent,
						para_id = %backed.candidate.descriptor.para_id,
						"Candidate backed",
					);

					// The provisioner waits on candidate-backing, which means
					// that we need to send unbounded messages to avoid cycles.
					//
					// Backed candidates are bounded by the number of validators,
					// parachains, and the block production rate of the relay chain.
					let message = ProvisionerMessage::ProvisionableData(
						self.parent,
						ProvisionableData::BackedCandidate(backed.receipt()),
					);
					ctx.send_unbounded_message(message);

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

		self.issue_new_misbehaviors(ctx.sender());

		// It is important that the child span is dropped before its parent span (`unbacked_span`)
		drop(import_statement_span);
		drop(unbacked_span);

		Ok(summary)
	}

	async fn handle_second_msg(
		&mut self,
		root_span: &jaeger::Span,
		ctx: &mut Context,
		candidate: CandidateReceipt,
		pov: PoV,
	) -> Result<(), Error> {
		let _timer = self.metrics.time_process_second();

		let candidate_hash = candidate.hash();
		let span = root_span
			.child("second")
			.with_stage(jaeger::Stage::CandidateBacking)
			.with_pov(&pov)
			.with_candidate(candidate_hash)
			.with_relay_parent(self.parent);

		// Sanity check that candidate is from our assignment.
		if Some(candidate.descriptor().para_id) != self.assignment {
			gum::debug!(
				target: LOG_TARGET,
				our_assignment = ?self.assignment,
				collation = ?candidate.descriptor().para_id,
				"Subsystem asked to second for para outside of our assignment",
			);

			return Ok(())
		}

		// If the message is a `CandidateBackingMessage::Second`, sign and dispatch a
		// Seconded statement only if we have not seconded any other candidate and
		// have not signed a Valid statement for the requested candidate.
		if self.seconded.is_none() {
			// This job has not seconded a candidate yet.

			if !self.issued_statements.contains(&candidate_hash) {
				let pov = Arc::new(pov);
				self.validate_and_second(&span, &root_span, ctx, &candidate, pov).await?;
			}
		}

		Ok(())
	}

	async fn handle_statement_message(
		&mut self,
		root_span: &jaeger::Span,
		ctx: &mut Context,
		statement: SignedFullStatement,
	) -> Result<(), Error> {
		let _timer = self.metrics.time_process_statement();
		let _span = root_span
			.child("statement")
			.with_stage(jaeger::Stage::CandidateBacking)
			.with_candidate(statement.payload().candidate_hash())
			.with_relay_parent(self.parent);

		match self.maybe_validate_and_import(&root_span, ctx, statement).await {
			Err(Error::ValidationFailed(_)) => Ok(()),
			Err(e) => Err(e),
			Ok(()) => Ok(()),
		}
	}

	fn handle_get_backed_candidates_message(
		&mut self,
		requested_candidates: Vec<CandidateHash>,
		tx: oneshot::Sender<Vec<BackedCandidate>>,
	) -> Result<(), Error> {
		let _timer = self.metrics.time_get_backed_candidates();

		let backed = requested_candidates
			.into_iter()
			.filter_map(|hash| {
				self.table
					.attested_candidate(&hash, &self.table_context)
					.and_then(|attested| table_attested_to_backed(attested, &self.table_context))
			})
			.collect();

		tx.send(backed).map_err(|data| Error::Send(data))?;
		Ok(())
	}

	/// Kick off validation work and distribute the result as a signed statement.
	async fn kick_off_validation_work(
		&mut self,
		ctx: &mut Context,
		attesting: AttestingData,
		span: Option<jaeger::Span>,
	) -> Result<(), Error> {
		let candidate_hash = attesting.candidate.hash();
		if self.issued_statements.contains(&candidate_hash) {
			return Ok(())
		}

		let descriptor = attesting.candidate.descriptor().clone();

		gum::debug!(
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

		let bg_sender = ctx.sender().clone();
		let pov = PoVData::FetchFromValidator {
			from_validator: attesting.from_validator,
			candidate_hash,
			pov_hash: attesting.pov_hash,
		};
		self.background_validate_and_make_available(
			ctx,
			BackgroundValidationParams {
				sender: bg_sender,
				tx_command: self.background_validation_tx.clone(),
				candidate: attesting.candidate,
				relay_parent: self.parent,
				pov,
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
		ctx: &mut Context,
		statement: SignedFullStatement,
	) -> Result<(), Error> {
		if let Some(summary) = self.import_statement(ctx, &statement, root_span).await? {
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

			self.kick_off_validation_work(ctx, attesting, span).await?;
		}
		Ok(())
	}

	async fn sign_statement(&mut self, statement: Statement) -> Option<SignedFullStatement> {
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
