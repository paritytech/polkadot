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
use polkadot_node_subsystem::PerLeafSpan;
use polkadot_node_subsystem_util::backing_implicit_view::View as ImplicitView;
use polkadot_primitives::v2::{
	CandidateHash, CommittedCandidateReceipt, CompactStatement, Hash, UncheckedSignedStatement,
	ValidatorId, ValidatorIndex, ValidatorSignature,
};

use std::collections::{HashMap, HashSet};

use crate::{LOG_TARGET, VC_THRESHOLD};

mod without_prospective;

/// The local node's view of the protocol state and messages.
pub struct View {
	implicit_view: ImplicitView,
	per_leaf: HashMap<Hash, LeafData>,
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

struct LeafData {
	mode: ProspectiveParachainsMode,
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
