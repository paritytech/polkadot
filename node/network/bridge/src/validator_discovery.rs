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
use std::borrow::Cow;
use std::collections::{HashSet, HashMap, hash_map};
use std::sync::Arc;

use async_trait::async_trait;
use futures::channel::mpsc;

use sc_network::multiaddr::{Multiaddr, Protocol};
use sc_authority_discovery::Service as AuthorityDiscoveryService;
use polkadot_node_network_protocol::PeerId;
use polkadot_primitives::v1::{AuthorityDiscoveryId, Block, Hash};
use polkadot_node_network_protocol::peer_set::{PeerSet, PerPeerSet};

const LOG_TARGET: &str = "validator_discovery";

/// An abstraction over networking for the purposes of validator discovery service.
#[async_trait]
pub trait Network: Send + 'static {
	/// Ask the network to connect to these nodes and not disconnect from them until removed from the priority group.
	async fn add_peers_to_reserved_set(&mut self, protocol: Cow<'static, str>, multiaddresses: HashSet<Multiaddr>) -> Result<(), String>;
	/// Remove the peers from the priority group.
	async fn remove_peers_from_reserved_set(&mut self, protocol: Cow<'static, str>, multiaddresses: HashSet<Multiaddr>) -> Result<(), String>;
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
	async fn add_peers_to_reserved_set(&mut self, protocol: Cow<'static, str>, multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
		sc_network::NetworkService::add_peers_to_reserved_set(&**self, protocol, multiaddresses)
	}

	async fn remove_peers_from_reserved_set(&mut self, protocol: Cow<'static, str>, multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
		sc_network::NetworkService::remove_peers_from_reserved_set(&**self, protocol, multiaddresses)
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
}

impl NonRevokedConnectionRequestState {
	/// Create a new instance of `ConnectToValidatorsState`.
	pub fn new(
		requested: Vec<AuthorityDiscoveryId>,
		pending: HashSet<AuthorityDiscoveryId>,
		sender: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
	) -> Self {
		Self {
			requested,
			pending,
			sender,
		}
	}

	pub fn on_authority_connected(
		&mut self,
		authority: &AuthorityDiscoveryId,
		peer_id: &PeerId,
	) {
		if self.pending.remove(authority) {
			// an error may happen if the request was revoked or
			// the channel's buffer is full, ignoring it is fine
			let _ = self.sender.try_send((authority.clone(), peer_id.clone()));
		}
	}

	/// Returns `true` if the request is revoked.
	pub fn is_revoked(&mut self) -> bool {
		self.sender.is_closed()
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

pub(crate) fn peer_id_from_multiaddr(addr: &Multiaddr) -> Option<PeerId> {
	addr.iter().last().and_then(|protocol| if let Protocol::P2p(multihash) = protocol {
		PeerId::from_multihash(multihash).ok()
	} else {
		None
	})
}


pub(super) struct Service<N, AD> {
	state: PerPeerSet<StatePerPeerSet>,
	// PhantomData used to make the struct generic instead of having generic methods
	_phantom: PhantomData<(N, AD)>,
}

#[derive(Default)]
struct StatePerPeerSet {
	// Peers that are connected to us and authority ids associated to them.
	connected_peers: HashMap<PeerId, HashSet<AuthorityDiscoveryId>>,
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

	/// Find connected validators using the given `validator_ids`.
	///
	/// Returns a [`HashMap`] that contains the found [`AuthorityDiscoveryId`]'s and their associated [`PeerId`]'s.
	#[tracing::instrument(level = "trace", skip(self, authority_discovery_service), fields(subsystem = LOG_TARGET))]
	async fn find_connected_validators(
		&mut self,
		validator_ids: &[AuthorityDiscoveryId],
		peer_set: PeerSet,
		authority_discovery_service: &mut AD,
	) -> HashMap<AuthorityDiscoveryId, PeerId> {
		let mut result = HashMap::new();
		let state = &mut self.state[peer_set];

		for id in validator_ids {
			// First check if we already cached the validator
			if let Some(pid) = state.connected_peers
				.iter()
				.find_map(|(pid, ids)| {
					if ids.contains(&id) {
						Some(pid)
					 } else {
						None
					}
				})
			{
				result.insert(id.clone(), pid.clone());
				continue;
			}

			// If not ask the authority discovery
			if let Some(addresses) = authority_discovery_service.get_addresses_by_authority_id(id.clone()).await {
				for peer_id in addresses.iter().filter_map(peer_id_from_multiaddr) {
					if let Some(ids) = state.connected_peers.get_mut(&peer_id) {
						ids.insert(id.clone());
						result.insert(id.clone(), peer_id);
					}
				}
			}
		}

		result
	}

	/// On a new connection request, a priority group update will be issued.
	/// It will ask the network to connect to the validators and not disconnect
	/// from them at least until all the pending requests containing them are revoked.
	///
	/// This method will also clean up all previously revoked requests.
	/// it takes `network_service` and `authority_discovery_service` by value
	/// and returns them as a workaround for the Future: Send requirement imposed by async fn impl.
	#[tracing::instrument(level = "trace", skip(self, connected, network_service, authority_discovery_service), fields(subsystem = LOG_TARGET))]
	pub async fn on_request(
		&mut self,
		validator_ids: Vec<AuthorityDiscoveryId>,
		peer_set: PeerSet,
		mut connected: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
		mut network_service: N,
		mut authority_discovery_service: AD,
	) -> (N, AD) {
		const MAX_ADDR_PER_PEER: usize = 3;

		let already_connected = self.find_connected_validators(
			&validator_ids,
			peer_set,
			&mut authority_discovery_service,
		).await;

		let state = &mut self.state[peer_set];
		// Increment the counter of how many times the validators were requested.
		validator_ids.iter().for_each(|id| *state.requested_validators.entry(id.clone()).or_default() += 1);

		// try to send already connected peers
		for (id, peer) in already_connected.iter() {
			match connected.try_send((id.clone(), peer.clone())) {
				Err(e) if e.is_disconnected() => {
					// the request is already revoked
					for peer_id in validator_ids {
						let _ = on_revoke(&mut state.requested_validators, peer_id);
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
		if let Err(e) = network_service.add_peers_to_reserved_set(
			peer_set.into_protocol_name(),
			multiaddr_to_add.clone(),
		).await {
			tracing::warn!(target: LOG_TARGET, err = ?e, "AuthorityDiscoveryService returned an invalid multiaddress");
		}
		// the addresses are known to be valid
		let _ = network_service.remove_peers_from_reserved_set(
			peer_set.into_protocol_name(),
			multiaddr_to_remove.clone()
		).await;

		let pending = validator_ids.iter()
			.cloned()
			.filter(|id| !already_connected.contains_key(id))
			.collect::<HashSet<_>>();

		state.non_revoked_discovery_requests.push(NonRevokedConnectionRequestState::new(
			validator_ids,
			pending,
			connected,
		));

		(network_service, authority_discovery_service)
	}

	/// Should be called when a peer connected.
	#[tracing::instrument(level = "trace", skip(self, authority_discovery_service), fields(subsystem = LOG_TARGET))]
	pub async fn on_peer_connected(
		&mut self,
		peer_id: PeerId,
		peer_set: PeerSet,
		authority_discovery_service: &mut AD,
	) {
		let state = &mut self.state[peer_set];
		// check if it's an authority we've been waiting for
		let maybe_authority = authority_discovery_service.get_authority_id_by_peer_id(peer_id.clone()).await;
		if let Some(authority) = maybe_authority {
			for request in state.non_revoked_discovery_requests.iter_mut() {
				let _ = request.on_authority_connected(&authority, &peer_id);
			}

			state.connected_peers.entry(peer_id).or_default().insert(authority);
		} else {
			state.connected_peers.insert(peer_id, Default::default());
		}
	}

	/// Should be called when a peer disconnected.
	pub fn on_peer_disconnected(&mut self, peer_id: &PeerId, peer_set: PeerSet) {
		self.state[peer_set].connected_peers.remove(peer_id);
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
		peers_set: HashSet<Multiaddr>,
	}

	#[derive(Default)]
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
		async fn add_peers_to_reserved_set(&mut self, _protocol: Cow<'static, str>, multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
			self.peers_set.extend(multiaddresses.into_iter());
			Ok(())
		}

		async fn remove_peers_from_reserved_set(&mut self, _protocol: Cow<'static, str>, multiaddresses: HashSet<Multiaddr>) -> Result<(), String> {
			self.peers_set.retain(|elem| !multiaddresses.contains(elem));
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
	fn request_is_revoked_when_the_receiver_is_dropped() {
		let (sender, receiver) = mpsc::channel(0);

		let mut request = NonRevokedConnectionRequestState::new(
			Vec::new(),
			HashSet::new(),
			sender,
		);

		assert!(!request.is_revoked());

		drop(receiver);

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

			service.on_peer_connected(peer_ids[0].clone(), PeerSet::Validation, &mut ads).await;

			let _ = service.on_request(
				req1,
				PeerSet::Validation,
				sender,
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

			let (_, mut ads) = service.on_request(
				req1,
				PeerSet::Validation,
				sender,
				ns,
				ads,
			).await;


			service.on_peer_connected(peer_ids[0].clone(), PeerSet::Validation, &mut ads).await;
			let reply1 = receiver.next().await.unwrap();
			assert_eq!(reply1.0, authority_ids[0]);
			assert_eq!(reply1.1, peer_ids[0]);

			service.on_peer_connected(peer_ids[1].clone(), PeerSet::Validation, &mut ads).await;
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

			service.on_peer_connected(peer_ids[0].clone(), PeerSet::Validation, &mut ads).await;
			service.on_peer_connected(peer_ids[1].clone(), PeerSet::Validation, &mut ads).await;

			let (ns, ads) = service.on_request(
				vec![authority_ids[0].clone()],
				PeerSet::Validation,
				sender,
				ns,
				ads,
			).await;

			let _ = receiver.next().await.unwrap();
			// revoke the request
			drop(receiver);

			let (sender, mut receiver) = mpsc::channel(1);

			let _ = service.on_request(
				vec![authority_ids[1].clone()],
				PeerSet::Validation,
				sender,
				ns,
				ads,
			).await;

			let reply = receiver.next().await.unwrap();
			assert_eq!(reply.0, authority_ids[1]);
			assert_eq!(reply.1, peer_ids[1]);
			let state = &service.state[PeerSet::Validation];
			assert_eq!(state.non_revoked_discovery_requests.len(), 1);
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

			service.on_peer_connected(peer_ids[0].clone(), PeerSet::Validation, &mut ads).await;
			service.on_peer_connected(peer_ids[1].clone(), PeerSet::Validation, &mut ads).await;

			let (ns, ads) = service.on_request(
				vec![authority_ids[0].clone(), authority_ids[2].clone()],
				PeerSet::Validation,
				sender,
				ns,
				ads,
			).await;

			let _ = receiver.next().await.unwrap();
			// revoke the first request
			drop(receiver);

			let (sender, mut receiver) = mpsc::channel(1);

			let (ns, ads) = service.on_request(
				vec![authority_ids[0].clone(), authority_ids[1].clone()],
				PeerSet::Validation,
				sender,
				ns,
				ads,
			).await;

			let _ = receiver.next().await.unwrap();
			let state = &service.state[PeerSet::Validation];
			assert_eq!(state.non_revoked_discovery_requests.len(), 1);
			assert_eq!(ns.peers_set.len(), 2);

			// revoke the second request
			drop(receiver);

			let (sender, mut receiver) = mpsc::channel(1);

			let (ns, _) = service.on_request(
				vec![authority_ids[0].clone()],
				PeerSet::Validation,
				sender,
				ns,
				ads,
			).await;

			let _ = receiver.next().await.unwrap();
			let state = &service.state[PeerSet::Validation];
			assert_eq!(state.non_revoked_discovery_requests.len(), 1);
			assert_eq!(ns.peers_set.len(), 1);
		});
	}

	/// A test for when a validator connects, but the authority discovery not yet knows that the connecting node
	/// is a validator. This can happen for example at startup of a node.
	#[test]
	fn handle_validator_connect_without_authority_discovery_knowing_it() {
		let mut service = new_service();

		let ns = TestNetwork::default();
		let mut ads = TestAuthorityDiscovery::default();

		let validator_peer_id = PeerId::random();
		let validator_id: AuthorityDiscoveryId = Sr25519Keyring::Alice.public().into();

		futures::executor::block_on(async move {
			let (sender, mut receiver) = mpsc::channel(1);

			service.on_peer_connected(validator_peer_id.clone(), PeerSet::Validation, &mut ads).await;

			let address = known_multiaddr()[0].clone().with(Protocol::P2p(validator_peer_id.clone().into()));
			ads.by_peer_id.insert(validator_peer_id.clone(), validator_id.clone());
			ads.by_authority_id.insert(validator_id.clone(), address);

			let _ = service.on_request(
				vec![validator_id.clone()],
				PeerSet::Validation,
				sender,
				ns,
				ads,
			).await;

			assert_eq!((validator_id.clone(), validator_peer_id.clone()), receiver.next().await.unwrap());
			let state = &service.state[PeerSet::Validation];
			assert!(
				state.connected_peers
					.get(&validator_peer_id)
					.unwrap()
					.contains(&validator_id)
			);
		});
	}
}
