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
		RuntimeApiMessage, RuntimeApiRequest, ChainApiMessage, ApprovalDistributionMessage,
	},
	SubsystemContext, SubsystemError, SubsystemResult,
};
use polkadot_primitives::v1::{
	Hash, SessionIndex, SessionInfo, CandidateEvent, Header, CandidateHash,
	CandidateReceipt, CoreIndex, GroupIndex, BlockNumber,
};
use polkadot_node_primitives::approval::{
	self as approval_types, BlockApprovalMeta, RelayVRFStory,
};
use sc_keystore::LocalKeystore;
use sp_consensus_slots::Slot;
use kvdb::KeyValueDB;

use futures::prelude::*;
use futures::channel::oneshot;
use bitvec::order::Lsb0 as BitOrderLsb0;

use std::collections::HashMap;
use std::convert::TryFrom;

use crate::approval_db;
use crate::persisted_entries::CandidateEntry;
use crate::criteria::{AssignmentCriteria, OurAssignment};
use crate::time::{slot_number_to_tick, Tick};

use super::{APPROVAL_SESSIONS, LOG_TARGET, State, DBReader};

/// A rolling window of sessions.
#[derive(Default)]
pub struct RollingSessionWindow {
	pub earliest_session: Option<SessionIndex>,
	pub session_info: Vec<SessionInfo>,
}

impl RollingSessionWindow {
	pub fn session_info(&self, index: SessionIndex) -> Option<&SessionInfo> {
		self.earliest_session.and_then(|earliest| {
			if index < earliest {
				None
			} else {
				self.session_info.get((index - earliest) as usize)
			}
		})

	}

	pub fn latest_session(&self) -> Option<SessionIndex> {
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
	db: &impl DBReader,
	head: Hash,
	header: &Header,
	finalized_number: BlockNumber,
) -> SubsystemResult<Vec<(Hash, Header)>> {
	const ANCESTRY_STEP: usize = 4;

	// Early exit if the block is in the DB or too early.
	{
		let already_known = db.load_block_entry(&head)?
			.is_some();

		let before_relevant = header.number <= finalized_number;

		if already_known || before_relevant {
			return Ok(Vec::new());
		}
	}

	let mut ancestry = vec![(head, header.clone())];

	// Early exit if the parent hash is in the DB.
	if db.load_block_entry(&header.parent_hash)?
		.is_some()
	{
		return Ok(ancestry);
	}

	'outer: loop {
		let &(ref last_hash, ref last_header) = ancestry.last()
			.expect("ancestry has length 1 at initialization and is only added to; qed");

		// If we iterated back to genesis, which can happen at the beginning of chains.
		if last_header.number <= 1 {
			break 'outer
		}

		let (tx, rx) = oneshot::channel();
		ctx.send_message(ChainApiMessage::Ancestors {
			hash: *last_hash,
			k: ANCESTRY_STEP,
			response_channel: tx,
		}.into()).await;

		// Continue past these errors.
		let batch_hashes = match rx.await {
			Err(_) | Ok(Err(_)) => break 'outer,
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
			if batch_headers.len() != batch_hashes.len() { break 'outer }
			batch_headers
		};

		for (hash, header) in batch_hashes.into_iter().zip(batch_headers) {
			let is_known = db.load_block_entry(&hash)?.is_some();

			let is_relevant = header.number > finalized_number;

			if is_known || !is_relevant {
				break 'outer
			}

			ancestry.push((hash, header));
		}
	}

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
			Ok(Ok(None)) => {
				tracing::warn!(
					target: LOG_TARGET,
					"Session {} is missing from session-info state of block {}",
					i,
					block_hash,
				);

				return Ok(None);
			}
			Ok(Err(e)) => return Err(SubsystemError::with_origin("approval-voting", e)),
			Err(e) => return Err(SubsystemError::with_origin("approval-voting", e)),
		};

		v.push(session_info);
	}

	Ok(Some(v))
}

// Sessions unavailable in state to cache.
#[derive(Debug)]
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
	block_hash: Hash,
	block_header: &Header,
) -> SubsystemResult<Result<(), SessionsUnavailable>> {
	let session_index = {
		let (s_tx, s_rx) = oneshot::channel();

		// The genesis is guaranteed to be at the beginning of the session and its parent state
		// is non-existent. Therefore if we're at the genesis, we request using its state and
		// not the parent.
		ctx.send_message(RuntimeApiMessage::Request(
			if block_header.number == 0 { block_hash } else { block_header.parent_hash },
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
			let overlap_start = window_start.saturating_sub(old_window_start);

			let fresh_start = if latest < window_start {
				window_start
			} else {
				latest + 1
			};

			match load_all_sessions(ctx, block_hash, fresh_start, session_index).await? {
				None => {
					tracing::warn!(
						target: LOG_TARGET,
						"Could not load sessions {}..={} from block {:?} in session {}",
						latest + 1, session_index, block_hash, session_index,
					);

					return Ok(Err(SessionsUnavailable));
				}
				Some(s) => {
					let outdated = std::cmp::min(overlap_start as usize, session_window.session_info.len());
					session_window.session_info.drain(..outdated);
					session_window.session_info.extend(s);
					// we need to account for this case:
					// window_start ................................... session_index
					//              old_window_start ........... latest
					let new_earliest = std::cmp::max(window_start, old_window_start);
					session_window.earliest_session = Some(new_earliest);
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

struct ImportedBlockInfoEnv<'a> {
	session_window: &'a RollingSessionWindow,
	assignment_criteria: &'a (dyn AssignmentCriteria + Send + Sync),
	keystore: &'a LocalKeystore,
}

// Computes information about the imported block. Returns `None` if the info couldn't be extracted -
// failure to communicate with overseer,
async fn imported_block_info(
	ctx: &mut impl SubsystemContext,
	env: ImportedBlockInfoEnv<'_>,
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

		if env.session_window.earliest_session.as_ref().map_or(true, |e| &session_index < e) {
			tracing::debug!(target: LOG_TARGET, "Block {} is from ancient session {}. Skipping",
				block_hash, session_index);

			return Ok(None);
		}

		session_index
	};

	let babe_epoch = {
		let (s_tx, s_rx) = oneshot::channel();

		// It's not obvious whether to use the hash or the parent hash for this, intuitively. We
		// want to use the block hash itself, and here's why:
		//
		// First off, 'epoch' in BABE means 'session' in other places. 'epoch' is the terminology from
		// the paper, which we fulfill using 'session's, which are a Substrate consensus concept.
		//
		// In BABE, the on-chain and off-chain view of the current epoch can differ at epoch boundaries
		// because epochs change precisely at a slot. When a block triggers a new epoch, the state of
		// its parent will still have the old epoch. Conversely, we have the invariant that every
		// block in BABE has the epoch _it was authored in_ within its post-state. So we use the
		// block, and not its parent.
		//
		// It's worth nothing that Polkadot session changes, at least for the purposes of parachains,
		// would function the same way, except for the fact that they're always delayed by one block.
		// This gives us the opposite invariant for sessions - the parent block's post-state gives
		// us the canonical information about the session index for any of its children, regardless
		// of which slot number they might be produced at.
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

	let session_info = match env.session_window.session_info(session_index) {
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
						let assignments = env.assignment_criteria.compute_assignments(
							&env.keystore,
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
	state: &mut State<impl DBReader>,
	db_writer: &dyn KeyValueDB,
	head: Hash,
	finalized_number: &Option<BlockNumber>,
) -> SubsystemResult<Vec<BlockImportedCandidates>> {
	// Update session info based on most recent head.

	let mut span = polkadot_node_jaeger::hash_span(&head, "approval-checking-import");

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

	let new_blocks = determine_new_blocks(ctx, &state.db, head, &header, finalized_number)
		.map_err(|e| SubsystemError::with_origin("approval-voting", e))
		.await?;

	span.add_string_tag("new-blocks", &format!("{}", new_blocks.len()));

	if new_blocks.is_empty() { return Ok(Vec::new()) }

	let mut approval_meta: Vec<BlockApprovalMeta> = Vec::with_capacity(new_blocks.len());
	let mut imported_candidates = Vec::with_capacity(new_blocks.len());

	// `determine_new_blocks` gives us a vec in backwards order. we want to move forwards.
	for (block_hash, block_header) in new_blocks.into_iter().rev() {
		let env = ImportedBlockInfoEnv {
			session_window: &state.session_window,
			assignment_criteria: &*state.assignment_criteria,
			keystore: &state.keystore,
		};

		let ImportedBlockInfo {
			included_candidates,
			session_index,
			assignments,
			n_validators,
			relay_vrf_story,
			slot,
		} = match imported_block_info(ctx, env, block_hash, &block_header).await? {
			Some(i) => i,
			None => continue,
		};

		let session_info = state.session_window.session_info(session_index)
			.expect("imported_block_info requires session to be available; qed");

		let (block_tick, no_show_duration) = {
			let block_tick = slot_number_to_tick(state.slot_duration_millis, slot);
			let no_show_duration = slot_number_to_tick(
				state.slot_duration_millis,
				Slot::from(u64::from(session_info.no_show_slots)),
			);
			(block_tick, no_show_duration)
		};
		let needed_approvals = session_info.needed_approvals;
		let validator_group_lens: Vec<usize> = session_info.validator_groups.iter().map(|v| v.len()).collect();
		// insta-approve candidates on low-node testnets:
		// cf. https://github.com/paritytech/polkadot/issues/2411
		let num_candidates = included_candidates.len();
		let approved_bitfield = {
			if needed_approvals == 0 {
				tracing::debug!(
					target: LOG_TARGET,
					block_hash = ?block_hash,
					"Insta-approving all candidates",
				);
				bitvec::bitvec![BitOrderLsb0, u8; 1; num_candidates]
			} else {
				let mut result = bitvec::bitvec![BitOrderLsb0, u8; 0; num_candidates];
				for (i, &(_, _, _, backing_group)) in included_candidates.iter().enumerate() {
					let backing_group_size = validator_group_lens.get(backing_group.0 as usize)
						.copied()
						.unwrap_or(0);
					let needed_approvals = usize::try_from(needed_approvals).expect("usize is at least u32; qed");
					if n_validators.saturating_sub(backing_group_size) < needed_approvals {
						result.set(i, true);
					}
				}
				if result.any() {
					tracing::debug!(
						target: LOG_TARGET,
						block_hash = ?block_hash,
						"Insta-approving {}/{} candidates as the number of validators is too low",
						result.count_ones(),
						result.len(),
					);
				}
				result
			}
		};

		let block_entry = approval_db::v1::BlockEntry {
			block_hash,
			session: session_index,
			slot,
			relay_vrf_story: relay_vrf_story.0,
			candidates: included_candidates.iter()
				.map(|(hash, _, core, _)| (*core, *hash)).collect(),
			approved_bitfield,
			children: Vec::new(),
		};

		let candidate_entries = approval_db::v1::add_block_entry(
			db_writer,
			block_header.parent_hash,
			block_header.number,
			block_entry,
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

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_node_subsystem_test_helpers::make_subsystem_context;
	use polkadot_node_primitives::approval::{VRFOutput, VRFProof};
	use polkadot_primitives::v1::ValidatorIndex;
	use polkadot_subsystem::messages::AllMessages;
	use sp_core::testing::TaskExecutor;
	use sp_runtime::{Digest, DigestItem};
	use sp_consensus_babe::Epoch as BabeEpoch;
	use sp_consensus_babe::digests::{CompatibleDigestItem, PreDigest, SecondaryVRFPreDigest};
	use sp_keyring::sr25519::Keyring as Sr25519Keyring;
	use assert_matches::assert_matches;
	use merlin::Transcript;
	use std::{pin::Pin, sync::Arc};

	use crate::{criteria, BlockEntry};

	#[derive(Default)]
	struct TestDB {
		block_entries: HashMap<Hash, BlockEntry>,
		candidate_entries: HashMap<CandidateHash, CandidateEntry>,
	}

	impl DBReader for TestDB {
		fn load_block_entry(
			&self,
			block_hash: &Hash,
		) -> SubsystemResult<Option<BlockEntry>> {
			Ok(self.block_entries.get(block_hash).map(|c| c.clone()))
		}

		fn load_candidate_entry(
			&self,
			candidate_hash: &CandidateHash,
		) -> SubsystemResult<Option<CandidateEntry>> {
			Ok(self.candidate_entries.get(candidate_hash).map(|c| c.clone()))
		}
	}

	#[derive(Default)]
	struct MockClock;

	impl crate::time::Clock for MockClock {
		fn tick_now(&self) -> Tick {
			42 // chosen by fair dice roll
		}

		fn wait(&self, _tick: Tick) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
			Box::pin(async move {
				()
			})
		}
	}

	fn blank_state() -> State<TestDB> {
		State {
			session_window: RollingSessionWindow::default(),
			keystore: Arc::new(LocalKeystore::in_memory()),
			slot_duration_millis: 6_000,
			db: TestDB::default(),
			clock: Box::new(MockClock::default()),
			assignment_criteria: Box::new(MockAssignmentCriteria),
		}
	}

	fn single_session_state(index: SessionIndex, info: SessionInfo)
		-> State<TestDB>
	{
		State {
			session_window: RollingSessionWindow {
				earliest_session: Some(index),
				session_info: vec![info],
			},
			..blank_state()
		}
	}

	#[derive(Clone)]
	struct TestChain {
		start_number: BlockNumber,
		headers: Vec<Header>,
		numbers: HashMap<Hash, BlockNumber>,
	}

	impl TestChain {
		fn new(start: BlockNumber, len: usize) -> Self {
			assert!(len > 0, "len must be at least 1");

			let base = Header {
				digest: Default::default(),
				extrinsics_root: Default::default(),
				number: start,
				state_root: Default::default(),
				parent_hash: Default::default(),
			};

			let base_hash = base.hash();

			let mut chain = TestChain {
				start_number: start,
				headers: vec![base],
				numbers: vec![(base_hash, start)].into_iter().collect(),
			};

			for _ in 1..len {
				chain.grow()
			}

			chain
		}

		fn grow(&mut self) {
			let next = {
				let last = self.headers.last().unwrap();
				Header {
					digest: Default::default(),
					extrinsics_root: Default::default(),
					number: last.number + 1,
					state_root: Default::default(),
					parent_hash: last.hash(),
				}
			};

			self.numbers.insert(next.hash(), next.number);
			self.headers.push(next);
		}

		fn header_by_number(&self, number: BlockNumber) -> Option<&Header> {
			if number < self.start_number {
				None
			} else {
				self.headers.get((number - self.start_number) as usize)
			}
		}

		fn header_by_hash(&self, hash: &Hash) -> Option<&Header> {
			self.numbers.get(hash).and_then(|n| self.header_by_number(*n))
		}

		fn hash_by_number(&self, number: BlockNumber) -> Option<Hash> {
			self.header_by_number(number).map(|h| h.hash())
		}

		fn ancestry(&self, hash: &Hash, k: BlockNumber) -> Vec<Hash> {
			let n = match self.numbers.get(hash) {
				None => return Vec::new(),
				Some(&n) => n,
			};

			(0..k)
				.map(|i| i + 1)
				.filter_map(|i| self.header_by_number(n - i))
				.map(|h| h.hash())
				.collect()
		}
	}

	struct MockAssignmentCriteria;

	impl AssignmentCriteria for MockAssignmentCriteria {
		fn compute_assignments(
			&self,
			_keystore: &LocalKeystore,
			_relay_vrf_story: polkadot_node_primitives::approval::RelayVRFStory,
			_config: &criteria::Config,
			_leaving_cores: Vec<(polkadot_primitives::v1::CoreIndex, polkadot_primitives::v1::GroupIndex)>,
		) -> HashMap<polkadot_primitives::v1::CoreIndex, criteria::OurAssignment> {
			HashMap::new()
		}

		fn check_assignment_cert(
			&self,
			_claimed_core_index: polkadot_primitives::v1::CoreIndex,
			_validator_index: polkadot_primitives::v1::ValidatorIndex,
			_config: &criteria::Config,
			_relay_vrf_story: polkadot_node_primitives::approval::RelayVRFStory,
			_assignment: &polkadot_node_primitives::approval::AssignmentCert,
			_backing_group: polkadot_primitives::v1::GroupIndex,
		) -> Result<polkadot_node_primitives::approval::DelayTranche, criteria::InvalidAssignment> {
			Ok(0)
		}
	}

	// used for generating assignments where the validity of the VRF doesn't matter.
	fn garbage_vrf() -> (VRFOutput, VRFProof) {
		let key = Sr25519Keyring::Alice.pair();
		let key: &schnorrkel::Keypair = key.as_ref();

		let (o, p, _) = key.vrf_sign(Transcript::new(b"test-garbage"));
		(VRFOutput(o.to_output()), VRFProof(p))
	}

	#[test]
	fn determine_new_blocks_back_to_finalized() {
		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let db = TestDB::default();

		let chain = TestChain::new(10, 9);

		let head = chain.header_by_number(18).unwrap().clone();
		let head_hash = head.hash();
		let finalized_number = 12;

		// Finalized block should be omitted. The head provided to `determine_new_blocks`
		// should be included.
		let expected_ancestry = (13..=18)
			.map(|n| chain.header_by_number(n).map(|h| (h.hash(), h.clone())).unwrap())
			.rev()
			.collect::<Vec<_>>();

		let test_fut = Box::pin(async move {
			let ancestry = determine_new_blocks(
				&mut ctx,
				&db,
				head_hash,
				&head,
				finalized_number,
			).await.unwrap();

			assert_eq!(
				ancestry,
				expected_ancestry,
			);
		});

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::Ancestors {
					hash: h,
					k,
					response_channel: tx,
				}) => {
					assert_eq!(h, head_hash);
					assert_eq!(k, 4);
					let _ = tx.send(Ok(chain.ancestry(&h, k as _)));
				}
			);

			for _ in 0..4 {
				assert_matches!(
					handle.recv().await,
					AllMessages::ChainApi(ChainApiMessage::BlockHeader(h, tx)) => {
						let _ = tx.send(Ok(chain.header_by_hash(&h).map(|h| h.clone())));
					}
				);
			}

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::Ancestors {
					hash: h,
					k,
					response_channel: tx,
				}) => {
					assert_eq!(h, chain.hash_by_number(14).unwrap());
					assert_eq!(k, 4);
					let _ = tx.send(Ok(chain.ancestry(&h, k as _)));
				}
			);

			for _ in 0..4 {
				assert_matches!(
					handle.recv().await,
					AllMessages::ChainApi(ChainApiMessage::BlockHeader(h, tx)) => {
						let _ = tx.send(Ok(chain.header_by_hash(&h).map(|h| h.clone())));
					}
				);
			}

		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn determine_new_blocks_back_to_known() {
		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let mut db = TestDB::default();

		let chain = TestChain::new(10, 9);

		let head = chain.header_by_number(18).unwrap().clone();
		let head_hash = head.hash();
		let finalized_number = 12;
		let known_number = 15;
		let known_hash = chain.hash_by_number(known_number).unwrap();

		db.block_entries.insert(
			known_hash,
			crate::approval_db::v1::BlockEntry {
				block_hash: known_hash,
				session: 1,
				slot: Slot::from(100),
				relay_vrf_story: Default::default(),
				candidates: Vec::new(),
				approved_bitfield: Default::default(),
				children: Vec::new(),
			}.into(),
		);

		// Known block should be omitted. The head provided to `determine_new_blocks`
		// should be included.
		let expected_ancestry = (16..=18)
			.map(|n| chain.header_by_number(n).map(|h| (h.hash(), h.clone())).unwrap())
			.rev()
			.collect::<Vec<_>>();

		let test_fut = Box::pin(async move {
			let ancestry = determine_new_blocks(
				&mut ctx,
				&db,
				head_hash,
				&head,
				finalized_number,
			).await.unwrap();

			assert_eq!(
				ancestry,
				expected_ancestry,
			);
		});

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::Ancestors {
					hash: h,
					k,
					response_channel: tx,
				}) => {
					assert_eq!(h, head_hash);
					assert_eq!(k, 4);
					let _ = tx.send(Ok(chain.ancestry(&h, k as _)));
				}
			);

			for _ in 0u32..4 {
				assert_matches!(
					handle.recv().await,
					AllMessages::ChainApi(ChainApiMessage::BlockHeader(h, tx)) => {
						let _ = tx.send(Ok(chain.header_by_hash(&h).map(|h| h.clone())));
					}
				);
			}
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn determine_new_blocks_already_known_is_empty() {
		let pool = TaskExecutor::new();
		let (mut ctx, _handle) = make_subsystem_context::<(), _>(pool.clone());

		let mut db = TestDB::default();

		let chain = TestChain::new(10, 9);

		let head = chain.header_by_number(18).unwrap().clone();
		let head_hash = head.hash();
		let finalized_number = 0;

		db.block_entries.insert(
			head_hash,
			crate::approval_db::v1::BlockEntry {
				block_hash: head_hash,
				session: 1,
				slot: Slot::from(100),
				relay_vrf_story: Default::default(),
				candidates: Vec::new(),
				approved_bitfield: Default::default(),
				children: Vec::new(),
			}.into(),
		);

		// Known block should be omitted.
		let expected_ancestry = Vec::new();

		let test_fut = Box::pin(async move {
			let ancestry = determine_new_blocks(
				&mut ctx,
				&db,
				head_hash,
				&head,
				finalized_number,
			).await.unwrap();

			assert_eq!(
				ancestry,
				expected_ancestry,
			);
		});

		futures::executor::block_on(test_fut);
	}

	#[test]
	fn determine_new_blocks_parent_known_is_fast() {
		let pool = TaskExecutor::new();
		let (mut ctx, _handle) = make_subsystem_context::<(), _>(pool.clone());

		let mut db = TestDB::default();

		let chain = TestChain::new(10, 9);

		let head = chain.header_by_number(18).unwrap().clone();
		let head_hash = head.hash();
		let finalized_number = 0;
		let parent_hash = chain.hash_by_number(17).unwrap();

		db.block_entries.insert(
			parent_hash,
			crate::approval_db::v1::BlockEntry {
				block_hash: parent_hash,
				session: 1,
				slot: Slot::from(100),
				relay_vrf_story: Default::default(),
				candidates: Vec::new(),
				approved_bitfield: Default::default(),
				children: Vec::new(),
			}.into(),
		);

		// New block should be the only new one.
		let expected_ancestry = vec![(head_hash, head.clone())];

		let test_fut = Box::pin(async move {
			let ancestry = determine_new_blocks(
				&mut ctx,
				&db,
				head_hash,
				&head,
				finalized_number,
			).await.unwrap();

			assert_eq!(
				ancestry,
				expected_ancestry,
			);
		});

		futures::executor::block_on(test_fut);
	}

	#[test]
	fn determine_new_block_before_finality_is_empty() {
		let pool = TaskExecutor::new();
		let (mut ctx, _handle) = make_subsystem_context::<(), _>(pool.clone());

		let chain = TestChain::new(10, 9);

		let head = chain.header_by_number(18).unwrap().clone();
		let head_hash = head.hash();
		let parent_hash = chain.hash_by_number(17).unwrap();
		let mut db = TestDB::default();

		db.block_entries.insert(
			parent_hash,
			crate::approval_db::v1::BlockEntry {
				block_hash: parent_hash,
				session: 1,
				slot: Slot::from(100),
				relay_vrf_story: Default::default(),
				candidates: Vec::new(),
				approved_bitfield: Default::default(),
				children: Vec::new(),
			}.into(),
		);

		let test_fut = Box::pin(async move {
			let after_finality = determine_new_blocks(
				&mut ctx,
				&db,
				head_hash,
				&head,
				17,
			).await.unwrap();

			let at_finality = determine_new_blocks(
				&mut ctx,
				&db,
				head_hash,
				&head,
				18,
			).await.unwrap();

			let before_finality = determine_new_blocks(
				&mut ctx,
				&db,
				head_hash,
				&head,
				19,
			).await.unwrap();

			assert_eq!(
				after_finality,
				vec![(head_hash, head.clone())],
			);

			assert_eq!(
				at_finality,
				Vec::new(),
			);

			assert_eq!(
				before_finality,
				Vec::new(),
			);
		});

		futures::executor::block_on(test_fut);
	}

	fn dummy_session_info(index: SessionIndex) -> SessionInfo {
		SessionInfo {
			validators: Vec::new(),
			discovery_keys: Vec::new(),
			assignment_keys: Vec::new(),
			validator_groups: Vec::new(),
			n_cores: index as _,
			zeroth_delay_tranche_width: index as _,
			relay_vrf_modulo_samples: index as _,
			n_delay_tranches: index as _,
			no_show_slots: index as _,
			needed_approvals: index as _,
		}
	}


	#[test]
	fn imported_block_info_is_good() {
		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let session = 5;
		let session_info = dummy_session_info(session);

		let slot = Slot::from(10);

		let header = Header {
			digest: {
				let mut d = Digest::default();
				let (vrf_output, vrf_proof) = garbage_vrf();
				d.push(DigestItem::babe_pre_digest(PreDigest::SecondaryVRF(
					SecondaryVRFPreDigest {
						authority_index: 0,
						slot,
						vrf_output,
						vrf_proof,
					}
				)));

				d
			},
			extrinsics_root: Default::default(),
			number: 5,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let hash = header.hash();
		let make_candidate = |para_id| {
			let mut r = CandidateReceipt::default();
			r.descriptor.para_id = para_id;
			r.descriptor.relay_parent = hash;
			r
		};
		let candidates = vec![
			(make_candidate(1.into()), CoreIndex(0), GroupIndex(2)),
			(make_candidate(2.into()), CoreIndex(1), GroupIndex(3)),
		];


		let inclusion_events = candidates.iter().cloned()
			.map(|(r, c, g)| CandidateEvent::CandidateIncluded(r, Vec::new().into(), c, g))
			.collect::<Vec<_>>();

		let test_fut = {
			let included_candidates = candidates.iter()
				.map(|(r, c, g)| (r.hash(), r.clone(), *c, *g))
				.collect::<Vec<_>>();

			let session_window = {
				let mut window = RollingSessionWindow::default();

				window.earliest_session = Some(session);
				window.session_info.push(session_info);

				window
			};

			let header = header.clone();
			Box::pin(async move {
				let env = ImportedBlockInfoEnv {
					session_window: &session_window,
					assignment_criteria: &MockAssignmentCriteria,
					keystore: &LocalKeystore::in_memory(),
				};

				let info = imported_block_info(
					&mut ctx,
					env,
					hash,
					&header,
				).await.unwrap().unwrap();

				assert_eq!(info.included_candidates, included_candidates);
				assert_eq!(info.session_index, session);
				assert!(info.assignments.is_empty());
				assert_eq!(info.n_validators, 0);
				assert_eq!(info.slot, slot);
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::CandidateEvents(c_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = c_tx.send(Ok(inclusion_events));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(c_tx),
				)) => {
					assert_eq!(h, header.parent_hash);
					let _ = c_tx.send(Ok(session));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::CurrentBabeEpoch(c_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = c_tx.send(Ok(BabeEpoch {
						epoch_index: session as _,
						start_slot: Slot::from(0),
						duration: 200,
						authorities: vec![(Sr25519Keyring::Alice.public().into(), 1)],
						randomness: [0u8; 32],
					}));
				}
			);
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn imported_block_info_fails_if_no_babe_vrf() {
		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let session = 5;
		let session_info = dummy_session_info(session);

		let header = Header {
			digest: Digest::default(),
			extrinsics_root: Default::default(),
			number: 5,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let hash = header.hash();
		let make_candidate = |para_id| {
			let mut r = CandidateReceipt::default();
			r.descriptor.para_id = para_id;
			r.descriptor.relay_parent = hash;
			r
		};
		let candidates = vec![
			(make_candidate(1.into()), CoreIndex(0), GroupIndex(2)),
			(make_candidate(2.into()), CoreIndex(1), GroupIndex(3)),
		];

		let inclusion_events = candidates.iter().cloned()
			.map(|(r, c, g)| CandidateEvent::CandidateIncluded(r, Vec::new().into(), c, g))
			.collect::<Vec<_>>();

		let test_fut = {
			let session_window = {
				let mut window = RollingSessionWindow::default();

				window.earliest_session = Some(session);
				window.session_info.push(session_info);

				window
			};

			let header = header.clone();
			Box::pin(async move {
				let env = ImportedBlockInfoEnv {
					session_window: &session_window,
					assignment_criteria: &MockAssignmentCriteria,
					keystore: &LocalKeystore::in_memory(),
				};

				let info = imported_block_info(
					&mut ctx,
					env,
					hash,
					&header,
				).await.unwrap();

				assert!(info.is_none());
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::CandidateEvents(c_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = c_tx.send(Ok(inclusion_events));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(c_tx),
				)) => {
					assert_eq!(h, header.parent_hash);
					let _ = c_tx.send(Ok(session));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::CurrentBabeEpoch(c_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = c_tx.send(Ok(BabeEpoch {
						epoch_index: session as _,
						start_slot: Slot::from(0),
						duration: 200,
						authorities: vec![(Sr25519Keyring::Alice.public().into(), 1)],
						randomness: [0u8; 32],
					}));
				}
			);
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn imported_block_info_fails_if_unknown_session() {
		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let session = 5;

		let header = Header {
			digest: Digest::default(),
			extrinsics_root: Default::default(),
			number: 5,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let hash = header.hash();
		let make_candidate = |para_id| {
			let mut r = CandidateReceipt::default();
			r.descriptor.para_id = para_id;
			r.descriptor.relay_parent = hash;
			r
		};
		let candidates = vec![
			(make_candidate(1.into()), CoreIndex(0), GroupIndex(2)),
			(make_candidate(2.into()), CoreIndex(1), GroupIndex(3)),
		];

		let inclusion_events = candidates.iter().cloned()
			.map(|(r, c, g)| CandidateEvent::CandidateIncluded(r, Vec::new().into(), c, g))
			.collect::<Vec<_>>();

		let test_fut = {
			let session_window = RollingSessionWindow::default();

			let header = header.clone();
			Box::pin(async move {
				let env = ImportedBlockInfoEnv {
					session_window: &session_window,
					assignment_criteria: &MockAssignmentCriteria,
					keystore: &LocalKeystore::in_memory(),
				};

				let info = imported_block_info(
					&mut ctx,
					env,
					hash,
					&header,
				).await.unwrap();

				assert!(info.is_none());
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::CandidateEvents(c_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = c_tx.send(Ok(inclusion_events));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(c_tx),
				)) => {
					assert_eq!(h, header.parent_hash);
					let _ = c_tx.send(Ok(session));
				}
			);
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn insta_approval_works() {
		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let session = 5;
		let irrelevant = 666;
		let session_info = SessionInfo {
			validators: vec![Sr25519Keyring::Alice.public().into(); 6],
			discovery_keys: Vec::new(),
			assignment_keys: Vec::new(),
			validator_groups: vec![vec![ValidatorIndex(0); 5], vec![ValidatorIndex(0); 2]],
			n_cores: 6,
			needed_approvals: 2,
			zeroth_delay_tranche_width: irrelevant,
			relay_vrf_modulo_samples: irrelevant,
			n_delay_tranches: irrelevant,
			no_show_slots: irrelevant,
		};

		let slot = Slot::from(10);

		let chain = TestChain::new(4, 1);
		let parent_hash = chain.header_by_number(4).unwrap().hash();

		let header = Header {
			digest: {
				let mut d = Digest::default();
				let (vrf_output, vrf_proof) = garbage_vrf();
				d.push(DigestItem::babe_pre_digest(PreDigest::SecondaryVRF(
					SecondaryVRFPreDigest {
						authority_index: 0,
						slot,
						vrf_output,
						vrf_proof,
					}
				)));

				d
			},
			extrinsics_root: Default::default(),
			number: 5,
			state_root: Default::default(),
			parent_hash,
		};

		let hash = header.hash();
		let make_candidate = |para_id| {
			let mut r = CandidateReceipt::default();
			r.descriptor.para_id = para_id;
			r.descriptor.relay_parent = hash;
			r
		};
		let candidates = vec![
			(make_candidate(1.into()), CoreIndex(0), GroupIndex(0)),
			(make_candidate(2.into()), CoreIndex(1), GroupIndex(1)),
		];
		let inclusion_events = candidates.iter().cloned()
			.map(|(r, c, g)| CandidateEvent::CandidateIncluded(r, Vec::new().into(), c, g))
			.collect::<Vec<_>>();

		let mut state = single_session_state(session, session_info);
		state.db.block_entries.insert(
			parent_hash.clone(),
			crate::approval_db::v1::BlockEntry {
				block_hash: parent_hash.clone(),
				session,
				slot,
				relay_vrf_story: Default::default(),
				candidates: Vec::new(),
				approved_bitfield: Default::default(),
				children: Vec::new(),
			}.into(),
		);

		let db_writer = kvdb_memorydb::create(1);

		let test_fut = {
			Box::pin(async move {
				let result = handle_new_head(
					&mut ctx,
					&mut state,
					&db_writer,
					hash,
					&Some(1),
				).await.unwrap();

				assert_eq!(result.len(), 1);
				let candidates = &result[0].imported_candidates;
				assert_eq!(candidates.len(), 2);
				assert_eq!(candidates[0].1.approvals().len(), 6);
				assert_eq!(candidates[1].1.approvals().len(), 6);
				// the first candidate should be insta-approved
				// the second should not
				let entry: BlockEntry = crate::approval_db::v1::load_block_entry(&db_writer, &hash)
					.unwrap()
					.unwrap()
					.into();
				assert!(entry.is_candidate_approved(&candidates[0].0));
				assert!(!entry.is_candidate_approved(&candidates[1].0));
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::BlockHeader(
					h,
					tx,
				)) => {
					assert_eq!(h, hash);
					let _ = tx.send(Ok(Some(header.clone())));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(c_tx),
				)) => {
					assert_eq!(h, parent_hash.clone());
					let _ = c_tx.send(Ok(session));
				}
			);

			// determine_new_blocks exits early as the parent_hash is in the DB

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::CandidateEvents(c_tx),
				)) => {
					assert_eq!(h, hash.clone());
					let _ = c_tx.send(Ok(inclusion_events));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(c_tx),
				)) => {
					assert_eq!(h, parent_hash.clone());
					let _ = c_tx.send(Ok(session));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::CurrentBabeEpoch(c_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = c_tx.send(Ok(BabeEpoch {
						epoch_index: session as _,
						start_slot: Slot::from(0),
						duration: 200,
						authorities: vec![(Sr25519Keyring::Alice.public().into(), 1)],
						randomness: [0u8; 32],
					}));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::ApprovalDistribution(ApprovalDistributionMessage::NewBlocks(
					approval_meta
				)) => {
					assert_eq!(approval_meta.len(), 1);
				}
			);
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	fn cache_session_info_test(
		expected_start_session: SessionIndex,
		session: SessionIndex,
		mut window: RollingSessionWindow,
		expect_requests_from: SessionIndex,
	) {
		let header = Header {
			digest: Digest::default(),
			extrinsics_root: Default::default(),
			number: 5,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let hash = header.hash();

		let test_fut = {
			let header = header.clone();
			Box::pin(async move {
				cache_session_info_for_head(
					&mut ctx,
					&mut window,
					hash,
					&header,
				).await.unwrap().unwrap();

				assert_eq!(window.earliest_session, Some(expected_start_session));
				assert_eq!(
					window.session_info,
					(expected_start_session..=session).map(dummy_session_info).collect::<Vec<_>>(),
				);
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, header.parent_hash);
					let _ = s_tx.send(Ok(session));
				}
			);

			for i in expect_requests_from..=session {
				assert_matches!(
					handle.recv().await,
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
						h,
						RuntimeApiRequest::SessionInfo(j, s_tx),
					)) => {
						assert_eq!(h, hash);
						assert_eq!(i, j);
						let _ = s_tx.send(Ok(Some(dummy_session_info(i))));
					}
				);
			}
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn cache_session_info_first_early() {
		cache_session_info_test(
			0,
			1,
			RollingSessionWindow::default(),
			0,
		);
	}

	#[test]
	fn cache_session_info_does_not_underflow() {
		let window = RollingSessionWindow {
			earliest_session: Some(1),
			session_info: vec![dummy_session_info(1)],
		};

		cache_session_info_test(
			1,
			2,
			window,
			2,
		);
	}

	#[test]
	fn cache_session_info_first_late() {
		cache_session_info_test(
			(100 as SessionIndex).saturating_sub(APPROVAL_SESSIONS - 1),
			100,
			RollingSessionWindow::default(),
			(100 as SessionIndex).saturating_sub(APPROVAL_SESSIONS - 1),
		);
	}

	#[test]
	fn cache_session_info_jump() {
		let window = RollingSessionWindow {
			earliest_session: Some(50),
			session_info: vec![dummy_session_info(50), dummy_session_info(51), dummy_session_info(52)],
		};

		cache_session_info_test(
			(100 as SessionIndex).saturating_sub(APPROVAL_SESSIONS - 1),
			100,
			window,
			(100 as SessionIndex).saturating_sub(APPROVAL_SESSIONS - 1),
		);
	}

	#[test]
	fn cache_session_info_roll_full() {
		let start = 99 - (APPROVAL_SESSIONS - 1);
		let window = RollingSessionWindow {
			earliest_session: Some(start),
			session_info: (start..=99).map(dummy_session_info).collect(),
		};

		cache_session_info_test(
			(100 as SessionIndex).saturating_sub(APPROVAL_SESSIONS - 1),
			100,
			window,
			100, // should only make one request.
		);
	}

	#[test]
	fn cache_session_info_roll_many_full() {
		let start = 97 - (APPROVAL_SESSIONS - 1);
		let window = RollingSessionWindow {
			earliest_session: Some(start),
			session_info: (start..=97).map(dummy_session_info).collect(),
		};

		cache_session_info_test(
			(100 as SessionIndex).saturating_sub(APPROVAL_SESSIONS - 1),
			100,
			window,
			98,
		);
	}

	#[test]
	fn cache_session_info_roll_early() {
		let start = 0;
		let window = RollingSessionWindow {
			earliest_session: Some(start),
			session_info: (0..=1).map(dummy_session_info).collect(),
		};

		cache_session_info_test(
			0,
			2,
			window,
			2, // should only make one request.
		);
	}

	#[test]
	fn cache_session_info_roll_many_early() {
		let start = 0;
		let window = RollingSessionWindow {
			earliest_session: Some(start),
			session_info: (0..=1).map(dummy_session_info).collect(),
		};

		cache_session_info_test(
			0,
			3,
			window,
			2,
		);
	}

	#[test]
	fn any_session_unavailable_for_caching_means_no_change() {
		let session: SessionIndex = 6;
		let start_session = session.saturating_sub(APPROVAL_SESSIONS - 1);

		let header = Header {
			digest: Digest::default(),
			extrinsics_root: Default::default(),
			number: 5,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let mut window = RollingSessionWindow::default();
		let hash = header.hash();

		let test_fut = {
			let header = header.clone();
			Box::pin(async move {
				let res = cache_session_info_for_head(
					&mut ctx,
					&mut window,
					hash,
					&header,
				).await.unwrap();

				assert_matches!(res, Err(SessionsUnavailable));
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, header.parent_hash);
					let _ = s_tx.send(Ok(session));
				}
			);

			for i in start_session..=session {
				assert_matches!(
					handle.recv().await,
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
						h,
						RuntimeApiRequest::SessionInfo(j, s_tx),
					)) => {
						assert_eq!(h, hash);
						assert_eq!(i, j);

						let _ = s_tx.send(Ok(if i == session {
							None
						} else {
							Some(dummy_session_info(i))
						}));
					}
				);
			}
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn request_session_info_for_genesis() {
		let session: SessionIndex = 0;

		let header = Header {
			digest: Digest::default(),
			extrinsics_root: Default::default(),
			number: 0,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let mut window = RollingSessionWindow::default();
		let hash = header.hash();

		let test_fut = {
			let header = header.clone();
			Box::pin(async move {
				cache_session_info_for_head(
					&mut ctx,
					&mut window,
					hash,
					&header,
				).await.unwrap().unwrap();

				assert_eq!(window.earliest_session, Some(session));
				assert_eq!(
					window.session_info,
					vec![dummy_session_info(session)],
				);
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = s_tx.send(Ok(session));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionInfo(s, s_tx),
				)) => {
					assert_eq!(h, hash);
					assert_eq!(s, session);

					let _ = s_tx.send(Ok(Some(dummy_session_info(s))));
				}
			);
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}
}
