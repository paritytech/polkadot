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

use crate::Network;

use core::marker::PhantomData;
use std::collections::{HashSet, HashMap, hash_map};

use async_trait::async_trait;
use futures::channel::oneshot;

use sc_network::multiaddr::Multiaddr;
use sc_authority_discovery::Service as AuthorityDiscoveryService;
use polkadot_node_network_protocol::PeerId;
use polkadot_primitives::v1::AuthorityDiscoveryId;
use polkadot_node_network_protocol::peer_set::{PeerSet, PerPeerSet};

const LOG_TARGET: &str = "parachain::validator-discovery";

/// An abstraction over the authority discovery service.
#[async_trait]
pub trait AuthorityDiscovery: Send + Clone + 'static {
	/// Get the addresses for the given [`AuthorityId`] from the local address cache.
	async fn get_addresses_by_authority_id(&mut self, authority: AuthorityDiscoveryId) -> Option<Vec<Multiaddr>>;
	/// Get the [`AuthorityId`] for the given [`PeerId`] from the local address cache.
	async fn get_authority_id_by_peer_id(&mut self, peer_id: PeerId) -> Option<AuthorityDiscoveryId>;
}

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
struct NonRevokedConnectionRequestState {
	requested: Vec<AuthorityDiscoveryId>,
	keep_alive: oneshot::Receiver<()>,
}

impl NonRevokedConnectionRequestState {
	/// Create a new instance of `ConnectToValidatorsState`.
	pub fn new(
		requested: Vec<AuthorityDiscoveryId>,
		keep_alive: oneshot::Receiver<()>,
	) -> Self {
		Self {
			requested,
			keep_alive,
		}
	}

	/// Returns `true` if the request is revoked.
	pub fn is_revoked(&mut self) -> bool {
		self.keep_alive.try_recv().is_err()
	}

	pub fn requested(&self) -> &[AuthorityDiscoveryId] {
		self.requested.as_ref()
	}
}

/// Will be called by [`Service::on_request`] when a request was revoked.
///
/// Takes the `map` of requested validators and the `id` of the validator that should be revoked.
///
/// Returns `Some(id)` iff the request counter is `0`.
fn on_revoke(map: &mut HashMap<AuthorityDiscoveryId, u64>, id: AuthorityDiscoveryId) -> Option<AuthorityDiscoveryId> {
	if let hash_map::Entry::Occupied(mut entry) = map.entry(id) {
		*entry.get_mut() = entry.get().saturating_sub(1);
		if *entry.get() == 0 {
			return Some(entry.remove_entry().0);
		}
	}

	None
}


pub(super) struct Service<N, AD> {
	state: PerPeerSet<StatePerPeerSet>,
	// PhantomData used to make the struct generic instead of having generic methods
	_phantom: PhantomData<(N, AD)>,
}

#[derive(Default)]
struct StatePerPeerSet {
	// The `u64` counts the number of pending non-revoked requests for this validator
	// note: the validators in this map are not necessarily present
	// in the `connected_validators` map.
	// Invariant: the value > 0 for non-revoked requests.
	requested_validators: HashMap<AuthorityDiscoveryId, u64>,
	non_revoked_discovery_requests: Vec<NonRevokedConnectionRequestState>,
}

impl<N: Network, AD: AuthorityDiscovery> Service<N, AD> {
	pub fn new() -> Self {
		Self {
			state: PerPeerSet::default(),
			_phantom: PhantomData,
		}
	}

	/// On a new connection request, a peer set update will be issued.
	/// It will ask the network to connect to the validators and not disconnect
	/// from them at least until all the pending requests containing them are revoked.
	///
	/// This method will also clean up all previously revoked requests.
	/// it takes `network_service` and `authority_discovery_service` by value
	/// and returns them as a workaround for the Future: Send requirement imposed by async fn impl.
	pub async fn on_request(
		&mut self,
		validator_ids: Vec<AuthorityDiscoveryId>,
		peer_set: PeerSet,
		keep_alive: oneshot::Receiver<()>,
		mut network_service: N,
		mut authority_discovery_service: AD,
	) -> (N, AD) {
		const MAX_ADDR_PER_PEER: usize = 3;

		let state = &mut self.state[peer_set];
		// Increment the counter of how many times the validators were requested.
		validator_ids.iter().for_each(|id| *state.requested_validators.entry(id.clone()).or_default() += 1);

		// collect multiaddress of validators
		let mut multiaddr_to_add = HashSet::new();
		for authority in validator_ids.iter() {
			let result = authority_discovery_service.get_addresses_by_authority_id(authority.clone()).await;
			if let Some(addresses) = result {
				// We might have several `PeerId`s per `AuthorityId`
				multiaddr_to_add.extend(addresses.into_iter().take(MAX_ADDR_PER_PEER));
			} else {
				tracing::debug!(target: LOG_TARGET, "Authority Discovery couldn't resolve {:?}", authority);
			}
		}

		// clean up revoked requests
		let mut revoked_indices = Vec::new();
		let mut revoked_validators = Vec::new();
		for (i, maybe_revoked) in state.non_revoked_discovery_requests.iter_mut().enumerate() {
			if maybe_revoked.is_revoked() {
				for id in maybe_revoked.requested() {
					if let Some(id) = on_revoke(&mut state.requested_validators, id.clone()) {
						revoked_validators.push(id);
					}
				}
				revoked_indices.push(i);
			}
		}

		// clean up revoked requests states
		//
		// note that the `.rev()` here is important to guarantee `swap_remove`
		// doesn't invalidate unprocessed `revoked_indices`
		for to_revoke in revoked_indices.into_iter().rev() {
			drop(state.non_revoked_discovery_requests.swap_remove(to_revoke));
		}

		// multiaddresses to remove
		let mut multiaddr_to_remove = HashSet::new();
		for id in revoked_validators.into_iter() {
			let result = authority_discovery_service.get_addresses_by_authority_id(id.clone()).await;
			if let Some(addresses) = result {
				multiaddr_to_remove.extend(addresses.into_iter());
			} else {
				tracing::debug!(
					target: LOG_TARGET,
					"Authority Discovery couldn't resolve {:?} on cleanup, a leak is possible",
					id,
				);
			}
		}

		// ask the network to connect to these nodes and not disconnect
		// from them until removed from the set
		if let Err(e) = network_service.add_to_peers_set(
			peer_set.into_protocol_name(),
			multiaddr_to_add.clone(),
		).await {
			tracing::warn!(target: LOG_TARGET, err = ?e, "AuthorityDiscoveryService returned an invalid multiaddress");
		}
		// the addresses are known to be valid
		let _ = network_service.remove_from_peers_set(
			peer_set.into_protocol_name(),
			multiaddr_to_remove.clone()
		).await;

		state.non_revoked_discovery_requests.push(NonRevokedConnectionRequestState::new(
			validator_ids,
			keep_alive,
		));

		(network_service, authority_discovery_service)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::network::{Network, NetworkAction};

	use std::{borrow::Cow, pin::Pin};
	use futures::{sink::Sink, stream::BoxStream};
	use sc_network::{Event as NetworkEvent, IfDisconnected};
	use sp_keyring::Sr25519Keyring;
	use polkadot_node_network_protocol::request_response::request::Requests;

	fn new_service() -> Service<TestNetwork, TestAuthorityDiscovery> {
		Service::new()
	}

	fn new_network() -> (TestNetwork, TestAuthorityDiscovery) {
		(TestNetwork::default(), TestAuthorityDiscovery::new())
	}

	#[derive(Default, Clone)]
	struct TestNetwork {
		peers_set: HashSet<Multiaddr>,
	}

	#[derive(Default, Clone)]
	struct TestAuthorityDiscovery {
		by_authority_id: HashMap<AuthorityDiscoveryId, Multiaddr>,
		by_peer_id: HashMap<PeerId, AuthorityDiscoveryId>,
	}

	impl TestAuthorityDiscovery {
		fn new() -> Self {
			let peer_ids = known_peer_ids();
			let authorities = known_authorities();
			let multiaddr = known_multiaddr();
			Self {
				by_authority_id: authorities.iter()
					.cloned()
					.zip(multiaddr.into_iter())
					.collect(),
				by_peer_id: peer_ids.into_iter()
					.zip(authorities.into_iter())
					.collect(),
			}
		}
	}

	#[async_trait]
	impl Network for TestNetwork {
		fn event_stream(&mut self) -> BoxStream<'static, NetworkEvent> {
			panic!()
		}

		async fn add_to_peers_set(&mut self, _protocol: Cow<'static, str>, multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
			self.peers_set.extend(multiaddresses.into_iter());
			Ok(())
		}

		async fn remove_from_peers_set(&mut self, _protocol: Cow<'static, str>, multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
			self.peers_set.retain(|elem| !multiaddresses.contains(elem));
			Ok(())
		}

		fn action_sink<'a>(&'a mut self)
			-> Pin<Box<dyn Sink<NetworkAction, Error = polkadot_subsystem::SubsystemError> + Send + 'a>>
		{
			panic!()
		}

		async fn start_request<AD: AuthorityDiscovery>(&self, _: &mut AD, _: Requests, _: IfDisconnected) {
		}
	}

	#[async_trait]
	impl AuthorityDiscovery for TestAuthorityDiscovery {
		async fn get_addresses_by_authority_id(&mut self, authority: AuthorityDiscoveryId) -> Option<Vec<Multiaddr>> {
			self.by_authority_id.get(&authority).cloned().map(|addr| vec![addr])
		}

		async fn get_authority_id_by_peer_id(&mut self, peer_id: PeerId) -> Option<AuthorityDiscoveryId> {
			self.by_peer_id.get(&peer_id).cloned()
		}
	}

	fn known_authorities() -> Vec<AuthorityDiscoveryId> {
		[
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
		].iter().map(|k| k.public().into()).collect()
	}

	fn known_peer_ids() -> Vec<PeerId> {
		(0..3).map(|_| PeerId::random()).collect()
	}

	fn known_multiaddr() -> Vec<Multiaddr> {
		vec![
			"/ip4/127.0.0.1/tcp/1234".parse().unwrap(),
			"/ip4/127.0.0.1/tcp/1235".parse().unwrap(),
			"/ip4/127.0.0.1/tcp/1236".parse().unwrap(),
		]
	}

	#[test]
	fn request_is_revoked_when_the_receiver_is_dropped() {
		let (keep_alive_handle, keep_alive) = oneshot::channel();

		let mut request = NonRevokedConnectionRequestState::new(
			Vec::new(),
			keep_alive,
		);

		assert!(!request.is_revoked());

		drop(keep_alive_handle);

		assert!(request.is_revoked());
	}

	// Test cleanup works.
	#[test]
	fn requests_are_removed_on_revoke() {
		let mut service = new_service();

		let (ns, ads) = new_network();

		let authority_ids: Vec<_> = ads.by_peer_id.values().cloned().collect();

		futures::executor::block_on(async move {
			let (keep_alive_handle, keep_alive) = oneshot::channel();

			let (ns, ads) = service.on_request(
				vec![authority_ids[0].clone()],
				PeerSet::Validation,
				keep_alive,
				ns,
				ads,
			).await;

			// revoke the request
			drop(keep_alive_handle);

			let (_keep_alive_handle, keep_alive) = oneshot::channel();

			let _ = service.on_request(
				vec![authority_ids[1].clone()],
				PeerSet::Validation,
				keep_alive,
				ns,
				ads,
			).await;

			let state = &service.state[PeerSet::Validation];
			assert_eq!(state.non_revoked_discovery_requests.len(), 1);
		});
	}

	// More complex test with overlapping revoked requests
	#[test]
	fn revoking_requests_with_overlapping_validator_sets() {
		let mut service = new_service();

		let (ns, ads) = new_network();

		let authority_ids: Vec<_> = ads.by_peer_id.values().cloned().collect();

		futures::executor::block_on(async move {
			let (keep_alive_handle, keep_alive) = oneshot::channel();

			let (ns, ads) = service.on_request(
				vec![authority_ids[0].clone(), authority_ids[2].clone()],
				PeerSet::Validation,
				keep_alive,
				ns,
				ads,
			).await;

			// revoke the first request
			drop(keep_alive_handle);

			let (keep_alive_handle, keep_alive) = oneshot::channel();

			let (ns, ads) = service.on_request(
				vec![authority_ids[0].clone(), authority_ids[1].clone()],
				PeerSet::Validation,
				keep_alive,
				ns,
				ads,
			).await;

			let state = &service.state[PeerSet::Validation];
			assert_eq!(state.non_revoked_discovery_requests.len(), 1);
			assert_eq!(ns.peers_set.len(), 2);

			// revoke the second request
			drop(keep_alive_handle);

			let (_keep_alive_handle, keep_alive) = oneshot::channel();

			let (ns, _) = service.on_request(
				vec![authority_ids[0].clone()],
				PeerSet::Validation,
				keep_alive,
				ns,
				ads,
			).await;

			let state = &service.state[PeerSet::Validation];
			assert_eq!(state.non_revoked_discovery_requests.len(), 1);
			assert_eq!(ns.peers_set.len(), 1);
		});
	}
}
