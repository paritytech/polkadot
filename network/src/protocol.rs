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

//! Polkadot-specific base networking protocol.
//!
//! This is implemented using the sc-network APIs for futures-based
//! notifications protocols. In some cases, we emulate request/response on top
//! of the notifications machinery, which is slightly less efficient but not
//! meaningfully so.
//!
//! We handle events from `sc-network` in a thin wrapper that forwards to a
//! background worker which also handles commands from other parts of the node.

use codec::{Decode, Encode};
use futures::channel::{mpsc, oneshot};
use futures::prelude::*;
use futures::task::{Spawn, SpawnExt};
use av_store::Store as AvailabilityStore;
use polkadot_primitives::{
	Hash,
	parachain::{
		PoVBlock, ValidatorId, ValidatorIndex, Collation, CandidateReceipt, OutgoingMessages,
		ErasureChunk,
	},
};
use polkadot_validation::{SharedTable, TableRouter, Network as ParachainNetwork, Validated};
use sc_network::{config::Roles, Event, PeerId, RequestId};

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use super::{cost, benefit, PolkadotNetworkService};

/// The current protocol version.
pub const VERSION: u32 = 1;
/// The minimum supported protocol version.
pub const MIN_SUPPORTED_VERSION: u32 = 1;

/// The engine ID of the polkadot network protocol.
pub const POLKADOT_ENGINE_ID: sp_runtime::ConsensusEngineId = *b"dot2";

pub use crate::legacy::gossip::ChainContext;

/// The role of the collator. Whether they're the primary or backup for this parachain.
// TODO [now]: extract CollatorPool to here.
#[derive(PartialEq, Debug, Clone, Copy, Encode, Decode)]
pub enum Role {
	/// Primary collators should send collations whenever it's time.
	Primary = 0,
	/// Backup collators should not.
	Backup = 1,
}

// Messages from the service API or network adapter.
enum ServiceToWorkerMsg {
	// basic peer messages.
	PeerConnected(PeerId, Roles),
	PeerMessage(PeerId, Vec<bytes::Bytes>),
	PeerDisconnected(PeerId),

	// service messages.
	BuildConsensusNetworking(Arc<SharedTable>, Vec<ValidatorId>),
	DropConsensusNetworking(Hash),
	LocalCollation(
		Hash, // relay-parent
		Collation,
		CandidateReceipt,
		(ValidatorIndex, Vec<ErasureChunk>),
	),
	FetchPoVBlock(
		Hash, // relay-parent
		CandidateReceipt,
		oneshot::Sender<PoVBlock>,
	),
}

/// An async handle to the network service.
#[derive(Clone)]
pub struct Service {
	sender: mpsc::Sender<ServiceToWorkerMsg>,
	network_service: Arc<PolkadotNetworkService>,
}

/// Registers the protocol.
///
/// You are very strongly encouraged to call this method very early on. Any connection open
/// will retain the protocols that were registered then, and not any new one.
pub fn start<C: ChainContext + 'static, SP: Spawn + Clone + 'static>(
	service: Arc<PolkadotNetworkService>,
	availability_store: AvailabilityStore,
	chain_context: C,
	executor: SP,
) -> Result<Service, futures::task::SpawnError> {
	const SERVICE_TO_WORKER_BUF: usize = 256;

	let mut event_stream = service.event_stream();
	let (mut worker_sender, worker_receiver) = mpsc::channel(SERVICE_TO_WORKER_BUF);

	let gossip_validator = crate::legacy::gossip::register_validator(
		service.clone(),
		chain_context,
		&executor,
	);
	executor.spawn(worker_loop(
		service.clone(),
		availability_store,
		gossip_validator,
		worker_receiver,
	))?;

	let polkadot_service = Service {
		sender: worker_sender.clone(),
		network_service: service.clone(),
	};

	executor.spawn(async move {
		while let Some(event) = event_stream.next().await {
			let res = match event {
				Event::Dht(_) => continue,
				Event::NotificationStreamOpened {
					remote,
					engine_id,
					roles,
				} => {
					if engine_id != POLKADOT_ENGINE_ID { continue }

					worker_sender.send(ServiceToWorkerMsg::PeerConnected(remote, roles)).await
				},
				Event::NotificationsStreamClosed {
					remote,
					engine_id,
				} => {
					if engine_id != POLKADOT_ENGINE_ID { continue }

					worker_sender.send(ServiceToWorkerMsg::PeerDisconnected(remote)).await
				},
				Event::NotificationsReceived {
					remote,
					messages,
				} => {
					let our_notifications = messages.into_iter()
						.filter_map(|(engine, message)| if engine == POLKADOT_ENGINE_ID {
							Some(message)
						} else {
							None
						})
						.collect();

					worker_sender.send(
						ServiceToWorkerMsg::PeerMessage(remote, our_notifications)
					).await
				}
			};

			if let Err(e) = res {
				// full is impossible here, as we've `await`ed the value being sent.
				if e.is_disconnected() {
					break
				}
			}
		}
	})?;

	Ok(polkadot_service)
}

/// Polkadot-specific messages.
#[derive(Debug, Encode, Decode)]
pub enum Message {
	/// As a validator, tell the peer your current session key.
	// TODO: do this with a cryptographic proof of some kind
	// https://github.com/paritytech/polkadot/issues/47
	ValidatorId(ValidatorId),
	/// Requesting parachain proof-of-validation block (relay_parent, candidate_hash).
	RequestPovBlock(RequestId, Hash, Hash),
	/// Provide requested proof-of-validation block data by candidate hash or nothing if unknown.
	PovBlock(RequestId, Option<PoVBlock>),
	/// Tell a collator their role.
	CollatorRole(Role),
	/// A collation provided by a peer. Relay parent and collation.
	Collation(Hash, Collation),
}

struct PeerData {
	roles: Roles,
}

async fn worker_loop(
	service: Arc<PolkadotNetworkService>,
	availability_store: AvailabilityStore,
	gossip_validator: crate::legacy::gossip::RegisteredMessageValidator<crate::PolkadotProtocol>,
	mut receiver: mpsc::Receiver<ServiceToWorkerMsg>,
) {
	let mut peers = HashMap::new();

	while let Some(message) = receiver.next().await {
		match message {
			ServiceToWorkerMsg::PeerConnected(remote, roles) => {
				peers.insert(remote, PeerData { roles });
			}
			ServiceToWorkerMsg::PeerDisconnected(remote) => {
				peers.remove(&remote);
			}
			ServiceToWorkerMsg::PeerMessage(remote, messages) => {
				for raw_message in messages {
					match Message::decode(&mut raw_message.as_ref()) {
						Ok(message) => {
							service.report_peer(remote.clone(), benefit::VALID_FORMAT);
							unimplemented!()
						},
						Err(_) => service.report_peer(remote.clone(), cost::INVALID_FORMAT),
					}
				}
			}

			ServiceToWorkerMsg::BuildConsensusNetworking(table, authorities) => {
				// glue: let gossip know about our new local leaf.
				let new_leaf_actions = gossip_validator.new_local_leaf(
					table.consensus_parent_hash().clone(),
					crate::legacy::gossip::MessageValidationData { authorities },
					|queue_root| availability_store.queue_by_root(queue_root),
				);

				// TODO [now]: feed checked statements into table.
				// process new_leaf_actions
				unimplemented!()
			}
			ServiceToWorkerMsg::DropConsensusNetworking(relay_parent) => {
				unimplemented!()
			}
			ServiceToWorkerMsg::LocalCollation(relay_parent, collation, receipt, chunks) => {
				distribute_local_collation(relay_parent, collation, receipt, chunks, &*service);
			}
			ServiceToWorkerMsg::FetchPoVBlock(relay_parent, candidate, sender) => {
				// create a filter on gossip for it and send to sender.
			}
		}
	}
}

fn distribute_local_collation(
	relay_parent: Hash,
	collation: Collation,
	receipt: CandidateReceipt,
	chunks: (ValidatorIndex, Vec<ErasureChunk>),
	network: &PolkadotNetworkService,
) {
	// produce a signed statement.
	let hash = receipt.hash();
	let erasure_root = receipt.erasure_root;
	let validated = Validated::collated_local(
		receipt,
		collation.pov.clone(),
		OutgoingMessages { outgoing_messages: Vec::new() },
	);

	unimplemented!();
}

// TODO [now]: implement `Collators`.

/// Routing logic for a particular attestation session.
#[derive(Clone)]
pub struct Router {
	inner: Arc<RouterInner>,
}

// note: do _not_ make this `Clone`: the drop implementation needs to _uniquely_
// send the `DropConsensusNetworking` message.
struct RouterInner {
	relay_parent: Hash,
	sender: mpsc::Sender<ServiceToWorkerMsg>,
}

impl Drop for RouterInner  {
	fn drop(&mut self) {
		let res = self.sender.try_send(
			ServiceToWorkerMsg::DropConsensusNetworking(self.relay_parent)
		);

		if let Err(e) = res {
			assert!(
				!e.is_full(),
				"futures 0.3 guarantees at least one free slot in the capacity \
				per sender; this is the first message sent via this sender; \
				therefore we will not have to wait for capacity; qed"
			);
			// other error variants (disconnection) are fine here.
		}
	}
}

impl ParachainNetwork for Service {
	type Error = mpsc::SendError;
	type TableRouter = Router;
	type BuildTableRouter = Pin<Box<dyn Future<Output=Result<Router,Self::Error>>>>;

	fn build_table_router(
		&self,
		table: Arc<SharedTable>,
		authorities: &[ValidatorId],
	) -> Self::BuildTableRouter {
		let authorities = authorities.to_vec();
		let mut sender = self.sender.clone();
		Box::pin(async move {
			let relay_parent = table.consensus_parent_hash().clone();
			sender.send(
				ServiceToWorkerMsg::BuildConsensusNetworking(table, authorities)
			).await?;

			let inner = Arc::new(RouterInner {
				sender,
				relay_parent,
			});
			Ok(Router { inner })
		})
	}
}

impl TableRouter for Router {
	type Error = future::Either<mpsc::SendError, oneshot::Canceled>;
	type SendLocalCollation = Pin<Box<dyn Future<Output = Result<(), Self::Error>>>>;
	type FetchValidationProof = Pin<Box<dyn Future<Output = Result<PoVBlock, Self::Error>>>>;

	fn local_collation(
		&self,
		collation: Collation,
		receipt: CandidateReceipt,
		_outgoing: OutgoingMessages,
		chunks: (ValidatorIndex, &[ErasureChunk]),
	) -> Self::SendLocalCollation {
		let message = ServiceToWorkerMsg::LocalCollation(
			self.inner.relay_parent.clone(),
			collation,
			receipt,
			(chunks.0, chunks.1.to_vec()),
		);
		let mut sender = self.inner.sender.clone();
		Box::pin(async move {
			sender.send(message).map_err(future::Either::Left).await
		})
	}

	fn fetch_pov_block(&self, candidate: &CandidateReceipt) -> Self::FetchValidationProof {
		let (tx, rx) = oneshot::channel();
		let message = ServiceToWorkerMsg::FetchPoVBlock(
			self.inner.relay_parent.clone(),
			candidate.clone(),
			tx,
		);

		let mut sender = self.inner.sender.clone();
		Box::pin(async move {
			sender.send(message).map_err(future::Either::Left).await?;
			rx.map_err(future::Either::Right).await
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn router_inner_drop_sends_worker_message() {
		let parent = [1; 32].into();

		let (sender, mut receiver) = mpsc::channel(0);
		drop(RouterInner {
			relay_parent: parent,
			sender,
		});

		match receiver.try_next() {
			Ok(Some(ServiceToWorkerMsg::DropConsensusNetworking(x))) => assert_eq!(parent, x),
			_ => panic!("message not sent"),
		}
	}
}
