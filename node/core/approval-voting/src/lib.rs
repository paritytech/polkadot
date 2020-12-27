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
	IndirectAssignmentCert, IndirectSignedApprovalVote
};
use sc_keystore::LocalKeystore;
use sp_consensus_slots::SlotNumber;

use futures::prelude::*;
use futures::channel::mpsc;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

mod aux_schema;

const APPROVAL_SESSIONS: SessionIndex = 6;
const LOG_TARGET: &str = "approval-voting";

/// A base unit of time, starting from the unix epoch, split into half-second intervals.
type Tick = u64;

const TICK_DURATION_MILLIS: u64 = 500;
const TICK_DURATION: Duration = Duration::from_millis(TICK_DURATION_MILLIS);

/// The approval voting subsystem.
pub struct ApprovalVotingSubsystem {
	// TODO [now]: keystore. chain config? aux-store.
}

impl<C: SubsystemContext<Message = ApprovalVotingMessage>> Subsystem<C> for ApprovalVotingSubsystem {
	fn start(self, ctx: C) -> SpawnedSubsystem {
		let future = run(ctx)
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

struct State {
	earliest_session: SessionIndex,
	session_info: Vec<SessionInfo>,
	keystore: LocalKeystore,
	// Tick -> [(Relay Block, Candidate Hash)]
	wakeups: BTreeMap<Tick, Vec<(Hash, Hash)>>,
	slot_duration: Duration,

	// These are connected to each other.
	approval_vote_tx: mpsc::Sender<ApprovalVoteRequest>,
	approval_vote_rx: mpsc::Receiver<ApprovalVoteRequest>,
}

fn tick_now() -> Tick {
	instant_to_tick(Instant::now())
}

fn instant_to_tick(instant: Instant) -> Tick {
	// TODO [now]
	unimplemented!()
}

fn tick_to_instant(tick: Tick) -> Instant {
	// TODO [now]
	unimplemented!()
}

fn slot_number_to_tick(slot: SlotNumber) -> Tick {
	// TODO [now]
	unimplemented!()
}

// Returns `None` if the tick has been reached or is already
// passed.
fn until_tick(tick: Tick) -> Option<Duration> {
	let now = Instant::now();
	let tick_onset = tick_to_instant(tick);
	if now < tick_onset {
		Some(tick_onset - now)
	} else {
		None
	}
}

async fn run(mut ctx: impl SubsystemContext<Message = ApprovalVotingMessage>)
	-> SubsystemResult<()>
{
	let mut state: State = unimplemented!();

	// TODO [now]: clear DB.

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
				if handle_from_overseer(&mut ctx, &mut state, next_msg?).await {
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
	state: &mut State,
	x: FromOverseer<ApprovalVotingMessage>,
) -> bool {
	match x {
		FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
			// TODO [now]
			false
		}
		FromOverseer::Signal(OverseerSignal::BlockFinalized(block_hash, block_number)) => {
			// TODO [now]
			false
		}
		FromOverseer::Signal(OverseerSignal::Conclude) => true,
		FromOverseer::Communication { msg } => match msg {
			ApprovalVotingMessage::CheckAndImportAssignment(a, res) => {
				let _ = res.send(check_and_import_assignment(ctx, state, a).await);
				false
			}
			ApprovalVotingMessage::CheckAndImportApproval(a, res) => {
				let _ = res.send(check_and_import_approval(ctx, state, a).await);
				false
			}
			ApprovalVotingMessage::ApprovedAncestor(_target, _lower_bound, _res ) => {
				// TODO [now]
				false
			}
		}
	}
}

async fn check_and_import_assignment(
	ctx: &mut impl SubsystemContext,
	state: &mut State,
	assignment: IndirectAssignmentCert,
) -> AssignmentCheckResult {
	// TODO [now]
	unimplemented!()
}

async fn check_and_import_approval(
	ctx: &mut impl SubsystemContext,
	state: &mut State,
	approval: IndirectSignedApprovalVote,
) -> ApprovalCheckResult {
	// TODO [now]
	unimplemented!()
}
