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

//! Utility function to make it easier to connect to validators.

use std::collections::HashMap;
use std::pin::Pin;

use futures::{
	channel::mpsc,
	task::{Poll, self},
	stream,
	StreamExt,
};
use streamunordered::{StreamUnordered, StreamYield};

use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	messages::{AllMessages, NetworkBridgeMessage},
	SubsystemContext,
};
use polkadot_primitives::v1::{
	Hash, ValidatorId, AuthorityDiscoveryId, SessionIndex, Id as ParaId,
};
use polkadot_node_network_protocol::peer_set::PeerSet;
use sc_network::PeerId;
use crate::Error;

/// Utility function to make it easier to connect to validators.
pub async fn connect_to_validators<Context: SubsystemContext>(
	ctx: &mut Context,
	relay_parent: Hash,
	validators: Vec<ValidatorId>,
	peer_set: PeerSet,
) -> Result<ConnectionRequest, Error> {
	let current_index = crate::request_session_index_for_child_ctx(relay_parent, ctx).await?.await??;
	connect_to_validators_in_session(
		ctx,
		relay_parent,
		validators,
		peer_set,
		current_index,
	).await
}

/// Utility function to make it easier to connect to validators in the given session.
pub async fn connect_to_validators_in_session<Context: SubsystemContext>(
	ctx: &mut Context,
	relay_parent: Hash,
	validators: Vec<ValidatorId>,
	peer_set: PeerSet,
	session_index: SessionIndex,
) -> Result<ConnectionRequest, Error> {
	let session_info = crate::request_session_info_ctx(
		relay_parent,
		session_index,
		ctx,
	).await?.await??;

	let (session_validators, discovery_keys) = match session_info {
		Some(info) => (info.validators, info.discovery_keys),
		None => return Err(RuntimeApiError::from(
			format!("No SessionInfo found for the index {}", session_index)
		).into()),
	};

	tracing::trace!(
		target: "network_bridge",
		validators = ?validators,
		discovery_keys = ?discovery_keys,
		session_index,
		"Trying to serve the validator discovery request",
	);

	let id_to_index = session_validators.iter()
		.zip(0usize..)
		.collect::<HashMap<_, _>>();

	// We assume the same ordering in authorities as in validators so we can do an index search
	let maybe_authorities: Vec<_> = validators.iter()
		.map(|id| {
			let validator_index = id_to_index.get(&id);
			validator_index.and_then(|i| discovery_keys.get(*i).cloned())
		})
		.collect();

	let authorities: Vec<_> = maybe_authorities.iter()
		.cloned()
		.filter_map(|id| id)
		.collect();

	let validator_map = validators.into_iter()
		.zip(maybe_authorities.into_iter())
		.filter_map(|(k, v)| v.map(|v| (v, k)))
		.collect::<HashMap<AuthorityDiscoveryId, ValidatorId>>();

	let connections = connect_to_authorities(ctx, authorities, peer_set).await;

	Ok(ConnectionRequest {
		validator_map,
		connections,
	})
}

async fn connect_to_authorities<Context: SubsystemContext>(
	ctx: &mut Context,
	validator_ids: Vec<AuthorityDiscoveryId>,
	peer_set: PeerSet,
) -> mpsc::Receiver<(AuthorityDiscoveryId, PeerId)> {
	const PEERS_CAPACITY: usize = 32;

	let (connected, connected_rx) = mpsc::channel(PEERS_CAPACITY);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ConnectToValidators {
			validator_ids,
			peer_set,
			connected,
		}
	)).await;

	connected_rx
}

/// Represents a discovered validator.
///
/// Result of [`ConnectionRequests::next`].
#[derive(Debug, PartialEq)]
pub struct DiscoveredValidator {
	/// The relay parent associated with the connection request that returned a result.
	pub relay_parent: Hash,
	/// The para ID associated with the connection request that returned a result.
	pub para_id: ParaId,
	/// The [`ValidatorId`] that was resolved.
	pub validator_id: ValidatorId,
	/// The [`PeerId`] associated to the validator id.
	pub peer_id: PeerId,
}

/// Used by [`ConnectionRequests::requests`] to map a [`ConnectionRequest`] item to a [`DiscoveredValidator`].
struct ConnectionRequestForRelayParentAndParaId {
	request: ConnectionRequest,
	relay_parent: Hash,
	para_id: ParaId,
}

impl stream::Stream for ConnectionRequestForRelayParentAndParaId {
	type Item = DiscoveredValidator;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<Option<Self::Item>> {
		self.request
			.poll_next_unpin(cx)
			.map(|r| r.map(|(validator_id, peer_id)| DiscoveredValidator {
				validator_id,
				peer_id,
				relay_parent: self.relay_parent,
				para_id: self.para_id,
			}))
	}
}

/// A struct that assists performing multiple concurrent connection requests.
///
/// This allows concurrent connections to validator sets at different `(relay_parents, para_id)`.
/// Use [`ConnectionRequests::next`] to wait for results of the added connection requests.
#[derive(Default)]
pub struct ConnectionRequests {
	/// Connection requests relay_parent -> para_id -> StreamUnordered token
	///
	/// Q: Why not (relay_parent, para_id) -> Stream?
	/// A: So that we can remove from it by relay_parent only.
	id_map: HashMap<Hash, HashMap<ParaId, usize>>,

	/// Connection requests themselves.
	requests: StreamUnordered<ConnectionRequestForRelayParentAndParaId>,
}

impl ConnectionRequests {
	/// Insert a new connection request.
	///
	/// If a `ConnectionRequest` under a given `relay_parent` and `para_id` already exists,
	/// it will be revoked and substituted with the given one.
	pub fn put(&mut self, relay_parent: Hash, para_id: ParaId, request: ConnectionRequest) {
		self.remove(&relay_parent, para_id);
		let token = self.requests.push(ConnectionRequestForRelayParentAndParaId {
			relay_parent,
			para_id,
			request,
		 });

		self.id_map.entry(relay_parent).or_default().insert(para_id, token);
	}

	/// Remove all connection requests by a given `relay_parent`.
	pub fn remove_all(&mut self, relay_parent: &Hash) {
		let map = self.id_map.remove(relay_parent);
		for token in map.map(|m| m.into_iter().map(|(_, v)| v)).into_iter().flatten() {
			Pin::new(&mut self.requests).remove(token);
		}
	}

	/// Remove a connection request by a given `relay_parent` and `para_id`.
	pub fn remove(&mut self, relay_parent: &Hash, para_id: ParaId) {
		if let Some(map) = self.id_map.get_mut(relay_parent) {
			if let Some(token) = map.remove(&para_id) {
				Pin::new(&mut self.requests).remove(token);
			}
		}
	}

	/// Is a connection at this relay parent and para_id already present in the request
	pub fn contains_request(&self, relay_parent: &Hash, para_id: ParaId) -> bool {
		self.id_map.get(relay_parent).map_or(false, |map| map.contains_key(&para_id))
	}

	/// Returns the next available connection request result.
	///
	/// # Note
	///
	/// When there are no active requests this will wait indefinitely, like an always pending future.
	pub async fn next(&mut self) -> DiscoveredValidator {
		loop {
			match self.requests.next().await {
				Some((StreamYield::Item(item), _)) => {
					return item
				},
				// Ignore finished requests, they are required to be removed.
				Some((StreamYield::Finished(_), _)) => (),
				None => futures::pending!(),
			}
		}
	}
}

/// A pending connection request to validators.
/// This struct implements `Stream` to allow for asynchronous
/// discovery of validator addresses.
///
/// NOTE: the request will be revoked on drop.
#[must_use = "dropping a request will result in its immediate revokation"]
pub struct ConnectionRequest {
	validator_map: HashMap<AuthorityDiscoveryId, ValidatorId>,
	#[must_use = "streams do nothing unless polled"]
	connections: mpsc::Receiver<(AuthorityDiscoveryId, PeerId)>,
}

impl stream::Stream for ConnectionRequest {
	type Item = (ValidatorId, PeerId);

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<Option<Self::Item>> {
		if self.validator_map.is_empty() {
			return Poll::Ready(None);
		}
		match Pin::new(&mut self.connections).poll_next(cx) {
			Poll::Ready(Some((id, peer_id))) => {
				if let Some(validator_id) = self.validator_map.remove(&id) {
					return Poll::Ready(Some((validator_id, peer_id)));
				} else {
					// unknown authority_id
					// should be unreachable
				}
			}
			_ => {},
		}
		Poll::Pending
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_primitives::v1::ValidatorPair;
	use sp_core::{Pair, Public};

	use futures::{executor, poll, SinkExt};

	async fn check_next_is_pending(connection_requests: &mut ConnectionRequests) {
		let next = connection_requests.next();
		futures::pin_mut!(next);
		assert_eq!(poll!(next), Poll::Pending);
	}

	#[test]
	fn adding_a_connection_request_works() {
		let mut connection_requests = ConnectionRequests::default();

		executor::block_on(async move {
			check_next_is_pending(&mut connection_requests).await;

			let validator_1 = ValidatorPair::generate().0.public();
			let validator_2 = ValidatorPair::generate().0.public();

			let auth_1 = AuthorityDiscoveryId::from_slice(&[1; 32]);
			let auth_2 = AuthorityDiscoveryId::from_slice(&[2; 32]);

			let mut validator_map = HashMap::new();
			validator_map.insert(auth_1.clone(), validator_1.clone());
			validator_map.insert(auth_2.clone(), validator_2.clone());

			let (mut rq1_tx, rq1_rx) = mpsc::channel(8);

			let peer_id_1 = PeerId::random();
			let peer_id_2 = PeerId::random();

			let connection_request_1 = ConnectionRequest {
				validator_map,
				connections: rq1_rx,
			};

			let relay_parent_1 = Hash::repeat_byte(1);
			let para_id = ParaId::from(3);
			connection_requests.put(relay_parent_1.clone(), para_id, connection_request_1);

			rq1_tx.send((auth_1, peer_id_1.clone())).await.unwrap();
			rq1_tx.send((auth_2, peer_id_2.clone())).await.unwrap();

			let res = connection_requests.next().await;
			assert_eq!(
				res,
				DiscoveredValidator { relay_parent: relay_parent_1, para_id, validator_id: validator_1, peer_id: peer_id_1 },
			);

			let res = connection_requests.next().await;
			assert_eq!(
				res,
				DiscoveredValidator { relay_parent: relay_parent_1, para_id, validator_id: validator_2, peer_id: peer_id_2 },
			);

			check_next_is_pending(&mut connection_requests).await;
		});
	}

	#[test]
	fn adding_two_connection_requests_works() {
		let mut connection_requests = ConnectionRequests::default();

		executor::block_on(async move {
			check_next_is_pending(&mut connection_requests).await;

			let validator_1 = ValidatorPair::generate().0.public();
			let validator_2 = ValidatorPair::generate().0.public();

			let auth_1 = AuthorityDiscoveryId::from_slice(&[1; 32]);
			let auth_2 = AuthorityDiscoveryId::from_slice(&[2; 32]);

			let mut validator_map_1 = HashMap::new();
			let mut validator_map_2 = HashMap::new();

			validator_map_1.insert(auth_1.clone(), validator_1.clone());
			validator_map_2.insert(auth_2.clone(), validator_2.clone());

			let (mut rq1_tx, rq1_rx) = mpsc::channel(8);

			let (mut rq2_tx, rq2_rx) = mpsc::channel(8);

			let peer_id_1 = PeerId::random();
			let peer_id_2 = PeerId::random();

			let connection_request_1 = ConnectionRequest {
				validator_map: validator_map_1,
				connections: rq1_rx,
			};

			let connection_request_2 = ConnectionRequest {
				validator_map: validator_map_2,
				connections: rq2_rx,
			};

			let relay_parent_1 = Hash::repeat_byte(1);
			let relay_parent_2 = Hash::repeat_byte(2);
			let para_id = ParaId::from(3);

			connection_requests.put(relay_parent_1.clone(), para_id, connection_request_1);
			connection_requests.put(relay_parent_2.clone(), para_id, connection_request_2);

			rq1_tx.send((auth_1, peer_id_1.clone())).await.unwrap();
			rq2_tx.send((auth_2, peer_id_2.clone())).await.unwrap();

			let res = connection_requests.next().await;
			assert_eq!(
				res,
				DiscoveredValidator { relay_parent: relay_parent_1, para_id, validator_id: validator_1, peer_id: peer_id_1 },
			);

			let res = connection_requests.next().await;
			assert_eq!(
				res,
				DiscoveredValidator { relay_parent: relay_parent_2, para_id, validator_id: validator_2, peer_id: peer_id_2 },
			);

			check_next_is_pending(&mut connection_requests).await;
		});
	}

	#[test]
	fn same_relay_parent_diffent_para_ids() {
		let mut connection_requests = ConnectionRequests::default();

		executor::block_on(async move {
			check_next_is_pending(&mut connection_requests).await;

			let validator_1 = ValidatorPair::generate().0.public();
			let validator_2 = ValidatorPair::generate().0.public();

			let auth_1 = AuthorityDiscoveryId::from_slice(&[1; 32]);
			let auth_2 = AuthorityDiscoveryId::from_slice(&[2; 32]);

			let mut validator_map_1 = HashMap::new();
			let mut validator_map_2 = HashMap::new();

			validator_map_1.insert(auth_1.clone(), validator_1.clone());
			validator_map_2.insert(auth_2.clone(), validator_2.clone());

			let (mut rq1_tx, rq1_rx) = mpsc::channel(8);
			let (mut rq2_tx, rq2_rx) = mpsc::channel(8);

			let peer_id_1 = PeerId::random();
			let peer_id_2 = PeerId::random();

			let connection_request_1 = ConnectionRequest {
				validator_map: validator_map_1,
				connections: rq1_rx,
			};

			let connection_request_2 = ConnectionRequest {
				validator_map: validator_map_2,
				connections: rq2_rx,
			};

			let relay_parent = Hash::repeat_byte(1);
			let para_id_1 = ParaId::from(1);
			let para_id_2 = ParaId::from(2);

			connection_requests.put(relay_parent.clone(), para_id_1, connection_request_1);
			connection_requests.put(relay_parent.clone(), para_id_2, connection_request_2);

			rq1_tx.send((auth_1, peer_id_1.clone())).await.unwrap();
			rq2_tx.send((auth_2, peer_id_2.clone())).await.unwrap();

			connection_requests.remove(&relay_parent, para_id_1);

			let res = connection_requests.next().await;
			assert_eq!(
				res,
				DiscoveredValidator { relay_parent, para_id: para_id_2, validator_id: validator_2, peer_id: peer_id_2 },
			);

			check_next_is_pending(&mut connection_requests).await;
		});
	}

	#[test]
	fn replacing_a_connection_request_works() {
		let mut connection_requests = ConnectionRequests::default();

		executor::block_on(async move {
			check_next_is_pending(&mut connection_requests).await;

			let validator_1 = ValidatorPair::generate().0.public();
			let validator_2 = ValidatorPair::generate().0.public();

			let auth_1 = AuthorityDiscoveryId::from_slice(&[1; 32]);
			let auth_2 = AuthorityDiscoveryId::from_slice(&[2; 32]);

			let mut validator_map_1 = HashMap::new();
			let mut validator_map_2 = HashMap::new();

			validator_map_1.insert(auth_1.clone(), validator_1.clone());
			validator_map_2.insert(auth_2.clone(), validator_2.clone());

			let (mut rq1_tx, rq1_rx) = mpsc::channel(8);

			let (mut rq2_tx, rq2_rx) = mpsc::channel(8);

			let peer_id_1 = PeerId::random();
			let peer_id_2 = PeerId::random();

			let connection_request_1 = ConnectionRequest {
				validator_map: validator_map_1,
				connections: rq1_rx,
			};

			let connection_request_2 = ConnectionRequest {
				validator_map: validator_map_2,
				connections: rq2_rx,
			};

			let relay_parent = Hash::repeat_byte(3);
			let para_id = ParaId::from(3);

			connection_requests.put(relay_parent.clone(), para_id, connection_request_1);

			rq1_tx.send((auth_1.clone(), peer_id_1.clone())).await.unwrap();

			let res = connection_requests.next().await;
			assert_eq!(res, DiscoveredValidator { relay_parent, para_id, validator_id: validator_1, peer_id: peer_id_1.clone() });

			connection_requests.put(relay_parent.clone(), para_id, connection_request_2);

			assert!(rq1_tx.send((auth_1, peer_id_1.clone())).await.is_err());

			rq2_tx.send((auth_2, peer_id_2.clone())).await.unwrap();

			let res = connection_requests.next().await;
			assert_eq!(res, DiscoveredValidator { relay_parent, para_id, validator_id: validator_2, peer_id: peer_id_2 });

			check_next_is_pending(&mut connection_requests).await;
		});
	}
}
