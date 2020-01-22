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

use codec::{Decode, Encode};
use futures::channel::mpsc;
use futures::prelude::*;
use futures::task::SpawnExt;
use polkadot_primitives::{Hash, parachain::{PoVBlock, ValidatorId, Collation}};
use sc_network::{config::Roles, Event, PeerId, RequestId};

use std::collections::HashMap;
use std::sync::Arc;

use super::{cost, benefit, PolkadotNetworkService};

/// The current protocol version.
pub const VERSION: u32 = 1;
/// The minimum supported protocol version.
pub const MIN_SUPPORTED_VERSION: u32 = 1;

/// The engine ID of the polkadot network protocol.
pub const POLKADOT_ENGINE_ID: sp_runtime::ConsensusEngineId = *b"dot2";

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
	PeerConnected(PeerId, Roles),
	PeerMessage(PeerId, Vec<bytes::Bytes>),
	PeerDisconnected(PeerId),
}

/// An async handle to the network service.
pub struct Service {
	sender: mpsc::Sender<ServiceToWorkerMsg>,
}

// TODO [now]: implement `ParachainNetwork`.

/// Registers the protocol.
///
/// You are very strongly encouraged to call this method very early on. Any connection open
/// will retain the protocols that were registered then, and not any new one.
pub fn start(
	service: Arc<PolkadotNetworkService>,
	executor: &impl futures::task::Spawn,
) -> Result<Service, futures::task::SpawnError> {
	const SERVICE_TO_WORKER_BUF: usize = 256;

	let mut event_stream = service.event_stream();
	let (mut worker_sender, worker_receiver) = mpsc::channel(SERVICE_TO_WORKER_BUF);

	// TODO [now]: instantiate gossip
	executor.spawn(worker_loop(service.clone(), worker_receiver))?;

	let polkadot_service = Service { sender: worker_sender.clone() };

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
						Ok(message) => unimplemented!(),
						Err(_) => service.report_peer(remote.clone(), cost::INVALID_FORMAT),
					}
				}
			}
		}
	}
}
