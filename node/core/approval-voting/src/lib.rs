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
		RuntimeApiMessage, RuntimeApiRequest, ChainApiMessage,
	},
	Subsystem, SubsystemContext, SubsystemError, SubsystemResult, SpawnedSubsystem,
	FromOverseer, OverseerSignal,
};
use polkadot_primitives::v1::{
	ValidatorIndex, Hash, SessionIndex, SessionInfo, CandidateEvent, Header
};
use polkadot_node_primitives::approval::{
	self as approval_types, IndirectAssignmentCert, IndirectSignedApprovalVote, DelayTranche,
	BlockApprovalMeta,
};
use sc_keystore::LocalKeystore;
use sp_consensus_slots::SlotNumber;
use sc_client_api::backend::AuxStore;
use sp_consensus_babe::Epoch as BabeEpoch;

use futures::prelude::*;
use futures::channel::{mpsc, oneshot};
use bitvec::order::Lsb0 as BitOrderLsb0;

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};
use std::sync::Arc;

use aux_schema::{TrancheEntry, ApprovalEntry, CandidateEntry, BlockEntry};

mod aux_schema;
mod criteria;

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

struct State<T> {
	earliest_session: SessionIndex,
	session_info: Vec<SessionInfo>,
	keystore: LocalKeystore,
	// Tick -> [(Relay Block, Candidate Hash)]
	wakeups: BTreeMap<Tick, Vec<(Hash, Hash)>>,
	slot_duration_millis: u64,
	db: Arc<T>,

	// These are connected to each other.
	approval_vote_tx: mpsc::Sender<ApprovalVoteRequest>,
}

impl<T> State<T> {
	fn session_info(&self, index: SessionIndex) -> Option<&SessionInfo> {
		if index < self.earliest_session {
			None
		} else {
			self.session_info.get((index - self.earliest_session) as usize)
		}
	}

	fn latest_session(&self) -> SessionIndex {
		self.earliest_session + (self.session_info.len() as SessionIndex).saturating_sub(1)
	}
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

fn tranche_now(slot_duration_millis: u64, base_slot: SlotNumber) -> DelayTranche {
	tick_now().saturating_sub(slot_number_to_tick(slot_duration_millis, base_slot)) as u32
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
	// TODO [now]
	let approval_vote_rx: mpsc::Receiver<ApprovalVoteRequest> = unimplemented!();
	let mut approval_vote_rx = approval_vote_rx.fuse();
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
			approval = approval_vote_rx.next().fuse() => {
				// TODO [now]
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
			for (head, _span) in update.activated {
				if let Err(e) = handle_new_head(ctx, state, head).await {
					return Err(SubsystemError::with_origin("db", e));
				}
			}
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

// Given a new chain-head hash, this determines the hashes of all new blocks we should track
// metadata for, given this head. The list will typically include the `head` hash provided unless
// that block is already known, in which case the list should be empty. This is guaranteed to be
// a subset of the ancestry of `head`, as well as `head`, starting from `head` and moving
// backwards.
//
// This won't return the entire ancestry of the head in the case of a fresh DB.
// TODO [now]: improve error handling.
async fn determine_new_blocks(
	ctx: &mut impl SubsystemContext,
	db: &impl AuxStore,
	head: Hash,
	header: &Header,
) -> SubsystemResult<Vec<(Hash, Header)>> {
	const MAX_ANCESTRY: usize = 64;
	const ANCESTRY_STEP: usize = 4;

	let mut ancestry = vec![(head, header.clone())];

	// Early exit if the parent hash is in the DB.
	if aux_schema::load_block_entry(db, &header.parent_hash)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?
		.is_some()
	{
		return Ok(ancestry);
	}

	while ancestry.len() < MAX_ANCESTRY {
		let &(ref last_hash, ref last_header) = ancestry.last()
			.expect("ancestry has length 1 at initialization and is only added to; qed");

		// If we iterated back to genesis, which can happen at the beginning of chains.
		if last_header.number <= 1 {
			break
		}

		let (tx, rx) = oneshot::channel();
		let ancestors = ctx.send_message(ChainApiMessage::Ancestors {
			hash: *last_hash,
			k: ANCESTRY_STEP,
			response_channel: tx,
		}.into()).await;

		// Continue past these errors.
		let batch_hashes = match rx.await {
			Err(_) | Ok(Err(_)) => break,
			Ok(Ok(ancestors)) => ancestors,
		};

		let batch_headers = {
			let (batch_senders, batch_receivers) = (0..batch_hashes.len())
				.map(|_| oneshot::channel())
				.unzip::<_, _, Vec<_>, Vec<_>>();

			for (hash, sender) in batch_hashes.iter().cloned().zip(batch_senders) {
				ctx.send_message(ChainApiMessage::BlockHeader(hash, sender).into()).await;
			}

			let mut requests = futures::stream::FuturesOrdered::new();
			batch_receivers.into_iter().map(|rx| async move {
				match rx.await {
					Err(_) | Ok(Err(_)) => None,
					Ok(Ok(h)) => h,
				}
			})
				.for_each(|x| requests.push(x));

			let batch_headers: Vec<_> = requests
				.flat_map(|x: Option<Header>| stream::iter(x))
				.collect()
				.await;

			// Any failed header fetch of the batch will yield a `None` result that will
			// be skipped. Any failure at this stage means we'll just ignore those blocks
			// as the chain DB has failed us.
			if batch_headers.len() != batch_hashes.len() { break }
			batch_headers
		};

		for (hash, header) in batch_hashes.into_iter().zip(batch_headers) {
			if aux_schema::load_block_entry(db, &hash)
				.map_err(|e| SubsystemError::with_origin("approval-voting", e))?
				.is_some()
			{
				break
			}

			ancestry.push((hash, header));
		}
	}

	ancestry.reverse();
	Ok(ancestry)
}

async fn handle_new_head(
	ctx: &mut impl SubsystemContext,
	state: &mut State<impl AuxStore>,
	head: Hash,
) -> SubsystemResult<()> {
	// Update session info based on most recent head.
	let header: Header = unimplemented!();

	{

		let session_index = {
			let (s_tx, s_rx) = oneshot::channel();
			ctx.send_message(RuntimeApiMessage::Request(
				header.parent_hash,
				RuntimeApiRequest::SessionIndexForChild(s_tx),
			).into()).await;

			match s_rx.await {
				Ok(Ok(s)) => s,
				Ok(Err(_)) => return Ok(()),
				Err(_) => return Ok(()),
			}
		};

		if session_index >= state.earliest_session {
			// Update the window of sessions.
			if session_index > state.latest_session() {
				let window_start = session_index.saturating_sub(APPROVAL_SESSIONS - 1);
				let old_window_end = state.latest_session();
				tracing::info!(
					target: LOG_TARGET, "Moving approval window from session {}..={} to {}..={}",
					state.earliest_session, old_window_end,
					window_start, session_index,
				);

				// keep some of the old window, if applicable.
				let old_window_start = std::mem::replace(&mut state.earliest_session, window_start);
				let overlap_start = session_index - old_window_end;
				state.session_info.drain(..overlap_start as usize);

				// load the end of the window.
				for i in state.session_info.len() as SessionIndex + window_start ..= session_index {
					let (tx, rx)= oneshot::channel();
					ctx.send_message(RuntimeApiMessage::Request(
						head,
						RuntimeApiRequest::SessionInfo(i, tx),
					).into()).await;

					let session_info = match rx.await {
						Ok(Ok(Some(s))) => s,
						Ok(Ok(None)) => unimplemented!(), // indicates a runtime error.
						Ok(Err(_)) => unimplemented!(), // TODO [now]: what to do if unavailable?
						Err(_) => unimplemented!(),
					};

					state.session_info.push(session_info);
				}
			}
		}
	}

	let new_blocks = determine_new_blocks(ctx, &*state.db, head, &header)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))
		.await?;

	let mut approval_meta: Vec<BlockApprovalMeta> = Vec::with_capacity(new_blocks.len());

	// `determine_new_blocks` gives us a vec in backwards order. we want to move forwards.
	for (block_hash, block_header) in new_blocks.into_iter().rev() {
		// Ignore any runtime API errors - that means these blocks are old and finalized.
		// Only unfinalized blocks factor into the approval voting process.

		// fetch candidates
		let included_candidates: Vec<_> = {
			let (c_tx, c_rx) = oneshot::channel();
			ctx.send_message(RuntimeApiMessage::Request(
				block_hash,
				RuntimeApiRequest::CandidateEvents(c_tx),
			).into()).await;

			let events: Vec<CandidateEvent> = match c_rx.await {
				Ok(Ok(events)) => events,
				Ok(Err(_)) => continue,
				Err(_) => continue,
			};

			events.into_iter().filter_map(|e| match e {
				CandidateEvent::CandidateIncluded(receipt, _, core, group)
					=> Some((receipt.hash(), receipt, core, group)),
				_ => None,
			}).collect()
		};

		// fetch session. ignore blocks that are too old, but unless sessions are really
		// short, that shouldn't happen.
		let session_index = {
			let (s_tx, s_rx) = oneshot::channel();
			ctx.send_message(RuntimeApiMessage::Request(
				block_header.parent_hash,
				RuntimeApiRequest::SessionIndexForChild(s_tx),
			).into()).await;

			let session_index = match s_rx.await {
				Ok(Ok(s)) => s,
				Ok(Err(_)) => continue,
				Err(_) => continue,
			};

			if session_index < state.earliest_session {
				tracing::debug!(target: LOG_TARGET, "Block {} is from ancient session {}. Skipping",
					block_hash, session_index);

				continue;
			}

			session_index
		};

		let babe_epoch = {
			let (s_tx, s_rx) = oneshot::channel();
			ctx.send_message(RuntimeApiMessage::Request(
				block_hash,
				RuntimeApiRequest::CurrentBabeEpoch(s_tx),
			).into()).await;

			match s_rx.await {
				Ok(Ok(s)) => s,
				Ok(Err(_)) => continue,
				Err(_) => continue,
			}
		};

		let session_info = match state.session_info(session_index) {
			Some(s) => s,
			None => {
				tracing::debug!(
					target: LOG_TARGET,
					"Session info unavailable for block {}",
					block_hash,
				);

				continue;
			}
		};

		let (assignments, slot, relay_vrf) = {
			let unsafe_vrf = approval_types::babe_unsafe_vrf_info(&block_header);

			match unsafe_vrf {
				Some(unsafe_vrf) => {
					let slot = unsafe_vrf.slot_number();

					match unsafe_vrf.compute_randomness(
						&babe_epoch.authorities,
						&babe_epoch.randomness,
						babe_epoch.epoch_index,
					) {
						Ok(relay_vrf) => {
							let assignments = criteria::compute_assignments(
								&state.keystore,
								relay_vrf.clone(),
								session_info,
								included_candidates.iter().map(|(_, _, core, _)| *core),
							);

							(assignments, slot, relay_vrf)
						},
						Err(_) => continue,
					}
				}
				_None=> {
					tracing::debug!(
						target: LOG_TARGET,
						"BABE VRF info unavailable for block {}",
						block_hash,
					);

					continue;
				}
			}
		};

		let n_validators = session_info.validators.len();
		aux_schema::add_block_entry(
			&*state.db,
			block_header.parent_hash,
			block_header.number,
			BlockEntry {
				block_hash: block_hash,
				session: session_index,
				slot,
				relay_vrf_story: relay_vrf,
				candidates: included_candidates.iter()
					.map(|(hash, _, core, _)| (*core, *hash)).collect(),
				approved_bitfield: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
				children: Vec::new(),
			},
			n_validators,
			|candidate_hash| {
				included_candidates.iter().find(|(hash, _, _, _)| candidate_hash == hash)
					.map(|(_, receipt, core, backing_group)| aux_schema::NewCandidateInfo {
						candidate: receipt.clone(),
						backing_group: *backing_group,
						our_assignment: assignments.get(core).map(|a| a.clone()),
					})
			}
		).map_err(|e| SubsystemError::with_origin("approval-voting", e))?;

		approval_meta.push(BlockApprovalMeta {
			hash: block_hash,
			number: block_header.number,
			parent_hash: block_header.parent_hash,
			candidates: included_candidates.iter().map(|(hash, _, _, _)| *hash).collect(),
			slot_number: slot,
		});
	}

	// TODO [now]: send block approval meta to approval distribution.

	// TODO [now]
	unimplemented!()
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
