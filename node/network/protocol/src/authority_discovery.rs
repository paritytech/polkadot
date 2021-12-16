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

//! Authority discovery service interfacing.

use std::{collections::HashSet, fmt::Debug};

use async_trait::async_trait;

use sc_authority_discovery::Service as AuthorityDiscoveryService;

use polkadot_primitives::v2::AuthorityDiscoveryId;
use sc_network::{Multiaddr, PeerId};

/// An abstraction over the authority discovery service.
///
/// Needed for mocking in tests mostly.
#[async_trait]
pub trait AuthorityDiscovery: Send + Debug + 'static {
	/// Get the addresses for the given [`AuthorityId`] from the local address cache.
	async fn get_addresses_by_authority_id(
		&mut self,
		authority: AuthorityDiscoveryId,
	) -> Option<HashSet<Multiaddr>>;
	/// Get the [`AuthorityId`] for the given [`PeerId`] from the local address cache.
	async fn get_authority_ids_by_peer_id(
		&mut self,
		peer_id: PeerId,
	) -> Option<HashSet<AuthorityDiscoveryId>>;
}

#[async_trait]
impl AuthorityDiscovery for AuthorityDiscoveryService {
	async fn get_addresses_by_authority_id(
		&mut self,
		authority: AuthorityDiscoveryId,
	) -> Option<HashSet<Multiaddr>> {
		AuthorityDiscoveryService::get_addresses_by_authority_id(self, authority).await
	}

	async fn get_authority_ids_by_peer_id(
		&mut self,
		peer_id: PeerId,
	) -> Option<HashSet<AuthorityDiscoveryId>> {
		AuthorityDiscoveryService::get_authority_ids_by_peer_id(self, peer_id).await
	}
}
