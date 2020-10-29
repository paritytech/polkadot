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

const PRIORITY_GROUP: &'static str = "parachain_validators";

/// An abstraction over networking for the purposes of validator discovery service.
#[async_trait]
pub trait Network: Send + 'static {
	/// Ask the network to connect to these nodes and not disconnect from them until removed from the priority group.
	async fn add_to_priority_group(&mut self, group_id: String, multiaddresses: HashSet<Multiaddr>) -> Result<(), String>;
	/// Remove the peers from the priority group.
	async fn remove_from_priority_group(&mut self, group_id: String, multiaddresses: HashSet<Multiaddr>) -> Result<(), String>;
}

/// An abstraction over the authority discovery service.
#[async_trait]
pub trait AuthorityDiscovery: Send + 'static {
	/// Get the addresses for the given [`AuthorityId`] from the local address cache.
	async fn get_addresses_by_authority_id(&mut self, authority: AuthorityDiscoveryId) -> Option<Vec<Multiaddr>>;
	/// Get the [`AuthorityId`] for the given [`PeerId`] from the local address cache.
	async fn get_authority_id_by_peer_id(&mut self, peer_id: PeerId) -> Option<AuthorityDiscoveryId>;
}

#[async_trait]
impl Network for Arc<sc_network::NetworkService<Block, Hash>> {
	async fn add_to_priority_group(&mut self, group_id: String, multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
		sc_network::NetworkService::add_to_priority_group(&**self, group_id, multiaddresses).await
	}

	async fn remove_from_priority_group(&mut self, group_id: String, multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
		sc_network::NetworkService::remove_from_priority_group(&**self, group_id, multiaddresses).await
	}
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
	pending: HashSet<AuthorityDiscoveryId>,
	sender: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
	revoke: oneshot::Receiver<()>,
}

impl NonRevokedConnectionRequestState {
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
	non_revoked_discovery_requests: Vec<NonRevokedConnectionRequestState>,
	// PhantomData used to make the struct generic instead of having generic methods
	network: PhantomData<N>,
	authority_discovery: PhantomData<AD>,
}

impl<N: Network, AD: AuthorityDiscovery> Service<N, AD> {
	pub fn new() -> Self {
		Self {
			connected_validators: HashMap::new(),
			requested_validators: HashMap::new(),
			non_revoked_discovery_requests: Vec::new(),
			network: PhantomData,
			authority_discovery: PhantomData,
		}
	}

	/// On a new connection request, a priority group update will be issued.
	/// It will ask the network to connect to the validators and not disconnect
	/// from them at least until all the pending requests containing them are revoked.
	///
	/// This method will also clean up all previously revoked requests.
	// it takes `network_service` and `authority_discovery_service` by value
	// and returns them as a workaround for the Future: Send requirement imposed by async fn impl.
	pub async fn on_request(
		&mut self,
		validator_ids: Vec<AuthorityDiscoveryId>,
		mut connected: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
		revoke: oneshot::Receiver<()>,
		mut network_service: N,
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
						let _ = on_revoke(&mut self.requested_validators, peer_id);
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
		let mut multiaddr_to_add = HashSet::new();
		for authority in validator_ids.iter().cloned() {
			let result = authority_discovery_service.get_addresses_by_authority_id(authority).await;
			if let Some(addresses) = result {
				// We might have several `PeerId`s per `AuthorityId`
				// depending on the number of sentry nodes,
				// so we limit the max number of sentries per node to connect to.
				// They are going to be removed soon though:
				// https://github.com/paritytech/substrate/issues/6845
				for addr in addresses.into_iter().take(MAX_ADDR_PER_PEER) {
					let _ = multiaddr_to_add.insert(addr);
				}
			}
		}

		// clean up revoked requests
		let mut revoked_indices = Vec::new();
		let mut revoked_validators = Vec::new();
		for (i, maybe_revoked) in self.non_revoked_discovery_requests.iter_mut().enumerate() {
			if maybe_revoked.is_revoked() {
				for id in maybe_revoked.requested() {
					if let Some(id) = on_revoke(&mut self.requested_validators, id.clone()) {
						revoked_validators.push(id);
					}
				}
				revoked_indices.push(i);
			}
		}

		// clean up revoked requests states
		for to_revoke in revoked_indices.into_iter().rev() {
			drop(self.non_revoked_discovery_requests.swap_remove(to_revoke));
		}

		// multiaddresses to remove
		let mut multiaddr_to_remove = HashSet::new();
		for id in revoked_validators.into_iter() {
			let result = authority_discovery_service.get_addresses_by_authority_id(id).await;
			if let Some(addresses) = result {
				for addr in addresses.into_iter().take(MAX_ADDR_PER_PEER) {
					let _ = multiaddr_to_remove.insert(addr);
				}
			}
		}

		// ask the network to connect to these nodes and not disconnect
		// from them until removed from the priority group
		if let Err(e) = network_service.add_to_priority_group(
			PRIORITY_GROUP.to_owned(),
			multiaddr_to_add,
		).await {
			log::warn!(target: super::TARGET, "AuthorityDiscoveryService returned an invalid multiaddress: {}", e);
		}
		// the addresses are known to be valid
		let _ = network_service.remove_from_priority_group(PRIORITY_GROUP.to_owned(), multiaddr_to_remove).await;

		let pending = validator_ids.iter()
			.cloned()
			.filter(|id| !self.connected_validators.contains_key(id))
			.collect::<HashSet<_>>();

		self.non_revoked_discovery_requests.push(NonRevokedConnectionRequestState::new(
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
			for request in self.non_revoked_discovery_requests.iter_mut() {
				let _ = request.on_authority_connected(&authority, peer_id);
			}
			let _ = self.connected_validators.insert(authority, peer_id.clone());
		}
	}

	pub async fn on_peer_disconnected(&mut self, peer_id: &PeerId, authority_discovery_service: &mut AD) {
		let maybe_authority = authority_discovery_service.get_authority_id_by_peer_id(peer_id.clone()).await;
		if let Some(authority) = maybe_authority {
			let _ = self.connected_validators.remove(&authority);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use futures::stream::StreamExt as _;

	use sp_keyring::Sr25519Keyring;


	fn new_service() -> Service<TestNetwork, TestAuthorityDiscovery> {
		Service::new()
	}

	fn new_network() -> (TestNetwork, TestAuthorityDiscovery) {
		(TestNetwork::default(), TestAuthorityDiscovery::new())
	}

	#[derive(Default)]
	struct TestNetwork {
		priority_group: HashSet<Multiaddr>,
	}

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
		async fn add_to_priority_group(&mut self, _group_id: String, multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
			self.priority_group.extend(multiaddresses.into_iter());
			Ok(())
		}

		async fn remove_from_priority_group(&mut self, _group_id: String, multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
			self.priority_group.retain(|elem| !multiaddresses.contains(elem));
			Ok(())
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
	fn request_is_revoked_on_send() {
		let (revoke_tx, revoke_rx) = oneshot::channel();
		let (sender, _receiver) = mpsc::channel(0);

		let mut request = NonRevokedConnectionRequestState::new(
			Vec::new(),
			HashSet::new(),
			sender,
			revoke_rx,
		);

		assert!(!request.is_revoked());

		revoke_tx.send(()).unwrap();

		assert!(request.is_revoked());
	}

	#[test]
	fn request_is_revoked_when_the_sender_is_dropped() {
		let (revoke_tx, revoke_rx) = oneshot::channel();
		let (sender, _receiver) = mpsc::channel(0);

		let mut request = NonRevokedConnectionRequestState::new(
			Vec::new(),
			HashSet::new(),
			sender,
			revoke_rx,
		);

		assert!(!request.is_revoked());

		drop(revoke_tx);

		assert!(request.is_revoked());
	}

	#[test]
	fn requests_are_fulfilled_immediately_for_already_connected_peers() {
		let mut service = new_service();

		let (ns, mut ads) = new_network();

		let peer_ids: Vec<_> = ads.by_peer_id.keys().cloned().collect();
		let authority_ids: Vec<_> = ads.by_peer_id.values().cloned().collect();

		futures::executor::block_on(async move {
			let req1 = vec![authority_ids[0].clone(), authority_ids[1].clone()];
			let (sender, mut receiver) = mpsc::channel(2);
			let (_revoke_tx, revoke_rx) = oneshot::channel();

			service.on_peer_connected(&peer_ids[0], &mut ads).await;

			let _ = service.on_request(
				req1,
				sender,
				revoke_rx,
				ns,
				ads,
			).await;


			// the results should be immediately available
			let reply1 = receiver.next().await.unwrap();
			assert_eq!(reply1.0, authority_ids[0]);
			assert_eq!(reply1.1, peer_ids[0]);
		});
	}

	#[test]
	fn requests_are_fulfilled_on_peer_connection() {
		let mut service = new_service();

		let (ns, ads) = new_network();

		let peer_ids: Vec<_> = ads.by_peer_id.keys().cloned().collect();
		let authority_ids: Vec<_> = ads.by_peer_id.values().cloned().collect();

		futures::executor::block_on(async move {
			let req1 = vec![authority_ids[0].clone(), authority_ids[1].clone()];
			let (sender, mut receiver) = mpsc::channel(2);
			let (_revoke_tx, revoke_rx) = oneshot::channel();

			let (_, mut ads) = service.on_request(
				req1,
				sender,
				revoke_rx,
				ns,
				ads,
			).await;


			service.on_peer_connected(&peer_ids[0], &mut ads).await;
			let reply1 = receiver.next().await.unwrap();
			assert_eq!(reply1.0, authority_ids[0]);
			assert_eq!(reply1.1, peer_ids[0]);

			service.on_peer_connected(&peer_ids[1], &mut ads).await;
			let reply2 = receiver.next().await.unwrap();
			assert_eq!(reply2.0, authority_ids[1]);
			assert_eq!(reply2.1, peer_ids[1]);
		});
	}

	// Test cleanup works.
	#[test]
	fn requests_are_removed_on_revoke() {
		let mut service = new_service();

		let (ns, mut ads) = new_network();

		let peer_ids: Vec<_> = ads.by_peer_id.keys().cloned().collect();
		let authority_ids: Vec<_> = ads.by_peer_id.values().cloned().collect();

		futures::executor::block_on(async move {
			let (sender, mut receiver) = mpsc::channel(1);
			let (revoke_tx, revoke_rx) = oneshot::channel();

			service.on_peer_connected(&peer_ids[0], &mut ads).await;
			service.on_peer_connected(&peer_ids[1], &mut ads).await;

			let (ns, ads) = service.on_request(
				vec![authority_ids[0].clone()],
				sender,
				revoke_rx,
				ns,
				ads,
			).await;

			let _ = receiver.next().await.unwrap();
			// revoke the request
			revoke_tx.send(()).unwrap();

			let (sender, mut receiver) = mpsc::channel(1);
			let (_revoke_tx, revoke_rx) = oneshot::channel();

			let _ = service.on_request(
				vec![authority_ids[1].clone()],
				sender,
				revoke_rx,
				ns,
				ads,
			).await;

			let reply = receiver.next().await.unwrap();
			assert_eq!(reply.0, authority_ids[1]);
			assert_eq!(reply.1, peer_ids[1]);
			assert_eq!(service.non_revoked_discovery_requests.len(), 1);
		});
	}

	// More complex test with overlapping revoked requests
	#[test]
	fn revoking_requests_with_overlapping_validator_sets() {
		let mut service = new_service();

		let (ns, mut ads) = new_network();

		let peer_ids: Vec<_> = ads.by_peer_id.keys().cloned().collect();
		let authority_ids: Vec<_> = ads.by_peer_id.values().cloned().collect();

		futures::executor::block_on(async move {
			let (sender, mut receiver) = mpsc::channel(1);
			let (revoke_tx, revoke_rx) = oneshot::channel();

			service.on_peer_connected(&peer_ids[0], &mut ads).await;
			service.on_peer_connected(&peer_ids[1], &mut ads).await;

			let (ns, ads) = service.on_request(
				vec![authority_ids[0].clone(), authority_ids[2].clone()],
				sender,
				revoke_rx,
				ns,
				ads,
			).await;

			let _ = receiver.next().await.unwrap();
			// revoke the first request
			revoke_tx.send(()).unwrap();

			let (sender, mut receiver) = mpsc::channel(1);
			let (revoke_tx, revoke_rx) = oneshot::channel();

			let (ns, ads) = service.on_request(
				vec![authority_ids[0].clone(), authority_ids[1].clone()],
				sender,
				revoke_rx,
				ns,
				ads,
			).await;

			let _ = receiver.next().await.unwrap();
			assert_eq!(service.non_revoked_discovery_requests.len(), 1);
			assert_eq!(ns.priority_group.len(), 2);

			// revoke the second request
			revoke_tx.send(()).unwrap();

			let (sender, mut receiver) = mpsc::channel(1);
			let (_revoke_tx, revoke_rx) = oneshot::channel();

			let (ns, _) = service.on_request(
				vec![authority_ids[0].clone()],
				sender,
				revoke_rx,
				ns,
				ads,
			).await;

			let _ = receiver.next().await.unwrap();
			assert_eq!(service.non_revoked_discovery_requests.len(), 1);
			assert_eq!(ns.priority_group.len(), 1);
		});
	}
}
