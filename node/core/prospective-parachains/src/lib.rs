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
use polkadot_primitives::vstaging::{Block, BlockId, CandidateHash, Hash, Id as ParaId};

const LOG_TARGET: &str = "parachain::prospective-parachains";

/// The Prospective Parachains Subsystem.
pub struct ProspectiveParachainsSubsystems {
	metrics: Metrics,
}

// TODO [now]: error types, fatal & non-fatal.

// TODO [now]: add this enum to the broader subsystem types.
pub enum ProspectiveParachainsMessage {}

struct FragmentTrees {
	para: ParaId,
	// Fragment nodes based on fragment head-data
	nodes: HashMap<Hash, (FragmentNode, usize)>,
	// The root hashes of this fragment-tree by head-data.
	roots: HashSet<Hash>,
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
	relay_parent: Hash,
	constraints: Constraints,
	relevant: HashSet<Hash>,
}

struct RelayBlockViewData {
	// Relevant fragments for each parachain that is scheduled.
	relevant_fragments: HashMap<ParaId, RelevantParaFragments>,
	block_info: RelayChainBlockInfo,
	base_constraints: Constraints,
	// TODO [now]: other stuff
}

struct View {
	// Active or recent relay-chain blocks by block hash.
	active_or_recent: HashMap<Hash, RelayBlockViewData>,
	fragment_trees: HashMap<ParaId, FragmentTrees>,
}

impl View {
	fn new() -> Self {
		View { active_or_recent: HashMap::new(), fragment_trees: HashMap::new() }
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
	// TODO [now]: get block info for all new blocks.
	// update ref counts for anything still relevant
	// clean up outgoing blocks
	// clean up unreferenced fragments

	unimplemented!()
}

#[derive(Clone)]
struct MetricsInner;

/// Prospective parachain metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

// TODO [now]: impl metrics
