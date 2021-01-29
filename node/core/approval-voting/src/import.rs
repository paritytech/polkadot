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

//! Block import logic for the approval voting subsystem.
//!
//! There are two major concerns when handling block import notifications.
//!   * Determining all new blocks.
//!   * Handling session changes
//!
//! When receiving a block import notification from the overseer, the
//! approval voting subsystem needs to account for the fact that there
//! may have been blocks missed by the notification. It needs to iterate
//! the ancestry of the block notification back to either the last finalized
//! block or a block that is already accounted for within the DB.
//!
//! We maintain a rolling window of session indices. This starts as empty

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

use crate::approval_db;
use crate::persisted_entries::{ApprovalEntry, CandidateEntry, BlockEntry};
use crate::criteria::{AssignmentCriteria, RealAssignmentCriteria, OurAssignment};
use crate::time::{slot_number_to_tick, Tick, Clock, ClockExt, SystemClock};

use super::{APPROVAL_SESSIONS, LOG_TARGET, State};

/// A rolling window of sessions.
#[derive(Default)]
pub struct RollingSessionWindow {
	pub earliest_session: Option<SessionIndex>,
	pub session_info: Vec<SessionInfo>,
}

impl RollingSessionWindow {
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
		let already_known = approval_db::v1::load_block_entry(db, &head)
			.map_err(|e| SubsystemError::with_origin("approval-voting", e))?
			.is_some();

		let before_relevant = header.number <= finalized_number;

		if already_known || before_relevant {
			return Ok(Vec::new());
		}
	}

	let mut ancestry = vec![(head, header.clone())];

	// Early exit if the parent hash is in the DB.
	if approval_db::v1::load_block_entry(db, &header.parent_hash)
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
			let is_known = approval_db::v1::load_block_entry(db, &hash)
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
	session_window: &mut RollingSessionWindow,
	db: &impl AuxStore,
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

	match session_window.earliest_session {
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
					session_window.earliest_session = Some(window_start);
					session_window.session_info = s;
				}
			}
		}
		Some(old_window_start) => {
			let latest = session_window.latest_session().expect("latest always exists if earliest does; qed");

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
					session_window.session_info.drain(..overlap_start as usize);
					session_window.session_info.extend(s);
					session_window.earliest_session = Some(window_start);
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
	state: &State<impl AuxStore>,
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

		if state.session_window.earliest_session.as_ref().map_or(true, |e| &session_index < e) {
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

	let session_info = match state.session_window.session_info(session_index) {
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
							&crate::criteria::Config::from(session_info),
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

/// Information about a block and imported candidates.
pub struct BlockImportedCandidates {
	pub block_hash: Hash,
	pub block_number: BlockNumber,
	pub block_tick: Tick,
	pub no_show_duration: Tick,
	pub imported_candidates: Vec<(CandidateHash, CandidateEntry)>,
}

/// Handle a new notification of a header. This will
///   * determine all blocks to import,
///   * extract candidate information from them
///   * update the rolling session window
///   * compute our assignments
///   * import the block and candidates to the approval DB
///   * and return information about all candidates imported under each block.
///
/// It is the responsibility of the caller to schedule wakeups for each block.
pub(crate) async fn handle_new_head(
	ctx: &mut impl SubsystemContext,
	state: &mut State<impl AuxStore>,
	head: Hash,
	finalized_number: &Option<BlockNumber>,
) -> SubsystemResult<Vec<BlockImportedCandidates>> {
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
				return Ok(Vec::new());
			}
			Ok(Some(h)) => h
		}
	};

	if let Err(SessionsUnavailable)
		= cache_session_info_for_head(
			ctx,
			&mut state.session_window,
			&*state.db,
			head,
			&header,
		).await?
	{
		tracing::warn!(
			target: LOG_TARGET,
			"Could not cache session info when processing head {:?}",
			head,
		);

		return Ok(Vec::new())
	}

	// If we've just started the node and haven't yet received any finality notifications,
	// we don't do any look-back. Approval voting is only for nodes were already online.
	let finalized_number = finalized_number.unwrap_or(header.number.saturating_sub(1));

	let new_blocks = determine_new_blocks(ctx, &*state.db, head, &header, finalized_number)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))
		.await?;

	let mut approval_meta: Vec<BlockApprovalMeta> = Vec::with_capacity(new_blocks.len());
	let mut imported_candidates = Vec::with_capacity(new_blocks.len());

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

		let candidate_entries = approval_db::v1::add_block_entry(
			&*state.db,
			block_header.parent_hash,
			block_header.number,
			approval_db::v1::BlockEntry {
				block_hash: block_hash,
				session: session_index,
				slot,
				relay_vrf_story: relay_vrf_story.0,
				candidates: included_candidates.iter()
					.map(|(hash, _, core, _)| (*core, *hash)).collect(),
				approved_bitfield: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
				children: Vec::new(),
			},
			n_validators,
			|candidate_hash| {
				included_candidates.iter().find(|(hash, _, _, _)| candidate_hash == hash)
					.map(|(_, receipt, core, backing_group)| approval_db::v1::NewCandidateInfo {
						candidate: receipt.clone(),
						backing_group: *backing_group,
						our_assignment: assignments.get(core).map(|a| a.clone().into()),
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
			let session_info = state.session_window.session_info(session_index)
				.expect("imported_block_info requires session to be available; qed");

			let block_tick = slot_number_to_tick(state.slot_duration_millis, slot);
			let no_show_duration = slot_number_to_tick(
				state.slot_duration_millis,
				Slot::from(u64::from(session_info.no_show_slots)),
			);

			(block_tick, no_show_duration)
		};

		imported_candidates.push(
			BlockImportedCandidates {
				block_hash,
				block_number: block_header.number,
				block_tick,
				no_show_duration,
				imported_candidates: candidate_entries
					.into_iter()
					.map(|(h, e)| (h, e.into()))
					.collect(),
			}
		);
	}

	ctx.send_message(ApprovalDistributionMessage::NewBlocks(approval_meta).into()).await;

	Ok(imported_candidates)
}
