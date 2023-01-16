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

use futures::prelude::*;
use parity_scale_codec::{Decode, Encode};
use parking_lot::Mutex;

use sp_consensus::SyncOracle;

use polkadot_node_network_protocol::{
	peer_set::{PeerSet, ProtocolVersion},
	PeerId, UnifiedReputationChange as Rep, View,
};

/// Peer set info for network initialization.
///
/// To be added to [`NetworkConfiguration::extra_sets`].
pub use polkadot_node_network_protocol::peer_set::{peer_sets_info, IsAuthority};

use std::{collections::HashMap, sync::Arc};

mod validator_discovery;

/// Actual interfacing to the network based on the `Network` trait.
///
/// Defines the `Network` trait with an implementation for an `Arc<NetworkService>`.
mod network;
use self::network::Network;

mod metrics;
pub use self::metrics::Metrics;

mod errors;
pub(crate) use self::errors::Error;

mod tx;
pub use self::tx::*;

mod rx;
pub use self::rx::*;

/// The maximum amount of heads a peer is allowed to have in their view at any time.
///
/// We use the same limit to compute the view sent to peers locally.
pub(crate) const MAX_VIEW_HEADS: usize = 5;

pub(crate) const MALFORMED_MESSAGE_COST: Rep = Rep::CostMajor("Malformed Network-bridge message");
pub(crate) const UNCONNECTED_PEERSET_COST: Rep =
	Rep::CostMinor("Message sent to un-connected peer-set");
pub(crate) const MALFORMED_VIEW_COST: Rep = Rep::CostMajor("Malformed view");
pub(crate) const EMPTY_VIEW_COST: Rep = Rep::CostMajor("Peer sent us an empty view");

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
pub(crate) struct Shared(Arc<Mutex<SharedInner>>);

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
