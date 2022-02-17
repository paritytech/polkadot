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
	collections::{hash_map::Entry as HEntry, HashMap, HashSet},
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
use polkadot_primitives::vstaging::{
	Block, BlockId, BlockNumber, CandidateHash, GroupIndex, GroupRotationInfo, Hash, Header,
	Id as ParaId, SessionIndex, ValidatorIndex,
};

use crate::error::{Error, FatalResult, NonFatal, NonFatalResult, Result};

mod error;

const LOG_TARGET: &str = "parachain::prospective-parachains";

/// The Prospective Parachains Subsystem.
pub struct ProspectiveParachainsSubsystems {
	metrics: Metrics,
}

// TODO [now]: add this enum to the broader subsystem types.
pub enum ProspectiveParachainsMessage {}

// TODO [now]: rename. more of a pile than a tree really.
// We only use it as a tree when traversing to select what to build upon.
struct FragmentTrees {
	para: ParaId,
	// Fragment nodes based on fragment head-data.
	nodes: HashMap<Hash, FragmentNode>,
}

impl FragmentTrees {
	fn is_empty(&self) -> bool {
		self.nodes.is_empty()
	}

	fn determine_relevant_fragments(&self, constraints: &Constraints) -> Vec<Hash> {
		unimplemented!()
	}

	// Retain fragments whose relay-parent passes the predicate.
	fn retain(&mut self, pred: impl Fn(&Hash) -> bool) {
		self.nodes.retain(|_, v| pred(&v.relay_parent()));
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

impl FragmentNode {
	fn relay_parent(&self) -> Hash {
		self.fragment.relay_parent().hash
	}
}

struct RelevantParaFragments {
	para: ParaId,
	base_constraints: Constraints,
}

struct RelayBlockViewData {
	// Relevant fragments for each parachain that is scheduled.
	scheduling: HashMap<ParaId, RelevantParaFragments>,
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

async fn run<Context>(mut ctx: Context) -> Result<()>
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
				// 1. Notification of new fragment (orphaned?)
				// 2. Notification of new fragment being backed
				// 3. Request for backable candidates
			},
		}
	}
}

// TODO [now]; non-fatal error type.
async fn update_view<Context>(
	view: &mut View,
	ctx: &mut Context,
	update: ActiveLeavesUpdate,
) -> NonFatalResult<()>
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

	let all_new: Vec<_> = relevant_blocks
		.iter()
		.filter(|(h, _hdr)| !view.active_or_recent.contains_key(h))
		.collect();

	{
		// Prune everything that was relevant but isn't anymore.
		let all_removed: Vec<_> = view
			.active_or_recent
			.keys()
			.cloned()
			.filter(|h| !relevant_blocks.contains_key(&h))
			.collect();

		for removed in all_removed {
			let _ = view.active_or_recent.remove(&removed);
		}

		// Add new blocks and get data if necessary. Dispatch work to backing subsystems.
		for (new_hash, new_header) in all_new {
			let block_info = RelayChainBlockInfo {
				hash: *new_hash,
				number: new_header.number,
				storage_root: new_header.state_root,
			};

			let scheduling_info = get_scheduling_info(ctx, *new_hash).await?;

			let mut relevant_fragments = HashMap::new();

			for core_info in scheduling_info.cores {
				// TODO [now]: construct RelayBlockViewData appropriately
			}

			view.active_or_recent.insert(
				*new_hash,
				RelayBlockViewData { scheduling: relevant_fragments, block_info },
			);
		}

		// TODO [now]: GC fragment trees:
		//   1. Keep only fragment trees for paras that are scheduled at any of our blocks.
		//   2. Keep only fragments that are built on any of our blocks.


		// TODO [now]: give all backing subsystems messages or signals.
		// There are, annoyingly, going to be race conditions with networking.
		// Move networking into a backing 'super-subsystem'?
		//
		// Which ones need to care about 'orphaned' fragments?
	}

	unimplemented!()
}

// TODO [now]: don't accept too many fragments per para per relay-parent
// Well I guess we're bounded/protected here by backing (Seconded messages)

async fn get_base_constraints<Context>(
	ctx: &mut Context,
	relay_block: Hash,
	para_id: ParaId,
) -> NonFatalResult<Constraints>
where
	Context: SubsystemContext<Message = ProspectiveParachainsMessage>,
	Context: overseer::SubsystemContext<Message = ProspectiveParachainsMessage>,
{
	unimplemented!()
}

// Scheduling info.
// - group rotation info: validator groups, group rotation info
// - information about parachains that are predictably going to be assigned
//   to each core. For now that's just parachains, but it's worth noting that
//   parathread claims are anchored to a specific core.
struct SchedulingInfo {
	validator_groups: Vec<Vec<ValidatorIndex>>,
	group_rotation_info: GroupRotationInfo,
	// One core per parachain. this should have same length as 'validator-groups'
	cores: Vec<CoreInfo>,
}

struct CoreInfo {
	para_id: ParaId,

	// (candidate hash, hash, timeout_at) if any
	pending_availability: Option<(CandidateHash, Hash, BlockNumber)>,
}

async fn get_scheduling_info<Context>(
	ctx: &mut Context,
	relay_block: Hash,
) -> NonFatalResult<SchedulingInfo>
where
	Context: SubsystemContext<Message = ProspectiveParachainsMessage>,
	Context: overseer::SubsystemContext<Message = ProspectiveParachainsMessage>,
{
	unimplemented!()
}

async fn find_all_relevant_blocks<Context>(
	ctx: &mut Context,
	active_leaves: &HashSet<Hash>,
) -> NonFatalResult<HashMap<Hash, Header>>
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
