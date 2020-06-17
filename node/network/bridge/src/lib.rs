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

use parity_scale_codec::{Encode, Decode};

use sc_network::{
	ObservedRole, ReputationChange, PeerId, config::ProtocolId as SubstrateProtocolId,
};
use sp_runtime::ConsensusEngineId;

use messages::{NetworkBridgeEvent, NetworkBridgeMessage};
use overseer::{Subsystem, SubsystemContext, SpawnedSubsystem};
use node_primitives::{ProtocolId, View};
use polkadot_primitives::{Block, Hash};

use std::collections::HashMap;
use std::sync::Arc;

const MAX_VIEW_HEADS: usize = 5;

/// The engine ID of the polkadot network protocol.
pub const POLKADOT_ENGINE_ID: ConsensusEngineId = *b"dot2";
/// The protocol name.
pub const POLKADOT_PROTOCOL_NAME: &[u8] = b"/polkadot/2";

/// Messages received on the network.
#[derive(Encode, Decode)]
pub enum Message {
	/// A message from a peer on a specific protocol.
	#[codec(index = "1")]
	ProtocolMessage(ProtocolId, Vec<u8>),
	/// A view update from a peer.
	#[codec(index = "2")]
	ViewUpdate(View),
}

/// The network bridge subsystem.
pub struct NetworkBridge(Arc<sc_network::NetworkService<Block, Hash>>);

impl NetworkBridge {
	/// Create a new network bridge subsystem with underlying network service.
	pub fn new(net_service: Arc<sc_network::NetworkService<Block, Hash>>) -> Self {
		NetworkBridge(net_service)
	}
}

impl Subsystem<NetworkBridgeMessage> for NetworkBridge {
	fn start(&mut self, ctx: SubsystemContext<NetworkBridgeMessage>) -> SpawnedSubsystem {
		unimplemented!();
		// TODO [now]: Spawn substrate-network notifications protocol & event stream.
	}
}

struct PeerData {
	/// Latest view sent by the peer.
	view: View,
}

struct ProtocolHandler {
	peers: HashMap<PeerId, PeerData>,
}
