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

use futures::channel::oneshot;

use sc_network::multiaddr::{self, Multiaddr};

pub use polkadot_node_network_protocol::authority_discovery::AuthorityDiscovery;
use polkadot_node_network_protocol::{
	peer_set::{PeerSet, PeerSetProtocolNames, PerPeerSet},
	PeerId,
};
use polkadot_primitives::v2::AuthorityDiscoveryId;

const LOG_TARGET: &str = "parachain::validator-discovery";

pub(super) struct Service<N, AD> {
	state: PerPeerSet<StatePerPeerSet>,
	peerset_protocol_names: PeerSetProtocolNames,
	// PhantomData used to make the struct generic instead of having generic methods
	_phantom: PhantomData<(N, AD)>,
}

#[derive(Default)]
struct StatePerPeerSet {
	previously_requested: HashSet<PeerId>,
}

impl<N: Network, AD: AuthorityDiscovery> Service<N, AD> {
	pub fn new(peerset_protocol_names: PeerSetProtocolNames) -> Self {
		Self { state: Default::default(), peerset_protocol_names, _phantom: PhantomData }
	}

	/// Connect to already resolved addresses.
	pub async fn on_resolved_request(
		&mut self,
		newly_requested: HashSet<Multiaddr>,
		peer_set: PeerSet,
		mut network_service: N,
	) -> N {
		let state = &mut self.state[peer_set];
		let new_peer_ids: HashSet<PeerId> = extract_peer_ids(newly_requested.iter().cloned());
		let num_peers = new_peer_ids.len();

		let peers_to_remove: Vec<PeerId> =
			state.previously_requested.difference(&new_peer_ids).cloned().collect();
		let removed = peers_to_remove.len();
		state.previously_requested = new_peer_ids;

		gum::debug!(
			target: LOG_TARGET,
			?peer_set,
			?num_peers,
			?removed,
			"New ConnectToValidators resolved request",
		);
		// ask the network to connect to these nodes and not disconnect
		// from them until removed from the set
		//
		// for peer-set management, the main protocol name should be used regardless of
		// the negotiated version.
		if let Err(e) = network_service
			.set_reserved_peers(
				self.peerset_protocol_names.get_main_name(peer_set),
				newly_requested,
			)
			.await
		{
			gum::warn!(target: LOG_TARGET, err = ?e, "AuthorityDiscoveryService returned an invalid multiaddress");
		}
		// the addresses are known to be valid
		//
		// for peer-set management, the main protocol name should be used regardless of
		// the negotiated version.
		let _ = network_service
			.remove_from_peers_set(
				self.peerset_protocol_names.get_main_name(peer_set),
				peers_to_remove,
			)
			.await;

		network_service
	}

	/// On a new connection request, a peer set update will be issued.
	/// It will ask the network to connect to the validators and not disconnect
	/// from them at least until the next request is issued for the same peer set.
	///
	/// This method will also disconnect from previously connected validators not in the `validator_ids` set.
	/// it takes `network_service` and `authority_discovery_service` by value
	/// and returns them as a workaround for the Future: Send requirement imposed by async function implementation.
	pub async fn on_request(
		&mut self,
		validator_ids: Vec<AuthorityDiscoveryId>,
		peer_set: PeerSet,
		failed: oneshot::Sender<usize>,
		network_service: N,
		mut authority_discovery_service: AD,
	) -> (N, AD) {
		// collect multiaddress of validators
		let mut failed_to_resolve: usize = 0;
		let mut newly_requested = HashSet::new();
		let requested = validator_ids.len();
		for authority in validator_ids.into_iter() {
			let result = authority_discovery_service
				.get_addresses_by_authority_id(authority.clone())
				.await;
			if let Some(addresses) = result {
				newly_requested.extend(addresses);
			} else {
				failed_to_resolve += 1;
				gum::debug!(
					target: LOG_TARGET,
					"Authority Discovery couldn't resolve {:?}",
					authority
				);
			}
		}

		gum::debug!(
			target: LOG_TARGET,
			?peer_set,
			?requested,
			?failed_to_resolve,
			"New ConnectToValidators request",
		);

		let r = self.on_resolved_request(newly_requested, peer_set, network_service).await;

		let _ = failed.send(failed_to_resolve);

		(r, authority_discovery_service)
	}
}

fn extract_peer_ids(multiaddr: impl Iterator<Item = Multiaddr>) -> HashSet<PeerId> {
	multiaddr
		.filter_map(|mut addr| match addr.pop() {
			Some(multiaddr::Protocol::P2p(key)) => PeerId::from_multihash(key).ok(),
			_ => None,
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::network::Network;

	use async_trait::async_trait;
	use futures::stream::BoxStream;
	use polkadot_node_network_protocol::{
		request_response::{outgoing::Requests, ReqProtocolNames},
		PeerId,
	};
	use polkadot_primitives::v2::Hash;
	use sc_network::{Event as NetworkEvent, IfDisconnected, ProtocolName};
	use sp_keyring::Sr25519Keyring;
	use std::collections::{HashMap, HashSet};

	fn new_service() -> Service<TestNetwork, TestAuthorityDiscovery> {
		let genesis_hash = Hash::repeat_byte(0xff);
		let fork_id = None;
		let protocol_names = PeerSetProtocolNames::new(genesis_hash, fork_id);

		Service::new(protocol_names)
	}

	fn new_network() -> (TestNetwork, TestAuthorityDiscovery) {
		(TestNetwork::default(), TestAuthorityDiscovery::new())
	}

	#[derive(Default, Clone)]
	struct TestNetwork {
		peers_set: HashSet<PeerId>,
	}

	#[derive(Default, Clone, Debug)]
	struct TestAuthorityDiscovery {
		by_authority_id: HashMap<AuthorityDiscoveryId, HashSet<Multiaddr>>,
		by_peer_id: HashMap<PeerId, HashSet<AuthorityDiscoveryId>>,
	}

	impl TestAuthorityDiscovery {
		fn new() -> Self {
			let peer_ids = known_peer_ids();
			let authorities = known_authorities();
			let multiaddr = known_multiaddr().into_iter().zip(peer_ids.iter().cloned()).map(
				|(mut addr, peer_id)| {
					addr.push(multiaddr::Protocol::P2p(peer_id.into()));
					HashSet::from([addr])
				},
			);
			Self {
				by_authority_id: authorities.iter().cloned().zip(multiaddr).collect(),
				by_peer_id: peer_ids
					.into_iter()
					.zip(authorities.into_iter().map(|a| HashSet::from([a])))
					.collect(),
			}
		}
	}

	#[async_trait]
	impl Network for TestNetwork {
		fn event_stream(&mut self) -> BoxStream<'static, NetworkEvent> {
			panic!()
		}

		async fn set_reserved_peers(
			&mut self,
			_protocol: ProtocolName,
			multiaddresses: HashSet<Multiaddr>,
		) -> Result<(), String> {
			self.peers_set = extract_peer_ids(multiaddresses.into_iter());
			Ok(())
		}

		async fn remove_from_peers_set(&mut self, _protocol: ProtocolName, peers: Vec<PeerId>) {
			self.peers_set.retain(|elem| !peers.contains(elem));
		}

		async fn start_request<AD: AuthorityDiscovery>(
			&self,
			_: &mut AD,
			_: Requests,
			_: &ReqProtocolNames,
			_: IfDisconnected,
		) {
		}

		fn report_peer(&self, _: PeerId, _: crate::Rep) {
			panic!()
		}

		fn disconnect_peer(&self, _: PeerId, _: ProtocolName) {
			panic!()
		}

		fn write_notification(&self, _: PeerId, _: ProtocolName, _: Vec<u8>) {
			panic!()
		}
	}

	#[async_trait]
	impl AuthorityDiscovery for TestAuthorityDiscovery {
		async fn get_addresses_by_authority_id(
			&mut self,
			authority: AuthorityDiscoveryId,
		) -> Option<HashSet<Multiaddr>> {
			self.by_authority_id.get(&authority).cloned()
		}

		async fn get_authority_ids_by_peer_id(
			&mut self,
			peer_id: PeerId,
		) -> Option<HashSet<AuthorityDiscoveryId>> {
			self.by_peer_id.get(&peer_id).cloned()
		}
	}

	fn known_authorities() -> Vec<AuthorityDiscoveryId> {
		[Sr25519Keyring::Alice, Sr25519Keyring::Bob, Sr25519Keyring::Charlie]
			.iter()
			.map(|k| k.public().into())
			.collect()
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

		let authority_ids: Vec<_> =
			ads.by_peer_id.values().map(|v| v.iter()).flatten().cloned().collect();

		futures::executor::block_on(async move {
			let (failed, _) = oneshot::channel();
			let (ns, ads) = service
				.on_request(vec![authority_ids[0].clone()], PeerSet::Validation, failed, ns, ads)
				.await;

			let (failed, _) = oneshot::channel();
			let (_, ads) = service
				.on_request(vec![authority_ids[1].clone()], PeerSet::Validation, failed, ns, ads)
				.await;

			let state = &service.state[PeerSet::Validation];
			assert_eq!(state.previously_requested.len(), 1);
			let peer_1 = extract_peer_ids(
				ads.by_authority_id.get(&authority_ids[1]).unwrap().clone().into_iter(),
			)
			.iter()
			.cloned()
			.next()
			.unwrap();
			assert!(state.previously_requested.contains(&peer_1));
		});
	}

	#[test]
	fn failed_resolution_is_reported_properly() {
		let mut service = new_service();

		let (ns, ads) = new_network();

		let authority_ids: Vec<_> =
			ads.by_peer_id.values().map(|v| v.iter()).flatten().cloned().collect();

		futures::executor::block_on(async move {
			let (failed, failed_rx) = oneshot::channel();
			let unknown = Sr25519Keyring::Ferdie.public().into();
			let (_, ads) = service
				.on_request(
					vec![authority_ids[0].clone(), unknown],
					PeerSet::Validation,
					failed,
					ns,
					ads,
				)
				.await;

			let state = &service.state[PeerSet::Validation];
			assert_eq!(state.previously_requested.len(), 1);
			let peer_0 = extract_peer_ids(
				ads.by_authority_id.get(&authority_ids[0]).unwrap().clone().into_iter(),
			)
			.iter()
			.cloned()
			.next()
			.unwrap();
			assert!(state.previously_requested.contains(&peer_0));

			let failed = failed_rx.await.unwrap();
			assert_eq!(failed, 1);
		});
	}
}
