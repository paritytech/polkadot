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

//! Handles the local node's view of the relay-chain state, allowed relay
//! parents of active leaves, and implements a message store for all known
//! statements.

use futures::{
	channel::{mpsc, oneshot},
	future::RemoteHandle,
	prelude::*,
};
use indexmap::IndexMap;

use polkadot_node_network_protocol::{self as net_protocol, PeerId, View as ActiveLeavesView};
use polkadot_node_primitives::SignedFullStatement;
use polkadot_node_subsystem::{
	messages::{RuntimeApiMessage, RuntimeApiRequest},
	overseer, ActivatedLeaf, ActiveLeavesUpdate, PerLeafSpan,
};
use polkadot_node_subsystem_util::{
	backing_implicit_view::{FetchError as ImplicitViewFetchError, View as ImplicitView},
	runtime,
};
use polkadot_primitives::v2::{
	CandidateHash, CommittedCandidateReceipt, CompactStatement, Hash, Id as ParaId,
	PersistedValidationData, UncheckedSignedStatement, ValidatorId, ValidatorIndex,
	ValidatorSignature,
};

use std::collections::{HashMap, HashSet};

use crate::{Error, LOG_TARGET, VC_THRESHOLD};

mod without_prospective;

/// The local node's view of the protocol state and messages.
pub struct View {
	implicit_view: ImplicitView,
	per_leaf: HashMap<Hash, ActiveLeafState>,
	/// State tracked for all relay-parents backing work is ongoing for. This includes
	/// all active leaves.
	///
	/// relay-parents fall into one of 3 categories.
	///   1. active leaves which do support prospective parachains
	///   2. active leaves which do not support prospective parachains
	///   3. relay-chain blocks which are ancestors of an active leaf and
	///      do support prospective parachains.
	///
	/// Relay-chain blocks which don't support prospective parachains are
	/// never included in the fragment trees of active leaves which do.
	///
	/// While it would be technically possible to support such leaves in
	/// fragment trees, it only benefits the transition period when asynchronous
	/// backing is being enabled and complicates code complexity.
	per_relay_parent: HashMap<Hash, RelayParentInfo>,
}

/// A peer's view of the protocol state and messages.
pub struct PeerView {
	active_leaves: ActiveLeavesView,
	/// Our understanding of the peer's knowledge of relay-parents and
	/// corresponding messages.
	///
	/// These are either active leaves we recognize or relay-parents that
	/// are implicit ancestors of active leaves we do recognize.
	///
	/// Furthermore, this is guaranteed to be an intersection of our own
	/// implicit/explicit view. The intersection defines the shared view,
	/// which determines the messages that are allowed to flow.
	known_relay_parents: HashMap<Hash, PeerRelayParentKnowledge>,
}

/// Whether a leaf has prospective parachains enabled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProspectiveParachainsMode {
	/// Prospective parachains are enabled at the leaf.
	Enabled,
	/// Prospective parachains are disabled at the leaf.
	Disabled,
}

struct ActiveLeafState {
	prospective_parachains_mode: ProspectiveParachainsMode,
}

enum RelayParentInfo {
	VStaging(RelayParentWithProspective),
	V2(without_prospective::RelayParentInfo),
}

enum PeerRelayParentKnowledge {
	VStaging(PeerRelayParentKnowledgeWithProspective),
	V2(without_prospective::PeerRelayParentKnowledge),
}

struct RelayParentWithProspective;
struct PeerRelayParentKnowledgeWithProspective;

#[overseer::contextbounds(StatementDistribution, prefix = self::overseer)]
async fn prospective_parachains_mode<Context>(
	ctx: &mut Context,
	leaf_hash: Hash,
) -> Result<ProspectiveParachainsMode, Error> {
	let (tx, rx) = oneshot::channel();
	ctx.send_message(RuntimeApiMessage::Request(leaf_hash, RuntimeApiRequest::Version(tx)))
		.await;

	let version = runtime::recv_runtime(rx).await?;

	// TODO [now]: proper staging API logic.
	// based on https://github.com/paritytech/substrate/issues/11577#issuecomment-1145347025
	// this is likely final & correct but we should make thes constants.
	if version == 3 {
		Ok(ProspectiveParachainsMode::Enabled)
	} else {
		if version != 2 {
			gum::warn!(
				target: LOG_TARGET,
				"Runtime API version is {}, expected 2 or 3. Prospective parachains are disabled",
				version
			);
		}
		Ok(ProspectiveParachainsMode::Disabled)
	}
}

/// Handle an active leaves update and update the view.
#[overseer::contextbounds(StatementDistribution, prefix = self::overseer)]
pub async fn handle_active_leaves_update<Context>(
	ctx: &mut Context,
	view: &mut View,
	update: ActiveLeavesUpdate,
) -> Result<(), Error> {
	enum LeafHasProspectiveParachains {
		Enabled(Result<Vec<ParaId>, ImplicitViewFetchError>),
		Disabled,
	}

	// Activate in implicit view before deactivate, per the docs on ImplicitView,
	// this is more efficient and also preserves more old data that can be
	// useful for understanding peers' views.
	let res = if let Some(leaf) = update.activated {
		// Only activate in implicit view if prospective
		// parachains are enabled.
		let mode = prospective_parachains_mode(ctx, leaf.hash).await?;
		let leaf_hash = leaf.hash;
		Some((
			leaf,
			match mode {
				ProspectiveParachainsMode::Disabled => LeafHasProspectiveParachains::Disabled,
				ProspectiveParachainsMode::Enabled => LeafHasProspectiveParachains::Enabled(
					view.implicit_view.activate_leaf(ctx.sender(), leaf_hash).await,
				),
			},
		))
	} else {
		None
	};

	for deactivated in update.deactivated {
		view.per_leaf.remove(&deactivated);
		view.implicit_view.deactivate_leaf(deactivated);
	}

	// clean up `per_relay_parent` according to ancestry of leaves.
	//
	// when prospective parachains are disabled, the implicit view is empty,
	// which means we'll clean up everything. This is correct.
	{
		let remaining: HashSet<_> = view.implicit_view.all_allowed_relay_parents().collect();
		view.per_relay_parent.retain(|r, _| remaining.contains(&r));
	}

	// Get relay parents which might be fresh but might be known already
	// that are explicit or implicit from the new active leaf.
	let fresh_relay_parents = match res {
		None => return Ok(()),
		Some((leaf, LeafHasProspectiveParachains::Disabled)) => {
			// defensive in this case - for enabled, this manifests as an error.
			if view.per_leaf.contains_key(&leaf.hash) {
				return Ok(())
			}

			view.per_leaf.insert(
				leaf.hash,
				ActiveLeafState {
					prospective_parachains_mode: ProspectiveParachainsMode::Disabled,
				},
			);

			vec![(leaf.hash, ProspectiveParachainsMode::Disabled)]
		},
		Some((leaf, LeafHasProspectiveParachains::Enabled(Ok(_)))) => {
			// TODO [now]: create fresh relay parents, clean up old candidates,
			// etc.
			unimplemented!();
		},
		Some((leaf, LeafHasProspectiveParachains::Enabled(Err(e)))) =>
			return Err(Error::ImplicitViewFetchError(leaf.hash, e)),
	};

	// TODO [now]: create new per-relay-parent entries using `fresh_relay_parents`.
	Ok(())
}
