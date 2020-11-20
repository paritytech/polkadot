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
	channel::{mpsc, oneshot},
	task::{Poll, self},
	stream,
};
use streamunordered::{StreamUnordered, StreamYield};
use thiserror::Error;

use polkadot_node_subsystem::{
	errors::RuntimeApiError, SubsystemError,
	messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest, NetworkBridgeMessage},
	SubsystemContext,
};
use polkadot_primitives::v1::{Hash, ValidatorId, AuthorityDiscoveryId};
use sc_network::PeerId;

/// Error when making a request to connect to validators.
#[derive(Debug, Error)]
pub enum Error {
	/// Attempted to send or receive on a oneshot channel which had been canceled
	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),
	/// A subsystem error.
	#[error(transparent)]
	Subsystem(#[from] SubsystemError),
	/// An error in the Runtime API.
	#[error(transparent)]
	RuntimeApi(#[from] RuntimeApiError),
}

/// Utility function to make it easier to connect to validators.
pub async fn connect_to_validators<Context: SubsystemContext>(
	ctx: &mut Context,
	relay_parent: Hash,
	validators: Vec<ValidatorId>,
) -> Result<ConnectionRequest, Error> {
	// ValidatorId -> AuthorityDiscoveryId
	let (tx, rx) = oneshot::channel();

	ctx.send_message(AllMessages::RuntimeApi(
		RuntimeApiMessage::Request(
			relay_parent,
			RuntimeApiRequest::ValidatorDiscovery(validators.clone(), tx),
		)
	)).await?;

	let maybe_authorities = rx.await??;
	let authorities: Vec<_> = maybe_authorities.iter()
		.cloned()
		.filter_map(|id| id)
		.collect();

	let validator_map = validators.into_iter()
		.zip(maybe_authorities.into_iter())
		.filter_map(|(k, v)| v.map(|v| (v, k)))
		.collect::<HashMap<AuthorityDiscoveryId, ValidatorId>>();

	let (connections, revoke) = connect_to_authorities(ctx, authorities).await?;

	Ok(ConnectionRequest {
		validator_map,
		connections,
		revoke,
	})
}

async fn connect_to_authorities<Context: SubsystemContext>(
	ctx: &mut Context,
	validator_ids: Vec<AuthorityDiscoveryId>,
) -> Result<(mpsc::Receiver<(AuthorityDiscoveryId, PeerId)>, oneshot::Sender<()>), Error> {
	const PEERS_CAPACITY: usize = 8;

	let (revoke_tx, revoke) = oneshot::channel();
	let (connected, connected_rx) = mpsc::channel(PEERS_CAPACITY);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ConnectToValidators {
			validator_ids,
			connected,
			revoke,
		}
	)).await?;

	Ok((connected_rx, revoke_tx))
}

/// A struct that assists performing multiple concurrent connection requests.
///
/// This allows concurrent connections to validator sets at different `relay_parents`
/// and multiplexes their results into a single `Stream`.
#[derive(Default)]
pub struct ConnectionRequests {
	// added connection requests relay_parent -> StreamUnordered token
	id_map: HashMap<Hash, usize>,

	// Connection requests themselves.
	requests: StreamUnordered<ConnectionRequest>,
}

impl ConnectionRequests {
	/// Insert a new connection request.
	///
	/// If a `ConnectionRequest` under a given `relay_parent` already exists it will
	/// be revoked and substituted with a new one.
	pub fn put(&mut self, relay_parent: Hash, request: ConnectionRequest) {
		self.remove(&relay_parent);
		let token = self.requests.push(request);

		self.id_map.insert(relay_parent, token);
	}

	/// Remove a connection request by a given `relay_parent`.
	pub fn remove(&mut self, relay_parent: &Hash) {
		if let Some(token) = self.id_map.remove(relay_parent) {
			Pin::new(&mut self.requests).remove(token);
		}
	}
}

impl stream::Stream for ConnectionRequests {
	/// (relay_parent, validator_id, peer_id).
	type Item = (Hash, ValidatorId, PeerId);

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context) -> Poll<Option<Self::Item>> {
		// If there are currently no requests going on, pend instead of
		// polling `StreamUnordered` which would lead to it terminating
		// and returning `Poll::Ready(None)`.
		if self.requests.is_empty() {
			return Poll::Pending;
		}

		match Pin::new(&mut self.requests).poll_next(cx) {
			Poll::Ready(Some((yielded, token))) => {
				match yielded {
					StreamYield::Item(item) => {
						if let Some((relay_parent, _)) = self.id_map.iter()
							.find(|(_, &val)| val == token)
						{
							return Poll::Ready(Some((*relay_parent, item.0, item.1)));
						}
					}
					StreamYield::Finished(_) => {
						// `ConnectionRequest` is fullfilled, but not revoked
					}
				}
			},
			_ => {},
		}

		Poll::Pending
	}
}

/// A pending connection request to validators.
/// This struct implements `Stream` to allow for asynchronous
/// discovery of validator addresses.
///
/// NOTE: you should call `revoke` on this struct
/// when you're no longer interested in the requested validators.
#[must_use = "dropping a request will result in its immediate revokation"]
pub struct ConnectionRequest {
	validator_map: HashMap<AuthorityDiscoveryId, ValidatorId>,
	#[must_use = "streams do nothing unless polled"]
	connections: mpsc::Receiver<(AuthorityDiscoveryId, PeerId)>,
	#[must_use = "a request should be revoked at some point"]
	revoke: oneshot::Sender<()>,
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

impl ConnectionRequest {
	/// By revoking the request the caller allows the network to
	/// free some peer slots thus freeing the resources.
	/// It doesn't necessarily lead to peers disconnection though.
	/// The revokation is enacted on in the next connection request.
	///
	/// This can be done either by calling this function or dropping the request.
	pub fn revoke(self) {
		if let Err(_) = self.revoke.send(()) {
			tracing::warn!(
				"Failed to revoke a validator connection request",
			);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_primitives::v1::ValidatorPair;
	use sp_core::{Pair, Public};

	use futures::{executor, poll, channel::{mpsc, oneshot}, StreamExt, SinkExt};

	#[test]
	fn adding_a_connection_request_works() {
		let mut connection_requests = ConnectionRequests::default();

		executor::block_on(async move {
			assert_eq!(poll!(Pin::new(&mut connection_requests).next()), Poll::Pending);

			let validator_1 = ValidatorPair::generate().0.public();
			let validator_2 = ValidatorPair::generate().0.public();

			let auth_1 = AuthorityDiscoveryId::from_slice(&[1; 32]);
			let auth_2 = AuthorityDiscoveryId::from_slice(&[2; 32]);

			let mut validator_map = HashMap::new();
			validator_map.insert(auth_1.clone(), validator_1.clone());
			validator_map.insert(auth_2.clone(), validator_2.clone());

			let (mut rq1_tx, rq1_rx) = mpsc::channel(8);
			let (revoke_1_tx, _revoke_1_rx) = oneshot::channel();

			let peer_id_1 = PeerId::random();
			let peer_id_2 = PeerId::random();

			let connection_request_1 = ConnectionRequest {
				validator_map,
				connections: rq1_rx,
				revoke: revoke_1_tx,
			};

			let relay_parent_1 = Hash::repeat_byte(1);

			connection_requests.put(relay_parent_1.clone(), connection_request_1);

			rq1_tx.send((auth_1, peer_id_1.clone())).await.unwrap();
			rq1_tx.send((auth_2, peer_id_2.clone())).await.unwrap();

			let res = Pin::new(&mut connection_requests).next().await.unwrap();
			assert_eq!(res, (relay_parent_1, validator_1, peer_id_1));

			let res = Pin::new(&mut connection_requests).next().await.unwrap();
			assert_eq!(res, (relay_parent_1, validator_2, peer_id_2));

			assert_eq!(
				poll!(Pin::new(&mut connection_requests).next()),
				Poll::Pending,
			);
		});
	}

	#[test]
	fn adding_two_connection_requests_works() {
		let mut connection_requests = ConnectionRequests::default();

		executor::block_on(async move {
			assert_eq!(poll!(Pin::new(&mut connection_requests).next()), Poll::Pending);

			let validator_1 = ValidatorPair::generate().0.public();
			let validator_2 = ValidatorPair::generate().0.public();

			let auth_1 = AuthorityDiscoveryId::from_slice(&[1; 32]);
			let auth_2 = AuthorityDiscoveryId::from_slice(&[2; 32]);

			let mut validator_map_1 = HashMap::new();
			let mut validator_map_2 = HashMap::new();

			validator_map_1.insert(auth_1.clone(), validator_1.clone());
			validator_map_2.insert(auth_2.clone(), validator_2.clone());

			let (mut rq1_tx, rq1_rx) = mpsc::channel(8);
			let (revoke_1_tx, _revoke_1_rx) = oneshot::channel();

			let (mut rq2_tx, rq2_rx) = mpsc::channel(8);
			let (revoke_2_tx, _revoke_2_rx) = oneshot::channel();

			let peer_id_1 = PeerId::random();
			let peer_id_2 = PeerId::random();

			let connection_request_1 = ConnectionRequest {
				validator_map: validator_map_1,
				connections: rq1_rx,
				revoke: revoke_1_tx,
			};

			let connection_request_2 = ConnectionRequest {
				validator_map: validator_map_2,
				connections: rq2_rx,
				revoke: revoke_2_tx,
			};

			let relay_parent_1 = Hash::repeat_byte(1);
			let relay_parent_2 = Hash::repeat_byte(2);

			connection_requests.put(relay_parent_1.clone(), connection_request_1);
			connection_requests.put(relay_parent_2.clone(), connection_request_2);

			rq1_tx.send((auth_1, peer_id_1.clone())).await.unwrap();
			rq2_tx.send((auth_2, peer_id_2.clone())).await.unwrap();

			let res = Pin::new(&mut connection_requests).next().await.unwrap();
			assert_eq!(res, (relay_parent_1, validator_1, peer_id_1));

			let res = Pin::new(&mut connection_requests).next().await.unwrap();
			assert_eq!(res, (relay_parent_2, validator_2, peer_id_2));

			assert_eq!(
				poll!(Pin::new(&mut connection_requests).next()),
				Poll::Pending,
			);
		});
	}

	#[test]
	fn replacing_a_connection_request_works() {
		let mut connection_requests = ConnectionRequests::default();

		executor::block_on(async move {
			assert_eq!(poll!(Pin::new(&mut connection_requests).next()), Poll::Pending);

			let validator_1 = ValidatorPair::generate().0.public();
			let validator_2 = ValidatorPair::generate().0.public();

			let auth_1 = AuthorityDiscoveryId::from_slice(&[1; 32]);
			let auth_2 = AuthorityDiscoveryId::from_slice(&[2; 32]);

			let mut validator_map_1 = HashMap::new();
			let mut validator_map_2 = HashMap::new();

			validator_map_1.insert(auth_1.clone(), validator_1.clone());
			validator_map_2.insert(auth_2.clone(), validator_2.clone());

			let (mut rq1_tx, rq1_rx) = mpsc::channel(8);
			let (revoke_1_tx, _revoke_1_rx) = oneshot::channel();

			let (mut rq2_tx, rq2_rx) = mpsc::channel(8);
			let (revoke_2_tx, _revoke_2_rx) = oneshot::channel();

			let peer_id_1 = PeerId::random();
			let peer_id_2 = PeerId::random();

			let connection_request_1 = ConnectionRequest {
				validator_map: validator_map_1,
				connections: rq1_rx,
				revoke: revoke_1_tx,
			};

			let connection_request_2 = ConnectionRequest {
				validator_map: validator_map_2,
				connections: rq2_rx,
				revoke: revoke_2_tx,
			};

			let relay_parent = Hash::repeat_byte(3);

			connection_requests.put(relay_parent.clone(), connection_request_1);

			rq1_tx.send((auth_1.clone(), peer_id_1.clone())).await.unwrap();

			let res = Pin::new(&mut connection_requests).next().await.unwrap();
			assert_eq!(res, (relay_parent, validator_1, peer_id_1.clone()));

			connection_requests.put(relay_parent.clone(), connection_request_2);

			assert!(rq1_tx.send((auth_1, peer_id_1.clone())).await.is_err());

			rq2_tx.send((auth_2, peer_id_2.clone())).await.unwrap();

			let res = Pin::new(&mut connection_requests).next().await.unwrap();
			assert_eq!(res, (relay_parent, validator_2, peer_id_2));

			assert_eq!(
				poll!(Pin::new(&mut connection_requests).next()),
				Poll::Pending,
			);
		});
	}
}
