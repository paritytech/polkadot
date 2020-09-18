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

//! A validator discovery service for the Network Bridge.

use core::marker::PhantomData;
use std::collections::{HashSet, HashMap, hash_map};
use std::sync::Arc;

use async_trait::async_trait;
use futures::channel::{mpsc, oneshot};

use sc_network::Multiaddr;
use sc_authority_discovery::Service as AuthorityDiscoveryService;
use polkadot_node_network_protocol::PeerId;
use polkadot_primitives::v1::{AuthorityDiscoveryId, Block, Hash};

// TODO (ordian): does it need to be dependent on the PeerSet?
const PRIORITY_GROUP: &'static str = "parachain_validators";

/// An abstraction over networking for the purposes of validator discovery service.
pub trait Network: Send + 'static {
	/// Ask the network to connect to these nodes and not disconnect from them until removed from the priority group.
	fn set_priority_group(&self, group_id: String, multiaddresses: HashSet<Multiaddr>) -> Result<(), String>;
	// TODO (ordian): we might want to add `add_to_priority_group` and `remove_from_priority_group`
}

/// An abstraction over the authority discovery service.
#[async_trait]
pub trait AuthorityDiscovery: Send + 'static {
	/// Get the addresses for the given [`AuthorityId`] from the local address cache.
	async fn get_addresses_by_authority_id(&mut self, authority: AuthorityDiscoveryId) -> Option<Vec<Multiaddr>>;
	/// Get the [`AuthorityId`] for the given [`PeerId`] from the local address cache.
	async fn get_authority_id_by_peer_id(&mut self, peer_id: PeerId) -> Option<AuthorityDiscoveryId>;
}

impl Network for Arc<sc_network::NetworkService<Block, Hash>> {
	fn set_priority_group(&self, group_id: String, multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
		sc_network::NetworkService::set_priority_group(&**self, group_id, multiaddresses)
	}
}

// TODO (ordian): for `Arc<_>`?
#[async_trait]
impl AuthorityDiscovery for AuthorityDiscoveryService {
	async fn get_addresses_by_authority_id(&mut self, authority: AuthorityDiscoveryId) -> Option<Vec<Multiaddr>> {
		AuthorityDiscoveryService::get_addresses_by_authority_id(self, authority).await
	}

	async fn get_authority_id_by_peer_id(&mut self, peer_id: PeerId) -> Option<AuthorityDiscoveryId> {
		AuthorityDiscoveryService::get_authority_id_by_peer_id(self, peer_id).await
	}
}


/// This struct tracks the state for one `ConnectToValidators` request.
struct PendingConnectionRequestState {
	requested: Vec<AuthorityDiscoveryId>,
	pending: HashSet<AuthorityDiscoveryId>,
	sender: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
	revoke: oneshot::Receiver<()>,
}

impl PendingConnectionRequestState {
	/// Create a new instance of `ConnectToValidatorsState`.
	pub fn new(
		requested: Vec<AuthorityDiscoveryId>,
		pending: HashSet<AuthorityDiscoveryId>,
		sender: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
		revoke: oneshot::Receiver<()>,
	) -> Self {
		Self {
			requested,
			pending,
			sender,
			revoke,
		}
	}

	pub fn on_authority_connected(&mut self, authority: &AuthorityDiscoveryId, peer_id: &PeerId) {
		if self.pending.remove(authority) {
			// an error may happen if the request was revoked or
			// the channel's buffer is full, ignoring it is fine
			let _ = self.sender.try_send((authority.clone(), peer_id.clone()));
		}
	}

	pub fn on_authority_disconnected(&mut self, authority: &AuthorityDiscoveryId) {
		// TODO (ordian): what do we actually want?
		let _ = self.pending.remove(authority);
	}

	/// Returns `true` if the request is revoked.
	pub fn is_revoked(&mut self) -> bool {
		self.revoke
			.try_recv()
			.map_or(true, |r| r.is_some())
	}

	pub fn requested(&self) -> &[AuthorityDiscoveryId] {
		self.requested.as_ref()
	}
}


pub(super) struct Service<N, AD> {
	// we assume one PeerId per AuthorityId is enough
	connected_validators: HashMap<AuthorityDiscoveryId, PeerId>,
	// the `u64` counts the number of pending non-revoked requests for this validator
	// note: the validators in this map are not necessarily present
	// in the `connected_validators` map.
	// Invariant: the value > 0 for non-revoked requests.
	requested_validators: HashMap<AuthorityDiscoveryId, u64>,
	// keep for the network priority_group updates
	validator_multiaddresses: HashSet<Multiaddr>,
	pending_discovery_requests: Vec<PendingConnectionRequestState>,
	// PhantomData used to make the struct generic instead of having generic methods
	network: PhantomData<N>,
	authority_discovery: PhantomData<AD>,
}

impl<N: Network, AD: AuthorityDiscovery> Service<N, AD> {
	pub fn new() -> Self {
		Self {
			connected_validators: HashMap::new(),
			requested_validators: HashMap::new(),
			validator_multiaddresses: HashSet::new(),
			pending_discovery_requests: Vec::new(),
			network: PhantomData,
			authority_discovery: PhantomData,
		}
	}

	// this method takes `network_service` and `authority_discovery_service` by value
	// and returns them as a workaround for the Future: Send requirement imposed by async fn impl.
	pub async fn on_request(
		&mut self,
		validator_ids: Vec<AuthorityDiscoveryId>,
		mut connected: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
		revoke: oneshot::Receiver<()>,
		network_service: N,
		mut authority_discovery_service: AD,
	) -> (N, AD) {
		const MAX_ADDR_PER_PEER: usize = 3;

		let already_connected = validator_ids.iter()
			.cloned()
			.filter_map(|id| {
				let counter = self.requested_validators.entry(id.clone()).or_default();
				// if the counter overflows, there is something really wrong going on
				*counter += 1;

				self.connected_validators
					.get(&id)
					.map(|peer| (id, peer.clone()))
			});


		let on_revoke = |map: &mut HashMap<AuthorityDiscoveryId, u64>, id: AuthorityDiscoveryId| -> Option<AuthorityDiscoveryId> {
			match map.entry(id) {
				hash_map::Entry::Occupied(mut entry) => {
					*entry.get_mut() -= 1;
					if *entry.get() == 0 {
						return Some(entry.remove_entry().0);
					}
				}
				hash_map::Entry::Vacant(_) => {
					// should be unreachable
				}
			}
			None
		};

		// try to send already connected peers
		for (id, peer) in already_connected {
			match connected.try_send((id, peer)) {
				Err(e) if e.is_disconnected() => {
					// the request is already revoked
					for peer_id in validator_ids {
						on_revoke(&mut self.requested_validators, peer_id);
					}
					return (network_service, authority_discovery_service);
				}
				Err(_) => {
					// the channel's buffer is full
					// ignore the error, the receiver will miss out some peers
					// but that's fine
					break;
				}
				Ok(()) => continue,
			}
		}

		// collect multiaddress of validators
		for authority in validator_ids.iter().cloned() {
			let result = authority_discovery_service.get_addresses_by_authority_id(authority).await;
			if let Some(addresses) = result {
				// We might have several `PeerId`s per `AuthorityId`
				// depending on the number of sentry nodes,
				// so we limit the max number of sentries per node to connect to.
				// They are going to be removed soon though:
				// https://github.com/paritytech/substrate/issues/6845
				for addr in addresses.into_iter().take(MAX_ADDR_PER_PEER) {
					self.validator_multiaddresses.insert(addr);
				}
			}
		}

		// clean up revoked requests
		let mut revoked_indices = Vec::new();
		let mut revoked_validators = Vec::new();
		for (i, pending) in self.pending_discovery_requests.iter_mut().enumerate() {
			if pending.is_revoked() {
				for id in pending.requested() {
					if let Some(id) = on_revoke(&mut self.requested_validators, id.clone()) {
						revoked_validators.push(id);
					}
				}
				revoked_indices.push(i);
			}
		}

		// clean up pending requests states
		for to_revoke in revoked_indices.into_iter().rev() {
			drop(self.pending_discovery_requests.swap_remove(to_revoke));
		}

		// multiaddresses to remove
		for id in revoked_validators.into_iter() {
			let result = authority_discovery_service.get_addresses_by_authority_id(id).await;
			if let Some(addresses) = result {
				for addr in addresses.into_iter().take(MAX_ADDR_PER_PEER) {
					self.validator_multiaddresses.remove(&addr);
				}
			}
		}

		// ask the network to connect to these nodes and not disconnect
		// from them until removed from the priority group
		// TODO (ordian): this clones the whole set of multaddresses
		// TODO (ordian): use add_to_priority_group for incremental updates?
		if let Err(e) = network_service.set_priority_group(
			PRIORITY_GROUP.to_owned(),
			self.validator_multiaddresses.clone(),
		) {
			log::warn!("NetworkBridge: AuthorityDiscoveryService returned an invalid multiaddress: {}", e);
		}

		let pending = validator_ids.iter()
			.cloned()
			.filter(|id| !self.connected_validators.contains_key(id))
			.collect::<HashSet<_>>();

		self.pending_discovery_requests.push(PendingConnectionRequestState::new(
			validator_ids,
			pending,
			connected,
			revoke,
		));
		(network_service, authority_discovery_service)
	}

	pub async fn on_peer_connected(&mut self, peer_id: &PeerId, authority_discovery_service: &mut AD) {
		// check if it's an authority we've been waiting for
		let maybe_authority = authority_discovery_service.get_authority_id_by_peer_id(peer_id.clone()).await;
		if let Some(authority) = maybe_authority {
			for pending in self.pending_discovery_requests.iter_mut() {
				pending.on_authority_connected(&authority, peer_id);
			}
			self.connected_validators.insert(authority, peer_id.clone());
		}
	}

	pub async fn on_peer_disconnected(&mut self, peer_id: &PeerId, authority_discovery_service: &mut AD) {
		let maybe_authority = authority_discovery_service.get_authority_id_by_peer_id(peer_id.clone()).await;
		if let Some(authority) = maybe_authority {
			for pending in self.pending_discovery_requests.iter_mut() {
				pending.on_authority_disconnected(&authority);
			}
			self.connected_validators.remove(&authority);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Default)]
	struct TestNetwork {
		peers: HashMap<AuthorityDiscoveryId, (Multiaddr, PeerId)>,
	}

	struct TestAuthorityDiscovery {
		by_authority_id: HashMap<AuthorityDiscoveryId, Multiaddr>,
		by_peer_id: HashMap<PeerId, AuthorityDiscoveryId>,
	}

	impl Network for TestNetwork {
		fn set_priority_group(&self, _group_id: String, _multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
			Ok(())
		}
	}

	#[async_trait]
	impl AuthorityDiscovery for TestDiscovery {
		async fn get_addresses_by_authority_id(&mut self, authority: AuthorityDiscoveryId) -> Option<Vec<Multiaddr>> {
			self.by_authority_id.get(authority).cloned().map(|addr| vec![addr])
		}

		async fn get_authority_id_by_peer_id(&mut self, peer_id: PeerId) -> Option<AuthorityDiscoveryId> {
			self.by_peer_id.get(&peer_id).cloned()
		}
	}

	#[test]
	fn it_works() {
		todo!()
	}
}
