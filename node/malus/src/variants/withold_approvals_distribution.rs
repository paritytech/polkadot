// Copyright (C) Parity Technologies (UK) Ltd.
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

//! This variant of Malus withold sending of approvals coming from approvals
//! distribution subsystem to just a subset of nodes. It is meant to be used for testing
//! finality is reached even nodes can not reach directly each other.
//! It enforces that peers are grouped in groups of fixed size, then it arranges
//! the groups in a ring topology.
//!
//!
//! Peers can send their messages only to peers in their group and to peers from next
//! group in the ring topology.
//! E.g If we 16 nodes split in 4 groups then:
//! (1, 2, 3, 4) -> (5, 6, 7, 8) -> (9, 10, 11, 12) -> (13, 14, 15, 16 )
//!     ^                                                       |
//!     |<------------------------------------------------------|
//!
//! Then node 5 will be able to send messages only to nodes in it's group (6, 7, 8) and to nodes
//! in the next group 9, 10, 11, 12

use polkadot_cli::{
	prepared_overseer_builder,
	service::{
		AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error, HeaderBackend, Overseer,
		OverseerConnector, OverseerGen, OverseerGenArgs, OverseerHandle, ParachainHost,
		ProvideRuntimeApi,
	},
	Cli,
};
use polkadot_node_subsystem::SpawnGlue;
use polkadot_node_subsystem_types::messages::network_bridge_event::PeerId;
use sp_core::traits::SpawnNamed;

use crate::interceptor::*;
use polkadot_node_network_protocol::{v1 as protocol_v1, Versioned};

use std::sync::Arc;
const LOG_TARGET: &str = "parachain::withold-approvals";

#[derive(Debug, clap::Parser)]
#[clap(rename_all = "kebab-case")]
#[allow(missing_docs)]
pub struct WitholdApprovalsDistributionOptions {
	/// Determines how many groups to create, the groups are connected in a ring shape, so
	/// a node from group N can send messages only to groups from N+1 and will receive messages from N-1
	/// This should helps us test that approval distribution works correctly even in situations where
	/// all nodes can't reach each other.
	#[clap(short, long, ignore_case = true, default_value_t = 4, value_parser = clap::value_parser!(u8).range(0..=100))]
	pub num_network_groups: u8,

	/// The group to which this node is assigned
	#[clap(short, long, ignore_case = true, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=100))]
	pub assigned_network_group: u8,

	#[clap(flatten)]
	pub cli: Cli,
}

pub(crate) struct WitholdApprovalsDistribution {
	pub num_network_groups: u8,
	pub assigned_network_group: u8,
}

impl OverseerGen for WitholdApprovalsDistribution {
	fn generate<'a, Spawner, RuntimeClient>(
		&self,
		connector: OverseerConnector,
		args: OverseerGenArgs<'a, Spawner, RuntimeClient>,
	) -> Result<(Overseer<SpawnGlue<Spawner>, Arc<RuntimeClient>>, OverseerHandle), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin,
	{
		let spawner = args.spawner.clone();
		let validation_filter = ApprovalsDistributionInterceptor::new(
			SpawnGlue(spawner),
			self.num_network_groups,
			self.assigned_network_group,
		);

		prepared_overseer_builder(args)?
			.replace_network_bridge_tx(move |cv_subsystem| {
				InterceptedSubsystem::new(cv_subsystem, validation_filter)
			})
			.build_with_connector(connector)
			.map_err(|e| e.into())
	}
}

#[derive(Clone, Debug)]
/// Replaces `NetworkBridgeTx`.
pub struct ApprovalsDistributionInterceptor<Spawner> {
	spawner: Spawner,
	known_peer_ids: Vec<PeerId>,
	num_network_groups: u8,
	assigned_network_group: u8,
}

impl<Spawner> ApprovalsDistributionInterceptor<Spawner>
where
	Spawner: overseer::gen::Spawner,
{
	pub fn new(spawner: Spawner, num_network_groups: u8, assigned_network_group: u8) -> Self {
		Self { spawner, known_peer_ids: vec![], num_network_groups, assigned_network_group }
	}

	fn can_send(&self, peer_id: PeerId) -> bool {
		let group_size = self.known_peer_ids.len() / (self.num_network_groups as usize) +
			if self.known_peer_ids.len() % self.num_network_groups as usize != 0 { 1 } else { 0 };

		let my_group_in_ring = self
			.known_peer_ids
			.chunks(group_size)
			.skip(self.assigned_network_group as usize)
			.next();

		let next_group_in_ring = self
			.known_peer_ids
			.chunks(group_size)
			.skip((self.assigned_network_group as usize + 1) % self.num_network_groups as usize)
			.next();

		my_group_in_ring
			.map(|my_group_peers| my_group_peers.contains(&peer_id))
			.unwrap_or(false) ||
			next_group_in_ring
				.map(|next_group_peers| next_group_peers.contains(&peer_id))
				.unwrap_or(false)
	}
}

impl<Sender, Spawner> MessageInterceptor<Sender> for ApprovalsDistributionInterceptor<Spawner>
where
	Sender: overseer::NetworkBridgeTxSenderTrait + Clone + Send + 'static,
	Spawner: overseer::gen::Spawner + Clone + 'static,
{
	type Message = NetworkBridgeTxMessage;

	// Capture all (approval and backing) candidate validation requests and depending on configuration fail them.
	fn intercept_incoming(
		&mut self,
		_subsystem_sender: &mut Sender,
		msg: FromOrchestra<Self::Message>,
	) -> Option<FromOrchestra<Self::Message>> {
		match msg {
			// Message sent by the approval voting subsystem
			FromOrchestra::Communication {
				msg: NetworkBridgeTxMessage::SendValidationMessage(peers, message),
			} => {
				let new_peers: Vec<&PeerId> =
					peers.iter().filter(|peer_id| !self.known_peer_ids.contains(peer_id)).collect();
				if !new_peers.is_empty() {
					self.known_peer_ids.extend(new_peers);
					self.known_peer_ids.sort();
				}

				match &message {
					Versioned::V1(protocol_v1::ValidationProtocol::ApprovalDistribution(
						protocol_v1::ApprovalDistributionMessage::Approvals(approvals),
					)) => {
						let num_peers_we_wanted = peers.len();
						let peers_we_can_send: Vec<PeerId> =
							peers.into_iter().filter(|peer_id| self.can_send(*peer_id)).collect();
						gum::info!(
							target: LOG_TARGET,
							"Malus message intercepted num_peers_we_can_send {:} num_peers_we_wanted_to_send {:} known_peers {:} peers {:} approval {:?}",
							peers_we_can_send.len(),
							num_peers_we_wanted,
							self.known_peer_ids.len(),
							peers_we_can_send
								.clone()
								.into_iter()
								.fold(String::new(), |accumulator, peer| {
									format!("{}:{}", accumulator, peer)
								}),
							approvals
						);
						Some(FromOrchestra::Communication {
							msg: NetworkBridgeTxMessage::SendValidationMessage(
								peers_we_can_send,
								message,
							),
						})
					},
					_ => Some(FromOrchestra::Communication {
						msg: NetworkBridgeTxMessage::SendValidationMessage(peers, message),
					}),
				}
			},
			msg => Some(msg),
		}
	}

	fn intercept_outgoing(
		&self,
		msg: overseer::NetworkBridgeTxOutgoingMessages,
	) -> Option<overseer::NetworkBridgeTxOutgoingMessages> {
		Some(msg)
	}
}

#[cfg(test)]
mod tests {
	use polkadot_node_network_protocol::PeerId;
	use polkadot_node_subsystem::Spawner;

	use super::ApprovalsDistributionInterceptor;

	#[derive(Debug, Clone)]
	struct DummySpanner;
	impl Spawner for DummySpanner {
		fn spawn_blocking(
			&self,
			_name: &'static str,
			_group: Option<&'static str>,
			_future: futures::future::BoxFuture<'static, ()>,
		) {
			todo!()
		}

		fn spawn(
			&self,
			_name: &'static str,
			_group: Option<&'static str>,
			_future: futures::future::BoxFuture<'static, ()>,
		) {
			todo!()
		}
	}

	#[test]
	fn test_can_send() {
		let test_topologies: Vec<Vec<PeerId>> = vec![
			(1..20).map(|_| PeerId::random()).collect(),
			(1..21).map(|_| PeerId::random()).collect(),
		];

		for peer_ids in test_topologies {
			let withold_approval_distribution = ApprovalsDistributionInterceptor {
				spawner: DummySpanner {},
				known_peer_ids: peer_ids.clone(),
				assigned_network_group: 1,
				num_network_groups: 4,
			};

			assert!(!withold_approval_distribution.can_send(peer_ids[0]));
			assert!(!withold_approval_distribution.can_send(peer_ids[4]));

			assert!(withold_approval_distribution.can_send(peer_ids[5]));
			assert!(withold_approval_distribution.can_send(peer_ids[9]));

			assert!(withold_approval_distribution.can_send(peer_ids[10]));
			assert!(withold_approval_distribution.can_send(peer_ids[14]));

			assert!(!withold_approval_distribution.can_send(peer_ids[15]));
			assert!(!withold_approval_distribution.can_send(peer_ids[18]));

			let withold_approval_distribution = ApprovalsDistributionInterceptor {
				spawner: DummySpanner {},
				known_peer_ids: peer_ids.clone(),
				assigned_network_group: 3,
				num_network_groups: 4,
			};

			assert!(withold_approval_distribution.can_send(peer_ids[0]));
			assert!(withold_approval_distribution.can_send(peer_ids[4]));

			assert!(!withold_approval_distribution.can_send(peer_ids[5]));
			assert!(!withold_approval_distribution.can_send(peer_ids[9]));

			assert!(!withold_approval_distribution.can_send(peer_ids[10]));
			assert!(!withold_approval_distribution.can_send(peer_ids[14]));

			assert!(withold_approval_distribution.can_send(peer_ids[15]));
			assert!(withold_approval_distribution.can_send(peer_ids[18]));

			let withold_approval_distribution = ApprovalsDistributionInterceptor {
				spawner: DummySpanner {},
				known_peer_ids: peer_ids.clone(),
				assigned_network_group: 0,
				num_network_groups: 4,
			};

			assert!(withold_approval_distribution.can_send(peer_ids[0]));
			assert!(withold_approval_distribution.can_send(peer_ids[4]));

			assert!(withold_approval_distribution.can_send(peer_ids[5]));
			assert!(withold_approval_distribution.can_send(peer_ids[9]));

			assert!(!withold_approval_distribution.can_send(peer_ids[10]));
			assert!(!withold_approval_distribution.can_send(peer_ids[14]));

			assert!(!withold_approval_distribution.can_send(peer_ids[15]));
			assert!(!withold_approval_distribution.can_send(peer_ids[18]));
		}
	}
}
