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
	Block, BlockId, BlockNumber, CandidateDescriptor, CandidateHash, CommittedCandidateReceipt,
	GroupIndex, GroupRotationInfo, Hash, Header, Id as ParaId, SessionIndex, ValidatorIndex,
};

use crate::{
	error::{Error, FatalResult, NonFatal, NonFatalResult, Result},
	fragment_tree::FragmentTree,
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

/// The Prospective Parachains Subsystem.
pub struct ProspectiveParachainsSubsystems {
	metrics: Metrics,
}

// TODO [now]: add this enum to the broader subsystem types.
pub enum ProspectiveParachainsMessage {}

struct ScheduledPara {
	para: ParaId,
	base_constraints: Constraints,
	validator_group: GroupIndex,
}

struct RelayBlockViewData {
	// Scheduling info for paras and upcoming paras.
	scheduling: HashMap<ParaId, ScheduledPara>,
	block_info: RelayChainBlockInfo,
	// TODO [now]: other stuff
}

struct View {
	// Active or recent relay-chain blocks by block hash.
	active_leaves: HashSet<Hash>,
	active_or_recent: HashMap<Hash, RelayBlockViewData>,

	// Fragment graphs, one for each parachain.
	// TODO [now]: make this per-para per active-leaf
	// TODO [now]: have global candidate storage per para id
	fragments: HashMap<ParaId, FragmentTree>,
}

impl View {
	fn new() -> Self {
		View {
			active_leaves: HashSet::new(),
			active_or_recent: HashMap::new(),
			fragments: HashMap::new(),
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
	// all para-ids that the core could accept blocks for in the near future.
	near_future: Vec<ParaId>,
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
	unimplemented!()
}

#[derive(Clone)]
struct MetricsInner;

/// Prospective parachain metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

// TODO [now]: impl metrics
