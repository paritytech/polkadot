// Copyright 2022 Parity Technologies (UK) Ltd.
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
//! This also handles concerns such as the relay-chain being forkful,
//! session changes, predicting validator group assignments.

use std::collections::{HashMap, HashSet};

use futures::{channel::oneshot, prelude::*};

use polkadot_node_subsystem::{
	messages::{
		ChainApiMessage, FragmentTreeMembership, HypotheticalDepthRequest,
		ProspectiveParachainsMessage, RuntimeApiMessage, RuntimeApiRequest,
	},
	overseer, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, SpawnedSubsystem, SubsystemError,
};
use polkadot_node_subsystem_util::inclusion_emulator::staging::{Constraints, RelayChainBlockInfo};
use polkadot_primitives::vstaging::{
	BlockNumber, CandidateHash, CommittedCandidateReceipt, CoreState, Hash, Id as ParaId,
	PersistedValidationData,
};

use crate::{
	error::{FatalError, FatalResult, JfyiError, JfyiErrorResult, Result},
	fragment_tree::{CandidateStorage, FragmentTree, Scope as TreeScope},
};

mod error;
mod fragment_tree;

const LOG_TARGET: &str = "parachain::prospective-parachains";

// The maximum depth the subsystem will allow. 'depth' is defined as the
// amount of blocks between the para head in a relay-chain block's state
// and a candidate with a particular relay-parent.
//
// This value is chosen mostly for reasons of resource-limitation.
// Without it, a malicious validator group could create arbitrarily long,
// useless prospective parachains and DoS honest nodes.
const MAX_DEPTH: usize = 4;

// The maximum ancestry we support.
const MAX_ANCESTRY: usize = 5;

struct RelayBlockViewData {
	// Scheduling info for paras and upcoming paras.
	fragment_trees: HashMap<ParaId, FragmentTree>,
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
pub struct ProspectiveParachainsSubsystem;

#[overseer::subsystem(ProspectiveParachains, error = SubsystemError, prefix = self::overseer)]
impl<Context> ProspectiveParachainsSubsystem
where
	Context: Send + Sync,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		SpawnedSubsystem {
			future: run(ctx)
				.map_err(|e| SubsystemError::with_origin("prospective-parachains", e))
				.boxed(),
			name: "prospective-parachains-subsystem",
		}
	}
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn run<Context>(mut ctx: Context) -> FatalResult<()> {
	let mut view = View::new();
	loop {
		crate::error::log_error(
			run_iteration(&mut ctx, &mut view).await,
			"Encountered issue during run iteration",
		)?;
	}
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn run_iteration<Context>(ctx: &mut Context, view: &mut View) -> Result<()> {
	loop {
		match ctx.recv().await.map_err(FatalError::SubsystemReceive)? {
			FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)) => {
				handle_active_leaves_update(&mut *ctx, view, update).await?;
			},
			FromOrchestra::Signal(OverseerSignal::BlockFinalized(..)) => {},
			FromOrchestra::Communication { msg } => match msg {
				ProspectiveParachainsMessage::CandidateSeconded(para, candidate, pvd, tx) =>
					handle_candidate_seconded(&mut *ctx, view, para, candidate, pvd, tx).await?,
				ProspectiveParachainsMessage::CandidateBacked(para, candidate_hash) =>
					handle_candidate_backed(&mut *ctx, view, para, candidate_hash).await?,
				ProspectiveParachainsMessage::GetBackableCandidate(
					relay_parent,
					para,
					required_path,
					tx,
				) => answer_get_backable_candidate(&view, relay_parent, para, required_path, tx),
				ProspectiveParachainsMessage::GetHypotheticalDepth(request, tx) =>
					answer_hypothetical_depths_request(&view, request, tx),
				ProspectiveParachainsMessage::GetTreeMembership(para, candidate, tx) =>
					answer_tree_membership_request(&view, para, candidate, tx),
				ProspectiveParachainsMessage::GetMinimumRelayParent(para, relay_parent, tx) =>
					answer_minimum_relay_parent_request(&view, para, relay_parent, tx),
			},
		}
	}
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn handle_active_leaves_update<Context>(
	ctx: &mut Context,
	view: &mut View,
	update: ActiveLeavesUpdate,
) -> JfyiErrorResult<()> {
	// 1. clean up inactive leaves
	// 2. determine all scheduled para at new block
	// 3. construct new fragment tree for each para for each new leaf
	// 4. prune candidate storage.

	for deactivated in &update.deactivated {
		view.active_leaves.remove(deactivated);
	}

	for activated in update.activated.into_iter() {
		let hash = activated.hash;
		let scheduled_paras = fetch_upcoming_paras(&mut *ctx, hash).await?;

		let block_info: RelayChainBlockInfo = match fetch_block_info(&mut *ctx, hash).await? {
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

		let ancestry = fetch_ancestry(&mut *ctx, hash, MAX_ANCESTRY).await?;

		// Find constraints.
		let mut fragment_trees = HashMap::new();
		for para in scheduled_paras {
			let candidate_storage =
				view.candidate_storage.entry(para).or_insert_with(CandidateStorage::new);

			let constraints = fetch_base_constraints(&mut *ctx, hash, para).await?;

			let constraints = match constraints {
				Some(c) => c,
				None => {
					// This indicates a runtime conflict of some kind.

					gum::debug!(
						target: LOG_TARGET,
						para_id = ?para,
						relay_parent = ?hash,
						"Failed to get inclusion constraints."
					);

					continue
				},
			};

			let scope = TreeScope::with_ancestors(
				para,
				block_info.clone(),
				constraints,
				MAX_DEPTH,
				ancestry.iter().cloned(),
			)
			.expect("ancestors are provided in reverse order and correctly; qed");

			let tree = FragmentTree::populate(scope, &*candidate_storage);
			fragment_trees.insert(para, tree);
		}

		view.active_leaves.insert(hash, RelayBlockViewData { fragment_trees });
	}

	if !update.deactivated.is_empty() {
		// This has potential to be a hotspot.
		prune_view_candidate_storage(view);
	}

	Ok(())
}

fn prune_view_candidate_storage(view: &mut View) {
	let active_leaves = &view.active_leaves;
	view.candidate_storage.retain(|para_id, storage| {
		let mut coverage = HashSet::new();
		let mut contained = false;
		for head in active_leaves.values() {
			if let Some(tree) = head.fragment_trees.get(&para_id) {
				coverage.extend(tree.candidates());
				contained = true;
			}
		}

		if !contained {
			return false
		}

		storage.retain(|h| coverage.contains(&h));

		// Even if `storage` is now empty, we retain.
		// This maintains a convenient invariant that para-id storage exists
		// as long as there's an active head which schedules the para.
		true
	})
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn handle_candidate_seconded<Context>(
	_ctx: &mut Context,
	view: &mut View,
	para: ParaId,
	candidate: CommittedCandidateReceipt,
	pvd: PersistedValidationData,
	tx: oneshot::Sender<FragmentTreeMembership>,
) -> JfyiErrorResult<()> {
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
		Err(crate::fragment_tree::CandidateStorageInsertionError::CandidateAlreadyKnown(_)) => {
			let _ = tx.send(Vec::new());
			return Ok(())
		},
		Err(
			crate::fragment_tree::CandidateStorageInsertionError::PersistedValidationDataMismatch,
		) => {
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
	let _ = tx.send(membership);

	Ok(())
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
				"Received instructio to back candidate",
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
			"Received instruction to mark unknown candidate as backed.",
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
	tx: oneshot::Sender<Option<CandidateHash>>,
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

	let _ = tx.send(tree.select_child(&required_path, |candidate| storage.is_backed(candidate)));
}

fn answer_hypothetical_depths_request(
	view: &View,
	request: HypotheticalDepthRequest,
	tx: oneshot::Sender<Vec<usize>>,
) {
	match view
		.active_leaves
		.get(&request.fragment_tree_relay_parent)
		.and_then(|l| l.fragment_trees.get(&request.candidate_para))
	{
		Some(fragment_tree) => {
			let depths = fragment_tree.hypothetical_depths(
				request.candidate_hash,
				request.parent_head_data_hash,
				request.candidate_relay_parent,
			);
			let _ = tx.send(depths);
		},
		None => {
			let _ = tx.send(Vec::new());
		},
	}
}

fn answer_tree_membership_request(
	view: &View,
	para: ParaId,
	candidate: CandidateHash,
	tx: oneshot::Sender<FragmentTreeMembership>,
) {
	let mut membership = Vec::new();
	for (relay_parent, view_data) in &view.active_leaves {
		if let Some(tree) = view_data.fragment_trees.get(&para) {
			if let Some(depths) = tree.candidate(&candidate) {
				membership.push((*relay_parent, depths));
			}
		}
	}
	let _ = tx.send(membership);
}

fn answer_minimum_relay_parent_request(
	view: &View,
	para: ParaId,
	relay_parent: Hash,
	tx: oneshot::Sender<Option<BlockNumber>>,
) {
	let res = view
		.active_leaves
		.get(&relay_parent)
		.and_then(|data| data.fragment_trees.get(&para))
		.map(|tree| tree.scope().earliest_relay_parent().number);

	let _ = tx.send(res);
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn fetch_base_constraints<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
	para_id: ParaId,
) -> JfyiErrorResult<Option<Constraints>> {
	let (tx, rx) = oneshot::channel();
	ctx.send_message(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::StagingValidityConstraints(para_id, tx),
	))
	.await;

	Ok(rx.await.map_err(JfyiError::RuntimeApiRequestCanceled)??.map(From::from))
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn fetch_upcoming_paras<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
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
	relay_hash: Hash,
	ancestors: usize,
) -> JfyiErrorResult<Vec<RelayChainBlockInfo>> {
	let (tx, rx) = oneshot::channel();
	ctx.send_message(ChainApiMessage::Ancestors {
		hash: relay_hash,
		k: ancestors,
		response_channel: tx,
	})
	.await;

	let hashes = rx.map_err(JfyiError::ChainApiRequestCanceled).await??;
	let mut block_info = Vec::with_capacity(hashes.len());
	for hash in hashes {
		match fetch_block_info(ctx, relay_hash).await? {
			None => {
				gum::warn!(
					target: LOG_TARGET,
					relay_hash = ?hash,
					"Failed to fetch info for hash returned from ancestry.",
				);

				// Return, however far we got.
				return Ok(block_info)
			},
			Some(info) => {
				block_info.push(info);
			},
		}
	}

	Ok(block_info)
}

#[overseer::contextbounds(ProspectiveParachains, prefix = self::overseer)]
async fn fetch_block_info<Context>(
	ctx: &mut Context,
	relay_hash: Hash,
) -> JfyiErrorResult<Option<RelayChainBlockInfo>> {
	let (tx, rx) = oneshot::channel();

	ctx.send_message(ChainApiMessage::BlockHeader(relay_hash, tx)).await;
	let header = rx.map_err(JfyiError::ChainApiRequestCanceled).await??;
	Ok(header.map(|header| RelayChainBlockInfo {
		hash: relay_hash,
		number: header.number,
		storage_root: header.state_root,
	}))
}

#[derive(Clone)]
struct MetricsInner;

/// Prospective parachain metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);
