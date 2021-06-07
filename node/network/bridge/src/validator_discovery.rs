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
use std::collections::HashSet;

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

pub(super) struct Service<N, AD> {
	state: PerPeerSet<StatePerPeerSet>,
	// PhantomData used to make the struct generic instead of having generic methods
	_phantom: PhantomData<(N, AD)>,
}

#[derive(Default)]
struct StatePerPeerSet {
	previously_requested: HashSet<Multiaddr>,
}

impl<N: Network, AD: AuthorityDiscovery> Service<N, AD> {
	pub fn new() -> Self {
		Self {
			state: Default::default(),
			_phantom: PhantomData,
		}
	}

	/// On a new connection request, a peer set update will be issued.
	/// It will ask the network to connect to the validators and not disconnect
	/// from them at least until the next request is issued for the same peer set.
	///
	/// This method will also disconnect from previously connected validators not in the `validator_ids` set.
	/// it takes `network_service` and `authority_discovery_service` by value
	/// and returns them as a workaround for the Future: Send requirement imposed by async fn impl.
	pub async fn on_request(
		&mut self,
		validator_ids: Vec<AuthorityDiscoveryId>,
		peer_set: PeerSet,
		failed: oneshot::Sender<usize>,
		mut network_service: N,
		mut authority_discovery_service: AD,
	) -> (N, AD) {
		// collect multiaddress of validators
		let mut failed_to_resolve: usize = 0;
		let mut newly_requested = HashSet::new();
		let requested = validator_ids.len();
		for authority in validator_ids.into_iter() {
			let result = authority_discovery_service.get_addresses_by_authority_id(authority.clone()).await;
			if let Some(addresses) = result {
				newly_requested.extend(addresses);
			} else {
				failed_to_resolve += 1;
				tracing::debug!(target: LOG_TARGET, "Authority Discovery couldn't resolve {:?}", authority);
			}
		}

		let state = &mut self.state[peer_set];
		// clean up revoked requests
		let multiaddr_to_remove: HashSet<_> = state.previously_requested
			.difference(&newly_requested)
			.cloned()
			.collect();
		let multiaddr_to_add: HashSet<_> = newly_requested.difference(&state.previously_requested)
			.cloned()
			.collect();
		state.previously_requested = newly_requested;

		tracing::debug!(
			target: LOG_TARGET,
			?peer_set,
			?requested,
			added = multiaddr_to_add.len(),
			removed = multiaddr_to_remove.len(),
			?failed_to_resolve,
			"New ConnectToValidators request",
		);
		// ask the network to connect to these nodes and not disconnect
		// from them until removed from the set
		if let Err(e) = network_service.add_to_peers_set(
			peer_set.into_protocol_name(),
			multiaddr_to_add,
		).await {
			tracing::warn!(target: LOG_TARGET, err = ?e, "AuthorityDiscoveryService returned an invalid multiaddress");
		}
		// the addresses are known to be valid
		let _ = network_service.remove_from_peers_set(
			peer_set.into_protocol_name(),
			multiaddr_to_remove
		).await;

		let _ = failed.send(failed_to_resolve);

		(network_service, authority_discovery_service)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::network::{Network, NetworkAction};

	use std::{borrow::Cow, pin::Pin, collections::HashMap};
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
	// Test cleanup works.
	#[test]
	fn old_multiaddrs_are_removed_on_new_request() {
		let mut service = new_service();

		let (ns, ads) = new_network();

		let authority_ids: Vec<_> = ads.by_peer_id.values().cloned().collect();

		futures::executor::block_on(async move {
			let (failed, _) = oneshot::channel();
			let (ns, ads) = service.on_request(
				vec![authority_ids[0].clone()],
				PeerSet::Validation,
				failed,
				ns,
				ads,
			).await;

			let (failed, _) = oneshot::channel();
			let (_, ads) = service.on_request(
				vec![authority_ids[1].clone()],
				PeerSet::Validation,
				failed,
				ns,
				ads,
			).await;

			let state = &service.state[PeerSet::Validation];
			assert_eq!(state.previously_requested.len(), 1);
			assert!(state.previously_requested.contains(ads.by_authority_id.get(&authority_ids[1]).unwrap()));
		});
	}

	#[test]
	fn failed_resolution_is_reported_properly() {
		let mut service = new_service();

		let (ns, ads) = new_network();

		let authority_ids: Vec<_> = ads.by_peer_id.values().cloned().collect();

		futures::executor::block_on(async move {
			let (failed, failed_rx) = oneshot::channel();
			let unknown = Sr25519Keyring::Ferdie.public().into();
			let (_, ads) = service.on_request(
				vec![authority_ids[0].clone(), unknown],
				PeerSet::Validation,
				failed,
				ns,
				ads,
			).await;

			let state = &service.state[PeerSet::Validation];
			assert_eq!(state.previously_requested.len(), 1);
			assert!(state.previously_requested.contains(ads.by_authority_id.get(&authority_ids[0]).unwrap()));

			let failed = failed_rx.await.unwrap();
			assert_eq!(failed, 1);
		});
	}
}
