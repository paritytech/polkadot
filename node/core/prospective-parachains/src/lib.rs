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
//! session changes, predicting validator group assignments, and
//! the re-backing of parachain blocks as a result of these changes.
//!
//! ## Re-backing
//!
//! Since this subsystems deals in enabling the collation and extension
//! of parachains in advance of actually being recorded on the relay-chain,
//! it is possible for the validator-group that initially backed the parablock
//! to be no longer assigned at the point that the parablock is submitted
//! to the relay-chain.
//!
//! This presents an issue, because the relay-chain only accepts blocks
//! which are backed by the currently-assigned group of validators, not
//! by the group of validators previously assigned to the parachain.
//!
//! In order to avoid wasting work at group rotation boundaries, we must
//! allow validators to re-validate the work of the preceding group.
//! This process is known as re-backing.
//!
//! What happens in practice is that validators observe that they are
//! scheduled to be assigned to a specific para in the near future.
//! And as a result, they dig into the existing fragment-trees to
//! re-back what already existed.

// TODO [now]: remove
#![allow(unused)]

use std::{
	collections::{HashMap, HashSet},
	collections::hash_map::Entry as HEntry,
	sync::Arc,
};

use futures::prelude::*;

use polkadot_node_subsystem::{
	overseer, ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, SubsystemContext,
	SubsystemError, SubsystemResult,
};
use polkadot_node_subsystem_util::{
	inclusion_emulator::staging::{
		ConstraintModifications, Constraints, Fragment, RelayChainBlockInfo,
	},
	metrics::{self, prometheus},
};
use polkadot_primitives::vstaging::{Block, BlockId, CandidateHash, Hash, Header, Id as ParaId};

use crate::error::{Error, FatalResult, NonFatal, Result};

mod error;

const LOG_TARGET: &str = "parachain::prospective-parachains";

/// The Prospective Parachains Subsystem.
pub struct ProspectiveParachainsSubsystems {
	metrics: Metrics,
}

// TODO [now]: add this enum to the broader subsystem types.
pub enum ProspectiveParachainsMessage {}

struct FragmentTrees {
	para: ParaId,
	// Fragment nodes based on fragment head-data
	nodes: HashMap<Hash, (FragmentNode, usize)>,
	// The root hashes of this fragment-tree by head-data.
	roots: HashSet<Hash>,
}

impl FragmentTrees {
	fn is_empty(&self) -> bool {
		self.nodes.is_empty()
	}

	fn determine_relevant_fragments(
		&self,
		constraints: &Constraints,
	) -> Vec<Hash> {
		unimplemented!()
	}

	fn add_refcount(&mut self, fragment_hash: &Hash) {
		if let Some(entry) = self.nodes.get_mut(fragment_hash) {
			entry.1 += 1;
		}
	}

	fn remove_refcount(&mut self, fragment_hash: Hash) {
		let node = match self.nodes.entry(fragment_hash) {
			HEntry::Vacant(_) => return,
			HEntry::Occupied(mut entry) => {
				if entry.get().1 == 1 {
					entry.remove().0
				} else {
					entry.get_mut().1 -= 1;
					return;
				}
			}
		};

		// TODO [now]: verify that this means 'it was present'
		if self.roots.remove(&fragment_hash) {
			for child in node.children {
				self.roots.insert(child);
			}
		}
	}
}

struct FragmentNode {
	// Head-data of the parent node.
	parent: Hash,
	// Head-data of children.
	// TODO [now]: make sure traversal detects loops.
	children: Vec<Hash>,
	fragment: Fragment,
}

// TODO [now] rename maybe
struct RelevantParaFragments {
	para: ParaId,
	base_constraints: Constraints,
	relevant: HashSet<Hash>,
}

struct RelayBlockViewData {
	// Relevant fragments for each parachain that is scheduled.
	relevant_fragments: HashMap<ParaId, RelevantParaFragments>,
	block_info: RelayChainBlockInfo,
	// TODO [now]: other stuff
}

struct View {
	// Active or recent relay-chain blocks by block hash.
	active_leaves: HashSet<Hash>,
	active_or_recent: HashMap<Hash, RelayBlockViewData>,

	// Fragment trees, one for each parachain.
	// TODO [now]: handle cleanup when these go obsolete.
	fragment_trees: HashMap<ParaId, FragmentTrees>,
}

impl View {
	fn new() -> Self {
		View {
			active_leaves: HashSet::new(),
			active_or_recent: HashMap::new(),
			fragment_trees: HashMap::new(),
		}
	}
}

async fn run<Context>(mut ctx: Context) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = ProspectiveParachainsMessage>,
	Context: overseer::SubsystemContext<Message = ProspectiveParachainsMessage>,
{
	let mut view = View::new();
	loop {
		match ctx.recv().await? {
			FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
				update_view(&mut view, &mut ctx, update).await?;
			},
			FromOverseer::Signal(OverseerSignal::BlockFinalized(..)) => {},
			FromOverseer::Communication { msg } => match msg {
				// TODO [now]: handle messages
			},
		}
	}
}

// TODO [now]; non-fatal error type.
async fn update_view<Context>(
	view: &mut View,
	ctx: &mut Context,
	update: ActiveLeavesUpdate,
) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = ProspectiveParachainsMessage>,
	Context: overseer::SubsystemContext<Message = ProspectiveParachainsMessage>,
{
	// TODO [now]: separate determining updates from updates themselves.

	// Update active_leaves
	{
		for activated in update.activated.into_iter() {
			view.active_leaves.insert(activated.hash);
		}

		for deactivated in update.deactivated.into_iter() {
			view.active_leaves.remove(&deactivated);
		}
	}

	// Find the set of blocks we care about.
	let relevant_blocks = find_all_relevant_blocks(ctx, &view.active_leaves).await?;

	// Prune everything that was relevant but isn't anymore.
	{
		let all_removed: Vec<_> = view
			.active_or_recent
			.keys()
			.cloned()
			.filter(|h| !relevant_blocks.contains_key(&h))
			.collect();

		for removed in all_removed {
			let view_data = view.active_or_recent.remove(&removed).expect(
				"key was gathered from iterating over all present keys; therefore is present; qed",
			);

			// TODO [now]: update fragment trees accordingly
			// TODO [now]: prune empty fragment trees
		}
	}

	// Add new blocks and get data if necessary.
	{
		let all_new: Vec<_> = relevant_blocks
			.iter()
			.filter(|(h, _hdr)| !view.active_or_recent.contains_key(h))
			.collect();

		for (new_hash, new_header) in all_new {
			let block_info = RelayChainBlockInfo {
				hash: *new_hash,
				number: new_header.number,
				storage_root: new_header.state_root,
			};

			let all_parachains = get_all_parachains(ctx, *new_hash).await?;

			let mut relevant_fragments = HashMap::new();
			for p in all_parachains {
				let constraints = get_base_constraints(ctx, *new_hash, p).await?;

				// TODO [now]: determine relevant fragments according to constraints.
				// TODO [now]: update ref counts in fragment trees
			}

			view.active_or_recent
				.insert(*new_hash, RelayBlockViewData { relevant_fragments, block_info });
		}
	}

	unimplemented!()
}

// TODO [now]; non-fatal error type.
async fn get_base_constraints<Context>(
	ctx: &mut Context,
	relay_block: Hash,
	para_id: ParaId,
) -> SubsystemResult<Constraints>
where
	Context: SubsystemContext<Message = ProspectiveParachainsMessage>,
	Context: overseer::SubsystemContext<Message = ProspectiveParachainsMessage>,
{
	unimplemented!()
}

// TODO [now]; non-fatal error type.
async fn get_all_parachains<Context>(
	ctx: &mut Context,
	relay_block: Hash,
) -> SubsystemResult<Vec<ParaId>>
where
	Context: SubsystemContext<Message = ProspectiveParachainsMessage>,
	Context: overseer::SubsystemContext<Message = ProspectiveParachainsMessage>,
{
	unimplemented!()
}

// TODO [now]; non-fatal error type.
async fn find_all_relevant_blocks<Context>(
	ctx: &mut Context,
	active_leaves: &HashSet<Hash>,
) -> SubsystemResult<HashMap<Hash, Header>>
where
	Context: SubsystemContext<Message = ProspectiveParachainsMessage>,
	Context: overseer::SubsystemContext<Message = ProspectiveParachainsMessage>,
{
	const LOOKBACK: usize = 2;
	unimplemented!()
}

#[derive(Clone)]
struct MetricsInner;

/// Prospective parachain metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

// TODO [now]: impl metrics
