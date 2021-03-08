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

use polkadot_subsystem::{
	messages::{
		AssignmentCheckResult, ApprovalCheckResult, ApprovalVotingMessage,
		RuntimeApiMessage, RuntimeApiRequest, ChainApiMessage, ApprovalDistributionMessage,
		ValidationFailed, CandidateValidationMessage, AvailabilityRecoveryMessage,
	},
	errors::RecoveryError,
	Subsystem, SubsystemContext, SubsystemError, SubsystemResult, SpawnedSubsystem,
	FromOverseer, OverseerSignal,
};
use polkadot_primitives::v1::{
	ValidatorIndex, Hash, SessionIndex, SessionInfo, CandidateHash,
	CandidateReceipt, BlockNumber, PersistedValidationData,
	ValidationCode, CandidateDescriptor, PoV, ValidatorPair, ValidatorSignature, ValidatorId,
	CandidateIndex, GroupIndex,
};
use polkadot_node_primitives::ValidationResult;
use polkadot_node_primitives::approval::{
	IndirectAssignmentCert, IndirectSignedApprovalVote, ApprovalVote, DelayTranche,
};
use polkadot_node_jaeger::Stage as JaegerStage;
use parity_scale_codec::Encode;
use sc_keystore::LocalKeystore;
use sp_consensus_slots::Slot;
use sp_runtime::traits::AppVerify;
use sp_application_crypto::Pair;
use kvdb::KeyValueDB;

use futures::prelude::*;
use futures::channel::{mpsc, oneshot};

use std::collections::{BTreeMap, HashMap};
use std::collections::btree_map::Entry;
use std::sync::Arc;

use approval_checking::RequiredTranches;
use persisted_entries::{ApprovalEntry, CandidateEntry, BlockEntry};
use criteria::{AssignmentCriteria, RealAssignmentCriteria};
use time::{slot_number_to_tick, Tick, Clock, ClockExt, SystemClock};

mod approval_checking;
mod approval_db;
mod criteria;
mod import;
mod time;
mod persisted_entries;

#[cfg(test)]
mod tests;

const APPROVAL_SESSIONS: SessionIndex = 6;
const LOG_TARGET: &str = "approval_voting";

/// Configuration for the approval voting subsystem
pub struct Config {
	/// The path where the approval-voting DB should be kept. This directory is completely removed when starting
	/// the service.
	pub path: std::path::PathBuf,
	/// The cache size, in bytes, to spend on approval checking metadata.
	pub cache_size: Option<usize>,
	/// The slot duration of the consensus algorithm, in milliseconds. Should be evenly
	/// divisible by 500.
	pub slot_duration_millis: u64,
}

/// The approval voting subsystem.
pub struct ApprovalVotingSubsystem {
	/// LocalKeystore is needed for assignment keys, but not necessarily approval keys.
	///
	/// We do a lot of VRF signing and need the keys to have low latency.
	keystore: Arc<LocalKeystore>,
	slot_duration_millis: u64,
	db: Arc<dyn KeyValueDB>,
}

impl ApprovalVotingSubsystem {
	/// Create a new approval voting subsystem with the given keystore, slot duration,
	/// which creates a DB at the given path. This function will delete the directory
	/// at the given path if it already exists.
	pub fn with_config(
		config: Config,
		keystore: Arc<LocalKeystore>,
	) -> std::io::Result<Self> {
		const DEFAULT_CACHE_SIZE: usize = 100 * 1024 * 1024; // 100MiB default should be fine unless finality stalls.

		let db = approval_db::v1::clear_and_recreate(
			&config.path,
			config.cache_size.unwrap_or(DEFAULT_CACHE_SIZE),
		)?;

		Ok(ApprovalVotingSubsystem {
			keystore,
			slot_duration_millis: config.slot_duration_millis,
			db,
		})
	}
}

impl<C> Subsystem<C> for ApprovalVotingSubsystem
	where C: SubsystemContext<Message = ApprovalVotingMessage>
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		let future = run::<C>(
			ctx,
			self,
			Box::new(SystemClock),
			Box::new(RealAssignmentCriteria),
		)
			.map_err(|e| SubsystemError::with_origin("approval-voting", e))
			.boxed();

		SpawnedSubsystem {
			name: "approval-voting-subsystem",
			future,
		}
	}
}

enum BackgroundRequest {
	ApprovalVote(ApprovalVoteRequest),
	CandidateValidation(
		PersistedValidationData,
		ValidationCode,
		CandidateDescriptor,
		Arc<PoV>,
		oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
	),
}

struct ApprovalVoteRequest {
	validator_index: ValidatorIndex,
	block_hash: Hash,
	candidate_index: usize,
}

#[derive(Default)]
struct Wakeups {
	// Tick -> [(Relay Block, Candidate Hash)]
	wakeups: BTreeMap<Tick, Vec<(Hash, CandidateHash)>>,
	reverse_wakeups: HashMap<(Hash, CandidateHash), Tick>,
}

impl Wakeups {
	// Returns the first tick there exist wakeups for, if any.
	fn first(&self) -> Option<Tick> {
		self.wakeups.keys().next().map(|t| *t)
	}

	// Schedules a wakeup at the given tick. no-op if there is already an earlier or equal wake-up
	// for these values. replaces any later wakeup.
	fn schedule(&mut self, block_hash: Hash, candidate_hash: CandidateHash, tick: Tick) {
		if let Some(prev) = self.reverse_wakeups.get(&(block_hash, candidate_hash)) {
			if prev <= &tick { return }

			// we are replacing previous wakeup with an earlier one.
			if let Entry::Occupied(mut entry) = self.wakeups.entry(*prev) {
				if let Some(pos) = entry.get().iter()
					.position(|x| x == &(block_hash, candidate_hash))
				{
					entry.get_mut().remove(pos);
				}

				if entry.get().is_empty() {
					let _ = entry.remove_entry();
				}
			}
		}

		self.reverse_wakeups.insert((block_hash, candidate_hash), tick);
		self.wakeups.entry(tick).or_default().push((block_hash, candidate_hash));
	}

	// Returns the next wakeup. this future never returns if there are no wakeups.
	async fn next(&mut self, clock: &(dyn Clock + Sync)) -> (Tick, Hash, CandidateHash) {
		match self.first() {
			None => future::pending().await,
			Some(tick) => {
				clock.wait(tick).await;
				match self.wakeups.entry(tick) {
					Entry::Vacant(_) => panic!("entry is known to exist since `first` was `Some`; qed"),
					Entry::Occupied(mut entry) => {
						let (hash, candidate_hash) = entry.get_mut().pop()
							.expect("empty entries are removed here and in `schedule`; no other mutation of this map; qed");

						if entry.get().is_empty() {
							let _ = entry.remove();
						}

						self.reverse_wakeups.remove(&(hash, candidate_hash));

						(tick, hash, candidate_hash)
					}
				}
			}
		}
	}
}

/// A read-only handle to a database.
trait DBReader {
	fn load_block_entry(
		&self,
		block_hash: &Hash,
	) -> SubsystemResult<Option<BlockEntry>>;

	fn load_candidate_entry(
		&self,
		candidate_hash: &CandidateHash,
	) -> SubsystemResult<Option<CandidateEntry>>;
}

// This is a submodule to enforce opacity of the inner DB type.
mod approval_db_v1_reader {
	use super::{
		DBReader, KeyValueDB, Hash, CandidateHash, BlockEntry, CandidateEntry,
		Arc, SubsystemResult, SubsystemError, approval_db,
	};

	/// A DB reader that uses the approval-db V1 under the hood.
	pub(super) struct ApprovalDBV1Reader<T: ?Sized>(Arc<T>);

	impl<T: ?Sized> From<Arc<T>> for ApprovalDBV1Reader<T> {
		fn from(a: Arc<T>) -> Self {
			ApprovalDBV1Reader(a)
		}
	}

	impl DBReader for ApprovalDBV1Reader<dyn KeyValueDB> {
		fn load_block_entry(
			&self,
			block_hash: &Hash,
		) -> SubsystemResult<Option<BlockEntry>> {
			approval_db::v1::load_block_entry(&*self.0, block_hash)
				.map(|e| e.map(Into::into))
				.map_err(|e| SubsystemError::with_origin("approval-voting", e))
		}

		fn load_candidate_entry(
			&self,
			candidate_hash: &CandidateHash,
		) -> SubsystemResult<Option<CandidateEntry>> {
			approval_db::v1::load_candidate_entry(&*self.0, candidate_hash)
				.map(|e| e.map(Into::into))
				.map_err(|e| SubsystemError::with_origin("approval-voting", e))
		}
	}
}
use approval_db_v1_reader::ApprovalDBV1Reader;

struct State<T> {
	session_window: import::RollingSessionWindow,
	keystore: Arc<LocalKeystore>,
	slot_duration_millis: u64,
	db: T,
	clock: Box<dyn Clock + Send + Sync>,
	assignment_criteria: Box<dyn AssignmentCriteria + Send + Sync>,
}

impl<T> State<T> {
	fn session_info(&self, i: SessionIndex) -> Option<&SessionInfo> {
		self.session_window.session_info(i)
	}
}

#[derive(Debug)]
enum Action {
	ScheduleWakeup {
		block_hash: Hash,
		candidate_hash: CandidateHash,
		tick: Tick,
	},
	WriteBlockEntry(BlockEntry),
	WriteCandidateEntry(CandidateHash, CandidateEntry),
	LaunchApproval {
		indirect_cert: IndirectAssignmentCert,
		candidate_index: CandidateIndex,
		session: SessionIndex,
		candidate: CandidateReceipt,
		backing_group: GroupIndex,
	},
	Conclude,
}

async fn run<C>(
	mut ctx: C,
	subsystem: ApprovalVotingSubsystem,
	clock: Box<dyn Clock + Send + Sync>,
	assignment_criteria: Box<dyn AssignmentCriteria + Send + Sync>,
) -> SubsystemResult<()>
	where C: SubsystemContext<Message = ApprovalVotingMessage>
{
	let (background_tx, background_rx) = mpsc::channel::<BackgroundRequest>(64);
	let mut state = State {
		session_window: Default::default(),
		keystore: subsystem.keystore,
		slot_duration_millis: subsystem.slot_duration_millis,
		db: ApprovalDBV1Reader::from(subsystem.db.clone()),
		clock,
		assignment_criteria,
	};

	let mut wakeups = Wakeups::default();

	let mut last_finalized_height: Option<BlockNumber> = None;
	let mut background_rx = background_rx.fuse();

	let db_writer = &*subsystem.db;

	loop {
		let actions = futures::select! {
			(tick, woken_block, woken_candidate) = wakeups.next(&*state.clock).fuse() => {
				process_wakeup(
					&mut state,
					woken_block,
					woken_candidate,
					tick,
				)?
			}
			next_msg = ctx.recv().fuse() => {
				handle_from_overseer(
					&mut ctx,
					&mut state,
					db_writer,
					next_msg?,
					&mut last_finalized_height,
				).await?
			}
			background_request = background_rx.next().fuse() => {
				if let Some(req) = background_request {
					handle_background_request(
						&mut ctx,
						&mut state,
						req,
					).await?
				} else {
					Vec::new()
				}
			}
		};

		if handle_actions(
			&mut ctx,
			&mut wakeups,
			db_writer,
			&background_tx,
			actions,
		).await? {
			break;
		}
	}

	Ok(())
}

// returns `true` if any of the actions was a `Conclude` command.
async fn handle_actions(
	ctx: &mut impl SubsystemContext,
	wakeups: &mut Wakeups,
	db: &dyn KeyValueDB,
	background_tx: &mpsc::Sender<BackgroundRequest>,
	actions: impl IntoIterator<Item = Action>,
) -> SubsystemResult<bool> {
	let mut transaction = approval_db::v1::Transaction::default();
	let mut conclude = false;

	for action in actions {
		match action {
			Action::ScheduleWakeup {
				block_hash,
				candidate_hash,
				tick,
			} => wakeups.schedule(block_hash, candidate_hash, tick),
			Action::WriteBlockEntry(block_entry) => {
				transaction.put_block_entry(block_entry.into());
			}
			Action::WriteCandidateEntry(candidate_hash, candidate_entry) => {
				transaction.put_candidate_entry(candidate_hash, candidate_entry.into());
			}
			Action::LaunchApproval {
				indirect_cert,
				candidate_index,
				session,
				candidate,
				backing_group,
			} => {
				let block_hash = indirect_cert.block_hash;
				let validator_index = indirect_cert.validator;

				ctx.send_message(ApprovalDistributionMessage::DistributeAssignment(
					indirect_cert,
					candidate_index,
				).into()).await;

				launch_approval(
					ctx,
					background_tx.clone(),
					session,
					&candidate,
					validator_index,
					block_hash,
					candidate_index as _,
					backing_group,
				).await?
			}
			Action::Conclude => { conclude = true; }
		}
	}

	transaction.write(db)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?;

	Ok(conclude)
}

// Handle an incoming signal from the overseer. Returns true if execution should conclude.
async fn handle_from_overseer(
	ctx: &mut impl SubsystemContext,
	state: &mut State<impl DBReader>,
	db_writer: &dyn KeyValueDB,
	x: FromOverseer<ApprovalVotingMessage>,
	last_finalized_height: &mut Option<BlockNumber>,
) -> SubsystemResult<Vec<Action>> {

	let actions = match x {
		FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
			let mut actions = Vec::new();

			for (head, _span) in update.activated {
				match import::handle_new_head(
					ctx,
					state,
					db_writer,
					head,
					&*last_finalized_height,
				).await {
					Err(e) => return Err(SubsystemError::with_origin("db", e)),
					Ok(block_imported_candidates) => {
						// Schedule wakeups for all imported candidates.
						for block_batch in block_imported_candidates {
							tracing::debug!(
								target: LOG_TARGET,
								"Imported new block {} with {} included candidates",
								block_batch.block_hash,
								block_batch.imported_candidates.len(),
							);

							for (c_hash, c_entry) in block_batch.imported_candidates {
								let our_tranche = c_entry
									.approval_entry(&block_batch.block_hash)
									.and_then(|a| a.our_assignment().map(|a| a.tranche()));

								if let Some(our_tranche) = our_tranche {
									let tick = our_tranche as Tick + block_batch.block_tick;
									tracing::trace!(
										target: LOG_TARGET,
										"Scheduling first wakeup at tranche {} for candidate {} in block ({}, tick={})",
										our_tranche,
										c_hash,
										block_batch.block_hash,
										block_batch.block_tick,
									);

									// Our first wakeup will just be the tranche of our assignment,
									// if any. This will likely be superseded by incoming assignments
									// and approvals which trigger rescheduling.
									actions.push(Action::ScheduleWakeup {
										block_hash: block_batch.block_hash,
										candidate_hash: c_hash,
										tick,
									});
								}
							}
						}
					}
				}
			}

			actions
		}
		FromOverseer::Signal(OverseerSignal::BlockFinalized(block_hash, block_number)) => {
			*last_finalized_height = Some(block_number);

			approval_db::v1::canonicalize(db_writer, block_number, block_hash)
				.map_err(|e| SubsystemError::with_origin("db", e))?;

			Vec::new()
		}
		FromOverseer::Signal(OverseerSignal::Conclude) => {
			vec![Action::Conclude]
		}
		FromOverseer::Communication { msg } => match msg {
			ApprovalVotingMessage::CheckAndImportAssignment(a, claimed_core, res) => {
				let (check_outcome, actions) = check_and_import_assignment(state, a, claimed_core)?;
				let _ = res.send(check_outcome);
				actions
			}
			ApprovalVotingMessage::CheckAndImportApproval(a, res) => {
				check_and_import_approval(state, a, |r| { let _ = res.send(r); })?.0
			}
			ApprovalVotingMessage::ApprovedAncestor(target, lower_bound, res ) => {
				match handle_approved_ancestor(ctx, &state.db, target, lower_bound).await {
					Ok(v) => {
						let _ = res.send(v);
					}
					Err(e) => {
						let _ = res.send(None);
						return Err(e);
					}
				}

				Vec::new()
			}
		}
	};

	Ok(actions)
}

async fn handle_background_request(
	ctx: &mut impl SubsystemContext,
	state: &State<impl DBReader>,
	request: BackgroundRequest,
) -> SubsystemResult<Vec<Action>> {
	match request {
		BackgroundRequest::ApprovalVote(vote_request) => {
			issue_approval(ctx, state, vote_request).await
		}
		BackgroundRequest::CandidateValidation(
			validation_data,
			validation_code,
			descriptor,
			pov,
			tx,
		) => {
			ctx.send_message(CandidateValidationMessage::ValidateFromExhaustive(
				validation_data,
				validation_code,
				descriptor,
				pov,
				tx,
			).into()).await;

			Ok(Vec::new())
		}
	}
}

async fn handle_approved_ancestor(
	ctx: &mut impl SubsystemContext,
	db: &impl DBReader,
	target: Hash,
	lower_bound: BlockNumber,
) -> SubsystemResult<Option<(Hash, BlockNumber)>> {
	const MAX_TRACING_WINDOW: usize = 200;

	use bitvec::{order::Lsb0, vec::BitVec};

	let mut span = polkadot_node_jaeger::hash_span(&target, "approved-ancestor");
	span.add_stage(JaegerStage::ApprovalChecking);

	let mut all_approved_max = None;

	let target_number = {
		let (tx, rx) = oneshot::channel();

		ctx.send_message(ChainApiMessage::BlockNumber(target, tx).into()).await;

		match rx.await? {
			Ok(Some(n)) => n,
			Ok(None) => return Ok(None),
			Err(_) => return Ok(None),
		}
	};

	if target_number <= lower_bound { return Ok(None) }

	span.add_string_tag("target-number", &format!("{}", target_number));
	span.add_string_tag("target-hash", &format!("{}", target));

	// request ancestors up to but not including the lower bound,
	// as a vote on the lower bound is implied if we cannot find
	// anything else.
	let ancestry = if target_number > lower_bound + 1 {
		let (tx, rx) = oneshot::channel();

		ctx.send_message(ChainApiMessage::Ancestors {
			hash: target,
			k: (target_number - (lower_bound + 1)) as usize,
			response_channel: tx,
		}.into()).await;

		match rx.await? {
			Ok(a) => a,
			Err(_) => return Ok(None),
		}
	} else {
		Vec::new()
	};

	let mut bits: BitVec<Lsb0, u8> = Default::default();
	for (i, block_hash) in std::iter::once(target).chain(ancestry).enumerate() {
		// Block entries should be present as the assumption is that
		// nothing here is finalized. If we encounter any missing block
		// entries we can fail.
		let entry = match db.load_block_entry(&block_hash)? {
			None => {
				tracing::trace!{
					target: LOG_TARGET,
					"Chain between ({}, {}) and {} not fully known. Forcing vote on {}",
					target,
					target_number,
					lower_bound,
					lower_bound,
				}
				return Ok(None);
			}
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
		} else {
			all_approved_max = None;
		}
	}

	tracing::trace!(
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
				if (target_number - i as u32) % 10 == 0 && i != bits.len() - 1 { s.push(' '); }
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

	match all_approved_max {
		Some((ref hash, ref number)) => {
			span.add_string_tag("approved-number", &format!("{}", number));
			span.add_string_tag("approved-hash", &format!("{:?}", hash));
		}
		None => {
			span.add_string_tag("reached-lower-bound", "true");
		}
	}

	Ok(all_approved_max)
}

fn approval_signing_payload(
	approval_vote: ApprovalVote,
	session_index: SessionIndex,
) -> Vec<u8> {
	(approval_vote, session_index).encode()
}

// `Option::cmp` treats `None` as less than `Some`.
fn min_prefer_some<T: std::cmp::Ord>(
	a: Option<T>,
	b: Option<T>,
) -> Option<T> {
	match (a, b) {
		(None, None) => None,
		(None, Some(x)) | (Some(x), None) => Some(x),
		(Some(x), Some(y)) => Some(std::cmp::min(x, y)),
	}
}

fn schedule_wakeup_action(
	approval_entry: &ApprovalEntry,
	block_hash: Hash,
	candidate_hash: CandidateHash,
	block_tick: Tick,
	required_tranches: RequiredTranches,
) -> Option<Action> {
	let maybe_action = match required_tranches {
		_ if approval_entry.is_approved() => None,
		RequiredTranches::All => None,
		RequiredTranches::Exact { next_no_show, .. } => next_no_show.map(|tick| Action::ScheduleWakeup {
			block_hash,
			candidate_hash,
			tick,
		}),
		RequiredTranches::Pending { considered, next_no_show, clock_drift, .. } => {
			// select the minimum of `next_no_show`, or the tick of the next non-empty tranche
			// after `considered`, including any tranche that might contain our own untriggered
			// assignment.
			let next_non_empty_tranche = {
				let next_announced = approval_entry.tranches().iter()
					.skip_while(|t| t.tranche() <= considered)
					.map(|t| t.tranche())
					.next();

				let our_untriggered = approval_entry
					.our_assignment()
					.and_then(|t| if !t.triggered() && t.tranche() > considered {
						Some(t.tranche())
					} else {
						None
					});

				// Apply the clock drift to these tranches.
				min_prefer_some(next_announced, our_untriggered)
					.map(|t| t as Tick + block_tick + clock_drift)
			};

			min_prefer_some(next_non_empty_tranche, next_no_show)
				.map(|tick| Action::ScheduleWakeup { block_hash, candidate_hash, tick })
		}
	};

	match maybe_action {
		Some(Action::ScheduleWakeup { ref tick, .. }) => tracing::debug!(
			target: LOG_TARGET,
			"Scheduling next wakeup at {} for candidate {} under block ({}, tick={})",
			tick,
			candidate_hash,
			block_hash,
			block_tick,
		),
		None => tracing::debug!(
			target: LOG_TARGET,
			"No wakeup needed for candidate {} under block ({}, tick={})",
			candidate_hash,
			block_hash,
			block_tick,
		),
		Some(_) => {} // unreachable
	}

	maybe_action
}

fn check_and_import_assignment(
	state: &State<impl DBReader>,
	assignment: IndirectAssignmentCert,
	candidate_index: CandidateIndex,
) -> SubsystemResult<(AssignmentCheckResult, Vec<Action>)> {
	const TICK_TOO_FAR_IN_FUTURE: Tick = 20; // 10 seconds.

	let tick_now = state.clock.tick_now();
	let block_entry = match state.db.load_block_entry(&assignment.block_hash)? {
		Some(b) => b,
		None => return Ok((AssignmentCheckResult::Bad, Vec::new())),
	};

	let session_info = match state.session_info(block_entry.session()) {
		Some(s) => s,
		None => {
			tracing::warn!(target: LOG_TARGET, "Unknown session info for {}", block_entry.session());
			return Ok((AssignmentCheckResult::Bad, Vec::new()));
		}
	};

	let (claimed_core_index, assigned_candidate_hash)
		= match block_entry.candidate(candidate_index as usize)
	{
		Some((c, h)) => (*c, *h),
		None => return Ok((AssignmentCheckResult::Bad, Vec::new())), // no candidate at core.
	};

	let mut candidate_entry = match state.db.load_candidate_entry(&assigned_candidate_hash)? {
		Some(c) => c,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Missing candidate entry {} referenced in live block {}",
				assigned_candidate_hash,
				assignment.block_hash,
			);

			return Ok((AssignmentCheckResult::Bad, Vec::new()));
		}
	};

	let res = {
		// import the assignment.
		let approval_entry = match
			candidate_entry.approval_entry_mut(&assignment.block_hash)
		{
			Some(a) => a,
			None => return Ok((AssignmentCheckResult::Bad, Vec::new())),
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
			Err(crate::criteria::InvalidAssignment) => return Ok((AssignmentCheckResult::Bad, Vec::new())),
			Ok(tranche) => {
				let current_tranche = state.clock.tranche_now(
					state.slot_duration_millis,
					block_entry.slot(),
				);

				let too_far_in_future = current_tranche + TICK_TOO_FAR_IN_FUTURE as DelayTranche;

				if tranche >= too_far_in_future {
					return Ok((AssignmentCheckResult::TooFarInFuture, Vec::new()));
				}

				tranche
			}
		};

		let is_duplicate =  approval_entry.is_assigned(assignment.validator);
		approval_entry.import_assignment(tranche, assignment.validator, tick_now);

		if is_duplicate {
			AssignmentCheckResult::AcceptedDuplicate
		} else {
			tracing::trace!(
				target: LOG_TARGET,
				"Imported assignment from validator {} on candidate {:?}",
				assignment.validator.0,
				(assigned_candidate_hash, candidate_entry.candidate_receipt().descriptor.para_id),
			);

			AssignmentCheckResult::Accepted
		}
	};

	// We check for approvals here because we may be late in seeing a block containing a
	// candidate for which we have already seen approvals by the same validator.
	//
	// For these candidates, we will receive the assignments potentially after a corresponding
	// approval, and so we must check for approval here.
	//
	// Note that this already produces actions for writing
	// the candidate entry and any modified block entries to disk.
	//
	// It also produces actions to schedule wakeups for the candidate.
	let actions = check_and_apply_full_approval(
		state,
		Some((assignment.block_hash, block_entry)),
		assigned_candidate_hash,
		candidate_entry,
		|h, _| h == &assignment.block_hash,
	)?;

	Ok((res, actions))
}

fn check_and_import_approval<T>(
	state: &State<impl DBReader>,
	approval: IndirectSignedApprovalVote,
	with_response: impl FnOnce(ApprovalCheckResult) -> T,
) -> SubsystemResult<(Vec<Action>, T)> {
	macro_rules! respond_early {
		($e: expr) => { {
			let t = with_response($e);
			return Ok((Vec::new(), t));
		} }
	}

	let block_entry = match state.db.load_block_entry(&approval.block_hash)? {
		Some(b) => b,
		None => respond_early!(ApprovalCheckResult::Bad)
	};

	let session_info = match state.session_info(block_entry.session()) {
		Some(s) => s,
		None => {
			tracing::warn!(target: LOG_TARGET, "Unknown session info for {}", block_entry.session());
			respond_early!(ApprovalCheckResult::Bad)
		}
	};

	let approved_candidate_hash = match block_entry.candidate(approval.candidate_index as usize) {
		Some((_, h)) => *h,
		None => respond_early!(ApprovalCheckResult::Bad)
	};

	let approval_payload = approval_signing_payload(
		ApprovalVote(approved_candidate_hash),
		block_entry.session(),
	);

	let pubkey = match session_info.validators.get(approval.validator.0 as usize) {
		Some(k) => k,
		None => respond_early!(ApprovalCheckResult::Bad)
	};

	let approval_sig_valid = approval.signature.verify(approval_payload.as_slice(), pubkey);

	if !approval_sig_valid {
		respond_early!(ApprovalCheckResult::Bad)
	}

	let candidate_entry = match state.db.load_candidate_entry(&approved_candidate_hash)? {
		Some(c) => c,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Unknown candidate entry for {}",
				approved_candidate_hash,
			);

			respond_early!(ApprovalCheckResult::Bad)
		}
	};

	// Don't accept approvals until assignment.
	if candidate_entry.approval_entry(&approval.block_hash)
		.map_or(true, |e| !e.is_assigned(approval.validator))
	{
		respond_early!(ApprovalCheckResult::Bad)
	}

	// importing the approval can be heavy as it may trigger acceptance for a series of blocks.
	let t = with_response(ApprovalCheckResult::Accepted);

	tracing::trace!(
		target: LOG_TARGET,
		"Importing approval vote from validator {:?} on candidate {:?}",
		(approval.validator, &pubkey),
		(approved_candidate_hash, candidate_entry.candidate_receipt().descriptor.para_id),
	);

	let actions = import_checked_approval(
		state,
		Some((approval.block_hash, block_entry)),
		approved_candidate_hash,
		candidate_entry,
		approval.validator,
	)?;

	Ok((actions, t))
}

fn import_checked_approval(
	state: &State<impl DBReader>,
	already_loaded: Option<(Hash, BlockEntry)>,
	candidate_hash: CandidateHash,
	mut candidate_entry: CandidateEntry,
	validator: ValidatorIndex,
) -> SubsystemResult<Vec<Action>> {
	if candidate_entry.mark_approval(validator) {
		// already approved - nothing to do here.
		return Ok(Vec::new());
	}

	// Check if this approval vote alters the approval state of any blocks.
	//
	// This may include blocks beyond the already loaded block.
	let actions = check_and_apply_full_approval(
		state,
		already_loaded,
		candidate_hash,
		candidate_entry,
		|_, a| a.is_assigned(validator),
	)?;

	Ok(actions)
}

// Checks the candidate for full approval under all blocks matching the given filter.
//
// If returning without error, is guaranteed to have produced actions
// to write all modified block entries. It also schedules wakeups for
// the candidate under any blocks filtered.
fn check_and_apply_full_approval(
	state: &State<impl DBReader>,
	mut already_loaded: Option<(Hash, BlockEntry)>,
	candidate_hash: CandidateHash,
	mut candidate_entry: CandidateEntry,
	filter: impl Fn(&Hash, &ApprovalEntry) -> bool,
) -> SubsystemResult<Vec<Action>> {
	// We only query this max once per hash.
	let db = &state.db;
	let mut load_block_entry = move |block_hash| -> SubsystemResult<Option<BlockEntry>> {
		if already_loaded.as_ref().map_or(false, |(h, _)| h == block_hash) {
			Ok(already_loaded.take().map(|(_, c)| c))
		} else {
			db.load_block_entry(block_hash)
		}
	};

	let mut newly_approved = Vec::new();
	let mut actions = Vec::new();
	for (block_hash, approval_entry) in candidate_entry.iter_approval_entries()
		.into_iter()
		.filter(|(h, a)| !a.is_approved() && filter(h, a))
	{
		let mut block_entry = match load_block_entry(block_hash)? {
			None => {
				tracing::warn!(
					target: LOG_TARGET,
					"Missing block entry {} referenced by candidate {}",
					block_hash,
					candidate_hash,
				);
				continue
			}
			Some(b) => b,
		};

		let session_info = match state.session_info(block_entry.session()) {
			Some(s) => s,
			None => {
				tracing::warn!(target: LOG_TARGET, "Unknown session info for {}", block_entry.session());
				continue
			}
		};

		let tranche_now = state.clock.tranche_now(state.slot_duration_millis, block_entry.slot());
		let block_tick = slot_number_to_tick(state.slot_duration_millis, block_entry.slot());
		let no_show_duration = slot_number_to_tick(
			state.slot_duration_millis,
			Slot::from(u64::from(session_info.no_show_slots)),
		);

		let required_tranches = approval_checking::tranches_to_approve(
			approval_entry,
			candidate_entry.approvals(),
			tranche_now,
			block_tick,
			no_show_duration,
			session_info.needed_approvals as _
		);

		let now_approved = approval_checking::check_approval(
			&candidate_entry,
			approval_entry,
			required_tranches.clone(),
		);

		if now_approved {
			tracing::trace!(
				target: LOG_TARGET,
				"Candidate approved {} under block {}",
				candidate_hash,
				block_hash,
			);

			newly_approved.push(*block_hash);
			block_entry.mark_approved_by_hash(&candidate_hash);

			actions.push(Action::WriteBlockEntry(block_entry));
		}

		actions.extend(schedule_wakeup_action(
			&approval_entry,
			*block_hash,
			candidate_hash,
			block_tick,
			required_tranches,
		));
	}

	for b in &newly_approved {
		if let Some(a) = candidate_entry.approval_entry_mut(b) {
			a.mark_approved();
		}
	}

	actions.push(Action::WriteCandidateEntry(candidate_hash, candidate_entry));
	Ok(actions)
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
		Some(ref assignment) => {
			match required_tranches {
				RequiredTranches::All => !approval_checking::check_approval(
					&candidate_entry,
					&approval_entry,
					RequiredTranches::All,
				),
				RequiredTranches::Pending {
					maximum_broadcast,
					clock_drift,
					..
				} => {
					let drifted_tranche_now
						= tranche_now.saturating_sub(clock_drift as DelayTranche);
					assignment.tranche() <= maximum_broadcast
						&& assignment.tranche() <= drifted_tranche_now
				}
				RequiredTranches::Exact { .. } => {
					// indicates that no new assignments are needed at the moment.
					false
				}
			}
		}
	}
}

fn process_wakeup(
	state: &State<impl DBReader>,
	relay_block: Hash,
	candidate_hash: CandidateHash,
	expected_tick: Tick,
) -> SubsystemResult<Vec<Action>> {
	let mut span = polkadot_node_jaeger::descriptor_span(
		(relay_block, candidate_hash, expected_tick),
		"process-approval-wakeup",
	);

	span.add_string_tag("relay-parent", &format!("{:?}", relay_block));
	span.add_string_tag("candidate-hash", &format!("{:?}", candidate_hash));
	span.add_string_tag("tick", &format!("{:?}", expected_tick));
	span.add_stage(JaegerStage::ApprovalChecking);

	let block_entry = state.db.load_block_entry(&relay_block)?;
	let candidate_entry = state.db.load_candidate_entry(&candidate_hash)?;

	// If either is not present, we have nothing to wakeup. Might have lost a race with finality
	let (block_entry, mut candidate_entry) = match (block_entry, candidate_entry) {
		(Some(b), Some(c)) => (b, c),
		_ => return Ok(Vec::new()),
	};

	let session_info = match state.session_info(block_entry.session()) {
		Some(i) => i,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Missing session info for live block {} in session {}",
				relay_block,
				block_entry.session(),
			);

			return Ok(Vec::new())
		}
	};

	let block_tick = slot_number_to_tick(state.slot_duration_millis, block_entry.slot());
	let no_show_duration = slot_number_to_tick(
		state.slot_duration_millis,
		Slot::from(u64::from(session_info.no_show_slots)),
	);

	let tranche_now = state.clock.tranche_now(state.slot_duration_millis, block_entry.slot());

	tracing::debug!(
		target: LOG_TARGET,
		"Processing wakeup at tranche {} for candidate {} under block {}",
		tranche_now,
		candidate_hash,
		relay_block,
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

	let (mut actions, maybe_cert) = if should_trigger {
		let maybe_cert = {
			let approval_entry = candidate_entry.approval_entry_mut(&relay_block)
				.expect("should_trigger only true if this fetched earlier; qed");

			approval_entry.trigger_our_assignment(state.clock.tick_now())
		};

		let actions = vec![Action::WriteCandidateEntry(candidate_hash, candidate_entry.clone())];

		(actions, maybe_cert)
	} else {
		(Vec::new(), None)
	};

	if let Some((cert, val_index)) = maybe_cert {
		let indirect_cert = IndirectAssignmentCert {
			block_hash: relay_block,
			validator: val_index,
			cert,
		};

		let index_in_candidate = block_entry.candidates().iter()
			.position(|(_, h)| &candidate_hash == h);

		if let Some(i) = index_in_candidate {
			tracing::debug!(
				target: LOG_TARGET,
				"Launching approval work for candidate {:?} in block {}",
				(&candidate_hash, candidate_entry.candidate_receipt().descriptor.para_id),
				relay_block,
			);

			// sanity: should always be present.
			actions.push(Action::LaunchApproval {
				indirect_cert,
				candidate_index: i as _,
				session: block_entry.session(),
				candidate: candidate_entry.candidate_receipt().clone(),
				backing_group,
			});
		}
	}

	let approval_entry = candidate_entry.approval_entry(&relay_block)
		.expect("this function returned earlier if not available; qed");

	// Although we ran this earlier in the function, we need to run again because we might have
	// imported our own assignment, which could change things.
	let tranches_to_approve = approval_checking::tranches_to_approve(
		&approval_entry,
		candidate_entry.approvals(),
		tranche_now,
		block_tick,
		no_show_duration,
		session_info.needed_approvals as _,
	);

	actions.extend(schedule_wakeup_action(
		&approval_entry,
		relay_block,
		candidate_hash,
		block_tick,
		tranches_to_approve,
	));

	Ok(actions)
}

async fn launch_approval(
	ctx: &mut impl SubsystemContext,
	mut background_tx: mpsc::Sender<BackgroundRequest>,
	session_index: SessionIndex,
	candidate: &CandidateReceipt,
	validator_index: ValidatorIndex,
	block_hash: Hash,
	candidate_index: usize,
	backing_group: GroupIndex,
) -> SubsystemResult<()> {
	let (a_tx, a_rx) = oneshot::channel();
	let (code_tx, code_rx) = oneshot::channel();
	let (context_num_tx, context_num_rx) = oneshot::channel();

	let candidate_hash = candidate.hash();

	tracing::debug!(
		target: LOG_TARGET,
		"Recovering data for candidate {:?}",
		(candidate_hash, candidate.descriptor.para_id),
	);

	ctx.send_message(AvailabilityRecoveryMessage::RecoverAvailableData(
		candidate.clone(),
		session_index,
		Some(backing_group),
		a_tx,
	).into()).await;

	ctx.send_message(
		ChainApiMessage::BlockNumber(candidate.descriptor.relay_parent, context_num_tx).into()
	).await;

	let in_context_number = match context_num_rx.await?
		.map_err(|e| SubsystemError::with_origin("chain-api", e))?
	{
		Some(n) => n,
		None => return Ok(()),
	};

	ctx.send_message(
		RuntimeApiMessage::Request(
			block_hash,
			RuntimeApiRequest::HistoricalValidationCode(
				candidate.descriptor.para_id,
				in_context_number,
				code_tx,
			),
		).into()
	).await;

	let candidate = candidate.clone();
	let background = async move {
		let available_data = match a_rx.await {
			Err(_) => return,
			Ok(Ok(a)) => a,
			Ok(Err(RecoveryError::Unavailable)) => {
				tracing::warn!(
					target: LOG_TARGET,
					"Data unavailable for candidate {:?}",
					(candidate_hash, candidate.descriptor.para_id),
				);
				// do nothing. we'll just be a no-show and that'll cause others to rise up.
				return;
			}
			Ok(Err(RecoveryError::Invalid)) => {
				tracing::warn!(
					target: LOG_TARGET,
					"Data recovery invalid for candidate {:?}",
					(candidate_hash, candidate.descriptor.para_id),
				);

				// TODO: dispute. Either the merkle trie is bad or the erasure root is.
				// https://github.com/paritytech/polkadot/issues/2176
				return;
			}
		};

		let validation_code = match code_rx.await {
			Err(_) => return,
			Ok(Err(_)) => return,
			Ok(Ok(Some(code))) => code,
			Ok(Ok(None)) => {
				tracing::warn!(
					target: LOG_TARGET,
					"Validation code unavailable for block {:?} in the state of block {:?} (a recent descendant)",
					candidate.descriptor.relay_parent,
					block_hash,
				);

				// No dispute necessary, as this indicates that the chain is not behaving
				// according to expectations.
				return;
			}
		};

		let (val_tx, val_rx) = oneshot::channel();

		let para_id = candidate.descriptor.para_id;
		let _ = background_tx.send(BackgroundRequest::CandidateValidation(
			available_data.validation_data,
			validation_code,
			candidate.descriptor,
			available_data.pov,
			val_tx,
		)).await;

		match val_rx.await {
			Err(_) => return,
			Ok(Ok(ValidationResult::Valid(_, _))) => {
				// Validation checked out. Issue an approval command. If the underlying service is unreachable,
				// then there isn't anything we can do.

				tracing::debug!(
					target: LOG_TARGET,
					"Candidate Valid {:?}",
					(candidate_hash, para_id),
				);

				let _ = background_tx.send(BackgroundRequest::ApprovalVote(ApprovalVoteRequest {
					validator_index,
					block_hash,
					candidate_index,
				})).await;
			}
			Ok(Ok(ValidationResult::Invalid(_))) => {
				tracing::warn!(
					target: LOG_TARGET,
					"Detected invalid candidate as an approval checker {:?}",
					(candidate_hash, para_id),
				);

				// TODO: issue dispute, but not for timeouts.
				// https://github.com/paritytech/polkadot/issues/2176
			}
			Ok(Err(_)) => return, // internal error.
		}
	};

	ctx.spawn("approval-checks", Box::pin(background)).await
}

// Issue and import a local approval vote. Should only be invoked after approval checks
// have been done.
async fn issue_approval(
	ctx: &mut impl SubsystemContext,
	state: &State<impl DBReader>,
	request: ApprovalVoteRequest,
) -> SubsystemResult<Vec<Action>> {
	let ApprovalVoteRequest { validator_index, block_hash, candidate_index } = request;

	let block_entry = match state.db.load_block_entry(&block_hash)? {
		Some(b) => b,
		None => return Ok(Vec::new()), // not a cause for alarm - just lost a race with pruning, most likely.
	};

	let session_info = match state.session_info(block_entry.session()) {
		Some(s) => s,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Missing session info for live block {} in session {}",
				block_hash,
				block_entry.session(),
			);

			return Ok(Vec::new());
		}
	};

	let candidate_hash = match block_entry.candidate(candidate_index) {
		Some((_, h)) => h.clone(),
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Received malformed request to approve out-of-bounds candidate index {} included at block {:?}",
				candidate_index,
				block_hash,
			);

			return Ok(Vec::new());
		}
	};

	let candidate_entry = match state.db.load_candidate_entry(&candidate_hash)? {
		Some(c) => c,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Missing entry for candidate index {} included at block {:?}",
				candidate_index,
				block_hash,
			);

			return Ok(Vec::new());
		}
	};

	let validator_pubkey = match session_info.validators.get(validator_index.0 as usize) {
		Some(p) => p,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Validator index {} out of bounds in session {}",
				validator_index.0,
				block_entry.session(),
			);

			return Ok(Vec::new());
		}
	};

	let sig = match sign_approval(
		&state.keystore,
		&validator_pubkey,
		candidate_hash,
		block_entry.session(),
	) {
		Some(sig) => sig,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Could not issue approval signature with validator index {} in session {}. Assignment key present but not validator key?",
				validator_index.0,
				block_entry.session(),
			);

			return Ok(Vec::new());
		}
	};

	tracing::debug!(
		target: LOG_TARGET,
		"Issuing approval vote for candidate {:?}",
		candidate_hash,
	);

	let actions = import_checked_approval(
		state,
		Some((block_hash, block_entry)),
		candidate_hash,
		candidate_entry,
		validator_index as _,
	)?;

	// dispatch to approval distribution.
	ctx.send_message(ApprovalDistributionMessage::DistributeApproval(IndirectSignedApprovalVote {
		block_hash,
		candidate_index: candidate_index as _,
		validator: validator_index,
		signature: sig,
	}).into()).await;

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

	let payload = approval_signing_payload(
		ApprovalVote(candidate_hash),
		session_index,
	);

	Some(key.sign(&payload[..]))
}
