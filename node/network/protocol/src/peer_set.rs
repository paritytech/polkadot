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

use super::ProtocolVersion;
use sc_network::config::{NonDefaultSetConfig, SetConfig};
use std::{
	borrow::Cow,
	ops::{Index, IndexMut},
};
use strum::{EnumIter, IntoEnumIterator};

// Only supported protocol versions should be defined here.
const VALIDATION_PROTOCOL_V1: &str = "/polkadot/validation/1";
const COLLATION_PROTOCOL_V1: &str = "/polkadot/collation/1";

/// The default validation protocol version.
pub const DEFAULT_VALIDATION_PROTOCOL_VERSION: ProtocolVersion = 1;

/// The default collation protocol version.
pub const DEFAULT_COLLATION_PROTOCOL_VERSION: ProtocolVersion = 1;

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
	/// Get `sc_network` peer set configurations for each peerset on the default version.
	///
	/// Those should be used in the network configuration to register the protocols with the
	/// network service.
	pub fn get_info(self, is_authority: IsAuthority) -> NonDefaultSetConfig {
		let version = self.get_default_version();
		let protocol = self
			.into_protocol_name(version)
			.expect("default version always has protocol name; qed");
		let max_notification_size = 100 * 1024;

		match self {
			PeerSet::Validation => NonDefaultSetConfig {
				notifications_protocol: protocol,
				fallback_names: Vec::new(),
				max_notification_size,
				set_config: sc_network::config::SetConfig {
					// we allow full nodes to connect to validators for gossip
					// to ensure any `MIN_GOSSIP_PEERS` always include reserved peers
					// we limit the amount of non-reserved slots to be less
					// than `MIN_GOSSIP_PEERS` in total
					in_peers: super::MIN_GOSSIP_PEERS as u32 / 2 - 1,
					out_peers: super::MIN_GOSSIP_PEERS as u32 / 2 - 1,
					reserved_nodes: Vec::new(),
					non_reserved_mode: sc_network::config::NonReservedPeerMode::Accept,
				},
			},
			PeerSet::Collation => NonDefaultSetConfig {
				notifications_protocol: protocol,
				fallback_names: Vec::new(),
				max_notification_size,
				set_config: SetConfig {
					// Non-authority nodes don't need to accept incoming connections on this peer set:
					in_peers: if is_authority == IsAuthority::Yes { 100 } else { 0 },
					out_peers: 0,
					reserved_nodes: Vec::new(),
					non_reserved_mode: if is_authority == IsAuthority::Yes {
						sc_network::config::NonReservedPeerMode::Accept
					} else {
						sc_network::config::NonReservedPeerMode::Deny
					},
				},
			},
		}
	}

	/// Get the default protocol version for this peer set.
	pub const fn get_default_version(self) -> ProtocolVersion {
		match self {
			PeerSet::Validation => DEFAULT_VALIDATION_PROTOCOL_VERSION,
			PeerSet::Collation => DEFAULT_COLLATION_PROTOCOL_VERSION,
		}
	}

	/// Get the default protocol name as a static str.
	pub const fn get_default_protocol_name(self) -> &'static str {
		match self {
			PeerSet::Validation => VALIDATION_PROTOCOL_V1,
			PeerSet::Collation => COLLATION_PROTOCOL_V1,
		}
	}

	/// Get the protocol name associated with each peer set
	/// and the given version, if any, as static str.
	pub const fn get_protocol_name_static(self, version: ProtocolVersion) -> Option<&'static str> {
		match (self, version) {
			(PeerSet::Validation, 1) => Some(VALIDATION_PROTOCOL_V1),
			(PeerSet::Collation, 1) => Some(COLLATION_PROTOCOL_V1),
			_ => None,
		}
	}

	/// Get the protocol name associated with each peer set as understood by Substrate.
	pub fn into_default_protocol_name(self) -> Cow<'static, str> {
		self.get_default_protocol_name().into()
	}

	/// Convert a peer set and the given version into a protocol name, if any,
	/// as understood by Substrate.
	pub fn into_protocol_name(self, version: ProtocolVersion) -> Option<Cow<'static, str>> {
		self.get_protocol_name_static(version).map(|n| n.into())
	}

	/// Try parsing a protocol name into a peer set and protocol version.
	///
	/// This only succeeds on supported versions.
	pub fn try_from_protocol_name(name: &Cow<'static, str>) -> Option<(PeerSet, ProtocolVersion)> {
		match name {
			n if n == VALIDATION_PROTOCOL_V1 => Some((PeerSet::Validation, 1)),
			n if n == COLLATION_PROTOCOL_V1 => Some((PeerSet::Collation, 1)),
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

/// Get `NonDefaultSetConfig`s for all available peer sets, at their default versions.
///
/// Should be used during network configuration (added to [`NetworkConfiguration::extra_sets`])
/// or shortly after startup to register the protocols with the network service.
pub fn peer_sets_info(is_authority: IsAuthority) -> Vec<sc_network::config::NonDefaultSetConfig> {
	PeerSet::iter().map(|s| s.get_info(is_authority)).collect()
}
