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

//! The Network Bridge Subsystem - handles _outgoing_ messages, from subsystem to the network.
use super::*;

use sp_consensus::SyncOracle;

use polkadot_node_network_protocol::{peer_set::PeerSet, v1 as protocol_v1, PeerId, Versioned};

use polkadot_node_subsystem::{
	errors::SubsystemError, messages::NetworkBridgeMessage, overseer, FromOrchestra,
	OverseerSignal, SpawnedSubsystem,
};

/// Peer set info for network initialization.
///
/// To be added to [`NetworkConfiguration::extra_sets`].
pub use polkadot_node_network_protocol::peer_set::{peer_sets_info, IsAuthority};

use crate::validator_discovery;

/// Actual interfacing to the network based on the `Network` trait.
///
/// Defines the `Network` trait with an implementation for an `Arc<NetworkService>`.
use crate::network::{send_message, Network};

use crate::metrics::Metrics;

#[cfg(test)]
mod tests;

// network bridge log target
const LOG_TARGET: &'static str = "parachain::network-bridge-outgoing";

/// The network bridge subsystem.
pub struct NetworkBridgeOut<N, AD> {
	/// `Network` trait implementing type.
	network_service: N,
	authority_discovery_service: AD,
	sync_oracle: Box<dyn SyncOracle + Send>,
	shared: Shared,
	metrics: Metrics,
}

impl<N, AD> NetworkBridgeOut<N, AD> {
	/// Create a new network bridge subsystem with underlying network service and authority discovery service.
	///
	/// This assumes that the network service has had the notifications protocol for the network
	/// bridge already registered. See [`peers_sets_info`](peers_sets_info).
	pub fn new(
		network_service: N,
		authority_discovery_service: AD,
		sync_oracle: Box<dyn SyncOracle + Send>,
		metrics: Metrics,
	) -> Self {
		let shared = Shared::default();
		Self { network_service, authority_discovery_service, sync_oracle, shared, metrics }
	}

	/// Obtain the shared internal state.
	///
	/// Should only be used for setting up `NetworkBridgeIn`.
	pub fn shared(&self) -> Shared {
		self.shared.clone()
	}
}

#[overseer::subsystem(NetworkBridgeOut, error = SubsystemError, prefix = self::overseer)]
impl<Net, AD, Context> NetworkBridgeOut<Net, AD>
where
	Net: Network + Sync,
	AD: validator_discovery::AuthorityDiscovery + Clone + Sync,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = run_network_out(self, ctx)
			.map_err(|e| SubsystemError::with_origin("network-bridge", e))
			.boxed();
		SpawnedSubsystem { name: "network-bridge-subsystem", future }
	}
}

#[overseer::contextbounds(NetworkBridgeOut, prefix = self::overseer)]
async fn handle_subsystem_messages<Context, N, AD>(
	mut ctx: Context,
	mut network_service: N,
	mut authority_discovery_service: AD,
	_shared: Shared,
	_sync_oracle: Box<dyn SyncOracle + Send>,
	metrics: Metrics,
) -> Result<(), Error>
where
	N: Network,
	AD: validator_discovery::AuthorityDiscovery + Clone,
{
	let mut validator_discovery = validator_discovery::Service::<N, AD>::new();

	loop {
		match ctx.recv().fuse().await? {
			FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOrchestra::Signal(_) => { /* handled by incoming */ },
			FromOrchestra::Communication { msg } => {
				(network_service, authority_discovery_service) =
					handle_incoming_subsystem_communication(
						&mut ctx,
						network_service,
						&mut validator_discovery,
						authority_discovery_service.clone(),
						msg,
						&metrics,
					)
					.await;
			},
		}
	}
}

#[overseer::contextbounds(NetworkBridgeOut, prefix = self::overseer)]
async fn handle_incoming_subsystem_communication<Context, N, AD>(
	_ctx: &mut Context,
	mut network_service: N,
	validator_discovery: &mut validator_discovery::Service<N, AD>,
	mut authority_discovery_service: AD,
	msg: NetworkBridgeMessage,
	metrics: &Metrics,
) -> (N, AD)
where
	N: Network,
	AD: validator_discovery::AuthorityDiscovery + Clone,
{
	match msg {
		NetworkBridgeMessage::ReportPeer(peer, rep) => {
			if !rep.is_benefit() {
				gum::debug!(target: LOG_TARGET, ?peer, ?rep, action = "ReportPeer");
			}

			metrics.on_report_event();
			network_service.report_peer(peer, rep);
		},
		NetworkBridgeMessage::DisconnectPeer(peer, peer_set) => {
			gum::trace!(
				target: LOG_TARGET,
				action = "DisconnectPeer",
				?peer,
				peer_set = ?peer_set,
			);

			network_service.disconnect_peer(peer, peer_set);
		},
		NetworkBridgeMessage::SendValidationMessage(peers, msg) => {
			gum::trace!(
				target: LOG_TARGET,
				action = "SendValidationMessages",
				num_messages = 1usize,
			);

			match msg {
				Versioned::V1(msg) => send_validation_message_v1(
					&mut network_service,
					peers,
					WireMessage::ProtocolMessage(msg),
					&metrics,
				),
			}
		},
		NetworkBridgeMessage::SendValidationMessages(msgs) => {
			gum::trace!(
				target: LOG_TARGET,
				action = "SendValidationMessages",
				num_messages = %msgs.len(),
			);

			for (peers, msg) in msgs {
				match msg {
					Versioned::V1(msg) => send_validation_message_v1(
						&mut network_service,
						peers,
						WireMessage::ProtocolMessage(msg),
						&metrics,
					),
				}
			}
		},
		NetworkBridgeMessage::SendCollationMessage(peers, msg) => {
			gum::trace!(
				target: LOG_TARGET,
				action = "SendCollationMessages",
				num_messages = 1usize,
			);

			match msg {
				Versioned::V1(msg) => send_collation_message_v1(
					&mut network_service,
					peers,
					WireMessage::ProtocolMessage(msg),
					&metrics,
				),
			}
		},
		NetworkBridgeMessage::SendCollationMessages(msgs) => {
			gum::trace!(
				target: LOG_TARGET,
				action = "SendCollationMessages",
				num_messages = %msgs.len(),
			);

			for (peers, msg) in msgs {
				match msg {
					Versioned::V1(msg) => send_collation_message_v1(
						&mut network_service,
						peers,
						WireMessage::ProtocolMessage(msg),
						&metrics,
					),
				}
			}
		},
		NetworkBridgeMessage::SendRequests(reqs, if_disconnected) => {
			gum::trace!(
				target: LOG_TARGET,
				action = "SendRequests",
				num_requests = %reqs.len(),
			);

			for req in reqs {
				network_service
					.start_request(&mut authority_discovery_service, req, if_disconnected)
					.await;
			}
		},
		NetworkBridgeMessage::ConnectToValidators { validator_ids, peer_set, failed } => {
			gum::trace!(
				target: LOG_TARGET,
				action = "ConnectToValidators",
				peer_set = ?peer_set,
				ids = ?validator_ids,
				"Received a validator connection request",
			);

			metrics.note_desired_peer_count(peer_set, validator_ids.len());

			let (network_service, ads) = validator_discovery
				.on_request(
					validator_ids,
					peer_set,
					failed,
					network_service,
					authority_discovery_service,
				)
				.await;

			return (network_service, ads)
		},
		NetworkBridgeMessage::ConnectToResolvedValidators { validator_addrs, peer_set } => {
			gum::trace!(
				target: LOG_TARGET,
				action = "ConnectToPeers",
				peer_set = ?peer_set,
				?validator_addrs,
				"Received a resolved validator connection request",
			);

			metrics.note_desired_peer_count(peer_set, validator_addrs.len());

			let all_addrs = validator_addrs.into_iter().flatten().collect();
			let network_service = validator_discovery
				.on_resolved_request(all_addrs, peer_set, network_service)
				.await;
			return (network_service, authority_discovery_service)
		},
	}
	(network_service, authority_discovery_service)
}

#[overseer::contextbounds(NetworkBridgeOut, prefix = self::overseer)]
async fn run_network_out<N, AD, Context>(
	bridge: NetworkBridgeOut<N, AD>,
	ctx: Context,
) -> Result<(), Error>
where
	N: Network,
	AD: validator_discovery::AuthorityDiscovery + Clone + Sync,
{
	let NetworkBridgeOut { network_service, authority_discovery_service, metrics, sync_oracle, shared, } =
		bridge;

	handle_subsystem_messages(
		ctx,
		network_service,
		authority_discovery_service,
		shared,
		sync_oracle,
		metrics,
	)
	.await?;

	Ok(())
}

fn send_validation_message_v1(
	net: &mut impl Network,
	peers: Vec<PeerId>,
	message: WireMessage<protocol_v1::ValidationProtocol>,
	metrics: &Metrics,
) {
	send_message(net, peers, PeerSet::Validation, 1, message, metrics);
}

fn send_collation_message_v1(
	net: &mut impl Network,
	peers: Vec<PeerId>,
	message: WireMessage<protocol_v1::CollationProtocol>,
	metrics: &Metrics,
) {
	send_message(net, peers, PeerSet::Collation, 1, message, metrics)
}
