// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! All peersets and protocols used for parachains.

use sc_network::config::{NonDefaultSetConfig, NonReservedPeerMode, SetConfig};
use std::{
	borrow::Cow,
	ops::{Index, IndexMut},
};
use strum::{EnumIter, IntoEnumIterator};

/// The peer-sets and thus the protocols which are used for the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum PeerSet {
	/// The validation peer-set is responsible for all messages related to candidate validation and
	/// communication among validators.
	Validation,
	/// The collation peer-set is used for validator<>collator communication.
	Collation,
}

/// Whether a node is an authority or not.
///
/// Peer set configuration gets adjusted accordingly.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum IsAuthority {
	/// Node is authority.
	Yes,
	/// Node is not an authority.
	No,
}

impl PeerSet {
	/// Get `sc_network` peer set configurations for each peerset.
	///
	/// Those should be used in the network configuration to register the protocols with the
	/// network service.
	pub fn get_info(self, is_authority: IsAuthority) -> NonDefaultSetConfig {
		let protocol = self.into_protocol_name();
		let max_notification_size = 100 * 1024;
		let set_config = match self {
			PeerSet::Validation => SetConfig {
				// we allow full nodes to connect to validators for gossip
				// to ensure any `MIN_GOSSIP_PEERS` always include reserved peers
				// we limit the amount of non-reserved slots to be less
				// than `MIN_GOSSIP_PEERS` in total
				in_peers: super::MIN_GOSSIP_PEERS as u32 / 2 - 1,
				out_peers: super::MIN_GOSSIP_PEERS as u32 / 2 - 1,
				reserved_nodes: Vec::new(),
				non_reserved_mode: NonReservedPeerMode::Accept,
			},
			PeerSet::Collation => SetConfig {
				// Non-authority nodes don't need to accept incoming connections on this peer set:
				in_peers: if is_authority == IsAuthority::Yes { 100 } else { 0 },
				out_peers: 0,
				reserved_nodes: Vec::new(),
				non_reserved_mode: if is_authority == IsAuthority::Yes {
					NonReservedPeerMode::Accept
				} else {
					NonReservedPeerMode::Deny
				},
			},
		};
		NonDefaultSetConfig::new(protocol, max_notification_size).with_config(set_config)
	}

	/// Get the protocol name associated with each peer set as static str.
	pub const fn get_protocol_name_static(self) -> &'static str {
		match self {
			PeerSet::Validation => "/polkadot/validation/1",
			PeerSet::Collation => "/polkadot/collation/1",
		}
	}

	/// Convert a peer set into a protocol name as understood by Substrate.
	pub fn into_protocol_name(self) -> Cow<'static, str> {
		self.get_protocol_name_static().into()
	}

	/// Try parsing a protocol name into a peer set.
	pub fn try_from_protocol_name(name: &Cow<'static, str>) -> Option<PeerSet> {
		match name {
			n if n == &PeerSet::Validation.into_protocol_name() => Some(PeerSet::Validation),
			n if n == &PeerSet::Collation.into_protocol_name() => Some(PeerSet::Collation),
			_ => None,
		}
	}
}

/// A small and nifty collection that allows to store data pertaining to each peer set.
#[derive(Debug, Default)]
pub struct PerPeerSet<T> {
	validation: T,
	collation: T,
}

impl<T> Index<PeerSet> for PerPeerSet<T> {
	type Output = T;
	fn index(&self, index: PeerSet) -> &T {
		match index {
			PeerSet::Validation => &self.validation,
			PeerSet::Collation => &self.collation,
		}
	}
}

impl<T> IndexMut<PeerSet> for PerPeerSet<T> {
	fn index_mut(&mut self, index: PeerSet) -> &mut T {
		match index {
			PeerSet::Validation => &mut self.validation,
			PeerSet::Collation => &mut self.collation,
		}
	}
}

/// Get `NonDefaultSetConfig`s for all available peer sets.
///
/// Should be used during network configuration (added to [`NetworkConfiguration::extra_sets`])
/// or shortly after startup to register the protocols with the network service.
pub fn peer_sets_info(is_authority: IsAuthority) -> Vec<NonDefaultSetConfig> {
	PeerSet::iter().map(|s| s.get_info(is_authority)).collect()
}
