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
	ValidatorIndex, Hash, SessionIndex, SessionInfo, CandidateEvent, Header, CandidateHash,
	CandidateReceipt, CoreIndex, GroupIndex, BlockNumber, PersistedValidationData,
	ValidationCode, CandidateDescriptor, PoV, ValidatorPair, ValidatorSignature, ValidatorId,
	CandidateIndex,
};
use polkadot_node_primitives::ValidationResult;
use polkadot_node_primitives::approval::{
	self as approval_types, IndirectAssignmentCert, IndirectSignedApprovalVote, DelayTranche,
	BlockApprovalMeta, RelayVRFStory, ApprovalVote,
};
use parity_scale_codec::Encode;
use sc_keystore::LocalKeystore;
use sp_consensus_slots::Slot;
use sc_client_api::backend::AuxStore;
use sp_runtime::traits::AppVerify;
use sp_application_crypto::Pair;

use futures::prelude::*;
use futures::channel::{mpsc, oneshot};
use bitvec::vec::BitVec;
use bitvec::order::Lsb0 as BitOrderLsb0;

use std::collections::{BTreeMap, HashMap};
use std::collections::btree_map::Entry;
use std::sync::Arc;
use std::ops::{RangeBounds, Bound as RangeBound};

use aux_schema::{ApprovalEntry, CandidateEntry, BlockEntry};
use criteria::{AssignmentCriteria, RealAssignmentCriteria, OurAssignment};
use time::{slot_number_to_tick,Tick, Clock, ClockExt, SystemClock};

mod aux_schema;
mod criteria;
mod time;

#[cfg(test)]
mod tests;

const APPROVAL_SESSIONS: SessionIndex = 6;
const LOG_TARGET: &str = "approval_voting";

/// The approval voting subsystem.
pub struct ApprovalVotingSubsystem<T> {
	keystore: LocalKeystore,
	slot_duration_millis: u64,
	db: Arc<T>,
}

impl<T, C> Subsystem<C> for ApprovalVotingSubsystem<T>
	where T: AuxStore + Send + Sync + 'static, C: SubsystemContext<Message = ApprovalVotingMessage> {
	fn start(self, ctx: C) -> SpawnedSubsystem {
		let future = run::<T, C>(
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
			if prev >= &tick { return }

			// we are replacing previous wakeup.
			if let Entry::Occupied(mut entry) = self.wakeups.entry(tick) {
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

	// drains all wakeups within the given range.
	// panics if the given range is empty.
	fn drain<'a, R: RangeBounds<Tick>>(&'a mut self, range: R)
		-> impl Iterator<Item = (Hash, CandidateHash)> + 'a
	{
		let reverse = &mut self.reverse_wakeups;

		// BTreeMap has no `drain` method :(
		let after = match range.end_bound() {
			RangeBound::Unbounded => BTreeMap::new(),
			RangeBound::Included(last) => self.wakeups.split_off(&(last + 1)),
			RangeBound::Excluded(last) => self.wakeups.split_off(&last),
		};
		let prev = std::mem::replace(&mut self.wakeups, after);
		prev.into_iter()
			.flat_map(|(_, wakeup)| wakeup)
			.inspect(move |&(ref b, ref c)| { let _ = reverse.remove(&(*b, *c)); })
	}
}

struct State<T> {
	earliest_session: Option<SessionIndex>,
	session_info: Vec<SessionInfo>,
	keystore: LocalKeystore,
	wakeups: Wakeups,
	slot_duration_millis: u64,
	db: Arc<T>,
	background_tx: mpsc::Sender<BackgroundRequest>,
	clock: Box<dyn Clock + Send + Sync>,
	assignment_criteria: Box<dyn AssignmentCriteria + Send + Sync>,
}

impl<T> State<T> {
	fn session_info(&self, index: SessionIndex) -> Option<&SessionInfo> {
		self.earliest_session.and_then(|earliest| {
			if index < earliest {
				None
			} else {
				self.session_info.get((index - earliest) as usize)
			}
		})

	}

	fn latest_session(&self) -> Option<SessionIndex> {
		self.earliest_session
			.map(|earliest| earliest + (self.session_info.len() as SessionIndex).saturating_sub(1))
	}
}

async fn run<T, C>(
	mut ctx: C,
	subsystem: ApprovalVotingSubsystem<T>,
	clock: Box<dyn Clock + Send + Sync>,
	assignment_criteria: Box<dyn AssignmentCriteria + Send + Sync>,
) -> SubsystemResult<()>
	where T: AuxStore + Send + Sync + 'static, C: SubsystemContext<Message = ApprovalVotingMessage>
{
	let (background_tx, background_rx) = mpsc::channel(64);
	let mut state: State<T> = State {
		earliest_session: None,
		session_info: Vec::new(),
		keystore: subsystem.keystore,
		wakeups: Default::default(),
		slot_duration_millis: subsystem.slot_duration_millis,
		db: subsystem.db,
		background_tx,
		clock,
		assignment_criteria,
	};

	let mut last_finalized_height = None;
	let mut background_rx = background_rx.fuse();

	if let Err(e) = aux_schema::clear(&*state.db) {
		tracing::warn!(target: LOG_TARGET, "Failed to clear DB: {:?}", e);
		return Err(SubsystemError::with_origin("db", e));
	}

	loop {
		let wait_til_next_tick = match state.wakeups.first() {
			None => future::Either::Left(future::pending()),
			Some(tick) => future::Either::Right(
				state.clock.wait(tick).map(move |()| tick)
			),
		};
		futures::pin_mut!(wait_til_next_tick);

		futures::select! {
			tick_wakeup = wait_til_next_tick.fuse() => {
				let woken = state.wakeups.drain(..=tick_wakeup).collect::<Vec<_>>();

				for (woken_block, woken_candidate) in woken {
					process_wakeup(
						&mut ctx,
						&mut state,
						woken_block,
						woken_candidate,
					).await?;
				}

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
			background_request = background_rx.next().fuse() => {
				if let Some(req) = background_request {
					handle_background_request(
						&mut ctx,
						&mut state,
						req,
					).await?
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
			ApprovalVotingMessage::ApprovedAncestor(target, lower_bound, res ) => {
				match handle_approved_ancestor(ctx, &*state.db, target, lower_bound).await {
					Ok(v) => {
						let _ = res.send(v);
						Ok(false)
					}
					Err(e) => {
						let _ = res.send(None);
						Err(e)
					}
				}
			}
		}
	}
}

async fn handle_background_request(
	ctx: &mut impl SubsystemContext,
	state: &mut State<impl AuxStore>,
	request: BackgroundRequest,
) -> SubsystemResult<()> {
	match request {
		BackgroundRequest::ApprovalVote(vote_request) => {
			issue_approval(ctx, state, vote_request).await?;
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
		}
	}

	Ok(())
}

async fn handle_approved_ancestor(
	ctx: &mut impl SubsystemContext,
	db: &impl AuxStore,
	target: Hash,
	lower_bound: BlockNumber,
) -> SubsystemResult<Option<Hash>> {
	let mut all_approved_max = None;

	let block_number = {
		let (tx, rx) = oneshot::channel();

		ctx.send_message(ChainApiMessage::BlockNumber(target, tx).into()).await;

		match rx.await? {
			Ok(Some(n)) => n,
			Ok(None) => return Ok(None),
			Err(_) => return Ok(None),
		}
	};

	if block_number <= lower_bound { return Ok(None) }

	// request ancestors up to but not including the lower bound,
	// as a vote on the lower bound is implied if we cannot find
	// anything else.
	let ancestry = if block_number > lower_bound + 1 {
		let (tx, rx) = oneshot::channel();

		ctx.send_message(ChainApiMessage::Ancestors {
			hash: target,
			k: (block_number - (lower_bound + 1)) as usize,
			response_channel: tx,
		}.into()).await;

		match rx.await? {
			Ok(a) => a,
			Err(_) => return Ok(None),
		}
	} else {
		Vec::new()
	};

	for block_hash in std::iter::once(target).chain(ancestry) {
		// Block entries should be present as the assumption is that
		// nothing here is finalized. If we encounter any missing block
		// entries we can fail.
		let entry = match aux_schema::load_block_entry(db, &block_hash)
			.map_err(|e| SubsystemError::with_origin("approval-voting", e))?
		{
			None => return Ok(None),
			Some(b) => b,
		};

		if entry.is_fully_approved() {
			if all_approved_max.is_none() {
				all_approved_max = Some(block_hash);
			}
		} else {
			all_approved_max = None;
		}
	}

	Ok(all_approved_max)
}

// Given a new chain-head hash, this determines the hashes of all new blocks we should track
// metadata for, given this head. The list will typically include the `head` hash provided unless
// that block is already known, in which case the list should be empty. This is guaranteed to be
// a subset of the ancestry of `head`, as well as `head`, starting from `head` and moving
// backwards.
//
// This returns the entire ancestry up to the last finalized block's height or the last item we
// have in the DB. This may be somewhat expensive when first recovering from major sync.
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
		ctx.send_message(ChainApiMessage::Ancestors {
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

async fn load_all_sessions(
	ctx: &mut impl SubsystemContext,
	block_hash: Hash,
	start: SessionIndex,
	end_inclusive: SessionIndex,
) -> SubsystemResult<Option<Vec<SessionInfo>>> {
	let mut v = Vec::new();
	for i in start..=end_inclusive {
		let (tx, rx)= oneshot::channel();
		ctx.send_message(RuntimeApiMessage::Request(
			block_hash,
			RuntimeApiRequest::SessionInfo(i, tx),
		).into()).await;

		let session_info = match rx.await {
			Ok(Ok(Some(s))) => s,
			Ok(Ok(None)) => return Ok(None),
			Ok(Err(e)) => return Err(SubsystemError::with_origin("approval-voting", e)),
			Err(e) => return Err(SubsystemError::with_origin("approval-voting", e)),
		};

		v.push(session_info);
	}

	Ok(Some(v))
}

// Sessions unavailable in state to cache.
struct SessionsUnavailable;

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
) -> SubsystemResult<Result<(), SessionsUnavailable>> {
	let session_index = {
		let (s_tx, s_rx) = oneshot::channel();
		ctx.send_message(RuntimeApiMessage::Request(
			block_header.parent_hash,
			RuntimeApiRequest::SessionIndexForChild(s_tx),
		).into()).await;

		match s_rx.await? {
			Ok(s) => s,
			Err(e) => return Err(SubsystemError::with_origin("approval-voting", e)),
		}
	};

	match state.earliest_session {
		None => {
			// First block processed on start-up.

			let window_start = session_index.saturating_sub(APPROVAL_SESSIONS - 1);

			tracing::info!(
				target: LOG_TARGET, "Loading approval window from session {}..={}",
				window_start, session_index,
			);

			match load_all_sessions(ctx, block_hash, window_start, session_index).await? {
				None => {
					tracing::warn!(
						target: LOG_TARGET,
						"Could not load sessions {}..={} from block {:?} in session {}",
						window_start, session_index, block_hash, session_index,
					);

					return Ok(Err(SessionsUnavailable));
				},
				Some(s) => {
					state.earliest_session = Some(window_start);
					state.session_info = s;
				}
			}
		}
		Some(old_window_start) => {
			let latest = state.latest_session().expect("latest always exists if earliest does; qed");

			// Either cached or ancient.
			if session_index <= latest { return Ok(Ok(())) }

			let old_window_end = latest;

			let window_start = session_index.saturating_sub(APPROVAL_SESSIONS - 1);
			tracing::info!(
				target: LOG_TARGET, "Moving approval window from session {}..={} to {}..={}",
				old_window_start, old_window_end,
				window_start, session_index,
			);

			// keep some of the old window, if applicable.
			let overlap_start = window_start - old_window_start;

			match load_all_sessions(ctx, block_hash, latest + 1, session_index).await? {
				None => {
					tracing::warn!(
						target: LOG_TARGET,
						"Could not load sessions {}..={} from block {:?} in session {}",
						latest + 1, session_index, block_hash, session_index,
					);

					return Ok(Err(SessionsUnavailable));
				}
				Some(s) => {
					state.session_info.drain(..overlap_start as usize);
					state.session_info.extend(s);
					state.earliest_session = Some(window_start);
				}
			}
		}
	}

	Ok(Ok(()))
}

struct ImportedBlockInfo {
	included_candidates: Vec<(CandidateHash, CandidateReceipt, CoreIndex, GroupIndex)>,
	session_index: SessionIndex,
	assignments: HashMap<CoreIndex, OurAssignment>,
	n_validators: usize,
	relay_vrf_story: RelayVRFStory,
	slot: Slot,
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

		if state.earliest_session.as_ref().map_or(true, |e| &session_index < e) {
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
				let slot = unsafe_vrf.slot();

				match unsafe_vrf.compute_randomness(
					&babe_epoch.authorities,
					&babe_epoch.randomness,
					babe_epoch.epoch_index,
				) {
					Ok(relay_vrf) => {
						let assignments = state.assignment_criteria.compute_assignments(
							&state.keystore,
							relay_vrf.clone(),
							&criteria::Config::from(session_info),
							included_candidates.iter()
								.map(|(_, _, core, group)| (*core, *group))
								.collect(),
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
	let header = {
		let (h_tx, h_rx) = oneshot::channel();
		ctx.send_message(ChainApiMessage::BlockHeader(head, h_tx).into()).await;

		match h_rx.await? {
			Err(e) => {
				return Err(SubsystemError::with_origin("approval-voting", e));
			}
			Ok(None) => {
				tracing::warn!(target: LOG_TARGET, "Missing header for new head {}", head);
				return Ok(());
			}
			Ok(Some(h)) => h
		}
	};

	if let Err(SessionsUnavailable)
		= cache_session_info_for_head(ctx, state, head, &header).await?
	{
		tracing::warn!(
			target: LOG_TARGET,
			"Could not cache session info when processing head {:?}",
			head,
		);

		return Ok(())
	}

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

		let candidate_entries = aux_schema::add_block_entry(
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
			slot,
		});

		let (block_tick, no_show_duration) = {
			let session_info = state.session_info(session_index)
				.expect("imported_block_info requires session to be available; qed");

			let block_tick = slot_number_to_tick(state.slot_duration_millis, slot);
			let no_show_duration = slot_number_to_tick(
				state.slot_duration_millis,
				Slot::from(u64::from(session_info.no_show_slots)),
			);

			(block_tick, no_show_duration)
		};

		// schedule a wakeup for each candidate in the block.

		for (candidate_hash, candidate_entry) in candidate_entries {
			schedule_wakeup(
				state,
				&candidate_entry,
				block_hash,
				block_tick,
				no_show_duration,
				candidate_hash,
			);
		}
	}

	ctx.send_message(ApprovalDistributionMessage::NewBlocks(approval_meta).into()).await;

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
	candidate_index: CandidateIndex,
) -> SubsystemResult<AssignmentCheckResult> {
	const SLOT_TOO_FAR_IN_FUTURE: u64 = 5;

	let tick_now = state.clock.tick_now();

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

	let (claimed_core_index, assigned_candidate_hash)
		= match block_entry.candidates.get(candidate_index as usize)
	{
		Some((c, h)) => (*c, *h),
		None => return Ok(AssignmentCheckResult::Bad), // no candidate at core.
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
		let assignment_entry = match
			candidate_entry.block_assignments.get_mut(&assignment.block_hash)
		{
			Some(a) => a,
			None => return Ok(AssignmentCheckResult::Bad),
		};


		let res = state.assignment_criteria.check_assignment_cert(
			claimed_core_index,
			assignment.validator,
			&criteria::Config::from(session_info),
			block_entry.relay_vrf_story.clone(),
			&assignment.cert,
			assignment_entry.backing_group,
		);

		let tranche = match res {
			Err(crate::criteria::InvalidAssignment) => return Ok(AssignmentCheckResult::Bad),
			Ok(tranche) => {
				let tranche_now_of_prev_slot = state.clock.tranche_now(
					state.slot_duration_millis,
					block_entry.slot.saturating_sub(SLOT_TOO_FAR_IN_FUTURE),
				);

				if tranche >= tranche_now_of_prev_slot {
					return Ok(AssignmentCheckResult::TooFarInFuture);
				}

				tranche
			}
		};

		let is_duplicate =  assignment_entry.is_assigned(assignment.validator);
		assignment_entry.import_assignment(tranche, assignment.validator, tick_now);

		if is_duplicate {
			AssignmentCheckResult::AcceptedDuplicate
		} else {
			AssignmentCheckResult::Accepted
		}
	};

	let (block_tick, no_show_duration) = {
		let block_tick = slot_number_to_tick(state.slot_duration_millis, block_entry.slot);
		let no_show_duration = slot_number_to_tick(
			state.slot_duration_millis,
			Slot::from(u64::from(session_info.no_show_slots)),
		);

		(block_tick, no_show_duration)
	};

	// We check for approvals here because we may be late in seeing a block containing a
	// candidate for which we have already seen approvals by the same validator.
	//
	// For these candidates, we will receive the assignments potentially after a corresponding
	// approval, and so we must check for approval here.
	//
	// Note that this writes the candidate entry and any modified block entries to disk.
	let candidate_entry = check_full_approvals(
		state,
		Some((assignment.block_hash, block_entry)),
		candidate_entry,
		|h, _| h == &assignment.block_hash,
	)?;

	schedule_wakeup(
		state,
		&candidate_entry,
		assignment.block_hash,
		block_tick,
		no_show_duration,
		assigned_candidate_hash,
	);

	Ok(res)
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
	check_full_approvals(state, already_loaded, candidate_entry, |_, a| a.is_assigned(validator))?;
	Ok(())
}

// Checks the candidate for approval under all blocks matching the given filter.
//
// If returning without error, is guaranteed to have written all modified block entries and the
// candidate entry itself.
fn check_full_approvals(
	state: &State<impl AuxStore>,
	mut already_loaded: Option<(Hash, BlockEntry)>,
	mut candidate_entry: CandidateEntry,
	filter: impl Fn(&Hash, &ApprovalEntry) -> bool,
) -> SubsystemResult<CandidateEntry> {
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

	let mut transaction = aux_schema::Transaction::default();

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

		let tranche_now = state.clock.tranche_now(state.slot_duration_millis, block_entry.slot);

		let required_tranches = tranches_to_approve(
			approval_entry,
			&candidate_entry.approvals,
			tranche_now,
			slot_number_to_tick(state.slot_duration_millis, block_entry.slot),
			slot_number_to_tick(state.slot_duration_millis, Slot::from(u64::from(session_info.no_show_slots))),
			session_info.needed_approvals as _
		);

		let now_approved = check_approval(
			&candidate_entry,
			approval_entry,
			required_tranches,
		);

		if now_approved {
			newly_approved.push(*block_hash);
			block_entry.mark_approved_by_hash(&candidate_hash);

			transaction.put_block_entry(block_entry);
		}
	}

	for b in &newly_approved {
		if let Some(a) = candidate_entry.block_assignments.get_mut(b) {
			a.mark_approved();
		}
	}

	transaction.put_candidate_entry(candidate_hash, candidate_entry.clone());
	transaction.write(db)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?;

	Ok(candidate_entry)
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

fn schedule_wakeup(
	state: &mut State<impl AuxStore>,
	candidate_entry: &CandidateEntry,
	block_hash: Hash,
	block_tick: Tick,
	no_show_duration: Tick,
	candidate_hash: CandidateHash,
) {
	if let Some(tick) = candidate_entry.next_wakeup(&block_hash, block_tick, no_show_duration) {
		state.wakeups.schedule(block_hash, candidate_hash, tick);
	}
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
	let (block_entry, mut candidate_entry) = match (block_entry, candidate_entry) {
		(Some(b), Some(c)) => (b, c),
		_ => return Ok(()),
	};

	let session_info = match state.session_info(block_entry.session) {
		Some(i) => i,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Missing session info for live block {} in session {}",
				relay_block,
				block_entry.session,
			);

			return Ok(())
		}
	};

	let block_tick = slot_number_to_tick(state.slot_duration_millis, block_entry.slot);
	let no_show_duration = slot_number_to_tick(
		state.slot_duration_millis,
		Slot::from(u64::from(session_info.no_show_slots)),
	);

	let should_broadcast = {
		let approval_entry = match candidate_entry.block_assignments.get(&relay_block) {
			Some(e) => e,
			None => return Ok(()),
		};

		let tranches_to_approve = tranches_to_approve(
			&approval_entry,
			&candidate_entry.approvals,
			state.clock.tranche_now(state.slot_duration_millis, block_entry.slot),
			block_tick,
			no_show_duration,
			session_info.needed_approvals as _,
		);

		match approval_entry.our_assignment {
			None => false,
			Some(ref assignment) if assignment.triggered() => false,
			Some(ref assignment) => {
				match tranches_to_approve {
					RequiredTranches::All => check_approval(
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

			approval_entry.trigger_our_assignment(state.clock.tick_now())
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
			ctx.send_message(ApprovalDistributionMessage::DistributeAssignment(
				indirect_cert,
				i as CandidateIndex,
			).into()).await;

			launch_approval(
				ctx,
				state.background_tx.clone(),
				block_entry.session,
				&candidate_entry.candidate,
				val_index,
				relay_block,
				i,
			).await?;
		}
	}

	schedule_wakeup(
		state,
		&candidate_entry,
		relay_block,
		block_tick,
		no_show_duration,
		candidate_hash,
	);

	Ok(())
}

async fn launch_approval(
	ctx: &mut impl SubsystemContext,
	mut background_tx: mpsc::Sender<BackgroundRequest>,
	session_index: SessionIndex,
	candidate: &CandidateReceipt,
	validator_index: ValidatorIndex,
	block_hash: Hash,
	candidate_index: usize,
) -> SubsystemResult<()> {
	let (a_tx, a_rx) = oneshot::channel();
	let (code_tx, code_rx) = oneshot::channel();
	let (context_num_tx, context_num_rx) = oneshot::channel();

	ctx.send_message(AvailabilityRecoveryMessage::RecoverAvailableData(
		candidate.clone(),
		session_index,
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
				// do nothing. we'll just be a no-show and that'll cause others to rise up.
				return;
			}
			Ok(Err(RecoveryError::Invalid)) => {
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

				let _ = background_tx.send(BackgroundRequest::ApprovalVote(ApprovalVoteRequest {
					validator_index,
					block_hash,
					candidate_index,
				})).await;
			}
			Ok(Ok(ValidationResult::Invalid(_))) => {
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
	state: &mut State<impl AuxStore>,
	request: ApprovalVoteRequest,
) -> SubsystemResult<()> {
	let ApprovalVoteRequest { validator_index, block_hash, candidate_index } = request;

	let block_entry = match aux_schema::load_block_entry(&*state.db, &block_hash)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?
	{
		Some(b) => b,
		None => return Ok(()), // not a cause for alarm - just lost a race with pruning, most likely.
	};

	let session_info = match state.session_info(block_entry.session) {
		Some(s) => s,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Missing session info for live block {} in session {}",
				block_hash,
				block_entry.session,
			);

			return Ok(());
		}
	};

	let candidate_hash = match block_entry.candidates.get(candidate_index) {
		Some((_, h)) => h,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Received malformed request to approve out-of-bounds candidate index {} included at block {:?}",
				candidate_index,
				block_hash,
			);

			return Ok(());
		}
	};

	let candidate_entry = match aux_schema::load_candidate_entry(&*state.db, &candidate_hash)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))?
	{
		Some(c) => c,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Missing entry for candidate index {} included at block {:?}",
				candidate_index,
				block_hash,
			);

			return Ok(());
		}
	};

	let validator_pubkey = match session_info.validators.get(validator_index as usize) {
		Some(p) => p,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Validator index {} out of bounds in session {}",
				validator_index,
				block_entry.session,
			);

			return Ok(());
		}
	};

	let sig = match sign_approval(
		&state.keystore,
		&validator_pubkey,
		*candidate_hash,
		block_entry.session,
	) {
		Some(sig) => sig,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Could not issue approval signature with validator index {} in session {}. Assignment key present but not validator key?",
				validator_index,
				block_entry.session,
			);

			return Ok(());
		}
	};

	import_checked_approval(
		state,
		Some((block_hash, block_entry)),
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

	Ok(())
}

// Sign an approval vote. Fails if the key isn't present in the store.
fn sign_approval(
	keystore: &LocalKeystore,
	public: &ValidatorId,
	candidate_hash: CandidateHash,
	session_index: SessionIndex,
) -> Option<ValidatorSignature> {
	let key = keystore.key_pair::<ValidatorPair>(public).ok()?;

	let payload = approval_signing_payload(
		ApprovalVote(candidate_hash),
		session_index,
	);

	Some(key.sign(&payload[..]))
}
