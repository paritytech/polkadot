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
use futures::prelude::*;
use futures::stream::BoxStream;

use sc_network::{
	ObservedRole, ReputationChange, PeerId, config::ProtocolId as SubstrateProtocolId,
	Event as NetworkEvent,
};
use sp_runtime::ConsensusEngineId;

use messages::{NetworkBridgeEvent, NetworkBridgeMessage, FromOverseer, OverseerSignal};
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

/// Information about the notifications protocol. Should be used during network configuration
/// or shortly after startup to register the protocol with the network service.
pub fn notifications_protocol_info() -> (ConsensusEngineId, std::borrow::Cow<'static, [u8]>) {
	(POLKADOT_ENGINE_ID, POLKADOT_PROTOCOL_NAME.into())
}

/// An abstraction over networking for the purposes of this subsystem.
pub trait Network: Clone + Send + 'static {
	/// Get a stream of all events occurring on the network. This may include events unrelated
	/// to the Polkadot protocol - the user of this function should filter only for events related
	/// to the [`POLKADOT_ENGINE_ID`](POLKADOT_ENGINE_ID).
	fn event_stream(&self) -> BoxStream<NetworkEvent>;

	/// Report a given peer as either beneficial (+) or costly (-) according to the given scalar.
	fn report_peer(&self, who: PeerId, cost_benefit: ReputationChange);

	/// Write a notification to a peer on the [`POLKADOT_ENGINE_ID`](POLKADOT_ENGINE_ID) topic.
	fn write_notification(&self, who: PeerId, message: Vec<u8>);
}

impl Network for Arc<sc_network::NetworkService<Block, Hash>> {
	fn event_stream(&self) -> BoxStream<NetworkEvent> {
		sc_network::NetworkService::event_stream(self, "polkadot-network-bridge").boxed()
	}

	fn report_peer(&self, who: PeerId, cost_benefit: ReputationChange) {
		sc_network::NetworkService::report_peer(self, who, cost_benefit)
	}

	fn write_notification(&self, who: PeerId, message: Vec<u8>) {
		sc_network::NetworkService::write_notification(self, who, POLKADOT_ENGINE_ID, message)
	}
}

/// The network bridge subsystem.
pub struct NetworkBridge<N>(N);

impl<N> NetworkBridge<N> {
	/// Create a new network bridge subsystem with underlying network service.
	///
	/// This assumes that the network service has had the notifications protocol for the network
	/// bridge already registered. See [`notifications_protocol_info`](notifications_protocol_info).
	pub fn new(net_service: N) -> Self {
		NetworkBridge(net_service)
	}
}

impl<N: Network> Subsystem<NetworkBridgeMessage> for NetworkBridge<N> {
	fn start(&mut self, ctx: SubsystemContext<NetworkBridgeMessage>) -> SpawnedSubsystem {
		SpawnedSubsystem(run_network(self.0.clone(), ctx).boxed())
	}
}

struct PeerData {
	/// Latest view sent by the peer.
	view: View,
}

async fn run_network(net: impl Network, mut ctx: SubsystemContext<NetworkBridgeMessage>) {
	let mut event_stream = net.event_stream().fuse();

	// TODO [now]
	// let peers = HashMap::new();
	// let event_listeners = HashMap::new();

	loop {
		let subsystem_next = ctx.recv().fuse();
		let mut net_event_next = event_stream.next().fuse();
		futures::pin_mut!(subsystem_next);

		futures::select! {
			subsystem_msg = subsystem_next => match subsystem_msg {
				Ok(FromOverseer::Signal(OverseerSignal::StartWork(relay_parent))) => {
					// TODO [now]: update local view and send view update to peers.
				}
				Ok(FromOverseer::Signal(OverseerSignal::StopWork(relay_parent))) => {
					// TODO [now]: update local view and send view update to peers.
				}
				Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => return,
				Ok(FromOverseer::Communication { msg }) => match msg {
					NetworkBridgeMessage::RegisterEventProducer(protocol_id, message_producer) => {
						// TODO [now]: add event producer.
					}
					NetworkBridgeMessage::ReportPeer(peer, rep) => {
						// TODO [now]: report a peer to network service.
					}
					NetworkBridgeMessage::SendMessage(peers, protocol, message) => {
						// TODO [now]: Send the message to all peers with `write_notification`.
					}
				},
				Err(e) => {
					// TODO [now]: log error.
					return;
				}
			},
			net_event = net_event_next => {
				// TODO [now]: Update peer tracker, filter out anything not to do with this
				// engine, and transform all updates to be sent to the overseer.
			},
		}
	}
}
