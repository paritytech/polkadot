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

//! The Approval Voting Subsystem.
//!
//! This subsystem is responsible for determining candidates to do approval checks
//! on, performing those approval checks, and tracking the assignments and approvals
//! of others. It uses this information to determine when candidates and blocks have
//! been sufficiently approved to finalize.

use polkadot_node_jaeger as jaeger;
use polkadot_node_primitives::{
	approval::{
		BlockApprovalMeta, DelayTranche, IndirectAssignmentCert, IndirectSignedApprovalVote,
	},
	ValidationResult, APPROVAL_EXECUTION_TIMEOUT,
};
use polkadot_node_subsystem::{
	errors::RecoveryError,
	messages::{
		ApprovalCheckError, ApprovalCheckResult, ApprovalDistributionMessage,
		ApprovalVotingMessage, AssignmentCheckError, AssignmentCheckResult,
		AvailabilityRecoveryMessage, BlockDescription, CandidateValidationMessage, ChainApiMessage,
		ChainSelectionMessage, DisputeCoordinatorMessage, HighestApprovedAncestorBlock,
		RuntimeApiMessage, RuntimeApiRequest,
	},
	overseer, FromOrchestra, OverseerSignal, SpawnedSubsystem, SubsystemError, SubsystemResult,
	SubsystemSender,
};
use polkadot_node_subsystem_util::{
	database::Database,
	metrics::{self, prometheus},
	rolling_session_window::{
		DatabaseParams, RollingSessionWindow, SessionWindowUpdate, SessionsUnavailable,
	},
	TimeoutExt,
};
use polkadot_primitives::v2::{
	ApprovalVote, BlockNumber, CandidateHash, CandidateIndex, CandidateReceipt, DisputeStatement,
	GroupIndex, Hash, SessionIndex, SessionInfo, ValidDisputeStatementKind, ValidatorId,
	ValidatorIndex, ValidatorPair, ValidatorSignature,
};
use sc_keystore::LocalKeystore;
use sp_application_crypto::Pair;
use sp_consensus::SyncOracle;
use sp_consensus_slots::Slot;

use futures::{
	channel::oneshot,
	future::{BoxFuture, RemoteHandle},
	prelude::*,
	stream::FuturesUnordered,
};

use std::{
	collections::{
		btree_map::Entry as BTMEntry, hash_map::Entry as HMEntry, BTreeMap, HashMap, HashSet,
	},
	num::NonZeroUsize,
	sync::Arc,
	time::Duration,
};

use approval_checking::RequiredTranches;
use criteria::{AssignmentCriteria, RealAssignmentCriteria};
use persisted_entries::{ApprovalEntry, BlockEntry, CandidateEntry};
use time::{slot_number_to_tick, Clock, ClockExt, SystemClock, Tick};

mod approval_checking;
mod approval_db;
mod backend;
mod criteria;
mod import;
mod ops;
mod persisted_entries;
mod time;

use crate::{
	approval_db::v1::{Config as DatabaseConfig, DbBackend},
	backend::{Backend, OverlayedBackend},
};

#[cfg(test)]
mod tests;

const APPROVAL_CHECKING_TIMEOUT: Duration = Duration::from_secs(120);
/// How long are we willing to wait for approval signatures?
///
/// Value rather arbitrarily: Should not be hit in practice, it exists to more easily diagnose dead
/// lock issues for example.
const WAIT_FOR_SIGS_TIMEOUT: Duration = Duration::from_millis(500);
const APPROVAL_CACHE_SIZE: NonZeroUsize = match NonZeroUsize::new(1024) {
	Some(cap) => cap,
	None => panic!("Approval cache size must be non-zero."),
};

const TICK_TOO_FAR_IN_FUTURE: Tick = 20; // 10 seconds.
const APPROVAL_DELAY: Tick = 2;
const LOG_TARGET: &str = "parachain::approval-voting";

/// Configuration for the approval voting subsystem
#[derive(Debug, Clone)]
pub struct Config {
	/// The column family in the DB where approval-voting data is stored.
	pub col_approval_data: u32,
	/// The of the DB where rolling session info is stored.
	pub col_session_data: u32,
	/// The slot duration of the consensus algorithm, in milliseconds. Should be evenly
	/// divisible by 500.
	pub slot_duration_millis: u64,
}

// The mode of the approval voting subsystem. It should start in a `Syncing` mode when it first
// starts, and then once it's reached the head of the chain it should move into the `Active` mode.
//
// In `Active` mode, the node is an active participant in the approvals protocol. When syncing,
// the node follows the new incoming blocks and finalized number, but does not yet participate.
//
// When transitioning from `Syncing` to `Active`, the node notifies the `ApprovalDistribution`
// subsystem of all unfinalized blocks and the candidates included within them, as well as all
// votes that the local node itself has cast on candidates within those blocks.
enum Mode {
	Active,
	Syncing(Box<dyn SyncOracle + Send>),
}

/// The approval voting subsystem.
pub struct ApprovalVotingSubsystem {
	/// `LocalKeystore` is needed for assignment keys, but not necessarily approval keys.
	///
	/// We do a lot of VRF signing and need the keys to have low latency.
	keystore: Arc<LocalKeystore>,
	db_config: DatabaseConfig,
	slot_duration_millis: u64,
	db: Arc<dyn Database>,
	mode: Mode,
	metrics: Metrics,
}

#[derive(Clone)]
struct MetricsInner {
	imported_candidates_total: prometheus::Counter<prometheus::U64>,
	assignments_produced: prometheus::Histogram,
	approvals_produced_total: prometheus::CounterVec<prometheus::U64>,
	no_shows_total: prometheus::Counter<prometheus::U64>,
	wakeups_triggered_total: prometheus::Counter<prometheus::U64>,
	candidate_approval_time_ticks: prometheus::Histogram,
	block_approval_time_ticks: prometheus::Histogram,
	time_db_transaction: prometheus::Histogram,
	time_recover_and_approve: prometheus::Histogram,
	candidate_signatures_requests_total: prometheus::Counter<prometheus::U64>,
}

/// Approval Voting metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_candidate_imported(&self) {
		if let Some(metrics) = &self.0 {
			metrics.imported_candidates_total.inc();
		}
	}

	fn on_assignment_produced(&self, tranche: DelayTranche) {
		if let Some(metrics) = &self.0 {
			metrics.assignments_produced.observe(tranche as f64);
		}
	}

	fn on_approval_stale(&self) {
		if let Some(metrics) = &self.0 {
			metrics.approvals_produced_total.with_label_values(&["stale"]).inc()
		}
	}

	fn on_approval_invalid(&self) {
		if let Some(metrics) = &self.0 {
			metrics.approvals_produced_total.with_label_values(&["invalid"]).inc()
		}
	}

	fn on_approval_unavailable(&self) {
		if let Some(metrics) = &self.0 {
			metrics.approvals_produced_total.with_label_values(&["unavailable"]).inc()
		}
	}

	fn on_approval_error(&self) {
		if let Some(metrics) = &self.0 {
			metrics.approvals_produced_total.with_label_values(&["internal error"]).inc()
		}
	}

	fn on_approval_produced(&self) {
		if let Some(metrics) = &self.0 {
			metrics.approvals_produced_total.with_label_values(&["success"]).inc()
		}
	}

	fn on_no_shows(&self, n: usize) {
		if let Some(metrics) = &self.0 {
			metrics.no_shows_total.inc_by(n as u64);
		}
	}

	fn on_wakeup(&self) {
		if let Some(metrics) = &self.0 {
			metrics.wakeups_triggered_total.inc();
		}
	}

	fn on_candidate_approved(&self, ticks: Tick) {
		if let Some(metrics) = &self.0 {
			metrics.candidate_approval_time_ticks.observe(ticks as f64);
		}
	}

	fn on_block_approved(&self, ticks: Tick) {
		if let Some(metrics) = &self.0 {
			metrics.block_approval_time_ticks.observe(ticks as f64);
		}
	}

	fn on_candidate_signatures_request(&self) {
		if let Some(metrics) = &self.0 {
			metrics.candidate_signatures_requests_total.inc();
		}
	}

	fn time_db_transaction(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.time_db_transaction.start_timer())
	}

	fn time_recover_and_approve(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.time_recover_and_approve.start_timer())
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(
		registry: &prometheus::Registry,
	) -> std::result::Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			imported_candidates_total: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_imported_candidates_total",
					"Number of candidates imported by the approval voting subsystem",
				)?,
				registry,
			)?,
			assignments_produced: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_assignments_produced",
						"Assignments and tranches produced by the approval voting subsystem",
					).buckets(vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 10.0, 15.0, 25.0, 40.0, 70.0]),
				)?,
				registry,
			)?,
			approvals_produced_total: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"polkadot_parachain_approvals_produced_total",
						"Number of approvals produced by the approval voting subsystem",
					),
					&["status"]
				)?,
				registry,
			)?,
			no_shows_total: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_approvals_no_shows_total",
					"Number of assignments which became no-shows in the approval voting subsystem",
				)?,
				registry,
			)?,
			wakeups_triggered_total: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_approvals_wakeups_total",
					"Number of times we woke up to process a candidate in the approval voting subsystem",
				)?,
				registry,
			)?,
			candidate_approval_time_ticks: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_approvals_candidate_approval_time_ticks",
						"Number of ticks (500ms) to approve candidates.",
					).buckets(vec![6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 72.0, 100.0, 144.0]),
				)?,
				registry,
			)?,
			block_approval_time_ticks: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_approvals_blockapproval_time_ticks",
						"Number of ticks (500ms) to approve blocks.",
					).buckets(vec![6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 72.0, 100.0, 144.0]),
				)?,
				registry,
			)?,
			time_db_transaction: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_time_approval_db_transaction",
						"Time spent writing an approval db transaction.",
					)
				)?,
				registry,
			)?,
			time_recover_and_approve: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"polkadot_parachain_time_recover_and_approve",
						"Time spent recovering and approving data in approval voting",
					)
				)?,
				registry,
			)?,
			candidate_signatures_requests_total: prometheus::register(
				prometheus::Counter::new(
					"polkadot_parachain_approval_candidate_signatures_requests_total",
					"Number of times signatures got requested by other subsystems",
				)?,
				registry,
			)?,
		};

		Ok(Metrics(Some(metrics)))
	}
}

impl ApprovalVotingSubsystem {
	/// Create a new approval voting subsystem with the given keystore, config, and database.
	pub fn with_config(
		config: Config,
		db: Arc<dyn Database>,
		keystore: Arc<LocalKeystore>,
		sync_oracle: Box<dyn SyncOracle + Send>,
		metrics: Metrics,
	) -> Self {
		ApprovalVotingSubsystem {
			keystore,
			slot_duration_millis: config.slot_duration_millis,
			db,
			db_config: DatabaseConfig {
				col_approval_data: config.col_approval_data,
				col_session_data: config.col_session_data,
			},
			mode: Mode::Syncing(sync_oracle),
			metrics,
		}
	}

	/// Revert to the block corresponding to the specified `hash`.
	/// The operation is not allowed for blocks older than the last finalized one.
	pub fn revert_to(&self, hash: Hash) -> Result<(), SubsystemError> {
		let config = approval_db::v1::Config {
			col_approval_data: self.db_config.col_approval_data,
			col_session_data: self.db_config.col_session_data,
		};
		let mut backend = approval_db::v1::DbBackend::new(self.db.clone(), config);
		let mut overlay = OverlayedBackend::new(&backend);

		ops::revert_to(&mut overlay, hash)?;

		let ops = overlay.into_write_ops();
		backend.write(ops)
	}
}

// Checks and logs approval vote db state. It is perfectly normal to start with an
// empty approval vote DB if we changed DB type or the node will sync from scratch.
fn db_sanity_check(db: Arc<dyn Database>, config: DatabaseConfig) -> SubsystemResult<()> {
	let backend = DbBackend::new(db, config);
	let all_blocks = backend.load_all_blocks()?;

	if all_blocks.is_empty() {
		gum::info!(target: LOG_TARGET, "Starting with an empty approval vote DB.",);
	} else {
		gum::debug!(
			target: LOG_TARGET,
			"Starting with {} blocks in approval vote DB.",
			all_blocks.len()
		);
	}

	Ok(())
}

#[overseer::subsystem(ApprovalVoting, error = SubsystemError, prefix = self::overseer)]
impl<Context: Send> ApprovalVotingSubsystem {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let backend = DbBackend::new(self.db.clone(), self.db_config);
		let future = run::<DbBackend, Context>(
			ctx,
			self,
			Box::new(SystemClock),
			Box::new(RealAssignmentCriteria),
			backend,
		)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))
		.boxed();

		SpawnedSubsystem { name: "approval-voting-subsystem", future }
	}
}

#[derive(Debug, Clone)]
struct ApprovalVoteRequest {
	validator_index: ValidatorIndex,
	block_hash: Hash,
}

#[derive(Default)]
struct Wakeups {
	// Tick -> [(Relay Block, Candidate Hash)]
	wakeups: BTreeMap<Tick, Vec<(Hash, CandidateHash)>>,
	reverse_wakeups: HashMap<(Hash, CandidateHash), Tick>,
	block_numbers: BTreeMap<BlockNumber, HashSet<Hash>>,
}

impl Wakeups {
	// Returns the first tick there exist wakeups for, if any.
	fn first(&self) -> Option<Tick> {
		self.wakeups.keys().next().map(|t| *t)
	}

	fn note_block(&mut self, block_hash: Hash, block_number: BlockNumber) {
		self.block_numbers.entry(block_number).or_default().insert(block_hash);
	}

	// Schedules a wakeup at the given tick. no-op if there is already an earlier or equal wake-up
	// for these values. replaces any later wakeup.
	fn schedule(
		&mut self,
		block_hash: Hash,
		block_number: BlockNumber,
		candidate_hash: CandidateHash,
		tick: Tick,
	) {
		if let Some(prev) = self.reverse_wakeups.get(&(block_hash, candidate_hash)) {
			if prev <= &tick {
				return
			}

			// we are replacing previous wakeup with an earlier one.
			if let BTMEntry::Occupied(mut entry) = self.wakeups.entry(*prev) {
				if let Some(pos) =
					entry.get().iter().position(|x| x == &(block_hash, candidate_hash))
				{
					entry.get_mut().remove(pos);
				}

				if entry.get().is_empty() {
					let _ = entry.remove_entry();
				}
			}
		} else {
			self.note_block(block_hash, block_number);
		}

		self.reverse_wakeups.insert((block_hash, candidate_hash), tick);
		self.wakeups.entry(tick).or_default().push((block_hash, candidate_hash));
	}

	fn prune_finalized_wakeups(&mut self, finalized_number: BlockNumber) {
		let after = self.block_numbers.split_off(&(finalized_number + 1));
		let pruned_blocks: HashSet<_> = std::mem::replace(&mut self.block_numbers, after)
			.into_iter()
			.flat_map(|(_number, hashes)| hashes)
			.collect();

		let mut pruned_wakeups = BTreeMap::new();
		self.reverse_wakeups.retain(|&(ref h, ref c_h), tick| {
			let live = !pruned_blocks.contains(h);
			if !live {
				pruned_wakeups.entry(*tick).or_insert_with(HashSet::new).insert((*h, *c_h));
			}
			live
		});

		for (tick, pruned) in pruned_wakeups {
			if let BTMEntry::Occupied(mut entry) = self.wakeups.entry(tick) {
				entry.get_mut().retain(|wakeup| !pruned.contains(wakeup));
				if entry.get().is_empty() {
					let _ = entry.remove();
				}
			}
		}
	}

	// Get the wakeup for a particular block/candidate combo, if any.
	fn wakeup_for(&self, block_hash: Hash, candidate_hash: CandidateHash) -> Option<Tick> {
		self.reverse_wakeups.get(&(block_hash, candidate_hash)).map(|t| *t)
	}

	// Returns the next wakeup. this future never returns if there are no wakeups.
	async fn next(&mut self, clock: &(dyn Clock + Sync)) -> (Tick, Hash, CandidateHash) {
		match self.first() {
			None => future::pending().await,
			Some(tick) => {
				clock.wait(tick).await;
				match self.wakeups.entry(tick) {
					BTMEntry::Vacant(_) => {
						panic!("entry is known to exist since `first` was `Some`; qed")
					},
					BTMEntry::Occupied(mut entry) => {
						let (hash, candidate_hash) = entry.get_mut().pop()
							.expect("empty entries are removed here and in `schedule`; no other mutation of this map; qed");

						if entry.get().is_empty() {
							let _ = entry.remove();
						}

						self.reverse_wakeups.remove(&(hash, candidate_hash));

						(tick, hash, candidate_hash)
					},
				}
			},
		}
	}
}

struct ApprovalStatus {
	required_tranches: RequiredTranches,
	tranche_now: DelayTranche,
	block_tick: Tick,
}

#[derive(Copy, Clone)]
enum ApprovalOutcome {
	Approved,
	Failed,
	TimedOut,
}

struct ApprovalState {
	validator_index: ValidatorIndex,
	candidate_hash: CandidateHash,
	approval_outcome: ApprovalOutcome,
}

impl ApprovalState {
	fn approved(validator_index: ValidatorIndex, candidate_hash: CandidateHash) -> Self {
		Self { validator_index, candidate_hash, approval_outcome: ApprovalOutcome::Approved }
	}
	fn failed(validator_index: ValidatorIndex, candidate_hash: CandidateHash) -> Self {
		Self { validator_index, candidate_hash, approval_outcome: ApprovalOutcome::Failed }
	}
}

struct CurrentlyCheckingSet {
	candidate_hash_map: HashMap<CandidateHash, HashSet<Hash>>,
	currently_checking: FuturesUnordered<BoxFuture<'static, ApprovalState>>,
}

impl Default for CurrentlyCheckingSet {
	fn default() -> Self {
		Self { candidate_hash_map: HashMap::new(), currently_checking: FuturesUnordered::new() }
	}
}

impl CurrentlyCheckingSet {
	// This function will lazily launch approval voting work whenever the
	// candidate is not already undergoing validation.
	pub async fn insert_relay_block_hash(
		&mut self,
		candidate_hash: CandidateHash,
		validator_index: ValidatorIndex,
		relay_block: Hash,
		launch_work: impl Future<Output = SubsystemResult<RemoteHandle<ApprovalState>>>,
	) -> SubsystemResult<()> {
		match self.candidate_hash_map.entry(candidate_hash) {
			HMEntry::Occupied(mut entry) => {
				// validation already undergoing. just add the relay hash if unknown.
				entry.get_mut().insert(relay_block);
			},
			HMEntry::Vacant(entry) => {
				// validation not ongoing. launch work and time out the remote handle.
				entry.insert(HashSet::new()).insert(relay_block);
				let work = launch_work.await?;
				self.currently_checking.push(Box::pin(async move {
					match work.timeout(APPROVAL_CHECKING_TIMEOUT).await {
						None => ApprovalState {
							candidate_hash,
							validator_index,
							approval_outcome: ApprovalOutcome::TimedOut,
						},
						Some(approval_state) => approval_state,
					}
				}));
			},
		}

		Ok(())
	}

	pub async fn next(
		&mut self,
		approvals_cache: &mut lru::LruCache<CandidateHash, ApprovalOutcome>,
	) -> (HashSet<Hash>, ApprovalState) {
		if !self.currently_checking.is_empty() {
			if let Some(approval_state) = self.currently_checking.next().await {
				let out = self
					.candidate_hash_map
					.remove(&approval_state.candidate_hash)
					.unwrap_or_default();
				approvals_cache.put(approval_state.candidate_hash, approval_state.approval_outcome);
				return (out, approval_state)
			}
		}

		future::pending().await
	}
}

struct State {
	session_window: Option<RollingSessionWindow>,
	keystore: Arc<LocalKeystore>,
	slot_duration_millis: u64,
	clock: Box<dyn Clock + Send + Sync>,
	assignment_criteria: Box<dyn AssignmentCriteria + Send + Sync>,
	// Require for `RollingSessionWindow`.
	db_config: DatabaseConfig,
	db: Arc<dyn Database>,
}

#[overseer::contextbounds(ApprovalVoting, prefix = self::overseer)]
impl State {
	fn session_info(&self, i: SessionIndex) -> Option<&SessionInfo> {
		self.session_window.as_ref().and_then(|w| w.session_info(i))
	}

	/// Bring `session_window` up to date.
	pub async fn cache_session_info_for_head<Context>(
		&mut self,
		ctx: &mut Context,
		head: Hash,
	) -> Result<Option<SessionWindowUpdate>, SessionsUnavailable>
	where
		<Context as overseer::SubsystemContext>::Sender: Sized + Send,
	{
		let session_window = self.session_window.take();
		match session_window {
			None => {
				let sender = ctx.sender().clone();
				self.session_window = Some(
					RollingSessionWindow::new(
						sender,
						head,
						DatabaseParams {
							db: self.db.clone(),
							db_column: self.db_config.col_session_data,
						},
					)
					.await?,
				);
				Ok(None)
			},
			Some(mut session_window) => {
				let r = session_window
					.cache_session_info_for_head(ctx.sender(), head)
					.await
					.map(Option::Some);
				self.session_window = Some(session_window);
				r
			},
		}
	}
	// Compute the required tranches for approval for this block and candidate combo.
	// Fails if there is no approval entry for the block under the candidate or no candidate entry
	// under the block, or if the session is out of bounds.
	fn approval_status<'a, 'b>(
		&'a self,
		block_entry: &'a BlockEntry,
		candidate_entry: &'b CandidateEntry,
	) -> Option<(&'b ApprovalEntry, ApprovalStatus)> {
		let session_info = match self.session_info(block_entry.session()) {
			Some(s) => s,
			None => {
				gum::warn!(
					target: LOG_TARGET,
					"Unknown session info for {}",
					block_entry.session()
				);
				return None
			},
		};
		let block_hash = block_entry.block_hash();

		let tranche_now = self.clock.tranche_now(self.slot_duration_millis, block_entry.slot());
		let block_tick = slot_number_to_tick(self.slot_duration_millis, block_entry.slot());
		let no_show_duration = slot_number_to_tick(
			self.slot_duration_millis,
			Slot::from(u64::from(session_info.no_show_slots)),
		);

		if let Some(approval_entry) = candidate_entry.approval_entry(&block_hash) {
			let required_tranches = approval_checking::tranches_to_approve(
				approval_entry,
				candidate_entry.approvals(),
				tranche_now,
				block_tick,
				no_show_duration,
				session_info.needed_approvals as _,
			);

			let status = ApprovalStatus { required_tranches, block_tick, tranche_now };

			Some((approval_entry, status))
		} else {
			None
		}
	}
}

#[derive(Debug, Clone)]
enum Action {
	ScheduleWakeup {
		block_hash: Hash,
		block_number: BlockNumber,
		candidate_hash: CandidateHash,
		tick: Tick,
	},
	LaunchApproval {
		candidate_hash: CandidateHash,
		indirect_cert: IndirectAssignmentCert,
		assignment_tranche: DelayTranche,
		relay_block_hash: Hash,
		candidate_index: CandidateIndex,
		session: SessionIndex,
		candidate: CandidateReceipt,
		backing_group: GroupIndex,
	},
	NoteApprovedInChainSelection(Hash),
	IssueApproval(CandidateHash, ApprovalVoteRequest),
	BecomeActive,
	Conclude,
}

#[overseer::contextbounds(ApprovalVoting, prefix = self::overseer)]
async fn run<B, Context>(
	mut ctx: Context,
	mut subsystem: ApprovalVotingSubsystem,
	clock: Box<dyn Clock + Send + Sync>,
	assignment_criteria: Box<dyn AssignmentCriteria + Send + Sync>,
	mut backend: B,
) -> SubsystemResult<()>
where
	B: Backend,
{
	if let Err(err) = db_sanity_check(subsystem.db.clone(), subsystem.db_config) {
		gum::warn!(target: LOG_TARGET, ?err, "Could not run approval vote DB sanity check");
	}

	let mut state = State {
		session_window: None,
		keystore: subsystem.keystore,
		slot_duration_millis: subsystem.slot_duration_millis,
		clock,
		assignment_criteria,
		db_config: subsystem.db_config,
		db: subsystem.db,
	};

	let mut wakeups = Wakeups::default();
	let mut currently_checking_set = CurrentlyCheckingSet::default();
	let mut approvals_cache = lru::LruCache::new(APPROVAL_CACHE_SIZE);

	let mut last_finalized_height: Option<BlockNumber> = {
		let (tx, rx) = oneshot::channel();
		ctx.send_message(ChainApiMessage::FinalizedBlockNumber(tx)).await;
		match rx.await? {
			Ok(number) => Some(number),
			Err(err) => {
				gum::warn!(target: LOG_TARGET, ?err, "Failed fetching finalized number");
				None
			},
		}
	};

	loop {
		let mut overlayed_db = OverlayedBackend::new(&backend);
		let actions = futures::select! {
			(tick, woken_block, woken_candidate) = wakeups.next(&*state.clock).fuse() => {
				subsystem.metrics.on_wakeup();
				process_wakeup(
					&mut state,
					&mut overlayed_db,
					woken_block,
					woken_candidate,
					tick,
					&subsystem.metrics,
				)?
			}
			next_msg = ctx.recv().fuse() => {
				let mut actions = handle_from_overseer(
					&mut ctx,
					&mut state,
					&mut overlayed_db,
					&subsystem.metrics,
					next_msg?,
					&mut last_finalized_height,
					&mut wakeups,
				).await?;

				if let Mode::Syncing(ref mut oracle) = subsystem.mode {
					if !oracle.is_major_syncing() {
						// note that we're active before processing other actions.
						actions.insert(0, Action::BecomeActive)
					}
				}

				actions
			}
			approval_state = currently_checking_set.next(&mut approvals_cache).fuse() => {
				let mut actions = Vec::new();
				let (
					relay_block_hashes,
					ApprovalState {
						validator_index,
						candidate_hash,
						approval_outcome,
					}
				) = approval_state;

				if matches!(approval_outcome, ApprovalOutcome::Approved) {
					let mut approvals: Vec<Action> = relay_block_hashes
						.into_iter()
						.map(|block_hash|
							Action::IssueApproval(
								candidate_hash,
								ApprovalVoteRequest {
									validator_index,
									block_hash,
								},
							)
						)
						.collect();
					actions.append(&mut approvals);
				}

				actions
			}
		};

		if handle_actions(
			&mut ctx,
			&mut state,
			&mut overlayed_db,
			&subsystem.metrics,
			&mut wakeups,
			&mut currently_checking_set,
			&mut approvals_cache,
			&mut subsystem.mode,
			actions,
		)
		.await?
		{
			break
		}

		if !overlayed_db.is_empty() {
			let _timer = subsystem.metrics.time_db_transaction();

			let ops = overlayed_db.into_write_ops();
			backend.write(ops)?;
		}
	}

	Ok(())
}

// Handle actions is a function that accepts a set of instructions
// and subsequently updates the underlying approvals_db in accordance
// with the linear set of instructions passed in. Therefore, actions
// must be processed in series to ensure that earlier actions are not
// negated/corrupted by later actions being executed out-of-order.
//
// However, certain Actions can cause additional actions to need to be
// processed by this function. In order to preserve linearity, we would
// need to handle these newly generated actions before we finalize
// completing additional actions in the submitted sequence of actions.
//
// Since recursive async functions are not not stable yet, we are
// forced to modify the actions iterator on the fly whenever a new set
// of actions are generated by handling a single action.
//
// This particular problem statement is specified in issue 3311:
// 	https://github.com/paritytech/polkadot/issues/3311
//
// returns `true` if any of the actions was a `Conclude` command.
#[overseer::contextbounds(ApprovalVoting, prefix = self::overseer)]
async fn handle_actions<Context>(
	ctx: &mut Context,
	state: &mut State,
	overlayed_db: &mut OverlayedBackend<'_, impl Backend>,
	metrics: &Metrics,
	wakeups: &mut Wakeups,
	currently_checking_set: &mut CurrentlyCheckingSet,
	approvals_cache: &mut lru::LruCache<CandidateHash, ApprovalOutcome>,
	mode: &mut Mode,
	actions: Vec<Action>,
) -> SubsystemResult<bool> {
	let mut conclude = false;

	let mut actions_iter = actions.into_iter();
	while let Some(action) = actions_iter.next() {
		match action {
			Action::ScheduleWakeup { block_hash, block_number, candidate_hash, tick } =>
				wakeups.schedule(block_hash, block_number, candidate_hash, tick),
			Action::IssueApproval(candidate_hash, approval_request) => {
				// Note that the IssueApproval action will create additional
				// actions that will need to all be processed before we can
				// handle the next action in the set passed to the ambient
				// function.
				//
				// In order to achieve this, we append the existing iterator
				// to the end of the iterator made up of these newly generated
				// actions.
				//
				// Note that chaining these iterators is O(n) as we must consume
				// the prior iterator.
				let next_actions: Vec<Action> = issue_approval(
					ctx,
					state,
					overlayed_db,
					metrics,
					candidate_hash,
					approval_request,
				)
				.await?
				.into_iter()
				.map(|v| v.clone())
				.chain(actions_iter)
				.collect();

				actions_iter = next_actions.into_iter();
			},
			Action::LaunchApproval {
				candidate_hash,
				indirect_cert,
				assignment_tranche,
				relay_block_hash,
				candidate_index,
				session,
				candidate,
				backing_group,
			} => {
				// Don't launch approval work if the node is syncing.
				if let Mode::Syncing(_) = *mode {
					continue
				}

				metrics.on_assignment_produced(assignment_tranche);
				let block_hash = indirect_cert.block_hash;
				let validator_index = indirect_cert.validator;

				ctx.send_unbounded_message(ApprovalDistributionMessage::DistributeAssignment(
					indirect_cert,
					candidate_index,
				));

				match approvals_cache.get(&candidate_hash) {
					Some(ApprovalOutcome::Approved) => {
						let new_actions: Vec<Action> = std::iter::once(Action::IssueApproval(
							candidate_hash,
							ApprovalVoteRequest { validator_index, block_hash },
						))
						.map(|v| v.clone())
						.chain(actions_iter)
						.collect();
						actions_iter = new_actions.into_iter();
					},
					None => {
						let ctx = &mut *ctx;
						currently_checking_set
							.insert_relay_block_hash(
								candidate_hash,
								validator_index,
								relay_block_hash,
								async move {
									launch_approval(
										ctx,
										metrics.clone(),
										session,
										candidate,
										validator_index,
										block_hash,
										backing_group,
									)
									.await
								},
							)
							.await?;
					},
					Some(_) => {},
				}
			},
			Action::NoteApprovedInChainSelection(block_hash) => {
				ctx.send_message(ChainSelectionMessage::Approved(block_hash)).await;
			},
			Action::BecomeActive => {
				*mode = Mode::Active;

				let messages = distribution_messages_for_activation(overlayed_db)?;

				ctx.send_messages(messages.into_iter()).await;
			},
			Action::Conclude => {
				conclude = true;
			},
		}
	}

	Ok(conclude)
}

fn distribution_messages_for_activation(
	db: &OverlayedBackend<'_, impl Backend>,
) -> SubsystemResult<Vec<ApprovalDistributionMessage>> {
	let all_blocks: Vec<Hash> = db.load_all_blocks()?;

	let mut approval_meta = Vec::with_capacity(all_blocks.len());
	let mut messages = Vec::new();

	messages.push(ApprovalDistributionMessage::NewBlocks(Vec::new())); // dummy value.

	for block_hash in all_blocks {
		let block_entry = match db.load_block_entry(&block_hash)? {
			Some(b) => b,
			None => {
				gum::warn!(target: LOG_TARGET, ?block_hash, "Missing block entry");

				continue
			},
		};
		approval_meta.push(BlockApprovalMeta {
			hash: block_hash,
			number: block_entry.block_number(),
			parent_hash: block_entry.parent_hash(),
			candidates: block_entry.candidates().iter().map(|(_, c_hash)| *c_hash).collect(),
			slot: block_entry.slot(),
			session: block_entry.session(),
		});

		for (i, (_, candidate_hash)) in block_entry.candidates().iter().enumerate() {
			let candidate_entry = match db.load_candidate_entry(&candidate_hash)? {
				Some(c) => c,
				None => {
					gum::warn!(
						target: LOG_TARGET,
						?block_hash,
						?candidate_hash,
						"Missing candidate entry",
					);

					continue
				},
			};

			match candidate_entry.approval_entry(&block_hash) {
				Some(approval_entry) => {
					match approval_entry.local_statements() {
						(None, None) | (None, Some(_)) => {}, // second is impossible case.
						(Some(assignment), None) => {
							messages.push(ApprovalDistributionMessage::DistributeAssignment(
								IndirectAssignmentCert {
									block_hash,
									validator: assignment.validator_index(),
									cert: assignment.cert().clone(),
								},
								i as _,
							));
						},
						(Some(assignment), Some(approval_sig)) => {
							messages.push(ApprovalDistributionMessage::DistributeAssignment(
								IndirectAssignmentCert {
									block_hash,
									validator: assignment.validator_index(),
									cert: assignment.cert().clone(),
								},
								i as _,
							));

							messages.push(ApprovalDistributionMessage::DistributeApproval(
								IndirectSignedApprovalVote {
									block_hash,
									candidate_index: i as _,
									validator: assignment.validator_index(),
									signature: approval_sig,
								},
							))
						},
					}
				},
				None => {
					gum::warn!(
						target: LOG_TARGET,
						?block_hash,
						?candidate_hash,
						"Missing approval entry",
					);
				},
			}
		}
	}

	messages[0] = ApprovalDistributionMessage::NewBlocks(approval_meta);
	Ok(messages)
}

// Handle an incoming signal from the overseer. Returns true if execution should conclude.
#[overseer::contextbounds(ApprovalVoting, prefix = self::overseer)]
async fn handle_from_overseer<Context>(
	ctx: &mut Context,
	state: &mut State,
	db: &mut OverlayedBackend<'_, impl Backend>,
	metrics: &Metrics,
	x: FromOrchestra<ApprovalVotingMessage>,
	last_finalized_height: &mut Option<BlockNumber>,
	wakeups: &mut Wakeups,
) -> SubsystemResult<Vec<Action>> {
	let actions = match x {
		FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)) => {
			let mut actions = Vec::new();

			if let Some(activated) = update.activated {
				let head = activated.hash;
				match import::handle_new_head(ctx, state, db, head, &*last_finalized_height).await {
					Err(e) => return Err(SubsystemError::with_origin("db", e)),
					Ok(block_imported_candidates) => {
						// Schedule wakeups for all imported candidates.
						for block_batch in block_imported_candidates {
							gum::debug!(
								target: LOG_TARGET,
								block_number = ?block_batch.block_number,
								block_hash = ?block_batch.block_hash,
								num_candidates = block_batch.imported_candidates.len(),
								"Imported new block.",
							);

							for (c_hash, c_entry) in block_batch.imported_candidates {
								metrics.on_candidate_imported();

								let our_tranche = c_entry
									.approval_entry(&block_batch.block_hash)
									.and_then(|a| a.our_assignment().map(|a| a.tranche()));

								if let Some(our_tranche) = our_tranche {
									let tick = our_tranche as Tick + block_batch.block_tick;
									gum::trace!(
										target: LOG_TARGET,
										tranche = our_tranche,
										candidate_hash = ?c_hash,
										block_hash = ?block_batch.block_hash,
										block_tick = block_batch.block_tick,
										"Scheduling first wakeup.",
									);

									// Our first wakeup will just be the tranche of our assignment,
									// if any. This will likely be superseded by incoming assignments
									// and approvals which trigger rescheduling.
									actions.push(Action::ScheduleWakeup {
										block_hash: block_batch.block_hash,
										block_number: block_batch.block_number,
										candidate_hash: c_hash,
										tick,
									});
								}
							}
						}
					},
				}
			}

			actions
		},
		FromOrchestra::Signal(OverseerSignal::BlockFinalized(block_hash, block_number)) => {
			gum::debug!(target: LOG_TARGET, ?block_hash, ?block_number, "Block finalized");
			*last_finalized_height = Some(block_number);

			crate::ops::canonicalize(db, block_number, block_hash)
				.map_err(|e| SubsystemError::with_origin("db", e))?;

			wakeups.prune_finalized_wakeups(block_number);

			Vec::new()
		},
		FromOrchestra::Signal(OverseerSignal::Conclude) => {
			vec![Action::Conclude]
		},
		FromOrchestra::Communication { msg } => match msg {
			ApprovalVotingMessage::CheckAndImportAssignment(a, claimed_core, res) => {
				let (check_outcome, actions) =
					check_and_import_assignment(state, db, a, claimed_core)?;
				let _ = res.send(check_outcome);

				actions
			},
			ApprovalVotingMessage::CheckAndImportApproval(a, res) =>
				check_and_import_approval(state, db, metrics, a, |r| {
					let _ = res.send(r);
				})?
				.0,
			ApprovalVotingMessage::ApprovedAncestor(target, lower_bound, res) => {
				match handle_approved_ancestor(ctx, db, target, lower_bound, wakeups).await {
					Ok(v) => {
						let _ = res.send(v);
					},
					Err(e) => {
						let _ = res.send(None);
						return Err(e)
					},
				}

				Vec::new()
			},
			ApprovalVotingMessage::GetApprovalSignaturesForCandidate(candidate_hash, tx) => {
				metrics.on_candidate_signatures_request();
				get_approval_signatures_for_candidate(ctx, db, candidate_hash, tx).await?;
				Vec::new()
			},
		},
	};

	Ok(actions)
}

/// Retrieve approval signatures.
///
/// This involves an unbounded message send to approval-distribution, the caller has to ensure that
/// calls to this function are infrequent and bounded.
#[overseer::contextbounds(ApprovalVoting, prefix = self::overseer)]
async fn get_approval_signatures_for_candidate<Context>(
	ctx: &mut Context,
	db: &OverlayedBackend<'_, impl Backend>,
	candidate_hash: CandidateHash,
	tx: oneshot::Sender<HashMap<ValidatorIndex, ValidatorSignature>>,
) -> SubsystemResult<()> {
	let send_votes = |votes| {
		if let Err(_) = tx.send(votes) {
			gum::debug!(
				target: LOG_TARGET,
				"Sending approval signatures back failed, as receiver got closed."
			);
		}
	};
	let entry = match db.load_candidate_entry(&candidate_hash)? {
		None => {
			send_votes(HashMap::new());
			gum::debug!(
				target: LOG_TARGET,
				?candidate_hash,
				"Sent back empty votes because the candidate was not found in db."
			);
			return Ok(())
		},
		Some(e) => e,
	};

	let relay_hashes = entry.block_assignments.keys();

	let mut candidate_indices = HashSet::new();
	// Retrieve `CoreIndices`/`CandidateIndices` as required by approval-distribution:
	for hash in relay_hashes {
		let entry = match db.load_block_entry(hash)? {
			None => {
				gum::debug!(
					target: LOG_TARGET,
					?candidate_hash,
					?hash,
					"Block entry for assignment missing."
				);
				continue
			},
			Some(e) => e,
		};
		for (candidate_index, (_core_index, c_hash)) in entry.candidates().iter().enumerate() {
			if c_hash == &candidate_hash {
				candidate_indices.insert((*hash, candidate_index as u32));
				break
			}
		}
	}

	let mut sender = ctx.sender().clone();
	let get_approvals = async move {
		let (tx_distribution, rx_distribution) = oneshot::channel();
		sender.send_unbounded_message(ApprovalDistributionMessage::GetApprovalSignatures(
			candidate_indices,
			tx_distribution,
		));

		// Because of the unbounded sending and the nature of the call (just fetching data from state),
		// this should not block long:
		match rx_distribution.timeout(WAIT_FOR_SIGS_TIMEOUT).await {
			None => {
				gum::warn!(
					target: LOG_TARGET,
					"Waiting for approval signatures timed out - dead lock?"
				);
			},
			Some(Err(_)) => gum::debug!(
				target: LOG_TARGET,
				"Request for approval signatures got cancelled by `approval-distribution`."
			),
			Some(Ok(votes)) => send_votes(votes),
		}
	};

	// No need to block subsystem on this (also required to break cycle).
	// We should not be sending this message frequently - caller must make sure this is bounded.
	gum::trace!(
		target: LOG_TARGET,
		?candidate_hash,
		"Spawning task for fetching sinatures from approval-distribution"
	);
	ctx.spawn("get-approval-signatures", Box::pin(get_approvals))
}

#[overseer::contextbounds(ApprovalVoting, prefix = self::overseer)]
async fn handle_approved_ancestor<Context>(
	ctx: &mut Context,
	db: &OverlayedBackend<'_, impl Backend>,
	target: Hash,
	lower_bound: BlockNumber,
	wakeups: &Wakeups,
) -> SubsystemResult<Option<HighestApprovedAncestorBlock>> {
	const MAX_TRACING_WINDOW: usize = 200;
	const ABNORMAL_DEPTH_THRESHOLD: usize = 5;

	use bitvec::{order::Lsb0, vec::BitVec};

	let mut span =
		jaeger::Span::new(&target, "approved-ancestor").with_stage(jaeger::Stage::ApprovalChecking);

	let mut all_approved_max = None;

	let target_number = {
		let (tx, rx) = oneshot::channel();

		ctx.send_message(ChainApiMessage::BlockNumber(target, tx)).await;

		match rx.await {
			Ok(Ok(Some(n))) => n,
			Ok(Ok(None)) => return Ok(None),
			Ok(Err(_)) | Err(_) => return Ok(None),
		}
	};

	if target_number <= lower_bound {
		return Ok(None)
	}

	span.add_string_fmt_debug_tag("target-number", target_number);
	span.add_string_fmt_debug_tag("target-hash", target);

	// request ancestors up to but not including the lower bound,
	// as a vote on the lower bound is implied if we cannot find
	// anything else.
	let ancestry = if target_number > lower_bound + 1 {
		let (tx, rx) = oneshot::channel();

		ctx.send_message(ChainApiMessage::Ancestors {
			hash: target,
			k: (target_number - (lower_bound + 1)) as usize,
			response_channel: tx,
		})
		.await;

		match rx.await {
			Ok(Ok(a)) => a,
			Err(_) | Ok(Err(_)) => return Ok(None),
		}
	} else {
		Vec::new()
	};

	let mut block_descriptions = Vec::new();

	let mut bits: BitVec<u8, Lsb0> = Default::default();
	for (i, block_hash) in std::iter::once(target).chain(ancestry).enumerate() {
		// Block entries should be present as the assumption is that
		// nothing here is finalized. If we encounter any missing block
		// entries we can fail.
		let entry = match db.load_block_entry(&block_hash)? {
			None => {
				let block_number = target_number.saturating_sub(i as u32);
				gum::info!(
					target: LOG_TARGET,
					unknown_number = ?block_number,
					unknown_hash = ?block_hash,
					"Chain between ({}, {}) and {} not fully known. Forcing vote on {}",
					target,
					target_number,
					lower_bound,
					lower_bound,
				);
				return Ok(None)
			},
			Some(b) => b,
		};

		// even if traversing millions of blocks this is fairly cheap and always dwarfed by the
		// disk lookups.
		bits.push(entry.is_fully_approved());
		if entry.is_fully_approved() {
			if all_approved_max.is_none() {
				// First iteration of the loop is target, i = 0. After that,
				// ancestry is moving backwards.
				all_approved_max = Some((block_hash, target_number - i as BlockNumber));
			}
			block_descriptions.push(BlockDescription {
				block_hash,
				session: entry.session(),
				candidates: entry
					.candidates()
					.iter()
					.map(|(_idx, candidate_hash)| *candidate_hash)
					.collect(),
			});
		} else if bits.len() <= ABNORMAL_DEPTH_THRESHOLD {
			all_approved_max = None;
			block_descriptions.clear();
		} else {
			all_approved_max = None;
			block_descriptions.clear();

			let unapproved: Vec<_> = entry.unapproved_candidates().collect();
			gum::debug!(
				target: LOG_TARGET,
				"Block {} is {} blocks deep and has {}/{} candidates unapproved",
				block_hash,
				bits.len() - 1,
				unapproved.len(),
				entry.candidates().len(),
			);

			for candidate_hash in unapproved {
				match db.load_candidate_entry(&candidate_hash)? {
					None => {
						gum::warn!(
							target: LOG_TARGET,
							?candidate_hash,
							"Missing expected candidate in DB",
						);

						continue
					},
					Some(c_entry) => match c_entry.approval_entry(&block_hash) {
						None => {
							gum::warn!(
								target: LOG_TARGET,
								?candidate_hash,
								?block_hash,
								"Missing expected approval entry under candidate.",
							);
						},
						Some(a_entry) => {
							let status = || {
								let n_assignments = a_entry.n_assignments();

								// Take the approvals, filtered by the assignments
								// for this block.
								let n_approvals = c_entry
									.approvals()
									.iter()
									.by_vals()
									.enumerate()
									.filter(|(i, approved)| {
										*approved && a_entry.is_assigned(ValidatorIndex(*i as _))
									})
									.count();

								format!(
									"{}/{}/{}",
									n_assignments,
									n_approvals,
									a_entry.n_validators(),
								)
							};

							match a_entry.our_assignment() {
								None => gum::debug!(
									target: LOG_TARGET,
									?candidate_hash,
									?block_hash,
									status = %status(),
									"no assignment."
								),
								Some(a) => {
									let tranche = a.tranche();
									let triggered = a.triggered();

									let next_wakeup =
										wakeups.wakeup_for(block_hash, candidate_hash);

									let approved =
										triggered && { a_entry.local_statements().1.is_some() };

									gum::debug!(
										target: LOG_TARGET,
										?candidate_hash,
										?block_hash,
										tranche,
										?next_wakeup,
										status = %status(),
										triggered,
										approved,
										"assigned."
									);
								},
							}
						},
					},
				}
			}
		}
	}

	gum::debug!(
		target: LOG_TARGET,
		"approved blocks {}-[{}]-{}",
		target_number,
		{
			// formatting to divide bits by groups of 10.
			// when comparing logs on multiple machines where the exact vote
			// targets may differ, this grouping is useful.
			let mut s = String::with_capacity(bits.len());
			for (i, bit) in bits.iter().enumerate().take(MAX_TRACING_WINDOW) {
				s.push(if *bit { '1' } else { '0' });
				if (target_number - i as u32) % 10 == 0 && i != bits.len() - 1 {
					s.push(' ');
				}
			}

			s
		},
		if bits.len() > MAX_TRACING_WINDOW {
			format!(
				"{}... (truncated due to large window)",
				target_number - MAX_TRACING_WINDOW as u32 + 1,
			)
		} else {
			format!("{}", lower_bound + 1)
		},
	);

	// `reverse()` to obtain the ascending order from lowest to highest
	// block within the candidates, which is the expected order
	block_descriptions.reverse();

	let all_approved_max =
		all_approved_max.map(|(hash, block_number)| HighestApprovedAncestorBlock {
			hash,
			number: block_number,
			descriptions: block_descriptions,
		});
	match all_approved_max {
		Some(HighestApprovedAncestorBlock { ref hash, ref number, .. }) => {
			span.add_uint_tag("approved-number", *number as u64);
			span.add_string_fmt_debug_tag("approved-hash", hash);
		},
		None => {
			span.add_string_tag("reached-lower-bound", "true");
		},
	}

	Ok(all_approved_max)
}

// `Option::cmp` treats `None` as less than `Some`.
fn min_prefer_some<T: std::cmp::Ord>(a: Option<T>, b: Option<T>) -> Option<T> {
	match (a, b) {
		(None, None) => None,
		(None, Some(x)) | (Some(x), None) => Some(x),
		(Some(x), Some(y)) => Some(std::cmp::min(x, y)),
	}
}

fn schedule_wakeup_action(
	approval_entry: &ApprovalEntry,
	block_hash: Hash,
	block_number: BlockNumber,
	candidate_hash: CandidateHash,
	block_tick: Tick,
	tick_now: Tick,
	required_tranches: RequiredTranches,
) -> Option<Action> {
	let maybe_action = match required_tranches {
		_ if approval_entry.is_approved() => None,
		RequiredTranches::All => None,
		RequiredTranches::Exact { next_no_show, last_assignment_tick, .. } => {
			// Take the earlier of the next no show or the last assignment tick + required delay,
			// only considering the latter if it is after the current moment.
			min_prefer_some(
				last_assignment_tick.map(|l| l + APPROVAL_DELAY).filter(|t| t > &tick_now),
				next_no_show,
			)
			.map(|tick| Action::ScheduleWakeup { block_hash, block_number, candidate_hash, tick })
		},
		RequiredTranches::Pending { considered, next_no_show, clock_drift, .. } => {
			// select the minimum of `next_no_show`, or the tick of the next non-empty tranche
			// after `considered`, including any tranche that might contain our own untriggered
			// assignment.
			let next_non_empty_tranche = {
				let next_announced = approval_entry
					.tranches()
					.iter()
					.skip_while(|t| t.tranche() <= considered)
					.map(|t| t.tranche())
					.next();

				let our_untriggered = approval_entry.our_assignment().and_then(|t| {
					if !t.triggered() && t.tranche() > considered {
						Some(t.tranche())
					} else {
						None
					}
				});

				// Apply the clock drift to these tranches.
				min_prefer_some(next_announced, our_untriggered)
					.map(|t| t as Tick + block_tick + clock_drift)
			};

			min_prefer_some(next_non_empty_tranche, next_no_show).map(|tick| {
				Action::ScheduleWakeup { block_hash, block_number, candidate_hash, tick }
			})
		},
	};

	match maybe_action {
		Some(Action::ScheduleWakeup { ref tick, .. }) => gum::trace!(
			target: LOG_TARGET,
			tick,
			?candidate_hash,
			?block_hash,
			block_tick,
			"Scheduling next wakeup.",
		),
		None => gum::trace!(
			target: LOG_TARGET,
			?candidate_hash,
			?block_hash,
			block_tick,
			"No wakeup needed.",
		),
		Some(_) => {}, // unreachable
	}

	maybe_action
}

fn check_and_import_assignment(
	state: &State,
	db: &mut OverlayedBackend<'_, impl Backend>,
	assignment: IndirectAssignmentCert,
	candidate_index: CandidateIndex,
) -> SubsystemResult<(AssignmentCheckResult, Vec<Action>)> {
	let tick_now = state.clock.tick_now();

	let block_entry = match db.load_block_entry(&assignment.block_hash)? {
		Some(b) => b,
		None =>
			return Ok((
				AssignmentCheckResult::Bad(AssignmentCheckError::UnknownBlock(
					assignment.block_hash,
				)),
				Vec::new(),
			)),
	};

	let session_info = match state.session_info(block_entry.session()) {
		Some(s) => s,
		None =>
			return Ok((
				AssignmentCheckResult::Bad(AssignmentCheckError::UnknownSessionIndex(
					block_entry.session(),
				)),
				Vec::new(),
			)),
	};

	let (claimed_core_index, assigned_candidate_hash) =
		match block_entry.candidate(candidate_index as usize) {
			Some((c, h)) => (*c, *h),
			None =>
				return Ok((
					AssignmentCheckResult::Bad(AssignmentCheckError::InvalidCandidateIndex(
						candidate_index,
					)),
					Vec::new(),
				)), // no candidate at core.
		};

	let mut candidate_entry = match db.load_candidate_entry(&assigned_candidate_hash)? {
		Some(c) => c,
		None =>
			return Ok((
				AssignmentCheckResult::Bad(AssignmentCheckError::InvalidCandidate(
					candidate_index,
					assigned_candidate_hash,
				)),
				Vec::new(),
			)),
	};

	let res = {
		// import the assignment.
		let approval_entry = match candidate_entry.approval_entry_mut(&assignment.block_hash) {
			Some(a) => a,
			None =>
				return Ok((
					AssignmentCheckResult::Bad(AssignmentCheckError::Internal(
						assignment.block_hash,
						assigned_candidate_hash,
					)),
					Vec::new(),
				)),
		};

		let res = state.assignment_criteria.check_assignment_cert(
			claimed_core_index,
			assignment.validator,
			&criteria::Config::from(session_info),
			block_entry.relay_vrf_story(),
			&assignment.cert,
			approval_entry.backing_group(),
		);

		let tranche = match res {
			Err(crate::criteria::InvalidAssignment(reason)) =>
				return Ok((
					AssignmentCheckResult::Bad(AssignmentCheckError::InvalidCert(
						assignment.validator,
						format!("{:?}", reason),
					)),
					Vec::new(),
				)),
			Ok(tranche) => {
				let current_tranche =
					state.clock.tranche_now(state.slot_duration_millis, block_entry.slot());

				let too_far_in_future = current_tranche + TICK_TOO_FAR_IN_FUTURE as DelayTranche;

				if tranche >= too_far_in_future {
					return Ok((AssignmentCheckResult::TooFarInFuture, Vec::new()))
				}

				tranche
			},
		};

		let is_duplicate = approval_entry.is_assigned(assignment.validator);
		approval_entry.import_assignment(tranche, assignment.validator, tick_now);

		if is_duplicate {
			AssignmentCheckResult::AcceptedDuplicate
		} else {
			gum::trace!(
				target: LOG_TARGET,
				validator = assignment.validator.0,
				candidate_hash = ?assigned_candidate_hash,
				para_id = ?candidate_entry.candidate_receipt().descriptor.para_id,
				"Imported assignment.",
			);

			AssignmentCheckResult::Accepted
		}
	};

	let mut actions = Vec::new();

	// We've imported a new approval, so we need to schedule a wake-up for when that might no-show.
	if let Some((approval_entry, status)) = state.approval_status(&block_entry, &candidate_entry) {
		actions.extend(schedule_wakeup_action(
			approval_entry,
			block_entry.block_hash(),
			block_entry.block_number(),
			assigned_candidate_hash,
			status.block_tick,
			tick_now,
			status.required_tranches,
		));
	}

	// We also write the candidate entry as it now contains the new candidate.
	db.write_candidate_entry(candidate_entry.into());

	Ok((res, actions))
}

fn check_and_import_approval<T>(
	state: &State,
	db: &mut OverlayedBackend<'_, impl Backend>,
	metrics: &Metrics,
	approval: IndirectSignedApprovalVote,
	with_response: impl FnOnce(ApprovalCheckResult) -> T,
) -> SubsystemResult<(Vec<Action>, T)> {
	macro_rules! respond_early {
		($e: expr) => {{
			let t = with_response($e);
			return Ok((Vec::new(), t))
		}};
	}

	let block_entry = match db.load_block_entry(&approval.block_hash)? {
		Some(b) => b,
		None => {
			respond_early!(ApprovalCheckResult::Bad(ApprovalCheckError::UnknownBlock(
				approval.block_hash
			),))
		},
	};

	let session_info = match state.session_info(block_entry.session()) {
		Some(s) => s,
		None => {
			respond_early!(ApprovalCheckResult::Bad(ApprovalCheckError::UnknownSessionIndex(
				block_entry.session()
			),))
		},
	};

	let approved_candidate_hash = match block_entry.candidate(approval.candidate_index as usize) {
		Some((_, h)) => *h,
		None => respond_early!(ApprovalCheckResult::Bad(
			ApprovalCheckError::InvalidCandidateIndex(approval.candidate_index),
		)),
	};

	let pubkey = match session_info.validators.get(approval.validator) {
		Some(k) => k,
		None => respond_early!(ApprovalCheckResult::Bad(
			ApprovalCheckError::InvalidValidatorIndex(approval.validator),
		)),
	};

	// Signature check:
	match DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking).check_signature(
		&pubkey,
		approved_candidate_hash,
		block_entry.session(),
		&approval.signature,
	) {
		Err(_) => respond_early!(ApprovalCheckResult::Bad(ApprovalCheckError::InvalidSignature(
			approval.validator
		),)),
		Ok(()) => {},
	};

	let candidate_entry = match db.load_candidate_entry(&approved_candidate_hash)? {
		Some(c) => c,
		None => {
			respond_early!(ApprovalCheckResult::Bad(ApprovalCheckError::InvalidCandidate(
				approval.candidate_index,
				approved_candidate_hash
			),))
		},
	};

	// Don't accept approvals until assignment.
	match candidate_entry.approval_entry(&approval.block_hash) {
		None => {
			respond_early!(ApprovalCheckResult::Bad(ApprovalCheckError::Internal(
				approval.block_hash,
				approved_candidate_hash
			),))
		},
		Some(e) if !e.is_assigned(approval.validator) => {
			respond_early!(ApprovalCheckResult::Bad(ApprovalCheckError::NoAssignment(
				approval.validator
			),))
		},
		_ => {},
	}

	// importing the approval can be heavy as it may trigger acceptance for a series of blocks.
	let t = with_response(ApprovalCheckResult::Accepted);

	gum::trace!(
		target: LOG_TARGET,
		validator_index = approval.validator.0,
		validator = ?pubkey,
		candidate_hash = ?approved_candidate_hash,
		para_id = ?candidate_entry.candidate_receipt().descriptor.para_id,
		"Importing approval vote",
	);

	let actions = advance_approval_state(
		state,
		db,
		&metrics,
		block_entry,
		approved_candidate_hash,
		candidate_entry,
		ApprovalStateTransition::RemoteApproval(approval.validator),
	);

	Ok((actions, t))
}

#[derive(Debug)]
enum ApprovalStateTransition {
	RemoteApproval(ValidatorIndex),
	LocalApproval(ValidatorIndex, ValidatorSignature),
	WakeupProcessed,
}

impl ApprovalStateTransition {
	fn validator_index(&self) -> Option<ValidatorIndex> {
		match *self {
			ApprovalStateTransition::RemoteApproval(v) |
			ApprovalStateTransition::LocalApproval(v, _) => Some(v),
			ApprovalStateTransition::WakeupProcessed => None,
		}
	}

	fn is_local_approval(&self) -> bool {
		match *self {
			ApprovalStateTransition::RemoteApproval(_) => false,
			ApprovalStateTransition::LocalApproval(_, _) => true,
			ApprovalStateTransition::WakeupProcessed => false,
		}
	}
}

// Advance the approval state, either by importing an approval vote which is already checked to be valid and corresponding to an assigned
// validator on the candidate and block, or by noting that there are no further wakeups or tranches needed. This updates the block entry and candidate entry as
// necessary and schedules any further wakeups.
fn advance_approval_state(
	state: &State,
	db: &mut OverlayedBackend<'_, impl Backend>,
	metrics: &Metrics,
	mut block_entry: BlockEntry,
	candidate_hash: CandidateHash,
	mut candidate_entry: CandidateEntry,
	transition: ApprovalStateTransition,
) -> Vec<Action> {
	let validator_index = transition.validator_index();

	let already_approved_by = validator_index.as_ref().map(|v| candidate_entry.mark_approval(*v));
	let candidate_approved_in_block = block_entry.is_candidate_approved(&candidate_hash);

	// Check for early exits.
	//
	// If the candidate was approved
	// but not the block, it means that we still need more approvals for the candidate under the
	// block.
	//
	// If the block was approved, but the validator hadn't approved it yet, we should still hold
	// onto the approval vote on-disk in case we restart and rebroadcast votes. Otherwise, our
	// assignment might manifest as a no-show.
	if !transition.is_local_approval() {
		// We don't store remote votes and there's nothing to store for processed wakeups,
		// so we can early exit as long at the candidate is already concluded under the
		// block i.e. we don't need more approvals.
		if candidate_approved_in_block {
			return Vec::new()
		}
	}

	let mut actions = Vec::new();
	let block_hash = block_entry.block_hash();
	let block_number = block_entry.block_number();

	let tick_now = state.clock.tick_now();

	let (is_approved, status) = if let Some((approval_entry, status)) =
		state.approval_status(&block_entry, &candidate_entry)
	{
		let check = approval_checking::check_approval(
			&candidate_entry,
			approval_entry,
			status.required_tranches.clone(),
		);

		// Check whether this is approved, while allowing a maximum
		// assignment tick of `now - APPROVAL_DELAY` - that is, that
		// all counted assignments are at least `APPROVAL_DELAY` ticks old.
		let is_approved = check.is_approved(tick_now.saturating_sub(APPROVAL_DELAY));

		if is_approved {
			gum::trace!(
				target: LOG_TARGET,
				?candidate_hash,
				?block_hash,
				"Candidate approved under block.",
			);

			let no_shows = check.known_no_shows();

			let was_block_approved = block_entry.is_fully_approved();
			block_entry.mark_approved_by_hash(&candidate_hash);
			let is_block_approved = block_entry.is_fully_approved();

			if no_shows != 0 {
				metrics.on_no_shows(no_shows);
			}

			metrics.on_candidate_approved(status.tranche_now as _);

			if is_block_approved && !was_block_approved {
				metrics.on_block_approved(status.tranche_now as _);
				actions.push(Action::NoteApprovedInChainSelection(block_hash));
			}

			db.write_block_entry(block_entry.into());
		}

		(is_approved, status)
	} else {
		gum::warn!(
			target: LOG_TARGET,
			?candidate_hash,
			?block_hash,
			?validator_index,
			"No approval entry for approval under block",
		);

		return Vec::new()
	};

	{
		let approval_entry = candidate_entry
			.approval_entry_mut(&block_hash)
			.expect("Approval entry just fetched; qed");

		let was_approved = approval_entry.is_approved();
		let newly_approved = is_approved && !was_approved;

		if is_approved {
			approval_entry.mark_approved();
		}

		if let ApprovalStateTransition::LocalApproval(_, ref sig) = transition {
			approval_entry.import_approval_sig(sig.clone());
		}

		actions.extend(schedule_wakeup_action(
			&approval_entry,
			block_hash,
			block_number,
			candidate_hash,
			status.block_tick,
			tick_now,
			status.required_tranches,
		));

		// We have no need to write the candidate entry if all of the following
		// is true:
		//
		// 1. This is not a local approval, as we don't store anything new in the approval entry.
		// 2. The candidate is not newly approved, as we haven't altered the approval entry's
		//	  approved flag with `mark_approved` above.
		// 3. The approver, if any, had already approved the candidate, as we haven't altered the bitfield.
		if transition.is_local_approval() || newly_approved || !already_approved_by.unwrap_or(true)
		{
			// In all other cases, we need to write the candidate entry.
			db.write_candidate_entry(candidate_entry);
		}
	}

	actions
}

fn should_trigger_assignment(
	approval_entry: &ApprovalEntry,
	candidate_entry: &CandidateEntry,
	required_tranches: RequiredTranches,
	tranche_now: DelayTranche,
) -> bool {
	match approval_entry.our_assignment() {
		None => false,
		Some(ref assignment) if assignment.triggered() => false,
		Some(ref assignment) if assignment.tranche() == 0 => true,
		Some(ref assignment) => {
			match required_tranches {
				RequiredTranches::All => !approval_checking::check_approval(
					&candidate_entry,
					&approval_entry,
					RequiredTranches::All,
				)
				.is_approved(Tick::max_value()), // when all are required, we are just waiting for the first 1/3+
				RequiredTranches::Pending { maximum_broadcast, clock_drift, .. } => {
					let drifted_tranche_now =
						tranche_now.saturating_sub(clock_drift as DelayTranche);
					assignment.tranche() <= maximum_broadcast &&
						assignment.tranche() <= drifted_tranche_now
				},
				RequiredTranches::Exact { .. } => {
					// indicates that no new assignments are needed at the moment.
					false
				},
			}
		},
	}
}

fn process_wakeup(
	state: &State,
	db: &mut OverlayedBackend<'_, impl Backend>,
	relay_block: Hash,
	candidate_hash: CandidateHash,
	expected_tick: Tick,
	metrics: &Metrics,
) -> SubsystemResult<Vec<Action>> {
	let _span = jaeger::Span::from_encodable(
		(relay_block, candidate_hash, expected_tick),
		"process-approval-wakeup",
	)
	.with_relay_parent(relay_block)
	.with_candidate(candidate_hash)
	.with_stage(jaeger::Stage::ApprovalChecking);

	let block_entry = db.load_block_entry(&relay_block)?;
	let candidate_entry = db.load_candidate_entry(&candidate_hash)?;

	// If either is not present, we have nothing to wakeup. Might have lost a race with finality
	let (block_entry, mut candidate_entry) = match (block_entry, candidate_entry) {
		(Some(b), Some(c)) => (b, c),
		_ => return Ok(Vec::new()),
	};

	let session_info = match state.session_info(block_entry.session()) {
		Some(i) => i,
		None => {
			gum::warn!(
				target: LOG_TARGET,
				"Missing session info for live block {} in session {}",
				relay_block,
				block_entry.session(),
			);

			return Ok(Vec::new())
		},
	};

	let block_tick = slot_number_to_tick(state.slot_duration_millis, block_entry.slot());
	let no_show_duration = slot_number_to_tick(
		state.slot_duration_millis,
		Slot::from(u64::from(session_info.no_show_slots)),
	);

	let tranche_now = state.clock.tranche_now(state.slot_duration_millis, block_entry.slot());

	gum::trace!(
		target: LOG_TARGET,
		tranche = tranche_now,
		?candidate_hash,
		block_hash = ?relay_block,
		"Processing wakeup",
	);

	let (should_trigger, backing_group) = {
		let approval_entry = match candidate_entry.approval_entry(&relay_block) {
			Some(e) => e,
			None => return Ok(Vec::new()),
		};

		let tranches_to_approve = approval_checking::tranches_to_approve(
			&approval_entry,
			candidate_entry.approvals(),
			tranche_now,
			block_tick,
			no_show_duration,
			session_info.needed_approvals as _,
		);

		let should_trigger = should_trigger_assignment(
			&approval_entry,
			&candidate_entry,
			tranches_to_approve,
			tranche_now,
		);

		(should_trigger, approval_entry.backing_group())
	};

	let mut actions = Vec::new();
	let candidate_receipt = candidate_entry.candidate_receipt().clone();

	let maybe_cert = if should_trigger {
		let maybe_cert = {
			let approval_entry = candidate_entry
				.approval_entry_mut(&relay_block)
				.expect("should_trigger only true if this fetched earlier; qed");

			approval_entry.trigger_our_assignment(state.clock.tick_now())
		};

		db.write_candidate_entry(candidate_entry.clone());

		maybe_cert
	} else {
		None
	};

	if let Some((cert, val_index, tranche)) = maybe_cert {
		let indirect_cert =
			IndirectAssignmentCert { block_hash: relay_block, validator: val_index, cert };

		let index_in_candidate =
			block_entry.candidates().iter().position(|(_, h)| &candidate_hash == h);

		if let Some(i) = index_in_candidate {
			gum::trace!(
				target: LOG_TARGET,
				?candidate_hash,
				para_id = ?candidate_receipt.descriptor.para_id,
				block_hash = ?relay_block,
				"Launching approval work.",
			);

			// sanity: should always be present.
			actions.push(Action::LaunchApproval {
				candidate_hash,
				indirect_cert,
				assignment_tranche: tranche,
				relay_block_hash: relay_block,
				candidate_index: i as _,
				session: block_entry.session(),
				candidate: candidate_receipt,
				backing_group,
			});
		}
	}

	// Although we checked approval earlier in this function,
	// this wakeup might have advanced the state to approved via
	// a no-show that was immediately covered and therefore
	// we need to check for that and advance the state on-disk.
	//
	// Note that this function also schedules a wakeup as necessary.
	actions.extend(advance_approval_state(
		state,
		db,
		metrics,
		block_entry,
		candidate_hash,
		candidate_entry,
		ApprovalStateTransition::WakeupProcessed,
	));

	Ok(actions)
}

// Launch approval work, returning an `AbortHandle` which corresponds to the background task
// spawned. When the background work is no longer needed, the `AbortHandle` should be dropped
// to cancel the background work and any requests it has spawned.
#[overseer::contextbounds(ApprovalVoting, prefix = self::overseer)]
async fn launch_approval<Context>(
	ctx: &mut Context,
	metrics: Metrics,
	session_index: SessionIndex,
	candidate: CandidateReceipt,
	validator_index: ValidatorIndex,
	block_hash: Hash,
	backing_group: GroupIndex,
) -> SubsystemResult<RemoteHandle<ApprovalState>> {
	let (a_tx, a_rx) = oneshot::channel();
	let (code_tx, code_rx) = oneshot::channel();

	// The background future returned by this function may
	// be dropped before completing. This guard is used to ensure that the approval
	// work is correctly counted as stale even if so.
	struct StaleGuard(Option<Metrics>);

	impl StaleGuard {
		fn take(mut self) -> Metrics {
			self.0.take().expect(
				"
				consumed after take; so this cannot be called twice; \
				nothing in this function reaches into the struct to avoid this API; \
				qed
			",
			)
		}
	}

	impl Drop for StaleGuard {
		fn drop(&mut self) {
			if let Some(metrics) = self.0.as_ref() {
				metrics.on_approval_stale();
			}
		}
	}

	let candidate_hash = candidate.hash();
	let para_id = candidate.descriptor.para_id;

	gum::trace!(target: LOG_TARGET, ?candidate_hash, ?para_id, "Recovering data.");

	let timer = metrics.time_recover_and_approve();
	ctx.send_message(AvailabilityRecoveryMessage::RecoverAvailableData(
		candidate.clone(),
		session_index,
		Some(backing_group),
		a_tx,
	))
	.await;

	ctx.send_message(RuntimeApiMessage::Request(
		block_hash,
		RuntimeApiRequest::ValidationCodeByHash(candidate.descriptor.validation_code_hash, code_tx),
	))
	.await;

	let candidate = candidate.clone();
	let metrics_guard = StaleGuard(Some(metrics));
	let mut sender = ctx.sender().clone();
	let background = async move {
		// Force the move of the timer into the background task.
		let _timer = timer;
		let _span = jaeger::Span::from_encodable((block_hash, candidate_hash), "launch-approval")
			.with_relay_parent(block_hash)
			.with_candidate(candidate_hash)
			.with_stage(jaeger::Stage::ApprovalChecking);

		let available_data = match a_rx.await {
			Err(_) => return ApprovalState::failed(validator_index, candidate_hash),
			Ok(Ok(a)) => a,
			Ok(Err(e)) => {
				match &e {
					&RecoveryError::Unavailable => {
						gum::warn!(
							target: LOG_TARGET,
							?para_id,
							?candidate_hash,
							"Data unavailable for candidate {:?}",
							(candidate_hash, candidate.descriptor.para_id),
						);
						// do nothing. we'll just be a no-show and that'll cause others to rise up.
						metrics_guard.take().on_approval_unavailable();
					},
					&RecoveryError::Invalid => {
						gum::warn!(
							target: LOG_TARGET,
							?para_id,
							?candidate_hash,
							"Data recovery invalid for candidate {:?}",
							(candidate_hash, candidate.descriptor.para_id),
						);
						issue_local_invalid_statement(
							&mut sender,
							session_index,
							candidate_hash,
							candidate.clone(),
						);
						metrics_guard.take().on_approval_invalid();
					},
				}
				return ApprovalState::failed(validator_index, candidate_hash)
			},
		};

		let validation_code = match code_rx.await {
			Err(_) => return ApprovalState::failed(validator_index, candidate_hash),
			Ok(Err(_)) => return ApprovalState::failed(validator_index, candidate_hash),
			Ok(Ok(Some(code))) => code,
			Ok(Ok(None)) => {
				gum::warn!(
					target: LOG_TARGET,
					"Validation code unavailable for block {:?} in the state of block {:?} (a recent descendant)",
					candidate.descriptor.relay_parent,
					block_hash,
				);

				// No dispute necessary, as this indicates that the chain is not behaving
				// according to expectations.
				metrics_guard.take().on_approval_unavailable();
				return ApprovalState::failed(validator_index, candidate_hash)
			},
		};

		let (val_tx, val_rx) = oneshot::channel();

		sender
			.send_message(CandidateValidationMessage::ValidateFromExhaustive(
				available_data.validation_data,
				validation_code,
				candidate.clone(),
				available_data.pov,
				APPROVAL_EXECUTION_TIMEOUT,
				val_tx,
			))
			.await;

		match val_rx.await {
			Err(_) => return ApprovalState::failed(validator_index, candidate_hash),
			Ok(Ok(ValidationResult::Valid(_, _))) => {
				// Validation checked out. Issue an approval command. If the underlying service is unreachable,
				// then there isn't anything we can do.

				gum::trace!(target: LOG_TARGET, ?candidate_hash, ?para_id, "Candidate Valid");

				let _ = metrics_guard.take();
				return ApprovalState::approved(validator_index, candidate_hash)
			},
			Ok(Ok(ValidationResult::Invalid(reason))) => {
				gum::warn!(
					target: LOG_TARGET,
					?reason,
					?candidate_hash,
					?para_id,
					"Detected invalid candidate as an approval checker.",
				);

				issue_local_invalid_statement(
					&mut sender,
					session_index,
					candidate_hash,
					candidate.clone(),
				);

				metrics_guard.take().on_approval_invalid();
				return ApprovalState::failed(validator_index, candidate_hash)
			},
			Ok(Err(e)) => {
				gum::error!(
					target: LOG_TARGET,
					err = ?e,
					?candidate_hash,
					?para_id,
					"Failed to validate candidate due to internal error",
				);
				metrics_guard.take().on_approval_error();
				return ApprovalState::failed(validator_index, candidate_hash)
			},
		}
	};

	let (background, remote_handle) = background.remote_handle();
	ctx.spawn("approval-checks", Box::pin(background)).map(move |()| remote_handle)
}

// Issue and import a local approval vote. Should only be invoked after approval checks
// have been done.
#[overseer::contextbounds(ApprovalVoting, prefix = self::overseer)]
async fn issue_approval<Context>(
	ctx: &mut Context,
	state: &mut State,
	db: &mut OverlayedBackend<'_, impl Backend>,
	metrics: &Metrics,
	candidate_hash: CandidateHash,
	ApprovalVoteRequest { validator_index, block_hash }: ApprovalVoteRequest,
) -> SubsystemResult<Vec<Action>> {
	let block_entry = match db.load_block_entry(&block_hash)? {
		Some(b) => b,
		None => {
			// not a cause for alarm - just lost a race with pruning, most likely.
			metrics.on_approval_stale();
			return Ok(Vec::new())
		},
	};

	let candidate_index = match block_entry.candidates().iter().position(|e| e.1 == candidate_hash)
	{
		None => {
			gum::warn!(
				target: LOG_TARGET,
				"Candidate hash {} is not present in the block entry's candidates for relay block {}",
				candidate_hash,
				block_entry.parent_hash(),
			);

			metrics.on_approval_error();
			return Ok(Vec::new())
		},
		Some(idx) => idx,
	};

	let session_info = match state.session_info(block_entry.session()) {
		Some(s) => s,
		None => {
			gum::warn!(
				target: LOG_TARGET,
				"Missing session info for live block {} in session {}",
				block_hash,
				block_entry.session(),
			);

			metrics.on_approval_error();
			return Ok(Vec::new())
		},
	};

	let candidate_hash = match block_entry.candidate(candidate_index as usize) {
		Some((_, h)) => *h,
		None => {
			gum::warn!(
				target: LOG_TARGET,
				"Received malformed request to approve out-of-bounds candidate index {} included at block {:?}",
				candidate_index,
				block_hash,
			);

			metrics.on_approval_error();
			return Ok(Vec::new())
		},
	};

	let candidate_entry = match db.load_candidate_entry(&candidate_hash)? {
		Some(c) => c,
		None => {
			gum::warn!(
				target: LOG_TARGET,
				"Missing entry for candidate index {} included at block {:?}",
				candidate_index,
				block_hash,
			);

			metrics.on_approval_error();
			return Ok(Vec::new())
		},
	};

	let validator_pubkey = match session_info.validators.get(validator_index) {
		Some(p) => p,
		None => {
			gum::warn!(
				target: LOG_TARGET,
				"Validator index {} out of bounds in session {}",
				validator_index.0,
				block_entry.session(),
			);

			metrics.on_approval_error();
			return Ok(Vec::new())
		},
	};

	let session = block_entry.session();
	let sig = match sign_approval(&state.keystore, &validator_pubkey, candidate_hash, session) {
		Some(sig) => sig,
		None => {
			gum::warn!(
				target: LOG_TARGET,
				validator_index = ?validator_index,
				session,
				"Could not issue approval signature. Assignment key present but not validator key?",
			);

			metrics.on_approval_error();
			return Ok(Vec::new())
		},
	};

	gum::trace!(
		target: LOG_TARGET,
		?candidate_hash,
		?block_hash,
		validator_index = validator_index.0,
		"Issuing approval vote",
	);

	let actions = advance_approval_state(
		state,
		db,
		metrics,
		block_entry,
		candidate_hash,
		candidate_entry,
		ApprovalStateTransition::LocalApproval(validator_index as _, sig.clone()),
	);

	metrics.on_approval_produced();

	// dispatch to approval distribution.
	ctx.send_unbounded_message(ApprovalDistributionMessage::DistributeApproval(
		IndirectSignedApprovalVote {
			block_hash,
			candidate_index: candidate_index as _,
			validator: validator_index,
			signature: sig,
		},
	));

	Ok(actions)
}

// Sign an approval vote. Fails if the key isn't present in the store.
fn sign_approval(
	keystore: &LocalKeystore,
	public: &ValidatorId,
	candidate_hash: CandidateHash,
	session_index: SessionIndex,
) -> Option<ValidatorSignature> {
	let key = keystore.key_pair::<ValidatorPair>(public).ok().flatten()?;

	let payload = ApprovalVote(candidate_hash).signing_payload(session_index);

	Some(key.sign(&payload[..]))
}

/// Send `IssueLocalStatement` to dispute-coordinator.
fn issue_local_invalid_statement<Sender>(
	sender: &mut Sender,
	session_index: SessionIndex,
	candidate_hash: CandidateHash,
	candidate: CandidateReceipt,
) where
	Sender: overseer::ApprovalVotingSenderTrait,
{
	// We need to send an unbounded message here to break a cycle:
	// DisputeCoordinatorMessage::IssueLocalStatement ->
	// ApprovalVotingMessage::GetApprovalSignaturesForCandidate.
	//
	// Use of unbounded _should_ be fine here as raising a dispute should be an
	// exceptional event. Even in case of bugs: There can be no more than
	// number of slots per block requests every block. Also for sending this
	// message a full recovery and validation procedure took place, which takes
	// longer than issuing a local statement + import.
	sender.send_unbounded_message(DisputeCoordinatorMessage::IssueLocalStatement(
		session_index,
		candidate_hash,
		candidate.clone(),
		false,
	));
}
