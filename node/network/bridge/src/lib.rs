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

//! The Network Bridge Subsystem - protocol multiplexer for Polkadot.
//!
//! Split into incoming (`..In`) and outgoing (`..Out`) subsystems.

#![deny(unused_crate_dependencies)]
#![warn(missing_docs)]

use always_assert::never;
use bytes::Bytes;
use futures::{prelude::*, stream::BoxStream};
use parity_scale_codec::{Decode, DecodeAll, Encode};
use parking_lot::Mutex;
use sc_network::Event as NetworkEvent;
use sp_consensus::SyncOracle;

use polkadot_node_network_protocol::{
	self as net_protocol,
	peer_set::{PeerSet, PerPeerSet},
	v1 as protocol_v1, ObservedRole, OurView, PeerId, ProtocolVersion,
	UnifiedReputationChange as Rep, Versioned, View,
};

use polkadot_node_subsystem::{
	errors::{SubsystemError, SubsystemResult},
	messages::{
		network_bridge_event::{NewGossipTopology, TopologyPeerInfo},
		ApprovalDistributionMessage, BitfieldDistributionMessage, CollatorProtocolMessage,
		GossipSupportMessage, NetworkBridgeEvent, NetworkBridgeInMessage, NetworkBridgeMessage,
		StatementDistributionMessage,
	},
	overseer, ActivatedLeaf, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, SpawnedSubsystem,
};
use polkadot_overseer::OverseerError;
use polkadot_primitives::v2::{AuthorityDiscoveryId, BlockNumber, Hash, ValidatorIndex};

/// Peer set info for network initialization.
///
/// To be added to [`NetworkConfiguration::extra_sets`].
pub use polkadot_node_network_protocol::peer_set::{peer_sets_info, IsAuthority};

use std::{
	collections::{hash_map, HashMap},
	iter::ExactSizeIterator,
	sync::Arc,
};

mod validator_discovery;

/// Actual interfacing to the network based on the `Network` trait.
///
/// Defines the `Network` trait with an implementation for an `Arc<NetworkService>`.
mod network;
use self::network::{send_message, Network};

use crate::network::get_peer_id_by_authority_id;

mod metrics;
use self::metrics::Metrics;

mod errors;
pub(crate) use self::errors::{Error, FatalError, JfyiError};

mod incoming;
pub use self::incoming::*;

mod outgoing;
pub use self::outgoing::*;

/// The maximum amount of heads a peer is allowed to have in their view at any time.
///
/// We use the same limit to compute the view sent to peers locally.
pub(crate) const MAX_VIEW_HEADS: usize = 5;

pub(crate) const MALFORMED_MESSAGE_COST: Rep = Rep::CostMajor("Malformed Network-bridge message");
pub(crate) const UNCONNECTED_PEERSET_COST: Rep =
	Rep::CostMinor("Message sent to un-connected peer-set");
pub(crate) const MALFORMED_VIEW_COST: Rep = Rep::CostMajor("Malformed view");
pub(crate) const EMPTY_VIEW_COST: Rep = Rep::CostMajor("Peer sent us an empty view");

// network bridge log target
pub(crate) const LOG_TARGET: &'static str = "parachain::network-bridge";

/// Messages from and to the network.
///
/// As transmitted to and received from subsystems.
#[derive(Debug, Encode, Decode, Clone)]
pub(crate) enum WireMessage<M> {
	/// A message from a peer on a specific protocol.
	#[codec(index = 1)]
	ProtocolMessage(M),
	/// A view update from a peer.
	#[codec(index = 2)]
	ViewUpdate(View),
}

pub(crate) struct PeerData {
	/// The Latest view sent by the peer.
	view: View,
	version: ProtocolVersion,
}

/// Shared state between incoming and outgoing.

#[derive(Default, Clone)]
pub struct Shared(Arc<Mutex<SharedInner>>);

#[derive(Default)]
struct SharedInner {
	local_view: Option<View>,
	validation_peers: HashMap<PeerId, PeerData>,
	collation_peers: HashMap<PeerId, PeerData>,
}

pub(crate) enum Mode {
	Syncing(Box<dyn SyncOracle + Send>),
	Active,
}
