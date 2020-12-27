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
	messages::{AssignmentCheckResult, ApprovalCheckResult, ApprovalVotingMessage},
	Subsystem, SubsystemContext, SubsystemError, SubsystemResult, SpawnedSubsystem,
	FromOverseer, OverseerSignal,
};
use polkadot_primitives::v1::{ValidatorIndex, Hash, SessionIndex, SessionInfo};
use polkadot_node_primitives::approval::{
	IndirectAssignmentCert, IndirectSignedApprovalVote, DelayTranche,
};
use sc_keystore::LocalKeystore;
use sp_consensus_slots::SlotNumber;
use sc_client_api::backend::AuxStore;

use futures::prelude::*;
use futures::channel::mpsc;

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};
use std::sync::Arc;

use aux_schema::{TrancheEntry, ApprovalEntry, CandidateEntry, BlockEntry};

mod aux_schema;

const APPROVAL_SESSIONS: SessionIndex = 6;
const LOG_TARGET: &str = "approval-voting";

/// A base unit of time, starting from the unix epoch, split into half-second intervals.
type Tick = u64;

const TICK_DURATION_MILLIS: u64 = 500;
const TICK_DURATION: Duration = Duration::from_millis(TICK_DURATION_MILLIS);

/// The approval voting subsystem.
pub struct ApprovalVotingSubsystem<T> {
	// TODO [now]: keystore. chain config? aux-store.
	_marker: std::marker::PhantomData<T>,
}

impl<T, C> Subsystem<C> for ApprovalVotingSubsystem<T>
	where T: AuxStore + Send + Sync + 'static, C: SubsystemContext<Message = ApprovalVotingMessage> {
	fn start(self, ctx: C) -> SpawnedSubsystem {
		let future = run::<T, C>(ctx)
			.map_err(|e| SubsystemError::with_origin("approval-voting", e))
			.boxed();

		SpawnedSubsystem {
			name: "approval-voting-subsystem",
			future,
		}
	}
}

struct ApprovalVoteRequest {
	validator_index: ValidatorIndex,
	block_hash: Hash,
	candidate_index: u32,
}

struct State<T: AuxStore> {
	earliest_session: SessionIndex,
	session_info: Vec<SessionInfo>,
	keystore: LocalKeystore,
	// Tick -> [(Relay Block, Candidate Hash)]
	wakeups: BTreeMap<Tick, Vec<(Hash, Hash)>>,
	slot_duration_millis: u64,
	db: Arc<T>,

	// These are connected to each other.
	approval_vote_tx: mpsc::Sender<ApprovalVoteRequest>,
	approval_vote_rx: mpsc::Receiver<ApprovalVoteRequest>,
}

fn tick_now() -> Tick {
	time_to_tick(SystemTime::now())
}

// returns '0' if before the unix epoch, otherwise, number of
// whole ticks elapsed since unix epoch.
fn time_to_tick(time: SystemTime) -> Tick {
	match time.duration_since(SystemTime::UNIX_EPOCH) {
		Err(_) => 0,
		Ok(d) => d.as_millis() as u64 / TICK_DURATION_MILLIS,
	}
}

fn tick_to_time(tick: Tick) -> SystemTime {
	SystemTime::UNIX_EPOCH + Duration::from_millis(TICK_DURATION_MILLIS * tick)
}

// assumes `slot_duration_millis` evenly divided by tick duration.
fn slot_number_to_tick(slot_duration_millis: u64, slot: SlotNumber) -> Tick {
	let ticks_per_slot = slot_duration_millis / TICK_DURATION_MILLIS;
	slot * ticks_per_slot
}

// Returns `None` if the tick has been reached or is already
// passed.
fn until_tick(tick: Tick) -> Option<Duration> {
	let now = SystemTime::now();
	let tick_onset = tick_to_time(tick);
	if now < tick_onset {
		tick_onset.duration_since(now).ok()
	} else {
		None
	}
}

async fn run<T, C>(mut ctx: C) -> SubsystemResult<()>
	where T: AuxStore + Send + Sync + 'static, C: SubsystemContext<Message = ApprovalVotingMessage>
{
	let mut state: State<T> = unimplemented!();

	if let Err(e) = aux_schema::clear(&*state.db) {
		tracing::warn!(target: LOG_TARGET, "Failed to clear DB: {:?}", e);
		return Err(SubsystemError::with_origin("db", e));
	}

	loop {
		let mut wait_til_next_tick = match state.wakeups.iter().next() {
			None => future::Either::Left(future::pending()),
			Some((&tick, _)) => future::Either::Right(async move {
				if let Some(until) = until_tick(tick) {
					futures_timer::Delay::new(until).await;
				}

				tick
			})
		};
		futures::pin_mut!(wait_til_next_tick);

		futures::select! {
			tick_wakeup = wait_til_next_tick.fuse() => {
				// TODO [now]
				// should always be "Some" in practice.
				let _woken = state.wakeups.remove(&tick_wakeup).unwrap_or_default();
			}
			next_msg = ctx.recv().fuse() => {
				if handle_from_overseer(&mut ctx, &mut state, next_msg?).await? {
					break
				}
			}
		}
	}

	Ok(())
}

// Handle an incoming signal from the overseer. Returns true if execution should conclude.
async fn handle_from_overseer(
	ctx: &mut impl SubsystemContext,
	state: &mut State<impl AuxStore>,
	x: FromOverseer<ApprovalVotingMessage>,
) -> SubsystemResult<bool> {
	match x {
		FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
			// TODO [now]
			Ok(false)
		}
		FromOverseer::Signal(OverseerSignal::BlockFinalized(block_hash, block_number)) => {
			aux_schema::canonicalize(&*state.db, block_number, block_hash)
				.map(|_| false)
				.map_err(|e| SubsystemError::with_origin("db", e))
		}
		FromOverseer::Signal(OverseerSignal::Conclude) => Ok(true),
		FromOverseer::Communication { msg } => match msg {
			ApprovalVotingMessage::CheckAndImportAssignment(a, res) => {
				let _ = res.send(check_and_import_assignment(ctx, state, a).await);
				Ok(false)
			}
			ApprovalVotingMessage::CheckAndImportApproval(a, res) => {
				let _ = res.send(check_and_import_approval(ctx, state, a).await);
				Ok(false)
			}
			ApprovalVotingMessage::ApprovedAncestor(_target, _lower_bound, _res ) => {
				// TODO [now]
				Ok(false)
			}
		}
	}
}

async fn check_and_import_assignment(
	ctx: &mut impl SubsystemContext,
	state: &mut State<impl AuxStore>,
	assignment: IndirectAssignmentCert,
) -> AssignmentCheckResult {
	// TODO [now]
	unimplemented!()
}

async fn check_and_import_approval(
	ctx: &mut impl SubsystemContext,
	state: &mut State<impl AuxStore>,
	approval: IndirectSignedApprovalVote,
) -> ApprovalCheckResult {
	// TODO [now]
	unimplemented!()
}

enum RequiredTranches {
	// All validators appear to be required, based on tranches already taken and remaining
	// no-shows.
	All,
	// More tranches required - We're awaiting more assignments. The given `DelayTranche`
	// indicates the upper bound of tranches that should broadcast based on the last no-show.
	Pending(DelayTranche),
	// An exact number of required tranches and a number of no-shows. This indicates that
	// the amount of `needed_approvals` are assigned and additionally all no-shows are
	// covered.
	Exact(DelayTranche, usize),
}

fn check_approval(
	block: &BlockEntry,
	candidate: &CandidateEntry,
	approval: &ApprovalEntry,
	required: RequiredTranches,
) -> bool {
	match required {
		RequiredTranches::Pending(_) => false,
		RequiredTranches::All => {
			let approvals = candidate.approvals();
			3 * approvals.count_ones() > 2 * approvals.len()
		}
		RequiredTranches::Exact(tranche, no_shows) => {
			let mut assigned_mask = approval.assignments_up_to(tranche);
			let approvals = candidate.approvals();

			let n_assigned = assigned_mask.count_ones();
			assigned_mask &= approvals.iter().cloned();
			let n_approved = assigned_mask.count_ones();

			// note: the process of computing `required` only chooses `exact` if
			// that will surpass a minimum amount of checks.
			// shouldn't typically go above, since all no-shows are supposed to be covered.
			n_approved + no_shows >= n_assigned
		}
	}
}
