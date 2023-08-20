// Copyright 2022-2023 Parity Technologies (UK) Ltd.
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

//! Implementation of the Prospective Parachains subsystem - this tracks and handles
//! prospective parachain fragments and informs other backing-stage subsystems
//! of work to be done.
//!
//! This is the main coordinator of work within the node for the collation and
//! backing phases of parachain consensus.
//!
//! This is primarily an implementation of "Fragment Trees", as described in
//! [`polkadot_node_subsystem_util::inclusion_emulator::staging`].
//!
//! This subsystem also handles concerns such as the relay-chain being forkful and session changes.

use std::{
	borrow::Cow,
	collections::{HashMap, HashSet},
};

use futures::{channel::oneshot, prelude::*};

use polkadot_node_subsystem::{
	messages::{
		ChainApiMessage, FragmentTreeMembership, HypotheticalCandidate,
		HypotheticalFrontierRequest, IntroduceCandidateRequest, ProspectiveParachainsMessage,
		ProspectiveValidationDataRequest, RuntimeApiMessage, RuntimeApiRequest,
	},
	overseer, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, SpawnedSubsystem, SubsystemError,
};
use polkadot_node_subsystem_util::{
	inclusion_emulator::staging::{Constraints, RelayChainBlockInfo},
	request_session_index_for_child,
	runtime::{prospective_parachains_mode, ProspectiveParachainsMode},
};
use polkadot_primitives::vstaging::{
	BlockNumber, CandidateHash, CandidatePendingAvailability, CommittedCandidateReceipt, CoreState,
	Hash, HeadData, Header, Id as ParaId, PersistedValidationData,
};

use crate::{
	error::{FatalError, FatalResult, JfyiError, JfyiErrorResult, Result},
	fragment_tree::{
		CandidateStorage, CandidateStorageInsertionError, FragmentTree, Scope as TreeScope,
	},
};

mod error;
mod fragment_tree;
#[cfg(test)]
mod tests;

mod metrics;
use self::metrics::Metrics;

const LOG_TARGET: &str = "parachain::prospective-parachains";

struct RelayBlockViewData {
	// Scheduling info for paras and upcoming paras.
	fragment_trees: HashMap<ParaId, FragmentTree>,
	pending_availability: HashSet<CandidateHash>,
}

struct View {
	// Active or recent relay-chain blocks by block hash.
	active_leaves: HashMap<Hash, RelayBlockViewData>,
	candidate_storage: HashMap<ParaId, CandidateStorage>,
}

impl View {
	fn new() -> Self {
		View { active_leaves: HashMap::new(), candidate_storage: HashMap::new() }
	}
}

/// The prospective parachains subsystem.
#[derive(Default)]
pub struct ProspectiveParachainsSubsystem {
	metrics: Metrics,
}

impl ProspectiveParachainsSubsystem {
	/// Create a new instance of the `ProspectiveParachainsSubsystem`.
	pub fn new(metrics: Metrics) -> Self {
		Self { metrics }
	}
}

#[overseer::subsystem(ProspectiveParachains, error = SubsystemError, prefix = self::overseer)]
impl<Context> ProspectiveParachainsSubsystem
where
	Context: Send + Sync,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		SpawnedSubsystem {
			future: run(ctx, self.metrics)
				.map_err(|e| SubsystemError::with_origin("prospective-parachains", e))
				.boxed(),
			name: "prospective-parachains-subsystem",
		}
	}
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn run<Context>(mut ctx: Context, metrics: Metrics) -> FatalResult<()> {
	let mut view = View::new();
	loop {
		crate::error::log_error(
			run_iteration(&mut ctx, &mut view, &metrics).await,
			"Encountered issue during run iteration",
		)?;
	}
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn run_iteration<Context>(
	ctx: &mut Context,
	view: &mut View,
	metrics: &Metrics,
) -> Result<()> {
	loop {
		match ctx.recv().await.map_err(FatalError::SubsystemReceive)? {
			FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)) => {
				handle_active_leaves_update(&mut *ctx, view, update, metrics).await?;
			},
			FromOrchestra::Signal(OverseerSignal::BlockFinalized(..)) => {},
			FromOrchestra::Communication { msg } => match msg {
				ProspectiveParachainsMessage::IntroduceCandidate(request, tx) =>
					handle_candidate_introduced(&mut *ctx, view, request, tx).await?,
				ProspectiveParachainsMessage::CandidateSeconded(para, candidate_hash) =>
					handle_candidate_seconded(view, para, candidate_hash),
				ProspectiveParachainsMessage::CandidateBacked(para, candidate_hash) =>
					handle_candidate_backed(&mut *ctx, view, para, candidate_hash).await?,
				ProspectiveParachainsMessage::GetBackableCandidate(
					relay_parent,
					para,
					required_path,
					tx,
				) => answer_get_backable_candidate(&view, relay_parent, para, required_path, tx),
				ProspectiveParachainsMessage::GetHypotheticalFrontier(request, tx) =>
					answer_hypothetical_frontier_request(&view, request, tx),
				ProspectiveParachainsMessage::GetTreeMembership(para, candidate, tx) =>
					answer_tree_membership_request(&view, para, candidate, tx),
				ProspectiveParachainsMessage::GetMinimumRelayParents(relay_parent, tx) =>
					answer_minimum_relay_parents_request(&view, relay_parent, tx),
				ProspectiveParachainsMessage::GetProspectiveValidationData(request, tx) =>
					answer_prospective_validation_data_request(&view, request, tx),
			},
		}
	}
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn handle_active_leaves_update<Context>(
	ctx: &mut Context,
	view: &mut View,
	update: ActiveLeavesUpdate,
	metrics: &Metrics,
) -> JfyiErrorResult<()> {
	// 1. clean up inactive leaves
	// 2. determine all scheduled para at new block
	// 3. construct new fragment tree for each para for each new leaf
	// 4. prune candidate storage.

	for deactivated in &update.deactivated {
		view.active_leaves.remove(deactivated);
	}

	let mut temp_header_cache = HashMap::new();
	for activated in update.activated.into_iter() {
		let hash = activated.hash;

		let mode = prospective_parachains_mode(ctx.sender(), hash)
			.await
			.map_err(JfyiError::Runtime)?;

		let ProspectiveParachainsMode::Enabled { max_candidate_depth, allowed_ancestry_len } = mode
		else {
			gum::trace!(
				target: LOG_TARGET,
				block_hash = ?hash,
				"Skipping leaf activation since async backing is disabled"
			);

			// Not a part of any allowed ancestry.
			return Ok(())
		};

		let mut pending_availability = HashSet::new();
		let scheduled_paras =
			fetch_upcoming_paras(&mut *ctx, hash, &mut pending_availability).await?;

		let block_info: RelayChainBlockInfo =
			match fetch_block_info(&mut *ctx, &mut temp_header_cache, hash).await? {
				None => {
					gum::warn!(
						target: LOG_TARGET,
						block_hash = ?hash,
						"Failed to get block info for newly activated leaf block."
					);

					// `update.activated` is an option, but we can use this
					// to exit the 'loop' and skip this block without skipping
					// pruning logic.
					continue
				},
				Some(info) => info,
			};

		let ancestry =
			fetch_ancestry(&mut *ctx, &mut temp_header_cache, hash, allowed_ancestry_len).await?;

		// Find constraints.
		let mut fragment_trees = HashMap::new();
		for para in scheduled_paras {
			let candidate_storage =
				view.candidate_storage.entry(para).or_insert_with(CandidateStorage::new);

			let backing_state = fetch_backing_state(&mut *ctx, hash, para).await?;

			let (constraints, pending_availability) = match backing_state {
				Some(c) => c,
				None => {
					// This indicates a runtime conflict of some kind.

					gum::debug!(
						target: LOG_TARGET,
						para_id = ?para,
						relay_parent = ?hash,
						"Failed to get inclusion backing state."
					);

					continue
				},
			};

			let pending_availability = preprocess_candidates_pending_availability(
				ctx,
				&mut temp_header_cache,
				constraints.required_parent.clone(),
				pending_availability,
			)
			.await?;
			let mut compact_pending = Vec::with_capacity(pending_availability.len());

			for c in pending_availability {
				let res = candidate_storage.add_candidate(c.candidate, c.persisted_validation_data);
				let candidate_hash = c.compact.candidate_hash;
				compact_pending.push(c.compact);

				match res {
					Ok(_) | Err(CandidateStorageInsertionError::CandidateAlreadyKnown(_)) => {
						// Anything on-chain is guaranteed to be backed.
						candidate_storage.mark_backed(&candidate_hash);
					},
					Err(err) => {
						gum::warn!(
							target: LOG_TARGET,
							?candidate_hash,
							para_id = ?para,
							?err,
							"Scraped invalid candidate pending availability",
						);
					},
				}
			}

			let scope = TreeScope::with_ancestors(
				para,
				block_info.clone(),
				constraints,
				compact_pending,
				max_candidate_depth,
				ancestry.iter().cloned(),
			)
			.expect("ancestors are provided in reverse order and correctly; qed");

			let tree = FragmentTree::populate(scope, &*candidate_storage);

			fragment_trees.insert(para, tree);
		}

		view.active_leaves
			.insert(hash, RelayBlockViewData { fragment_trees, pending_availability });
	}

	if !update.deactivated.is_empty() {
		// This has potential to be a hotspot.
		prune_view_candidate_storage(view, metrics);
	}

	Ok(())
}

fn prune_view_candidate_storage(view: &mut View, metrics: &Metrics) {
	metrics.time_prune_view_candidate_storage();

	let active_leaves = &view.active_leaves;
	let mut live_candidates = HashSet::new();
	let mut live_paras = HashSet::new();
	for sub_view in active_leaves.values() {
		for (para_id, fragment_tree) in &sub_view.fragment_trees {
			live_candidates.extend(fragment_tree.candidates());
			live_paras.insert(*para_id);
		}

		live_candidates.extend(sub_view.pending_availability.iter().cloned());
	}

	view.candidate_storage.retain(|para_id, storage| {
		if !live_paras.contains(&para_id) {
			return false
		}

		storage.retain(|h| live_candidates.contains(&h));

		// Even if `storage` is now empty, we retain.
		// This maintains a convenient invariant that para-id storage exists
		// as long as there's an active head which schedules the para.
		true
	})
}

struct ImportablePendingAvailability {
	candidate: CommittedCandidateReceipt,
	persisted_validation_data: PersistedValidationData,
	compact: crate::fragment_tree::PendingAvailability,
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn preprocess_candidates_pending_availability<Context>(
	ctx: &mut Context,
	cache: &mut HashMap<Hash, Header>,
	required_parent: HeadData,
	pending_availability: Vec<CandidatePendingAvailability>,
) -> JfyiErrorResult<Vec<ImportablePendingAvailability>> {
	let mut required_parent = required_parent;

	let mut importable = Vec::new();
	let expected_count = pending_availability.len();

	for (i, pending) in pending_availability.into_iter().enumerate() {
		let relay_parent =
			match fetch_block_info(ctx, cache, pending.descriptor.relay_parent).await? {
				None => {
					gum::debug!(
						target: LOG_TARGET,
						?pending.candidate_hash,
						?pending.descriptor.para_id,
						index = ?i,
						?expected_count,
						"Had to stop processing pending candidates early due to missing info.",
					);

					break
				},
				Some(b) => b,
			};

		let next_required_parent = pending.commitments.head_data.clone();
		importable.push(ImportablePendingAvailability {
			candidate: CommittedCandidateReceipt {
				descriptor: pending.descriptor,
				commitments: pending.commitments,
			},
			persisted_validation_data: PersistedValidationData {
				parent_head: required_parent,
				max_pov_size: pending.max_pov_size,
				relay_parent_number: relay_parent.number,
				relay_parent_storage_root: relay_parent.storage_root,
			},
			compact: crate::fragment_tree::PendingAvailability {
				candidate_hash: pending.candidate_hash,
				relay_parent,
			},
		});

		required_parent = next_required_parent;
	}

	Ok(importable)
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn handle_candidate_introduced<Context>(
	_ctx: &mut Context,
	view: &mut View,
	request: IntroduceCandidateRequest,
	tx: oneshot::Sender<FragmentTreeMembership>,
) -> JfyiErrorResult<()> {
	let IntroduceCandidateRequest {
		candidate_para: para,
		candidate_receipt: candidate,
		persisted_validation_data: pvd,
	} = request;

	// Add the candidate to storage.
	// Then attempt to add it to all trees.
	let storage = match view.candidate_storage.get_mut(&para) {
		None => {
			gum::warn!(
				target: LOG_TARGET,
				para_id = ?para,
				candidate_hash = ?candidate.hash(),
				"Received seconded candidate for inactive para",
			);

			let _ = tx.send(Vec::new());
			return Ok(())
		},
		Some(storage) => storage,
	};

	let candidate_hash = match storage.add_candidate(candidate, pvd) {
		Ok(c) => c,
		Err(CandidateStorageInsertionError::CandidateAlreadyKnown(c)) => {
			// Candidate known - return existing fragment tree membership.
			let _ = tx.send(fragment_tree_membership(&view.active_leaves, para, c));
			return Ok(())
		},
		Err(CandidateStorageInsertionError::PersistedValidationDataMismatch) => {
			// We can't log the candidate hash without either doing more ~expensive
			// hashing but this branch indicates something is seriously wrong elsewhere
			// so it's doubtful that it would affect debugging.

			gum::warn!(
				target: LOG_TARGET,
				para = ?para,
				"Received seconded candidate had mismatching validation data",
			);

			let _ = tx.send(Vec::new());
			return Ok(())
		},
	};

	let mut membership = Vec::new();
	for (relay_parent, leaf_data) in &mut view.active_leaves {
		if let Some(tree) = leaf_data.fragment_trees.get_mut(&para) {
			tree.add_and_populate(candidate_hash, &*storage);
			if let Some(depths) = tree.candidate(&candidate_hash) {
				membership.push((*relay_parent, depths));
			}
		}
	}

	if membership.is_empty() {
		storage.remove_candidate(&candidate_hash);
	}

	let _ = tx.send(membership);

	Ok(())
}

fn handle_candidate_seconded(view: &mut View, para: ParaId, candidate_hash: CandidateHash) {
	let storage = match view.candidate_storage.get_mut(&para) {
		None => {
			gum::warn!(
				target: LOG_TARGET,
				para_id = ?para,
				?candidate_hash,
				"Received instruction to second unknown candidate",
			);

			return
		},
		Some(storage) => storage,
	};

	if !storage.contains(&candidate_hash) {
		gum::warn!(
			target: LOG_TARGET,
			para_id = ?para,
			?candidate_hash,
			"Received instruction to second unknown candidate",
		);

		return
	}

	storage.mark_seconded(&candidate_hash);
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn handle_candidate_backed<Context>(
	_ctx: &mut Context,
	view: &mut View,
	para: ParaId,
	candidate_hash: CandidateHash,
) -> JfyiErrorResult<()> {
	let storage = match view.candidate_storage.get_mut(&para) {
		None => {
			gum::warn!(
				target: LOG_TARGET,
				para_id = ?para,
				?candidate_hash,
				"Received instruction to back unknown candidate",
			);

			return Ok(())
		},
		Some(storage) => storage,
	};

	if !storage.contains(&candidate_hash) {
		gum::warn!(
			target: LOG_TARGET,
			para_id = ?para,
			?candidate_hash,
			"Received instruction to back unknown candidate",
		);

		return Ok(())
	}

	if storage.is_backed(&candidate_hash) {
		gum::debug!(
			target: LOG_TARGET,
			para_id = ?para,
			?candidate_hash,
			"Received redundant instruction to mark candidate as backed",
		);

		return Ok(())
	}

	storage.mark_backed(&candidate_hash);
	Ok(())
}

fn answer_get_backable_candidate(
	view: &View,
	relay_parent: Hash,
	para: ParaId,
	required_path: Vec<CandidateHash>,
	tx: oneshot::Sender<Option<(CandidateHash, Hash)>>,
) {
	let data = match view.active_leaves.get(&relay_parent) {
		None => {
			gum::debug!(
				target: LOG_TARGET,
				?relay_parent,
				para_id = ?para,
				"Requested backable candidate for inactive relay-parent."
			);

			let _ = tx.send(None);
			return
		},
		Some(d) => d,
	};

	let tree = match data.fragment_trees.get(&para) {
		None => {
			gum::debug!(
				target: LOG_TARGET,
				?relay_parent,
				para_id = ?para,
				"Requested backable candidate for inactive para."
			);

			let _ = tx.send(None);
			return
		},
		Some(tree) => tree,
	};

	let storage = match view.candidate_storage.get(&para) {
		None => {
			gum::warn!(
				target: LOG_TARGET,
				?relay_parent,
				para_id = ?para,
				"No candidate storage for active para",
			);

			let _ = tx.send(None);
			return
		},
		Some(s) => s,
	};

	let Some(child_hash) =
		tree.select_child(&required_path, |candidate| storage.is_backed(candidate))
	else {
		let _ = tx.send(None);
		return
	};
	let Some(candidate_relay_parent) = storage.relay_parent_by_candidate_hash(&child_hash) else {
		gum::error!(
			target: LOG_TARGET,
			?child_hash,
			para_id = ?para,
			"Candidate is present in fragment tree but not in candidate's storage!",
		);
		let _ = tx.send(None);
		return
	};

	let _ = tx.send(Some((child_hash, candidate_relay_parent)));
}

fn answer_hypothetical_frontier_request(
	view: &View,
	request: HypotheticalFrontierRequest,
	tx: oneshot::Sender<Vec<(HypotheticalCandidate, FragmentTreeMembership)>>,
) {
	let mut response = Vec::with_capacity(request.candidates.len());
	for candidate in request.candidates {
		response.push((candidate, Vec::new()));
	}

	let required_active_leaf = request.fragment_tree_relay_parent;
	for (active_leaf, leaf_view) in view
		.active_leaves
		.iter()
		.filter(|(h, _)| required_active_leaf.as_ref().map_or(true, |x| h == &x))
	{
		for &mut (ref c, ref mut membership) in &mut response {
			let fragment_tree = match leaf_view.fragment_trees.get(&c.candidate_para()) {
				None => continue,
				Some(f) => f,
			};
			let candidate_storage = match view.candidate_storage.get(&c.candidate_para()) {
				None => continue,
				Some(storage) => storage,
			};

			let candidate_hash = c.candidate_hash();
			let hypothetical = match c {
				HypotheticalCandidate::Complete { receipt, persisted_validation_data, .. } =>
					fragment_tree::HypotheticalCandidate::Complete {
						receipt: Cow::Borrowed(receipt),
						persisted_validation_data: Cow::Borrowed(persisted_validation_data),
					},
				HypotheticalCandidate::Incomplete {
					parent_head_data_hash,
					candidate_relay_parent,
					..
				} => fragment_tree::HypotheticalCandidate::Incomplete {
					relay_parent: *candidate_relay_parent,
					parent_head_data_hash: *parent_head_data_hash,
				},
			};

			let depths = fragment_tree.hypothetical_depths(
				candidate_hash,
				hypothetical,
				candidate_storage,
				request.backed_in_path_only,
			);

			if !depths.is_empty() {
				membership.push((*active_leaf, depths));
			}
		}
	}

	let _ = tx.send(response);
}

fn fragment_tree_membership(
	active_leaves: &HashMap<Hash, RelayBlockViewData>,
	para: ParaId,
	candidate: CandidateHash,
) -> FragmentTreeMembership {
	let mut membership = Vec::new();
	for (relay_parent, view_data) in active_leaves {
		if let Some(tree) = view_data.fragment_trees.get(&para) {
			if let Some(depths) = tree.candidate(&candidate) {
				membership.push((*relay_parent, depths));
			}
		}
	}
	membership
}

fn answer_tree_membership_request(
	view: &View,
	para: ParaId,
	candidate: CandidateHash,
	tx: oneshot::Sender<FragmentTreeMembership>,
) {
	let _ = tx.send(fragment_tree_membership(&view.active_leaves, para, candidate));
}

fn answer_minimum_relay_parents_request(
	view: &View,
	relay_parent: Hash,
	tx: oneshot::Sender<Vec<(ParaId, BlockNumber)>>,
) {
	let mut v = Vec::new();
	if let Some(leaf_data) = view.active_leaves.get(&relay_parent) {
		for (para_id, fragment_tree) in &leaf_data.fragment_trees {
			v.push((*para_id, fragment_tree.scope().earliest_relay_parent().number));
		}
	}

	let _ = tx.send(v);
}

fn answer_prospective_validation_data_request(
	view: &View,
	request: ProspectiveValidationDataRequest,
	tx: oneshot::Sender<Option<PersistedValidationData>>,
) {
	// 1. Try to get the head-data from the candidate store if known.
	// 2. Otherwise, it might exist as the base in some relay-parent and we can find it by iterating
	//    fragment trees.
	// 3. Otherwise, it is unknown.
	// 4. Also try to find the relay parent block info by scanning fragment trees.
	// 5. If head data and relay parent block info are found - success. Otherwise, failure.

	let storage = match view.candidate_storage.get(&request.para_id) {
		None => {
			let _ = tx.send(None);
			return
		},
		Some(s) => s,
	};

	let mut head_data =
		storage.head_data_by_hash(&request.parent_head_data_hash).map(|x| x.clone());
	let mut relay_parent_info = None;
	let mut max_pov_size = None;

	for fragment_tree in view
		.active_leaves
		.values()
		.filter_map(|x| x.fragment_trees.get(&request.para_id))
	{
		if head_data.is_some() && relay_parent_info.is_some() && max_pov_size.is_some() {
			break
		}
		if relay_parent_info.is_none() {
			relay_parent_info =
				fragment_tree.scope().ancestor_by_hash(&request.candidate_relay_parent);
		}
		if head_data.is_none() {
			let required_parent = &fragment_tree.scope().base_constraints().required_parent;
			if required_parent.hash() == request.parent_head_data_hash {
				head_data = Some(required_parent.clone());
			}
		}
		if max_pov_size.is_none() {
			let contains_ancestor = fragment_tree
				.scope()
				.ancestor_by_hash(&request.candidate_relay_parent)
				.is_some();
			if contains_ancestor {
				// We are leaning hard on two assumptions here.
				// 1. That the fragment tree never contains allowed relay-parents whose session for
				//    children is different from that of the base block's.
				// 2. That the max_pov_size is only configurable per session.
				max_pov_size = Some(fragment_tree.scope().base_constraints().max_pov_size);
			}
		}
	}

	let _ = tx.send(match (head_data, relay_parent_info, max_pov_size) {
		(Some(h), Some(i), Some(m)) => Some(PersistedValidationData {
			parent_head: h,
			relay_parent_number: i.number,
			relay_parent_storage_root: i.storage_root,
			max_pov_size: m as _,
		}),
		_ => None,
	});
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn fetch_backing_state<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	para_id: ParaId,
) -> JfyiErrorResult<Option<(Constraints, Vec<CandidatePendingAvailability>)>> {
	let (tx, rx) = oneshot::channel();
	ctx.send_message(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::StagingParaBackingState(para_id, tx),
	))
	.await;

	Ok(rx
		.await
		.map_err(JfyiError::RuntimeApiRequestCanceled)??
		.map(|s| (From::from(s.constraints), s.pending_availability)))
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn fetch_upcoming_paras<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	pending_availability: &mut HashSet<CandidateHash>,
) -> JfyiErrorResult<Vec<ParaId>> {
	let (tx, rx) = oneshot::channel();

	// This'll have to get more sophisticated with parathreads,
	// but for now we can just use the `AvailabilityCores`.
	ctx.send_message(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::AvailabilityCores(tx),
	))
	.await;

	let cores = rx.await.map_err(JfyiError::RuntimeApiRequestCanceled)??;
	let mut upcoming = HashSet::new();
	for core in cores {
		match core {
			CoreState::Occupied(occupied) => {
				pending_availability.insert(occupied.candidate_hash);

				if let Some(next_up_on_available) = occupied.next_up_on_available {
					upcoming.insert(next_up_on_available.para_id);
				}
				if let Some(next_up_on_time_out) = occupied.next_up_on_time_out {
					upcoming.insert(next_up_on_time_out.para_id);
				}
			},
			CoreState::Scheduled(scheduled) => {
				upcoming.insert(scheduled.para_id);
			},
			CoreState::Free => {},
		}
	}

	Ok(upcoming.into_iter().collect())
}

// Fetch ancestors in descending order, up to the amount requested.
#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn fetch_ancestry<Context>(
	ctx: &mut Context,
	cache: &mut HashMap<Hash, Header>,
	relay_hash: Hash,
	ancestors: usize,
) -> JfyiErrorResult<Vec<RelayChainBlockInfo>> {
	if ancestors == 0 {
		return Ok(Vec::new())
	}

	let (tx, rx) = oneshot::channel();
	ctx.send_message(ChainApiMessage::Ancestors {
		hash: relay_hash,
		k: ancestors,
		response_channel: tx,
	})
	.await;

	let hashes = rx.map_err(JfyiError::ChainApiRequestCanceled).await??;
	let required_session = request_session_index_for_child(relay_hash, ctx.sender())
		.await
		.await
		.map_err(JfyiError::RuntimeApiRequestCanceled)??;

	let mut block_info = Vec::with_capacity(hashes.len());
	for hash in hashes {
		let info = match fetch_block_info(ctx, cache, hash).await? {
			None => {
				gum::warn!(
					target: LOG_TARGET,
					relay_hash = ?hash,
					"Failed to fetch info for hash returned from ancestry.",
				);

				// Return, however far we got.
				break
			},
			Some(info) => info,
		};

		// The relay chain cannot accept blocks backed from previous sessions, with
		// potentially previous validators. This is a technical limitation we need to
		// respect here.

		let session = request_session_index_for_child(hash, ctx.sender())
			.await
			.await
			.map_err(JfyiError::RuntimeApiRequestCanceled)??;

		if session == required_session {
			block_info.push(info);
		} else {
			break
		}
	}

	Ok(block_info)
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn fetch_block_header_with_cache<Context>(
	ctx: &mut Context,
	cache: &mut HashMap<Hash, Header>,
	relay_hash: Hash,
) -> JfyiErrorResult<Option<Header>> {
	if let Some(h) = cache.get(&relay_hash) {
		return Ok(Some(h.clone()))
	}

	let (tx, rx) = oneshot::channel();

	ctx.send_message(ChainApiMessage::BlockHeader(relay_hash, tx)).await;
	let header = rx.map_err(JfyiError::ChainApiRequestCanceled).await??;
	if let Some(ref h) = header {
		cache.insert(relay_hash, h.clone());
	}
	Ok(header)
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn fetch_block_info<Context>(
	ctx: &mut Context,
	cache: &mut HashMap<Hash, Header>,
	relay_hash: Hash,
) -> JfyiErrorResult<Option<RelayChainBlockInfo>> {
	let header = fetch_block_header_with_cache(ctx, cache, relay_hash).await?;

	Ok(header.map(|header| RelayChainBlockInfo {
		hash: relay_hash,
		number: header.number,
		storage_root: header.state_root,
	}))
}
