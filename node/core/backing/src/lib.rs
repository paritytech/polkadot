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

//! Implements the `CandidateBackingSubsystem`.
//!
//! This subsystem maintains the entire responsibility of tracking parachain
//! candidates which can be backed, as well as the issuance of statements
//! about candidates when run on a validator node.
//!
//! There are two types of statements: `Seconded` and `Valid`.
//! `Seconded` implies `Valid`, and nothing should be stated as
//! `Valid` unless its already been `Seconded`.
//!
//! Validators may only second candidates which fall under their own group
//! assignment, and they may only second one candidate per depth per active leaf.
//! Candidates which are stated as either `Second` or `Valid` by a majority of the
//! assigned group of validators may be backed on-chain and proceed to the availability
//! stage.
//!
//! Depth is a concept relating to asynchronous backing, by which validators
//! short sub-chains of candidates are backed and extended off-chain, and then placed
//! asynchronously into blocks of the relay chain as those are authored and as the
//! relay-chain state becomes ready for them. Asynchronous backing allows parachains to
//! grow mostly independently from the state of the relay chain, which gives more time for
//! parachains to be validated and thereby increases performance.
//!
//! Most of the work of asynchronous backing is handled by the Prospective Parachains
//! subsystem. The 'depth' of a parachain block with respect to a relay chain block is
//! a measure of how many parachain blocks are between the most recent included parachain block
//! in the post-state of the relay-chain block and the candidate. For instance,
//! a candidate that descends directly from the most recent parachain block in the relay-chain
//! state has depth 0. The child of that candidate would have depth 1. And so on.
//!
//! The candidate backing subsystem keeps track of a set of 'active leaves' which are the
//! most recent blocks in the relay-chain (which is in fact a tree) which could be built
//! upon. Depth is always measured against active leaves, and the valid relay-parent that
//! each candidate can have is determined by the active leaves. The Prospective Parachains
//! subsystem enforces that the relay-parent increases monotonoically, so that logic
//! is not handled here. By communicating with the Prospective Parachains subsystem,
//! this subsystem extrapolates an "implicit view" from the set of currently active leaves,
//! which determines the set of all recent relay-chain block hashes which could be relay-parents
//! for candidates backed in children of the active leaves.
//!
//! In fact, this subsystem relies on the Statement Distribution subsystem to prevent spam
//! by enforcing the rule that each validator may second at most one candidate per depth per
//! active leaf. This bounds the number of candidates that the system needs to consider and
//! is not handled within this subsystem, except for candidates seconded locally.
//!
//! This subsystem also handles relay-chain heads which don't support asynchronous backing.
//! For such active leaves, the only valid relay-parent is the leaf hash itself and the only
//! allowed depth is 0.

#![deny(unused_crate_dependencies)]

use std::{
	collections::{BTreeMap, HashMap, HashSet},
	sync::Arc,
};

use bitvec::vec::BitVec;
use futures::{
	channel::{mpsc, oneshot},
	future::BoxFuture,
	stream::FuturesOrdered,
	FutureExt, SinkExt, StreamExt, TryFutureExt,
};

use error::{Error, FatalResult};
use polkadot_node_primitives::{
	AvailableData, InvalidCandidate, PoV, SignedDisputeStatement, SignedFullStatementWithPVD,
	StatementWithPVD, ValidationResult, BACKING_EXECUTION_TIMEOUT,
};
use polkadot_node_subsystem::{
	messages::{
		AvailabilityDistributionMessage, AvailabilityStoreMessage, CandidateBackingMessage,
		CandidateValidationMessage, CollatorProtocolMessage, DisputeCoordinatorMessage,
		HypotheticalDepthRequest, ProspectiveParachainsMessage, ProvisionableData,
		ProvisionerMessage, RuntimeApiMessage, RuntimeApiRequest, StatementDistributionMessage,
	},
	overseer, ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, SubsystemError,
};
use polkadot_node_subsystem_util::{
	self as util,
	backing_implicit_view::{FetchError as ImplicitViewFetchError, View as ImplicitView},
	request_from_runtime, request_session_index_for_child, request_validator_groups,
	request_validators, Validator,
};
use polkadot_primitives::v2::{
	BackedCandidate, CandidateCommitments, CandidateHash, CandidateReceipt,
	CommittedCandidateReceipt, CoreIndex, CoreState, Hash, Id as ParaId, PersistedValidationData,
	SessionIndex, SigningContext, ValidationCode, ValidatorId, ValidatorIndex, ValidatorSignature,
	ValidityAttestation,
};
use sp_keystore::SyncCryptoStorePtr;
use statement_table::{
	generic::AttestedCandidate as TableAttestedCandidate,
	v2::{
		SignedStatement as TableSignedStatement, Statement as TableStatement,
		Summary as TableSummary,
	},
	Config as TableConfig, Context as TableContextTrait, Table,
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
			ValidatedCandidateCommand::Second(Ok(ref outputs)) => outputs.candidate.hash(),
			ValidatedCandidateCommand::Second(Err(ref candidate)) => candidate.hash(),
			ValidatedCandidateCommand::Attest(Ok(ref outputs)) => outputs.candidate.hash(),
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

struct PerRelayParentState {
	prospective_parachains_mode: ProspectiveParachainsMode,
	/// The hash of the relay parent on top of which this job is doing it's work.
	parent: Hash,
	/// The session index this corresponds to.
	session_index: SessionIndex,
	/// The `ParaId` assigned to the local validator at this relay parent.
	assignment: Option<ParaId>,
	/// The candidates that are backed by enough validators in their group, by hash.
	backed: HashSet<CandidateHash>,
	/// The table of candidates and statements under this relay-parent.
	table: Table<TableContext>,
	/// The table context, including groups.
	table_context: TableContext,
	/// We issued `Seconded` or `Valid` statements on about these candidates.
	issued_statements: HashSet<CandidateHash>,
	/// These candidates are undergoing validation in the background.
	awaiting_validation: HashSet<CandidateHash>,
	/// Data needed for retrying in case of `ValidatedCandidateCommand::AttestNoPoV`.
	fallbacks: HashMap<CandidateHash, AttestingData>,
}

struct PerCandidateState {
	persisted_validation_data: PersistedValidationData,
	seconded_locally: bool,
	para_id: ParaId,
	relay_parent: Hash,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ProspectiveParachainsMode {
	// v2 runtime API: no prospective parachains.
	Disabled,
	// vstaging runtime API: prospective parachains.
	Enabled,
}

impl ProspectiveParachainsMode {
	fn is_enabled(&self) -> bool {
		self == &ProspectiveParachainsMode::Enabled
	}
}

struct ActiveLeafState {
	prospective_parachains_mode: ProspectiveParachainsMode,
	/// The candidates seconded at various depths under this active
	/// leaf. A candidate can only be seconded when its hypothetical
	/// depth under every active leaf has an empty entry in this map.
	///
	/// When prospective parachains are disabled, the only depth
	/// which is allowed is '0'.
	seconded_at_depth: BTreeMap<usize, CandidateHash>,
}

/// The state of the subsystem.
struct State {
	/// The utility for managing the implicit and explicit views in a consistent way.
	///
	/// We only feed leaves which have prospective parachains enabled to this view.
	implicit_view: ImplicitView,
	/// State tracked for all active leaves, whether or not they have prospective parachains
	/// enabled.
	per_leaf: HashMap<Hash, ActiveLeafState>,
	/// State tracked for all relay-parents backing work is ongoing for. This includes
	/// all active leaves.
	///
	/// relay-parents fall into one of 3 categories.
	///   1. active leaves which do support prospective parachains
	///   2. active leaves which do not support prospective parachains
	///   3. relay-chain blocks which are ancestors of an active leaf and
	///      do support prospective parachains.
	///
	/// Relay-chain blocks which don't support prospective parachains are
	/// never included in the fragment trees of active leaves which do.
	///
	/// While it would be technically possible to support such leaves in
	/// fragment trees, it only benefits the transition period when asynchronous
	/// backing is being enabled and complicates code complexity.
	per_relay_parent: HashMap<Hash, PerRelayParentState>,
	/// State tracked for all candidates relevant to the implicit view.
	///
	/// This is guaranteed to have an entry for each candidate with a relay parent in the implicit
	/// or explicit view for which a `Seconded` statement has been successfully imported.
	per_candidate: HashMap<CandidateHash, PerCandidateState>,
	/// A cloneable sender which is dispatched to background candidate validation tasks to inform
	/// the main task of the result.
	background_validation_tx: mpsc::Sender<(Hash, ValidatedCandidateCommand)>,
	/// The handle to the keystore used for signing.
	keystore: SyncCryptoStorePtr,
}

impl State {
	fn new(
		background_validation_tx: mpsc::Sender<(Hash, ValidatedCandidateCommand)>,
		keystore: SyncCryptoStorePtr,
	) -> Self {
		State {
			implicit_view: ImplicitView::default(),
			per_leaf: HashMap::default(),
			per_relay_parent: HashMap::default(),
			per_candidate: HashMap::new(),
			background_validation_tx,
			keystore,
		}
	}
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn run<Context>(
	mut ctx: Context,
	keystore: SyncCryptoStorePtr,
	metrics: Metrics,
) -> FatalResult<()> {
	let (background_validation_tx, mut background_validation_rx) = mpsc::channel(16);
	let mut state = State::new(background_validation_tx, keystore);

	loop {
		let res =
			run_iteration(&mut ctx, &mut state, &metrics, &mut background_validation_rx).await;

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
	state: &mut State,
	metrics: &Metrics,
	background_validation_rx: &mut mpsc::Receiver<(Hash, ValidatedCandidateCommand)>,
) -> Result<(), Error> {
	loop {
		futures::select!(
			validated_command = background_validation_rx.next().fuse() => {
				if let Some((relay_parent, command)) = validated_command {
					handle_validated_candidate_command(
						&mut *ctx,
						state,
						relay_parent,
						command,
						metrics,
					).await?;
				} else {
					panic!("background_validation_tx always alive at this point; qed");
				}
			}
			from_overseer = ctx.recv().fuse() => {
				match from_overseer? {
					FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
						handle_active_leaves_update(
							&mut *ctx,
							update,
							state,
						).await?;
					}
					FromOverseer::Signal(OverseerSignal::BlockFinalized(..)) => {}
					FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
					FromOverseer::Communication { msg } => {
						handle_communication(&mut *ctx, state, msg, metrics).await?;
					}
				}
			}
		)
	}
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
		self.groups
			.get(group)
			.map_or(false, |g| g.iter().position(|a| a == authority).is_some())
	}

	fn requisite_votes(&self, group: &ParaId) -> usize {
		self.groups.get(group).map_or(usize::MAX, |g| minimum_votes(g.len()))
	}
}

struct InvalidErasureRoot;

// It looks like it's not possible to do an `impl From` given the current state of
// the code. So this does the necessary conversion.
fn primitive_statement_to_table(s: &SignedFullStatementWithPVD) -> TableSignedStatement {
	let statement = match s.payload() {
		StatementWithPVD::Seconded(c, _) => TableStatement::Seconded(c.clone()),
		StatementWithPVD::Valid(h) => TableStatement::Valid(h.clone()),
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
	validation_data: PersistedValidationData,
	expected_erasure_root: Hash,
) -> Result<Result<(), InvalidErasureRoot>, Error> {
	let available_data = AvailableData { pov, validation_data };

	{
		let chunks = erasure_coding::obtain_chunks_v1(n_validators, &available_data)?;

		let branches = erasure_coding::branches(chunks.as_ref());
		let erasure_root = branches.root();

		if erasure_root != expected_erasure_root {
			return Ok(Err(InvalidErasureRoot))
		}
	}

	{
		store_available_data(sender, n_validators as u32, candidate_hash, available_data).await?;
	}

	Ok(Ok(()))
}

async fn request_pov(
	sender: &mut impl overseer::CandidateBackingSenderTrait,
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
	sender: &mut impl overseer::CandidateBackingSenderTrait,
	pvd: PersistedValidationData,
	code: ValidationCode,
	candidate_receipt: CandidateReceipt,
	pov: Arc<PoV>,
) -> Result<ValidationResult, Error> {
	let (tx, rx) = oneshot::channel();

	sender
		.send_message(CandidateValidationMessage::ValidateFromExhaustive(
			pvd,
			code,
			candidate_receipt,
			pov,
			BACKING_EXECUTION_TIMEOUT,
			tx,
		))
		.await;

	match rx.await {
		Ok(Ok(validation_result)) => Ok(validation_result),
		Ok(Err(err)) => Err(Error::ValidationFailed(err)),
		Err(err) => Err(Error::ValidateFromExhaustive(err)),
	}
}

struct BackgroundValidationOutputs {
	candidate: CandidateReceipt,
	commitments: CandidateCommitments,
	persisted_validation_data: PersistedValidationData,
}

type BackgroundValidationResult = Result<BackgroundValidationOutputs, CandidateReceipt>;

struct BackgroundValidationParams<S: overseer::CandidateBackingSenderTrait, F> {
	sender: S,
	tx_command: mpsc::Sender<(Hash, ValidatedCandidateCommand)>,
	candidate: CandidateReceipt,
	relay_parent: Hash,
	persisted_validation_data: PersistedValidationData,
	pov: PoVData,
	n_validators: usize,
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
		persisted_validation_data,
		pov,
		n_validators,
		make_command,
	} = params;

	let validation_code = {
		let validation_code_hash = candidate.descriptor().validation_code_hash;
		let (tx, rx) = oneshot::channel();
		sender
			.send_message(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::ValidationCodeByHash(validation_code_hash, tx),
			))
			.await;

		let code = rx.await.map_err(Error::RuntimeApiUnavailable)?;
		match code {
			Err(e) => return Err(Error::FetchValidationCode(validation_code_hash, e)),
			Ok(None) => return Err(Error::NoValidationCode(validation_code_hash)),
			Ok(Some(c)) => c,
		}
	};

	let pov = match pov {
		PoVData::Ready(pov) => pov,
		PoVData::FetchFromValidator { from_validator, candidate_hash, pov_hash } =>
			match request_pov(&mut sender, relay_parent, from_validator, candidate_hash, pov_hash)
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
			},
	};

	let v = {
		request_candidate_validation(
			&mut sender,
			persisted_validation_data,
			validation_code,
			candidate.clone(),
			pov.clone(),
		)
		.await?
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
				validation_data.clone(),
				candidate.descriptor.erasure_root,
			)
			.await?;

			match erasure_valid {
				Ok(()) => Ok(BackgroundValidationOutputs {
					candidate,
					commitments,
					persisted_validation_data: validation_data,
				}),
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

struct ValidatorIndexOutOfBounds;

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn handle_communication<Context>(
	ctx: &mut Context,
	state: &mut State,
	message: CandidateBackingMessage,
	metrics: &Metrics,
) -> Result<(), Error> {
	match message {
		CandidateBackingMessage::Second(_relay_parent, candidate, pvd, pov) => {
			handle_second_message(ctx, state, candidate, pvd, pov, metrics).await?;
		},
		CandidateBackingMessage::Statement(relay_parent, statement) => {
			handle_statement_message(ctx, state, relay_parent, statement, metrics).await?;
		},
		CandidateBackingMessage::GetBackedCandidates(relay_parent, requested_candidates, tx) =>
			if let Some(rp_state) = state.per_relay_parent.get(&relay_parent) {
				handle_get_backed_candidates_message(rp_state, requested_candidates, tx, metrics)?;
			},
	}

	Ok(())
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn prospective_parachains_mode<Context>(
	ctx: &mut Context,
	leaf_hash: Hash,
) -> Result<ProspectiveParachainsMode, Error> {
	// TODO: call a Runtime API once staging version is available
	// https://github.com/paritytech/substrate/discussions/11338

	let (tx, rx) = oneshot::channel();
	ctx.send_message(RuntimeApiMessage::Request(leaf_hash, RuntimeApiRequest::Version(tx)))
		.await;

	let version = rx
		.await
		.map_err(Error::RuntimeApiUnavailable)?
		.map_err(Error::FetchRuntimeApiVersion)?;

	if version == 3 {
		Ok(ProspectiveParachainsMode::Enabled)
	} else {
		if version != 2 {
			gum::warn!(
				target: LOG_TARGET,
				"Runtime API version is {}, expected 2 or 3. Prospective parachains are disabled",
				version
			);
		}
		Ok(ProspectiveParachainsMode::Disabled)
	}
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn handle_active_leaves_update<Context>(
	ctx: &mut Context,
	update: ActiveLeavesUpdate,
	state: &mut State,
) -> Result<(), Error> {
	enum LeafHasProspectiveParachains {
		Enabled(Result<Vec<ParaId>, ImplicitViewFetchError>),
		Disabled,
	}

	// Activate in implicit view before deactivate, per the docs
	// on ImplicitView, this is more efficient.
	let res = if let Some(leaf) = update.activated {
		// Only activate in implicit view if prospective
		// parachains are enabled.
		let mode = prospective_parachains_mode(ctx, leaf.hash).await?;

		let leaf_hash = leaf.hash;
		Some((
			leaf,
			match mode {
				ProspectiveParachainsMode::Disabled => LeafHasProspectiveParachains::Disabled,
				ProspectiveParachainsMode::Enabled => LeafHasProspectiveParachains::Enabled(
					state.implicit_view.activate_leaf(ctx.sender(), leaf_hash).await,
				),
			},
		))
	} else {
		None
	};

	for deactivated in update.deactivated {
		state.per_leaf.remove(&deactivated);
		state.implicit_view.deactivate_leaf(deactivated);
	}

	// clean up `per_relay_parent` according to ancestry
	// of leaves. we do this so we can clean up candidates right after
	// as a result.
	//
	// when prospective parachains are disabled, the implicit view is empty,
	// which means we'll clean up everything. This is correct.
	for relay_parent in state.implicit_view.all_allowed_relay_parents() {
		state.per_relay_parent.remove(relay_parent);
	}

	// clean up `per_candidate` according to which relay-parents
	// are known.
	//
	// when prospective parachains are disabled, we clean up all candidates
	// because we've cleaned up all relay parents. this is correct.
	state
		.per_candidate
		.retain(|_, pc| state.per_relay_parent.contains_key(&pc.relay_parent));

	// Get relay parents which might be fresh but might be known already
	// that are explicit or implicit from the new active leaf.
	let fresh_relay_parents = match res {
		None => return Ok(()),
		Some((leaf, LeafHasProspectiveParachains::Disabled)) => {
			// defensive in this case - for enabled, this manifests as an error.
			if state.per_leaf.contains_key(&leaf.hash) {
				return Ok(())
			}

			state.per_leaf.insert(
				leaf.hash,
				ActiveLeafState {
					prospective_parachains_mode: ProspectiveParachainsMode::Disabled,
					// This is empty because the only allowed relay-parent and depth
					// when prospective parachains are disabled is the leaf hash and 0,
					// respectively. We've just learned about the leaf hash, so we cannot
					// have any candidates seconded with it as a relay-parent yet.
					seconded_at_depth: BTreeMap::new(),
				},
			);

			vec![leaf.hash]
		},
		Some((leaf, LeafHasProspectiveParachains::Enabled(Ok(_)))) => {
			let fresh_relay_parents =
				state.implicit_view.known_allowed_relay_parents_under(&leaf.hash, None);

			// At this point, all candidates outside of the implicit view
			// have been cleaned up. For all which remain, which we've seconded,
			// we ask the prospective parachains subsystem where they land in the fragment
			// tree for the given active leaf. This comprises our `seconded_at_depth`.

			let remaining_seconded = state
				.per_candidate
				.iter()
				.filter(|(_, cd)| cd.seconded_locally)
				.map(|(c_hash, cd)| (*c_hash, cd.para_id));

			// one-to-one correspondence to remaining_seconded
			let mut membership_answers = FuturesOrdered::new();

			for (candidate_hash, para_id) in remaining_seconded {
				let (tx, rx) = oneshot::channel();
				membership_answers.push(rx.map_ok(move |membership| (candidate_hash, membership)));

				ctx.send_message(ProspectiveParachainsMessage::GetTreeMembership(
					para_id,
					candidate_hash,
					tx,
				))
				.await;
			}

			let mut seconded_at_depth = BTreeMap::new();
			for response in membership_answers.next().await {
				match response {
					Err(oneshot::Canceled) => {
						gum::warn!(
							target: LOG_TARGET,
							"Prospective parachains subsystem unreachable for membership request",
						);

						continue
					},
					Ok((candidate_hash, membership)) => {
						// This request gives membership in all fragment trees. We have some
						// wasted data here, and it can be optimized if it proves
						// relevant to performance.
						if let Some((_, depths)) =
							membership.into_iter().find(|(leaf_hash, _)| leaf_hash == &leaf.hash)
						{
							for depth in depths {
								seconded_at_depth.insert(depth, candidate_hash);
							}
						}
					},
				}
			}

			state.per_leaf.insert(
				leaf.hash,
				ActiveLeafState {
					prospective_parachains_mode: ProspectiveParachainsMode::Enabled,
					seconded_at_depth,
				},
			);

			match fresh_relay_parents {
				Some(f) => f.to_vec(),
				None => {
					gum::warn!(
						target: LOG_TARGET,
						leaf_hash = ?leaf.hash,
						"Implicit view gave no relay-parents"
					);

					vec![leaf.hash]
				},
			}
		},
		Some((leaf, LeafHasProspectiveParachains::Enabled(Err(e)))) => {
			gum::debug!(
				target: LOG_TARGET,
				leaf_hash = ?leaf.hash,
				err = ?e,
				"Failed to load implicit view for leaf."
			);

			return Ok(())
		},
	};

	// add entries in `per_relay_parent`. for all new relay-parents.
	for maybe_new in fresh_relay_parents {
		if state.per_relay_parent.contains_key(&maybe_new) {
			continue
		}

		let mode = match state.per_leaf.get(&maybe_new) {
			None => {
				// If the relay-parent isn't a leaf itself,
				// then it is guaranteed by the prospective parachains
				// subsystem that it is an ancestor of a leaf which
				// has prospective parachains enabled and that the
				// block itself did.
				ProspectiveParachainsMode::Enabled
			},
			Some(l) => l.prospective_parachains_mode,
		};

		// construct a `PerRelayParent` from the runtime API
		// and insert it.
		let per = construct_per_relay_parent_state(ctx, maybe_new, &state.keystore, mode).await?;

		if let Some(per) = per {
			state.per_relay_parent.insert(maybe_new, per);
		}
	}

	Ok(())
}

/// Load the data necessary to do backing work on top of a relay-parent.
#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn construct_per_relay_parent_state<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	keystore: &SyncCryptoStorePtr,
	mode: ProspectiveParachainsMode,
) -> Result<Option<PerRelayParentState>, Error> {
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
					return Ok(None);
				}
			}
		}
	}

	let parent = relay_parent;

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

				return Ok(None)
			},
		};

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
	let table_config = TableConfig {
		allow_multiple_seconded: match mode {
			ProspectiveParachainsMode::Enabled => true,
			ProspectiveParachainsMode::Disabled => false,
		},
	};

	// TODO [now]: I've removed the `required_collator` more broadly,
	// because it's not used in practice and was intended for parathreads.
	//
	// We should attempt parathreads another way, I think, so it makes sense
	// to remove.
	let assignment = assignment.map(|(a, _required_collator)| a);

	Ok(Some(PerRelayParentState {
		prospective_parachains_mode: mode,
		parent,
		session_index,
		assignment,
		backed: HashSet::new(),
		table: Table::new(table_config),
		table_context,
		issued_statements: HashSet::new(),
		awaiting_validation: HashSet::new(),
		fallbacks: HashMap::new(),
	}))
}

enum SecondingAllowed {
	No,
	Yes(Vec<(Hash, Vec<usize>)>),
}

/// Checks whether a candidate can be seconded based on its hypothetical
/// depths in the fragment tree and what we've already seconded in all
/// active leaves.
#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn seconding_sanity_check<Context>(
	ctx: &mut Context,
	active_leaves: &HashMap<Hash, ActiveLeafState>,
	implicit_view: &ImplicitView,
	candidate_hash: CandidateHash,
	candidate_para: ParaId,
	parent_head_data_hash: Hash,
	head_data_hash: Hash,
	candidate_relay_parent: Hash,
) -> SecondingAllowed {
	// Note that `GetHypotheticalDepths` doesn't account for recursion,
	// i.e. candidates can appear at multiple depths in the tree and in fact
	// at all depths, and we don't know what depths a candidate will ultimately occupy
	// because that's dependent on other candidates we haven't yet received.
	//
	// The only way to effectively rule this out is to have candidate receipts
	// directly commit to the parachain block number or some other incrementing
	// counter. That requires a major primitives format upgrade, so for now
	// we just rule out trivial cycles.
	if parent_head_data_hash == head_data_hash {
		return SecondingAllowed::No
	}

	let mut membership = Vec::new();
	let mut responses = FuturesOrdered::<BoxFuture<'_, Result<_, oneshot::Canceled>>>::new();

	for (head, leaf_state) in active_leaves {
		if leaf_state.prospective_parachains_mode.is_enabled() {
			// Check that the candidate relay parent is allowed for para, skip the
			// leaf otherwise.
			let allowed_parents_for_para =
				implicit_view.known_allowed_relay_parents_under(head, Some(candidate_para));
			if !allowed_parents_for_para.unwrap_or_default().contains(&candidate_relay_parent) {
				continue
			}

			let (tx, rx) = oneshot::channel();
			ctx.send_message(ProspectiveParachainsMessage::GetHypotheticalDepth(
				HypotheticalDepthRequest {
					candidate_hash,
					candidate_para,
					parent_head_data_hash,
					candidate_relay_parent,
					fragment_tree_relay_parent: *head,
				},
				tx,
			))
			.await;
			responses.push(rx.map_ok(move |depths| (depths, head, leaf_state)).boxed());
		} else {
			if head == &candidate_relay_parent {
				if leaf_state.seconded_at_depth.contains_key(&0) {
					// The leaf is already occupied.
					return SecondingAllowed::No
				}
				responses.push(futures::future::ok((vec![0], head, leaf_state)).boxed());
			}
		}
	}

	if responses.is_empty() {
		return SecondingAllowed::No
	}

	while let Some(response) = responses.next().await {
		match response {
			Err(oneshot::Canceled) => {
				gum::warn!(
					target: LOG_TARGET,
					"Failed to reach prospective parachains subsystem for hypothetical depths",
				);

				return SecondingAllowed::No
			},
			Ok((depths, head, leaf_state)) => {
				for depth in &depths {
					if leaf_state.seconded_at_depth.contains_key(&depth) {
						gum::debug!(
							target: LOG_TARGET,
							?candidate_hash,
							depth,
							leaf_hash = ?head,
							"Refusing to second candidate at depth - already occupied."
						);

						return SecondingAllowed::No
					}
				}

				membership.push((*head, depths));
			},
		}
	}

	// At this point we've checked the depths of the candidate against all active
	// leaves.
	SecondingAllowed::Yes(membership)
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn handle_validated_candidate_command<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	command: ValidatedCandidateCommand,
	metrics: &Metrics,
) -> Result<(), Error> {
	match state.per_relay_parent.get_mut(&relay_parent) {
		Some(rp_state) => {
			let candidate_hash = command.candidate_hash();
			rp_state.awaiting_validation.remove(&candidate_hash);

			match command {
				ValidatedCandidateCommand::Second(res) => match res {
					Ok(outputs) => {
						let BackgroundValidationOutputs {
							candidate,
							commitments,
							persisted_validation_data,
						} = outputs;

						if rp_state.issued_statements.contains(&candidate_hash) {
							return Ok(())
						}

						// sanity check that we're allowed to second the candidate
						// and that it doesn't conflict with other candidates we've
						// seconded.
						let fragment_tree_membership = match seconding_sanity_check(
							ctx,
							&state.per_leaf,
							&state.implicit_view,
							candidate_hash,
							candidate.descriptor().para_id,
							persisted_validation_data.parent_head.hash(),
							commitments.head_data.hash(),
							candidate.descriptor().relay_parent,
						)
						.await
						{
							SecondingAllowed::No => return Ok(()),
							SecondingAllowed::Yes(membership) => membership,
						};

						let statement = StatementWithPVD::Seconded(
							CommittedCandidateReceipt {
								descriptor: candidate.descriptor.clone(),
								commitments,
							},
							persisted_validation_data,
						);

						// If we get an Error::RejectedByProspectiveParachains,
						// then the statement has not been distributed or imported into
						// the table.
						let res = sign_import_and_distribute_statement(
							ctx,
							rp_state,
							&mut state.per_candidate,
							statement,
							state.keystore.clone(),
							metrics,
						)
						.await;

						if let Err(Error::RejectedByProspectiveParachains) = res {
							let candidate_hash = candidate.hash();
							gum::debug!(
								target: LOG_TARGET,
								relay_parent = ?candidate.descriptor().relay_parent,
								?candidate_hash,
								"Attempted to second candidate but was rejected by prospective parachains",
							);

							// Ensure the collator is reported.
							ctx.send_message(CollatorProtocolMessage::Invalid(
								candidate.descriptor().relay_parent,
								candidate,
							))
							.await;

							return Ok(())
						}

						if let Some(stmt) = res? {
							match state.per_candidate.get_mut(&candidate_hash) {
								None => {
									gum::warn!(
										target: LOG_TARGET,
										?candidate_hash,
										"Missing `per_candidate` for seconded candidate.",
									);
								},
								Some(p) => p.seconded_locally = true,
							}

							// update seconded depths in active leaves.
							for (leaf, depths) in fragment_tree_membership {
								let leaf_data = match state.per_leaf.get_mut(&leaf) {
									None => {
										gum::warn!(
											target: LOG_TARGET,
											leaf_hash = ?leaf,
											"Missing `per_leaf` for known active leaf."
										);

										continue
									},
									Some(d) => d,
								};

								for depth in depths {
									leaf_data.seconded_at_depth.insert(depth, candidate_hash);
								}
							}

							rp_state.issued_statements.insert(candidate_hash);

							metrics.on_candidate_seconded();
							ctx.send_message(CollatorProtocolMessage::Seconded(
								rp_state.parent,
								StatementWithPVD::drop_pvd_from_signed(stmt),
							))
							.await;
						}
					},
					Err(candidate) => {
						ctx.send_message(CollatorProtocolMessage::Invalid(
							rp_state.parent,
							candidate,
						))
						.await;
					},
				},
				ValidatedCandidateCommand::Attest(res) => {
					// We are done - avoid new validation spawns:
					rp_state.fallbacks.remove(&candidate_hash);
					// sanity check.
					if !rp_state.issued_statements.contains(&candidate_hash) {
						if res.is_ok() {
							let statement = StatementWithPVD::Valid(candidate_hash);

							sign_import_and_distribute_statement(
								ctx,
								rp_state,
								&mut state.per_candidate,
								statement,
								state.keystore.clone(),
								metrics,
							)
							.await?;
						}
						rp_state.issued_statements.insert(candidate_hash);
					}
				},
				ValidatedCandidateCommand::AttestNoPoV(candidate_hash) => {
					if let Some(attesting) = rp_state.fallbacks.get_mut(&candidate_hash) {
						if let Some(index) = attesting.backing.pop() {
							attesting.from_validator = index;
							let attesting = attesting.clone();

							// The candidate state should be available because we've
							// validated it before, the relay-parent is still around,
							// and candidates are pruned on the basis of relay-parents.
							//
							// If it's not, then no point in validating it anyway.
							if let Some(pvd) = state
								.per_candidate
								.get(&candidate_hash)
								.map(|pc| pc.persisted_validation_data.clone())
							{
								kick_off_validation_work(
									ctx,
									rp_state,
									pvd,
									&state.background_validation_tx,
									attesting,
								)
								.await?;
							}
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
		},
		None => {
			// simple race condition; can be ignored = this relay-parent
			// is no longer relevant.
		},
	}

	Ok(())
}

async fn sign_statement(
	rp_state: &PerRelayParentState,
	statement: StatementWithPVD,
	keystore: SyncCryptoStorePtr,
	metrics: &Metrics,
) -> Option<SignedFullStatementWithPVD> {
	let signed = rp_state
		.table_context
		.validator
		.as_ref()?
		.sign(keystore, statement)
		.await
		.ok()
		.flatten()?;
	metrics.on_statement_signed();
	Some(signed)
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
#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn dispatch_new_statement_to_dispute_coordinator<Context>(
	ctx: &mut Context,
	rp_state: &PerRelayParentState,
	candidate_hash: CandidateHash,
	statement: &SignedFullStatementWithPVD,
) -> Result<(), ValidatorIndexOutOfBounds> {
	// Dispatch the statement to the dispute coordinator.
	let validator_index = statement.validator_index();
	let signing_context =
		SigningContext { parent_hash: rp_state.parent, session_index: rp_state.session_index };

	let validator_public = match rp_state.table_context.validators.get(validator_index.0 as usize) {
		None => return Err(ValidatorIndexOutOfBounds),
		Some(v) => v,
	};

	let maybe_candidate_receipt = match statement.payload() {
		StatementWithPVD::Seconded(receipt, _) => Some(receipt.to_plain()),
		StatementWithPVD::Valid(candidate_hash) => {
			// Valid statements are only supposed to be imported
			// once we've seen at least one `Seconded` statement.
			rp_state.table.get_candidate(&candidate_hash).map(|c| c.to_plain())
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
		ctx.send_message(DisputeCoordinatorMessage::ImportStatements {
			candidate_hash,
			candidate_receipt,
			session: rp_state.session_index,
			statements: vec![(dispute_statement, validator_index)],
			pending_confirmation: None,
		})
		.await;
	}

	Ok(())
}

/// Import a statement into the statement table and return the summary of the import.
///
/// This will fail with `Error::RejectedByProspectiveParachains` if the message type
/// is seconded, the candidate is fresh,
/// and any of the following are true:
/// 1. There is no `PersistedValidationData` attached.
/// 2. Prospective parachains are enabled for the relay parent and the prospective parachains
///    subsystem returned an empty `FragmentTreeMembership`
///    i.e. did not recognize the candidate as being applicable to any of the active leaves.
#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn import_statement<Context>(
	ctx: &mut Context,
	rp_state: &mut PerRelayParentState,
	per_candidate: &mut HashMap<CandidateHash, PerCandidateState>,
	statement: &SignedFullStatementWithPVD,
) -> Result<Option<TableSummary>, Error> {
	gum::debug!(
		target: LOG_TARGET,
		statement = ?statement.payload().to_compact(),
		validator_index = statement.validator_index().0,
		"Importing statement",
	);

	let candidate_hash = statement.payload().candidate_hash();

	// If this is a new candidate (statement is 'seconded' and candidate is unknown),
	// we need to create an entry in the `PerCandidateState` map.
	//
	// If the relay parent supports prospective parachains, we also need
	// to inform the prospective parachains subsystem of the seconded candidate
	// If `ProspectiveParachainsMessage::Second` fails, then we return
	// Error::RejectedByProspectiveParachains.
	//
	// Persisted Validation Data should be available - it may already be available
	// if this is a candidate we are seconding.
	//
	// We should also not accept any candidates which have no valid depths under any of
	// our active leaves.
	if let StatementWithPVD::Seconded(candidate, pvd) = statement.payload() {
		if !per_candidate.contains_key(&candidate_hash) {
			per_candidate.insert(
				candidate_hash,
				PerCandidateState {
					persisted_validation_data: pvd.clone(),
					// This is set after importing when seconding locally.
					seconded_locally: false,
					para_id: candidate.descriptor().para_id,
					relay_parent: candidate.descriptor().relay_parent,
				},
			);

			if rp_state.prospective_parachains_mode.is_enabled() {
				let (tx, rx) = oneshot::channel();
				ctx.send_message(ProspectiveParachainsMessage::CandidateSeconded(
					candidate.descriptor().para_id,
					candidate.clone(),
					pvd.clone(),
					tx,
				))
				.await;

				match rx.await {
					Err(oneshot::Canceled) => {
						gum::warn!(
							target: LOG_TARGET,
							"Could not reach the Prospective Parachains subsystem."
						);

						return Err(Error::RejectedByProspectiveParachains)
					},
					Ok(membership) =>
						if membership.is_empty() {
							return Err(Error::RejectedByProspectiveParachains)
						},
				}
			}
		}
	}

	if let Err(ValidatorIndexOutOfBounds) =
		dispatch_new_statement_to_dispute_coordinator(ctx, rp_state, candidate_hash, statement)
			.await
	{
		gum::warn!(
			target: LOG_TARGET,
			session_index = ?rp_state.session_index,
			relay_parent = ?rp_state.parent,
			validator_index = statement.validator_index().0,
			"Supposedly 'Signed' statement has validator index out of bounds."
		);

		return Ok(None)
	}

	let stmt = primitive_statement_to_table(statement);

	let summary = rp_state.table.import_statement(&rp_state.table_context, stmt);

	if let Some(attested) = summary
		.as_ref()
		.and_then(|s| rp_state.table.attested_candidate(&s.candidate, &rp_state.table_context))
	{
		// `HashSet::insert` returns true if the thing wasn't in there already.
		if rp_state.backed.insert(candidate_hash) {
			if let Some(backed) = table_attested_to_backed(attested, &rp_state.table_context) {
				let para_id = backed.candidate.descriptor.para_id;
				gum::debug!(
					target: LOG_TARGET,
					candidate_hash = ?candidate_hash,
					relay_parent = ?rp_state.parent,
					%para_id,
					"Candidate backed",
				);

				// Inform the prospective parachains subsystem
				// that the candidate is now backed.
				if rp_state.prospective_parachains_mode.is_enabled() {
					ctx.send_message(ProspectiveParachainsMessage::CandidateBacked(
						para_id,
						candidate_hash,
					))
					.await;
				}

				// The provisioner waits on candidate-backing, which means
				// that we need to send unbounded messages to avoid cycles.
				//
				// Backed candidates are bounded by the number of validators,
				// parachains, and the block production rate of the relay chain.
				let message = ProvisionerMessage::ProvisionableData(
					rp_state.parent,
					ProvisionableData::BackedCandidate(backed.receipt()),
				);
				ctx.send_unbounded_message(message);
			}
		}
	}

	issue_new_misbehaviors(ctx, rp_state.parent, &mut rp_state.table);

	Ok(summary)
}

/// Check if there have happened any new misbehaviors and issue necessary messages.
#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
fn issue_new_misbehaviors<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	table: &mut Table<TableContext>,
) {
	// collect the misbehaviors to avoid double mutable self borrow issues
	let misbehaviors: Vec<_> = table.drain_misbehaviors().collect();
	for (validator_id, report) in misbehaviors {
		// The provisioner waits on candidate-backing, which means
		// that we need to send unbounded messages to avoid cycles.
		//
		// Misbehaviors are bounded by the number of validators and
		// the block production protocol.
		ctx.send_unbounded_message(ProvisionerMessage::ProvisionableData(
			relay_parent,
			ProvisionableData::MisbehaviorReport(relay_parent, validator_id, report),
		));
	}
}

/// Sign, import, and distribute a statement.
#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn sign_import_and_distribute_statement<Context>(
	ctx: &mut Context,
	rp_state: &mut PerRelayParentState,
	per_candidate: &mut HashMap<CandidateHash, PerCandidateState>,
	statement: StatementWithPVD,
	keystore: SyncCryptoStorePtr,
	metrics: &Metrics,
) -> Result<Option<SignedFullStatementWithPVD>, Error> {
	if let Some(signed_statement) = sign_statement(&*rp_state, statement, keystore, metrics).await {
		import_statement(ctx, rp_state, per_candidate, &signed_statement).await?;

		let smsg = StatementDistributionMessage::Share(
			rp_state.parent,
			StatementWithPVD::drop_pvd_from_signed(signed_statement.clone()),
		);
		ctx.send_unbounded_message(smsg);

		Ok(Some(signed_statement))
	} else {
		Ok(None)
	}
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn background_validate_and_make_available<Context>(
	ctx: &mut Context,
	rp_state: &mut PerRelayParentState,
	params: BackgroundValidationParams<
		impl overseer::CandidateBackingSenderTrait,
		impl Fn(BackgroundValidationResult) -> ValidatedCandidateCommand + Send + 'static + Sync,
	>,
) -> Result<(), Error> {
	let candidate_hash = params.candidate.hash();
	if rp_state.awaiting_validation.insert(candidate_hash) {
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

/// Kick off validation work and distribute the result as a signed statement.
#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn kick_off_validation_work<Context>(
	ctx: &mut Context,
	rp_state: &mut PerRelayParentState,
	persisted_validation_data: PersistedValidationData,
	background_validation_tx: &mpsc::Sender<(Hash, ValidatedCandidateCommand)>,
	attesting: AttestingData,
) -> Result<(), Error> {
	let candidate_hash = attesting.candidate.hash();
	if rp_state.issued_statements.contains(&candidate_hash) {
		return Ok(())
	}

	gum::debug!(
		target: LOG_TARGET,
		candidate_hash = ?candidate_hash,
		candidate_receipt = ?attesting.candidate,
		"Kicking off validation",
	);

	let bg_sender = ctx.sender().clone();
	let pov = PoVData::FetchFromValidator {
		from_validator: attesting.from_validator,
		candidate_hash,
		pov_hash: attesting.pov_hash,
	};

	background_validate_and_make_available(
		ctx,
		rp_state,
		BackgroundValidationParams {
			sender: bg_sender,
			tx_command: background_validation_tx.clone(),
			candidate: attesting.candidate,
			relay_parent: rp_state.parent,
			persisted_validation_data,
			pov,
			n_validators: rp_state.table_context.validators.len(),
			make_command: ValidatedCandidateCommand::Attest,
		},
	)
	.await
}

/// Import the statement and kick off validation work if it is a part of our assignment.
#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn maybe_validate_and_import<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	statement: SignedFullStatementWithPVD,
) -> Result<(), Error> {
	let rp_state = match state.per_relay_parent.get_mut(&relay_parent) {
		Some(r) => r,
		None => {
			gum::trace!(
				target: LOG_TARGET,
				?relay_parent,
				"Received statement for unknown relay-parent"
			);

			return Ok(())
		},
	};

	let res = import_statement(ctx, rp_state, &mut state.per_candidate, &statement).await;

	// if we get an Error::RejectedByProspectiveParachains,
	// we will do nothing.
	if let Err(Error::RejectedByProspectiveParachains) = res {
		gum::debug!(
			target: LOG_TARGET,
			?relay_parent,
			"Statement rejected by prospective parachains."
		);

		return Ok(())
	}

	if let Some(summary) = res? {
		// import_statement already takes care of communicating with the
		// prospective parachains subsystem. At this point, the candidate
		// has already been accepted into the fragment trees.

		let candidate_hash = summary.candidate;

		if Some(summary.group_id) != rp_state.assignment {
			return Ok(())
		}
		let attesting = match statement.payload() {
			StatementWithPVD::Seconded(receipt, _) => {
				let attesting = AttestingData {
					candidate: rp_state
						.table
						.get_candidate(&candidate_hash)
						.ok_or(Error::CandidateNotFound)?
						.to_plain(),
					pov_hash: receipt.descriptor.pov_hash,
					from_validator: statement.validator_index(),
					backing: Vec::new(),
				};
				rp_state.fallbacks.insert(summary.candidate, attesting.clone());
				attesting
			},
			StatementWithPVD::Valid(candidate_hash) => {
				if let Some(attesting) = rp_state.fallbacks.get_mut(candidate_hash) {
					let our_index = rp_state.table_context.validator.as_ref().map(|v| v.index());
					if our_index == Some(statement.validator_index()) {
						return Ok(())
					}

					if rp_state.awaiting_validation.contains(candidate_hash) {
						// Job already running:
						attesting.backing.push(statement.validator_index());
						return Ok(())
					} else {
						// No job, so start another with current validator:
						attesting.from_validator = statement.validator_index();
						attesting.clone()
					}
				} else {
					return Ok(())
				}
			},
		};

		// After `import_statement` succeeds, the candidate entry is guaranteed
		// to exist.
		if let Some(pvd) = state
			.per_candidate
			.get(&candidate_hash)
			.map(|pc| pc.persisted_validation_data.clone())
		{
			kick_off_validation_work(
				ctx,
				rp_state,
				pvd,
				&state.background_validation_tx,
				attesting,
			)
			.await?;
		}
	}
	Ok(())
}

/// Kick off background validation with intent to second.
#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn validate_and_second<Context>(
	ctx: &mut Context,
	rp_state: &mut PerRelayParentState,
	persisted_validation_data: PersistedValidationData,
	candidate: &CandidateReceipt,
	pov: Arc<PoV>,
	background_validation_tx: &mpsc::Sender<(Hash, ValidatedCandidateCommand)>,
) -> Result<(), Error> {
	let candidate_hash = candidate.hash();

	gum::debug!(
		target: LOG_TARGET,
		candidate_hash = ?candidate_hash,
		candidate_receipt = ?candidate,
		"Validate and second candidate",
	);

	let bg_sender = ctx.sender().clone();
	background_validate_and_make_available(
		ctx,
		rp_state,
		BackgroundValidationParams {
			sender: bg_sender,
			tx_command: background_validation_tx.clone(),
			candidate: candidate.clone(),
			relay_parent: rp_state.parent,
			persisted_validation_data,
			pov: PoVData::Ready(pov),
			n_validators: rp_state.table_context.validators.len(),
			make_command: ValidatedCandidateCommand::Second,
		},
	)
	.await?;

	Ok(())
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn handle_second_message<Context>(
	ctx: &mut Context,
	state: &mut State,
	candidate: CandidateReceipt,
	persisted_validation_data: PersistedValidationData,
	pov: PoV,
	metrics: &Metrics,
) -> Result<(), Error> {
	let _timer = metrics.time_process_second();

	let candidate_hash = candidate.hash();
	let relay_parent = candidate.descriptor().relay_parent;

	if candidate.descriptor().persisted_validation_data_hash != persisted_validation_data.hash() {
		gum::warn!(
			target: LOG_TARGET,
			?candidate_hash,
			"Candidate backing was asked to second candidate with wrong PVD",
		);

		return Ok(())
	}

	let rp_state = match state.per_relay_parent.get_mut(&relay_parent) {
		None => {
			gum::trace!(
				target: LOG_TARGET,
				?relay_parent,
				?candidate_hash,
				"We were asked to second a candidate outside of our view."
			);

			return Ok(())
		},
		Some(r) => r,
	};

	// Sanity check that candidate is from our assignment.
	if Some(candidate.descriptor().para_id) != rp_state.assignment {
		gum::debug!(
			target: LOG_TARGET,
			our_assignment = ?rp_state.assignment,
			collation = ?candidate.descriptor().para_id,
			"Subsystem asked to second for para outside of our assignment",
		);

		return Ok(())
	}

	// If the message is a `CandidateBackingMessage::Second`, sign and dispatch a
	// Seconded statement only if we have not signed a Valid statement for the requested candidate.
	//
	// The actual logic of issuing the signed statement checks that this isn't
	// conflicting with other seconded candidates. Not doing that check here
	// gives other subsystems the ability to get us to execute arbitrary candidates,
	// but no more.
	if !rp_state.issued_statements.contains(&candidate_hash) {
		let pov = Arc::new(pov);

		validate_and_second(
			ctx,
			rp_state,
			persisted_validation_data,
			&candidate,
			pov,
			&state.background_validation_tx,
		)
		.await?;
	}

	Ok(())
}

#[overseer::contextbounds(CandidateBacking, prefix = self::overseer)]
async fn handle_statement_message<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	statement: SignedFullStatementWithPVD,
	metrics: &Metrics,
) -> Result<(), Error> {
	let _timer = metrics.time_process_statement();

	match maybe_validate_and_import(ctx, state, relay_parent, statement).await {
		Err(Error::ValidationFailed(_)) => Ok(()),
		Err(e) => Err(e),
		Ok(()) => Ok(()),
	}
}

fn handle_get_backed_candidates_message(
	rp_state: &PerRelayParentState,
	requested_candidates: Vec<CandidateHash>,
	tx: oneshot::Sender<Vec<BackedCandidate>>,
	metrics: &Metrics,
) -> Result<(), Error> {
	let _timer = metrics.time_get_backed_candidates();

	let backed = requested_candidates
		.into_iter()
		.filter_map(|hash| {
			rp_state
				.table
				.attested_candidate(&hash, &rp_state.table_context)
				.and_then(|attested| table_attested_to_backed(attested, &rp_state.table_context))
		})
		.collect();

	tx.send(backed).map_err(|data| Error::Send(data))?;
	Ok(())
}
