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
	},
	Subsystem, SubsystemContext, SubsystemError, SubsystemResult, SpawnedSubsystem,
	FromOverseer, OverseerSignal,
};
use polkadot_primitives::v1::{
	ValidatorIndex, Hash, SessionIndex, SessionInfo, CandidateEvent, Header, CandidateHash,
	CandidateReceipt, CoreIndex, GroupIndex, AvailableData, BlockNumber,
};
use polkadot_node_primitives::approval::{
	self as approval_types, IndirectAssignmentCert, IndirectSignedApprovalVote, DelayTranche,
	BlockApprovalMeta, RelayVRFStory, ApprovalVote,
};
use parity_scale_codec::Encode;
use sc_keystore::LocalKeystore;
use sp_consensus_slots::SlotNumber;
use sc_client_api::backend::AuxStore;
use sp_consensus_babe::Epoch as BabeEpoch;
use sp_runtime::traits::AppVerify;

use futures::prelude::*;
use futures::channel::{mpsc, oneshot};
use bitvec::vec::BitVec;
use bitvec::order::Lsb0 as BitOrderLsb0;

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};
use std::sync::Arc;

use aux_schema::{TrancheEntry, ApprovalEntry, CandidateEntry, BlockEntry};
use criteria::OurAssignment;

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
	let mut last_finalized_height = None;

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
				if handle_from_overseer(
					&mut ctx,
					&mut state,
					next_msg?,
					&mut last_finalized_height,
				).await? {
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
	last_finalized_height: &mut Option<BlockNumber>,
) -> SubsystemResult<bool> {
	match x {
		FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
			for (head, _span) in update.activated {
				if let Err(e) = handle_new_head(ctx, state, head, &*last_finalized_height).await {
					return Err(SubsystemError::with_origin("db", e));
				}
			}
			Ok(false)
		}
		FromOverseer::Signal(OverseerSignal::BlockFinalized(block_hash, block_number)) => {
			*last_finalized_height = Some(block_number);

			aux_schema::canonicalize(&*state.db, block_number, block_hash)
				.map(|_| false)
				.map_err(|e| SubsystemError::with_origin("db", e))
		}
		FromOverseer::Signal(OverseerSignal::Conclude) => Ok(true),
		FromOverseer::Communication { msg } => match msg {
			ApprovalVotingMessage::CheckAndImportAssignment(a, claimed_core, res) => {
				let _ = res.send(check_and_import_assignment(state, a, claimed_core)?);
				Ok(false)
			}
			ApprovalVotingMessage::CheckAndImportApproval(a, res) => {
				check_and_import_approval(state, a, res)?;
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
// This returns the entire ancestry up to the last finalized block's height or the last item we
// have in the DB. This may be somewhat expensive when first recovering from major sync.
//
// TODO [now]: improve error handling.
async fn determine_new_blocks(
	ctx: &mut impl SubsystemContext,
	db: &impl AuxStore,
	head: Hash,
	header: &Header,
	finalized_number: BlockNumber,
) -> SubsystemResult<Vec<(Hash, Header)>> {
	const ANCESTRY_STEP: usize = 4;

	// Early exit if the block is in the DB or too early.
	{
		let already_known = aux_schema::load_block_entry(db, &head)
			.map_err(|e| SubsystemError::with_origin("approval-voting", e))?
			.is_some();

		let before_relevant = header.number <= finalized_number;

		if already_known || before_relevant {
			return Ok(Vec::new());
		}
	}

	let mut ancestry = vec![(head, header.clone())];

	// Early exit if the parent hash is in the DB.
	if aux_schema::load_block_entry(db, &header.parent_hash)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?
		.is_some()
	{
		return Ok(ancestry);
	}

	loop {
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
			let is_known = aux_schema::load_block_entry(db, &hash)
				.map_err(|e| SubsystemError::with_origin("approval-voting", e))?
				.is_some();

			let is_relevant = header.number > finalized_number;

			if is_known || !is_relevant {
				break
			}

			ancestry.push((hash, header));
		}
	}

	ancestry.reverse();
	Ok(ancestry)
}

// When inspecting a new import notification, updates the session info cache to match
// the session of the imported block.
//
// this only needs to be called on heads where we are directly notified about import, as sessions do
// not change often and import notifications are expected to be typically increasing in session number.
//
// some backwards drift in session index is acceptable.
async fn cache_session_info_for_head(
	ctx: &mut impl SubsystemContext,
	state: &mut State<impl AuxStore>,
	block_hash: Hash,
	block_header: &Header,
) -> SubsystemResult<()> {
	let session_index = {
		let (s_tx, s_rx) = oneshot::channel();
		ctx.send_message(RuntimeApiMessage::Request(
			block_header.parent_hash,
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
					block_hash,
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

	Ok(())
}

struct ImportedBlockInfo {
	included_candidates: Vec<(CandidateHash, CandidateReceipt, CoreIndex, GroupIndex)>,
	session_index: SessionIndex,
	assignments: HashMap<CoreIndex, OurAssignment>,
	n_validators: usize,
	relay_vrf_story: RelayVRFStory,
	slot: SlotNumber,
}

// Computes information about the imported block. Returns `None` if the info couldn't be extracted -
// failure to communicate with overseer,
async fn imported_block_info(
	ctx: &mut impl SubsystemContext,
	state: &'_ State<impl AuxStore>,
	block_hash: Hash,
	block_header: &Header,
) -> SubsystemResult<Option<ImportedBlockInfo>> {
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
			Ok(Err(_)) => return Ok(None),
			Err(_) => return Ok(None),
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
			Ok(Err(_)) => return Ok(None),
			Err(_) => return Ok(None),
		};

		if session_index < state.earliest_session {
			tracing::debug!(target: LOG_TARGET, "Block {} is from ancient session {}. Skipping",
				block_hash, session_index);

			return Ok(None);
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
			Ok(Err(_)) => return Ok(None),
			Err(_) => return Ok(None),
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

			return Ok(None);
		}
	};

	let (assignments, slot, relay_vrf_story) = {
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
					Err(_) => return Ok(None),
				}
			}
			None => {
				tracing::debug!(
					target: LOG_TARGET,
					"BABE VRF info unavailable for block {}",
					block_hash,
				);

				return Ok(None);
			}
		}
	};

	Ok(Some(ImportedBlockInfo {
		included_candidates,
		session_index,
		assignments,
		n_validators: session_info.validators.len(),
		relay_vrf_story,
		slot,
	}))
}

async fn handle_new_head(
	ctx: &mut impl SubsystemContext,
	state: &mut State<impl AuxStore>,
	head: Hash,
	finalized_number: &Option<BlockNumber>,
) -> SubsystemResult<()> {
	// Update session info based on most recent head.
	let header: Header = unimplemented!();

	cache_session_info_for_head(ctx, state, head, &header).await?;

	// If we've just started the node and haven't yet received any finality notifications,
	// we don't do any look-back. Approval voting is only for nodes were already online.
	let finalized_number = finalized_number.unwrap_or(header.number.saturating_sub(1));

	let new_blocks = determine_new_blocks(ctx, &*state.db, head, &header, finalized_number)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))
		.await?;

	let mut approval_meta: Vec<BlockApprovalMeta> = Vec::with_capacity(new_blocks.len());

	// `determine_new_blocks` gives us a vec in backwards order. we want to move forwards.
	for (block_hash, block_header) in new_blocks.into_iter().rev() {
		let ImportedBlockInfo {
			included_candidates,
			session_index,
			assignments,
			n_validators,
			relay_vrf_story,
			slot,
		} = match imported_block_info(ctx, &*state, block_hash, &block_header).await? {
			Some(i) => i,
			None => continue,
		};

		aux_schema::add_block_entry(
			&*state.db,
			block_header.parent_hash,
			block_header.number,
			BlockEntry {
				block_hash: block_hash,
				session: session_index,
				slot,
				relay_vrf_story,
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

	ctx.send_message(ApprovalDistributionMessage::NewBlocks(approval_meta).into()).await;

	// TODO [now]: schedule wakeup for each imported block. May issue trigger of assignment and broadcast
	// of messages, so we need to have already notified distribution about the new blocks.

	Ok(())
}

fn approval_signing_payload(
	approval_vote: ApprovalVote,
	session_index: SessionIndex,
) -> Vec<u8> {
	(approval_vote, session_index).encode()
}

fn check_and_import_assignment(
	state: &mut State<impl AuxStore>,
	assignment: IndirectAssignmentCert,
	claimed_core_index: CoreIndex,
) -> SubsystemResult<AssignmentCheckResult> {
	const TOO_FAR_IN_FUTURE: SlotNumber = 5;

	let tick_now = tick_now();

	let block_entry = aux_schema::load_block_entry(&*state.db, &assignment.block_hash)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?;

	let block_entry = match block_entry {
		Some(b) => b,
		None => return Ok(AssignmentCheckResult::Bad),
	};

	let session_info = match state.session_info(block_entry.session) {
		Some(s) => s,
		None => {
			tracing::warn!(target: LOG_TARGET, "Unknown session info for {}", block_entry.session);
			return Ok(AssignmentCheckResult::Bad);
		}
	};

	let assigned_candidate_hash = match block_entry.candidates.iter()
		.find(|(c, _)| c == &claimed_core_index)
		.map(|(_, h)| *h)
	{
		Some(a) => a,
		None => return Ok(AssignmentCheckResult::Bad), // no candidate at core.
	};

	let res = crate::criteria::check_assignment_cert(
		claimed_core_index,
		assignment.validator,
		&session_info,
		block_entry.relay_vrf_story.clone(),
		&assignment.cert,
	);

	let tranche = match res {
		Err(crate::criteria::InvalidAssignment) => return Ok(AssignmentCheckResult::Bad),
		Ok(tranche) => {
			let tranche_now_of_prev_slot = tranche_now(
				state.slot_duration_millis,
				block_entry.slot.saturating_sub(TOO_FAR_IN_FUTURE),
			);

			if tranche >= tranche_now_of_prev_slot {
				return Ok(AssignmentCheckResult::TooFarInFuture);
			}

			tranche
		}
	};

	let mut candidate_entry = match aux_schema::load_candidate_entry(
		&*state.db,
		&assigned_candidate_hash,
	)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?
	{
		Some(c) => c,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Missing candidate entry {} referenced in live block {}",
				assigned_candidate_hash,
				assignment.block_hash,
			);

			return Ok(AssignmentCheckResult::Bad);
		}
	};

	let res = {
		// import the assignment.
		let mut assignment_entry = match
			candidate_entry.block_assignments.get_mut(&assignment.block_hash)
		{
			Some(a) => a,
			None => return Ok(AssignmentCheckResult::Bad),
		};

		// Check that the validator was not part of the backing group
		// and not already assigned.
		let is_in_backing = is_in_backing_group(
			&session_info.validator_groups,
			assignment.validator,
			assignment_entry.backing_group,
		);

		if is_in_backing {
			return Ok(AssignmentCheckResult::Bad);
		}

		let is_duplicate =  assignment_entry.is_assigned(assignment.validator);
		assignment_entry.import_assignment(tranche, assignment.validator, tick_now);

		if is_duplicate {
			AssignmentCheckResult::AcceptedDuplicate
		} else {
			AssignmentCheckResult::Accepted
		}
	};

	// update candidate entry on disk.
	aux_schema::write_candidate_entry(&*state.db, &assigned_candidate_hash, &candidate_entry)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?;

	// We check for approvals here because we may be late in seeing a block containing a
	// candidate for which we have already seen approvals by the same validator.
	//
	// For these candidates, we will receive the assignments potentially after a corresponding
	// approval, and so we must check for approval here.
	check_full_approvals(
		state,
		Some((assignment.block_hash, block_entry)),
		&mut candidate_entry,
		|h, _| h == &assignment.block_hash,
	)?;

	// TODO [now]: schedule wakeup

	Ok(res)
}

fn is_in_backing_group(
	validator_groups: &[Vec<ValidatorIndex>],
	validator: ValidatorIndex,
	group: GroupIndex,
) -> bool {
	validator_groups.get(group.0 as usize).map_or(false, |g| g.contains(&validator))
}

fn check_and_import_approval(
	state: &mut State<impl AuxStore>,
	approval: IndirectSignedApprovalVote,
	response: oneshot::Sender<ApprovalCheckResult>,
) -> SubsystemResult<()> {
	macro_rules! respond {
		($e: expr) => { {
			let _ = response.send($e);
			return Ok(());
		} }
	}

	let block_entry = aux_schema::load_block_entry(&*state.db, &approval.block_hash)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?;

	let block_entry = match block_entry {
		Some(b) => b,
		None => respond!(ApprovalCheckResult::Bad)
	};

	let session_info = match state.session_info(block_entry.session) {
		Some(s) => s,
		None => {
			tracing::warn!(target: LOG_TARGET, "Unknown session info for {}", block_entry.session);
			respond!(ApprovalCheckResult::Bad)
		}
	};

	let approved_candidate_hash = match block_entry.candidates.get(approval.candidate_index as usize) {
		Some((_, h)) => *h,
		None => respond!(ApprovalCheckResult::Bad)
	};

	let approval_payload = approval_signing_payload(
		ApprovalVote(approved_candidate_hash),
		block_entry.session,
	);

	let pubkey = match session_info.validators.get(approval.validator as usize) {
		Some(k) => k,
		None => respond!(ApprovalCheckResult::Bad)
	};

	let approval_sig_valid = approval.signature.verify(approval_payload.as_slice(), pubkey);

	if !approval_sig_valid {
		respond!(ApprovalCheckResult::Bad)
	}

	let candidate_entry = match aux_schema::load_candidate_entry(&*state.db, &approved_candidate_hash)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?
	{
		Some(c) => c,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Unknown candidate entry for {}",
				approved_candidate_hash,
			);

			respond!(ApprovalCheckResult::Bad)
		}
	};

	// importing the approval can be heavy as it may trigger acceptance for a series of blocks.
	let _ = response.send(ApprovalCheckResult::Accepted);

	import_checked_approval(
		state,
		Some((approval.block_hash, block_entry)),
		candidate_entry,
		approval.validator,
	)
}

fn import_checked_approval(
	state: &State<impl AuxStore>,
	already_loaded: Option<(Hash, BlockEntry)>,
	mut candidate_entry: CandidateEntry,
	validator: ValidatorIndex,
) -> SubsystemResult<()> {
	if candidate_entry.mark_approval(validator) {
		// already approved - nothing to do here.
		return Ok(());
	}

	// Check if this approval vote alters the approval state of any blocks.
	//
	// This may include blocks beyond the already loaded block.
	check_full_approvals(state, already_loaded, &mut candidate_entry, |_, a| a.is_assigned(validator))

	// TODO [now]: write to disk.
}

fn check_full_approvals(
	state: &State<impl AuxStore>,
	mut already_loaded: Option<(Hash, BlockEntry)>,
	candidate_entry: &mut CandidateEntry,
	filter: impl Fn(&Hash, &ApprovalEntry) -> bool,
) -> SubsystemResult<()> {
	// We only query this max once per hash.
	let db = &*state.db;
	let mut load_block_entry = move |block_hash| -> SubsystemResult<Option<BlockEntry>> {
		if already_loaded.as_ref().map_or(false, |(h, _)| h == block_hash) {
			Ok(already_loaded.take().map(|(_, c)| c))
		} else {
			aux_schema::load_block_entry(db, block_hash)
				.map_err(|e| SubsystemError::with_origin("approval-voting", e))
		}
	};

	let candidate_hash = candidate_entry.candidate.hash();

	let mut newly_approved = Vec::new();
	for (block_hash, approval_entry) in candidate_entry.block_assignments.iter()
		.filter(|(h, a)| !a.is_approved() && filter(h, a))
	{
		let mut block_entry = match load_block_entry(block_hash)? {
			None => {
				tracing::warn!(
					target: LOG_TARGET,
					"Missing block entry {} referenced by candidate {}",
					block_hash,
					candidate_entry.candidate.hash(),
				);
				continue
			}
			Some(b) => b,
		};

		let session_info = match state.session_info(block_entry.session) {
			Some(s) => s,
			None => {
				tracing::warn!(target: LOG_TARGET, "Unknown session info for {}", block_entry.session);
				continue
			}
		};

		let tranche_now = tranche_now(state.slot_duration_millis, block_entry.slot);

		let required_tranches = tranches_to_approve(
			approval_entry,
			&candidate_entry.approvals,
			tranche_now,
			slot_number_to_tick(state.slot_duration_millis, block_entry.slot),
			slot_number_to_tick(state.slot_duration_millis, session_info.no_show_slots as _),
			session_info.needed_approvals as _
		);

		let now_approved = check_approval(
			&block_entry,
			&candidate_entry,
			approval_entry,
			required_tranches,
		);

		if now_approved {
			newly_approved.push(*block_hash);
			block_entry.mark_approved_by_hash(&candidate_hash);

			aux_schema::write_block_entry(db, &block_entry)
				.map_err(|e| SubsystemError::with_origin("approval-voting", e))?;
		}
	}

	for b in &newly_approved {
		if let Some(a) = candidate_entry.block_assignments.get_mut(b) {
			a.mark_approved();
		}
	}

	Ok(())
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

fn tranches_to_approve(
	approval_entry: &ApprovalEntry,
	approvals: &BitVec<BitOrderLsb0, u8>,
	tranche_now: DelayTranche,
	block_tick: Tick,
	no_show_duration: Tick,
	needed_approvals: usize,
) -> RequiredTranches {
	// This function progresses through a series of states while looping over the tranches
	// that we are aware of. First, we perform an initial count of the number of assignments
	// until we reach the number of needed assignments for approval. As we progress, we count the
	// number of no-shows in each tranche.
	//
	// Then, if there are any no-shows, we proceed into a series of subsequent states for covering
	// no-shows.
	//
	// We cover each no-show by a non-empty tranche, keeping track of the amount of further
	// no-shows encountered along the way. Once all of the no-shows we were previously aware
	// of are covered, we then progress to cover the no-shows we encountered while covering those,
	// and so on.
	enum State {
		// (assignments, no-shows)
		InitialCount(usize, usize),
		// (assignments, covered no-shows, covering no-shows, uncovered no-shows),
		CoverNoShows(usize, usize, usize, usize),
	}

	impl State {
		fn output(
			&self,
			tranche: DelayTranche,
			needed_approvals: usize,
			n_validators: usize,
		) -> RequiredTranches {
			match *self {
				State::InitialCount(assignments, no_shows) =>
					if assignments >= needed_approvals && no_shows == 0 {
						RequiredTranches::Exact(tranche, 0)
					} else {
						// If we have no-shows pending before we have seen enough assignments,
						// this can happen. In this case we want assignments to broadcast based
						// on timer, so we treat it as though there are no uncovered no-shows.
						RequiredTranches::Pending(tranche)
					},
				State::CoverNoShows(total_assignments, covered, covering, uncovered) =>
					if covering == 0 && uncovered == 0 {
						RequiredTranches::Exact(tranche, covered)
					} else if total_assignments + covering + uncovered >= n_validators  {
						RequiredTranches::All
					} else {
						RequiredTranches::Pending(tranche + (covering + uncovered) as DelayTranche)
					},
			}
		}
	}

	let tick_now = tranche_now as Tick + block_tick;
	let n_validators = approval_entry.assignments.len();

	approval_entry.tranches.iter()
		.take_while(|t| t.tranche <= tranche_now)
		.scan(Some(State::InitialCount(0, 0)), |state, tranche| {
			let s = match state.take() {
				None => return None,
				Some(s) => s,
			};

			let n_assignments = tranche.assignments.len();

			// count no-shows. An assignment is a no-show if there is no corresponding approval vote
			// after a fixed duration.
			let no_shows = tranche.assignments.iter().filter(|(v_index, tick)| {
				tick + no_show_duration >= tick_now
					&& *approvals.get(*v_index as usize).unwrap_or(&true)
			}).count();

			*state = Some(match s {
				State::InitialCount(total_assignments, no_shows_so_far) => {
					let no_shows = no_shows + no_shows_so_far;
					let total_assignments = total_assignments + n_assignments;
					if total_assignments >= needed_approvals {
						if no_shows == 0 {
							// Note that this state will never be advanced
							// as we will return `RequiredTranches::Exact`.
							State::InitialCount(total_assignments, 0)
						} else {
							// We reached our desired assignment count, but had no-shows.
							// Begin covering them.
							State::CoverNoShows(total_assignments, 0, no_shows, 0)
						}
					} else {
						// Keep counting
						State::InitialCount(total_assignments, no_shows)
					}
				}
				State::CoverNoShows(total_assignments, covered, covering, uncovered) => {
					let uncovered = no_shows + uncovered;
					let total_assignments = total_assignments + n_assignments;

					if n_assignments == 0 {
						// no-shows are only covered by non-empty tranches.
						State::CoverNoShows(total_assignments, covered, covering, uncovered)
					} else if covering == 1 {
						// Progress onto another round of covering uncovered no-shows.
						// Note that if `uncovered` is 0, this state will never be advanced
						// as we will return `RequiredTranches::Exact`.
						State::CoverNoShows(total_assignments, covered + 1, uncovered, 0)
					} else {
						// we covered one no-show with a non-empty tranche. continue doing so.
						State::CoverNoShows(total_assignments, covered + 1, covering - 1, uncovered)
					}
				}
			});

			let output = s.output(tranche.tranche, needed_approvals, n_validators);
			match output {
				RequiredTranches::Exact(_, _) | RequiredTranches::All => {
					// Wipe the state clean so the next iteration of this closure will terminate
					// the iterator. This guarantees that we can call `last` further down to see
					// either a `Finished` or `Pending` result
					*state = None;
				}
				RequiredTranches::Pending(_) => {
					// Pending results are only interesting when they are the last result of the iterator
					// i.e. we never achieve a satisfactory level of assignment.
				}
			}

			Some(output)
		})
		.last()
		// The iterator is empty only when we are aware of no assignments up to the current tranche.
		// Any assignments up to now should be broadcast. Typically this will happen when
		// `tranche_now == 0`.
		.unwrap_or(RequiredTranches::Pending(tranche_now))
}

async fn process_wakeup(
	ctx: &mut impl SubsystemContext,
	state: &mut State<impl AuxStore>,
	relay_block: Hash,
	candidate_hash: CandidateHash,
) -> SubsystemResult<()> {
	let block_entry = aux_schema::load_block_entry(&*state.db, &relay_block)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?;

	let candidate_entry = aux_schema::load_candidate_entry(&*state.db, &candidate_hash)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?;

	// If either is not present, we have nothing to wakeup. Might have lost a race with finality
	let (mut block_entry, mut candidate_entry) = match (block_entry, candidate_entry) {
		(Some(b), Some(c)) => (b, c),
		_ => return Ok(()),
	};

	let session_info = match state.session_info(block_entry.session) {
		Some(i) => i,
		None => return Ok(()), // TODO [now]: log?
	};

	let should_broadcast = {
		let approval_entry = match candidate_entry.block_assignments.get(&relay_block) {
			Some(e) => e,
			None => return Ok(()),
		};

		let tranches_to_approve = tranches_to_approve(
			&approval_entry,
			&candidate_entry.approvals,
			tranche_now(state.slot_duration_millis, block_entry.slot),
			slot_number_to_tick(state.slot_duration_millis, block_entry.slot),
			slot_number_to_tick(state.slot_duration_millis, session_info.no_show_slots as _),
			session_info.needed_approvals as _,
		);

		match approval_entry.our_assignment {
			None => false,
			Some(ref assignment) if assignment.triggered() => false,
			Some(ref assignment) => {
				match tranches_to_approve {
					RequiredTranches::All => check_approval(
						&block_entry,
						&candidate_entry,
						&approval_entry,
						RequiredTranches::All,
					),
					RequiredTranches::Pending(max) => assignment.tranche() <= max,
					RequiredTranches::Exact(_, _) => {
						// indicates that no new assignments are needed at the moment.
						false
					}
				}
			}
		}
	};

	let maybe_cert = if should_broadcast {
		let maybe_cert = {
			let approval_entry = candidate_entry.block_assignments.get_mut(&relay_block)
				.expect("should_broadcast only true if this fetched earlier; qed");

			approval_entry.trigger_our_assignment(tick_now())
		};

		aux_schema::write_candidate_entry(&*state.db, &candidate_hash, &candidate_entry)
			.map_err(|e| SubsystemError::with_origin("approval-voting", e))?;

		maybe_cert
	} else {
		None
	};

	if let Some((cert, val_index)) = maybe_cert {
		let indirect_cert = IndirectAssignmentCert {
			block_hash: relay_block,
			validator: val_index,
			cert,
		};

		let index_in_candidate = block_entry.candidates.iter()
			.position(|(_, h)| &candidate_hash == h);

		if let Some(i) = index_in_candidate {
			// sanity: should always be present.
			ctx.send_message(
				ApprovalDistributionMessage::DistributeAssignment(indirect_cert, i as u32).into()
			).await;

			launch_approval(
				ctx,
				state.approval_vote_tx.clone(),
				block_entry.session,
				&candidate_entry.candidate,
				val_index,
				relay_block,
				i as u32,
			).await?;
		}
	}

	// TODO [now]: schedule a new wakeup.

	Ok(())
}

async fn launch_approval(
	ctx: &mut impl SubsystemContext,
	approval_vote_tx: mpsc::Sender<ApprovalVoteRequest>,
	session_index: SessionIndex,
	candidate: &CandidateReceipt,
	validator_index: ValidatorIndex,
	block_hash: Hash,
	candidate_index: u32,
) -> SubsystemResult<()> {
	let (_a_tx, a_rx) = oneshot::channel::<AvailableData>();
	let (code_tx, code_rx) = oneshot::channel();
	let (context_num_tx, context_num_rx) = oneshot::channel();

	// TODO [now]
	// ctx.send_message(
	// 	AvailabilityRecoveryMessage::RecoverAvailableData(candidate, session_index, a_tx).into()
	// ).await?;

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

	let background = async move {
		let available_data = match a_rx.await {
			Err(_) => return,
			Ok(a) => a,
		};

		let validation_code = match code_rx.await {
			Err(_) => return,
			Ok(Err(_)) => return,
			Ok(code) => code,
		};

		// TODO [now]: issue a `CandidateValidationMessage::ValidateFromExhaustive`.

		// TODO [now]: issue an `approval_vote_tx` message.
	};

	Ok(())
}
