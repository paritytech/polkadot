// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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

use std::convert::TryFrom;

pub use sc_network::{ReputationChange, PeerId};
use polkadot_node_network_protocol::{WrongVariant, ObservedRole, OurView, View};

/// Events from network.
#[derive(Debug, Clone, PartialEq)]
pub enum NetworkBridgeEvent<M> {
	/// A peer has connected.
	PeerConnected(PeerId, ObservedRole),

	/// A peer has disconnected.
	PeerDisconnected(PeerId),

	/// Peer has sent a message.
	PeerMessage(PeerId, M),

	/// Peer's `View` has changed.
	PeerViewChange(PeerId, View),

	/// Our view has changed.
	OurViewChange(OurView),
}

impl<M> NetworkBridgeEvent<M> {
	/// Focus an overarching network-bridge event into some more specific variant.
	///
	/// This tries to transform M in `PeerMessage` to a message type specific to a subsystem.
	/// It is used to dispatch events coming from a peer set to the various subsystems that are
	/// handled within that peer set. More concretly a `ValidationProtocol` will be transformed
	/// for example into a `BitfieldDistributionMessage` in case of the `BitfieldDistribution`
	/// constructor.
	///
	/// Therefore a NetworkBridgeEvent<ValidationProtocol> will become for example a
	/// NetworkBridgeEvent<BitfieldDistributionMessage>, with the more specific message type
	/// `BitfieldDistributionMessage`.
	///
	/// This acts as a call to `clone`, except in the case where the event is a message event,
	/// in which case the clone can be expensive and it only clones if the message type can
	/// be focused.
	pub fn focus<'a, T>(&'a self) -> Result<NetworkBridgeEvent<T>, WrongVariant>
		where T: 'a + Clone, &'a T: TryFrom<&'a M, Error = WrongVariant>
	{
		Ok(match *self {
			NetworkBridgeEvent::PeerConnected(ref peer, ref role)
				=> NetworkBridgeEvent::PeerConnected(peer.clone(), role.clone()),
			NetworkBridgeEvent::PeerDisconnected(ref peer)
				=> NetworkBridgeEvent::PeerDisconnected(peer.clone()),
			NetworkBridgeEvent::PeerMessage(ref peer, ref msg)
				=> NetworkBridgeEvent::PeerMessage(peer.clone(), <&'a T>::try_from(msg)?.clone()),
			NetworkBridgeEvent::PeerViewChange(ref peer, ref view)
				=> NetworkBridgeEvent::PeerViewChange(peer.clone(), view.clone()),
			NetworkBridgeEvent::OurViewChange(ref view)
				=> NetworkBridgeEvent::OurViewChange(view.clone()),
		})
	}
}
