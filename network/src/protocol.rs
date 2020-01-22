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
use futures::prelude::*;
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

struct ProtocolHandler {
	service: Arc<PolkadotNetworkService>,
	peers: HashMap<PeerId, PeerData>,
}

impl ProtocolHandler {
	fn peer_connected(&mut self, peer: PeerId, roles: Roles) {
		self.peers.insert(peer, PeerData { roles });
	}

	fn peer_disconnected(&mut self, peer: PeerId) {
		self.peers.remove(&peer);
	}

	fn handle_notification(&mut self, peer: PeerId, mut notification: &[u8]) {
		match Message::decode(&mut notification) {
			Ok(msg) => {
				unimplemented!();
			}
			Err(_) => {
				self.service.report_peer(peer, cost::INVALID_FORMAT);
			}
		}
	}
}

/// Registers the protocol and returns a future for handling it.
///
/// You are very strongly encouraged to call this method very early on. Any connection open
/// will retain the protocols that were registered then, and not any new one.
pub fn start_protocol(service: Arc<PolkadotNetworkService>)
	-> impl Future<Output = ()> + Send + 'static
{
	let mut event_stream = service.event_stream();

	let mut handler = ProtocolHandler {
		service,
		peers: HashMap::new(),
	};
	async move {
		while let Some(event) = event_stream.next().await {
			match event {
				Event::Dht(_) => continue,
				Event::NotificationStreamOpened {
					remote,
					engine_id,
					roles,
				} => {
					if engine_id != POLKADOT_ENGINE_ID { continue }

					handler.peer_connected(remote, roles);
				},
				Event::NotificationsStreamClosed {
					remote,
					engine_id,
				} => {
					if engine_id != POLKADOT_ENGINE_ID { continue }

					handler.peer_disconnected(remote);
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
						});

					for notification in our_notifications {
						handler.handle_notification(remote.clone(), &notification.as_ref())
					}
				}
			}
		}
	}
}
