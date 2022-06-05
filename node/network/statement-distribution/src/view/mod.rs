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

// TODO [now]: remove at end
#![allow(unused)]

use futures::{
	channel::{mpsc, oneshot},
	future::RemoteHandle,
	prelude::*,
	stream::FuturesUnordered,
};
use indexmap::IndexMap;

use polkadot_node_network_protocol::{self as net_protocol, PeerId, View as ActiveLeavesView};
use polkadot_node_primitives::SignedFullStatement;
use polkadot_node_subsystem::{
	messages::{RuntimeApiMessage, RuntimeApiRequest},
	overseer, ActivatedLeaf, ActiveLeavesUpdate, Span,
};
use polkadot_node_subsystem_util::{
	backing_implicit_view::{FetchError as ImplicitViewFetchError, View as ImplicitView},
	runtime::{self, RuntimeInfo},
};
use polkadot_primitives::v2::{
	CandidateHash, CommittedCandidateReceipt, CompactStatement, Hash, Id as ParaId,
	OccupiedCoreAssumption, PersistedValidationData, UncheckedSignedStatement, ValidatorId,
	ValidatorIndex, ValidatorSignature,
};

use std::collections::{HashMap, HashSet};

use crate::{Error, LOG_TARGET, VC_THRESHOLD};

mod with_prospective;
mod without_prospective;

/// The local node's view of the protocol state and messages.
pub struct View {
	// The view for all explicit and implicit view relay-parents which
	// support prospective parachains. Relay-parents in here are exclusive
	// with those in `without_prospective`.
	with_prospective: with_prospective::View,
	// The view for all explicit view relay-parents which don't support
	// prospective parachains.
	// Relay-parents in here are exclusive with those in `with_prospectives`.
	without_prospective: without_prospective::View,
}

/// A peer's view of the protocol state and messages.
///
/// The [`PeerView`] should be synchronized with the [`View`] every time the
/// [`View`] changes by using [`PeerView::synchronize_with_our_view`].
///
/// When the peer updates their active leaves,
pub struct PeerView {
	active_leaves: ActiveLeavesView,
	mode: ProspectiveParachainsMode,
	with_prospective: with_prospective::PeerView,
	without_prospective: without_prospective::PeerView,
}

impl PeerView {
	/// Create a new [`PeerView`]. The mode should be set according to the
	/// peer's network protocol version at a higher level.
	///
	/// Peers which don't support prospective parachains never will, at least
	/// until they reconnect.
	pub fn new(mode: ProspectiveParachainsMode) -> Self {
		PeerView {
			active_leaves: Default::default(),
			mode,
			with_prospective: Default::default(),
			without_prospective: Default::default(),
		}
	}

	/// Synchronize a peer view with our own. This should be called for each
	/// peer after our active leaves are updated.
	///
	/// This will prune and update internal state within the peer view and may return a set of
	/// [`StatementFingerprint`]s from our view which are relevant to the peer. It doesn't automatically
	/// send the statements to the peer, as higher-level network-topology should determine
	/// what is actually sent to the peer. Everything returned is guaranteed to pass
	/// `can_send` checks.
	pub fn synchronize_with_our_view(
		&mut self,
		our_view: &View,
	) -> Vec<StatementFingerprint> {
		// No synchronization needed when prospective parachains are
		// disabled for the peer as every leaf is a blank slate.
		if let ProspectiveParachainsMode::Disabled = self.mode {
			return Vec::new()
		}

		// TODO [now]
		// If mode is prospective, then we prune the peer view to only
		// contain candidates matching our own needs. If there are leaves in our
		// view which we previously didn't recognize, then we figure out everything we
		// can now send and send that.
		unimplemented!()
	}

	/// Update a peer's active leaves. This should be called every time the peer
	/// issues a view update over the network.
	///
	/// This will prune and update internal state within the peer view and may return a set of
	/// [`StatementFingerprint`]s from our view which are relevant to the peer. It doesn't automatically
	/// send the statements to the peer, as higher-level network-topology should determine
	/// what is actually sent to the peer. Everything returned is guaranteed to pass
	/// `can_send` checks.
	pub fn handle_view_update(
		&mut self,
		our_view: &View,
		new_active_leaves: ActiveLeavesView,
	) -> Vec<StatementFingerprint> {
		// TODO [now]: update the local active-leaves view.

		// TODO [now]: prune & create fresh entries accordingly in the
		// without-prospective.

		if let ProspectiveParachainsMode::Disabled = self.mode {
			return Vec::new()
		}

		// TODO [now]: For with-prospective, we do a few things:
		// 1. clean up old per-relay-parent / per-active-leaf
		// 2. create new per-relay-parent state only for leaves we have in
		//    common. initialize the depths of sent/received candidates based on
		//    what we know in our view.

		unimplemented!()
	}
}

/// A light fingerprint of a statement.
pub type StatementFingerprint = (CompactStatement, ValidatorIndex);

/// Whether a leaf has prospective parachains enabled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProspectiveParachainsMode {
	/// Prospective parachains are enabled at the leaf.
	Enabled,
	/// Prospective parachains are disabled at the leaf.
	Disabled,
}

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
pub async fn handle_view_active_leaves_update<Context>(
	ctx: &mut Context,
	runtime: &mut RuntimeInfo,
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
					view.with_prospective.implicit_view_mut().activate_leaf(ctx.sender(), leaf_hash).await,
				),
			},
		))
	} else {
		None
	};

	for deactivated in update.deactivated {
		view.without_prospective.deactivate_leaf(&deactivated);
		// TODO [now] clean up implicit view
	}

	// Get relay parents which might be fresh but might be known already
	// that are explicit or implicit from the new active leaf.
	let fresh_relay_parents = match res {
		None => return Ok(()),
		Some((leaf, LeafHasProspectiveParachains::Disabled)) => {
			// defensive in this case - for enabled, this manifests as an error.
			if view.without_prospective.contains(&leaf.hash) {
				return Ok(())
			}

			vec![(leaf.hash, ProspectiveParachainsMode::Disabled)]
		},
		Some((leaf, LeafHasProspectiveParachains::Enabled(Ok(_)))) => {
			// TODO [now]: discover fresh relay parents, clean up old candidates,
			// etc.
			unimplemented!();
		},
		Some((leaf, LeafHasProspectiveParachains::Enabled(Err(e)))) =>
			return Err(Error::ImplicitViewFetchError(leaf.hash, e)),
	};

	for (relay_parent, mode) in fresh_relay_parents {
		match mode {
			ProspectiveParachainsMode::Enabled => {
				// TODO [now]
				unimplemented!()
			}
			ProspectiveParachainsMode::Disabled => {
				let relay_parent_info = construct_per_relay_parent_info_without_prospective(
					ctx,
					runtime,
					relay_parent,
				).await?;

				view.without_prospective.activate_leaf(relay_parent, relay_parent_info);
			}
		}
	}
	Ok(())
}

#[overseer::contextbounds(StatementDistribution, prefix = self::overseer)]
async fn construct_per_relay_parent_info_without_prospective<Context>(
	ctx: &mut Context,
	runtime: &mut RuntimeInfo,
	relay_parent: Hash,
) -> Result<without_prospective::RelayParentInfo, Error> {
	let span = Span::new(&relay_parent, "statement-distribution-no-prospective");

	// Retrieve the parachain validators at the child of the head we track.
	let session_index = runtime.get_session_index_for_child(ctx.sender(), relay_parent).await?;
	let info = runtime
		.get_session_info_by_index(ctx.sender(), relay_parent, session_index)
		.await?;
	let session_info = &info.session_info;

	let valid_pvds = fetch_allowed_pvds_without_prospective(ctx, relay_parent).await?;

	Ok(without_prospective::RelayParentInfo::new(
		session_info.validators.clone(),
		session_index,
		valid_pvds,
		span,
	))
}

#[overseer::contextbounds(StatementDistribution, prefix = self::overseer)]
async fn fetch_allowed_pvds_without_prospective<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
) -> Result<HashMap<ParaId, PersistedValidationData>, Error> {
	// Load availability cores
	let availability_cores = {
		let (tx, rx) = oneshot::channel();
		ctx.send_message(RuntimeApiMessage::Request(
			relay_parent,
			RuntimeApiRequest::AvailabilityCores(tx),
		))
		.await;

		let cores = runtime::recv_runtime(rx).await?;
		cores
	};

	// determine persisted validation data for
	// all parachains at this relay-parent.
	let mut valid_pvds = HashMap::new();
	let mut responses = FuturesUnordered::new();
	for core in availability_cores {
		let para_id = match core.para_id() {
			Some(p) => p,
			None => continue,
		};

		let assumption = match core.is_occupied() {
			true => {
				// note - this means that we'll reject candidates
				// building on top of a timed-out assumption but
				// that rarely happens in practice. limit code
				// complexity at the cost of adding an extra block's
				// wait time for backing after a timeout (same as status quo).
				OccupiedCoreAssumption::Included
			},
			false => OccupiedCoreAssumption::Free,
		};

		let (tx, rx) = oneshot::channel();

		ctx.send_message(RuntimeApiMessage::Request(
			relay_parent,
			RuntimeApiRequest::PersistedValidationData(para_id, assumption, tx),
		))
		.await;

		responses.push(runtime::recv_runtime(rx).map_ok(move |pvd| (para_id, pvd)));
	}

	while let Some(res) = responses.next().await {
		let (para_id, maybe_pvd) = res?;
		match maybe_pvd {
			None => gum::warn!(
				target: LOG_TARGET,
				?relay_parent,
				?para_id,
				"Potential runtime issue: ccupied core assumption lead to no PVD from runtime API"
			),
			Some(pvd) => {
				let _ = valid_pvds.insert(para_id, pvd);
			},
		}
	}

	Ok(valid_pvds)
}

// A value used for comparison of stored statements to each other.
//
// The compact version of the statement, the validator index, and the signature of the validator
// is enough to differentiate between all types of equivocations, as long as the signature is
// actually checked to be valid. The same statement with 2 signatures and 2 statements with
// different (or same) signatures wll all be correctly judged to be unequal with this comparator.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
struct StoredStatementComparator {
	compact: CompactStatement,
	validator_index: ValidatorIndex,
	signature: ValidatorSignature,
}

impl<'a> From<(&'a StoredStatementComparator, &'a SignedFullStatement)> for StoredStatement<'a> {
	fn from(
		(comparator, statement): (&'a StoredStatementComparator, &'a SignedFullStatement),
	) -> Self {
		Self { comparator, statement }
	}
}

// A statement stored while a relay chain head is active.
#[derive(Debug, Copy, Clone)]
struct StoredStatement<'a> {
	comparator: &'a StoredStatementComparator,
	statement: &'a SignedFullStatement,
}

impl<'a> StoredStatement<'a> {
	fn compact(&self) -> &'a CompactStatement {
		&self.comparator.compact
	}

	fn fingerprint(&self) -> (CompactStatement, ValidatorIndex) {
		(self.comparator.compact.clone(), self.statement.validator_index())
	}
}
