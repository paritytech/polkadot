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

use super::*;
use futures::{channel::oneshot, executor, stream::BoxStream};
use polkadot_node_subsystem_util::TimeoutExt;

use async_trait::async_trait;
use parking_lot::Mutex;
use std::{
	borrow::Cow,
	collections::HashSet,
	sync::atomic::{AtomicBool, Ordering},
};

use sc_network::{Event as NetworkEvent, IfDisconnected};

use polkadot_node_network_protocol::{
	request_response::outgoing::Requests, ObservedRole, Versioned,
};
use polkadot_node_subsystem::{FromOrchestra, OverseerSignal};
use polkadot_node_subsystem_test_helpers::TestSubsystemContextHandle;
use polkadot_node_subsystem_util::metered;
use polkadot_primitives::v2::AuthorityDiscoveryId;
use polkadot_primitives_test_helpers::dummy_collator_signature;
use sc_network::Multiaddr;
use sp_keyring::Sr25519Keyring;

const TIMEOUT: std::time::Duration = polkadot_node_subsystem_test_helpers::TestSubsystemContextHandle::<NetworkBridgeMessage>::TIMEOUT;

use crate::{network::Network, validator_discovery::AuthorityDiscovery, Rep};

#[derive(Debug, PartialEq)]
pub enum NetworkAction {
	/// Note a change in reputation for a peer.
	ReputationChange(PeerId, Rep),
	/// Disconnect a peer from the given peer-set.
	DisconnectPeer(PeerId, PeerSet),
	/// Write a notification to a given peer on the given peer-set.
	WriteNotification(PeerId, PeerSet, Vec<u8>),
}

// The subsystem's view of the network - only supports a single call to `event_stream`.
#[derive(Clone)]
struct TestNetwork {
	net_events: Arc<Mutex<Option<metered::MeteredReceiver<NetworkEvent>>>>,
	action_tx: Arc<Mutex<metered::UnboundedMeteredSender<NetworkAction>>>,
}

#[derive(Clone, Debug)]
struct TestAuthorityDiscovery;

// The test's view of the network. This receives updates from the subsystem in the form
// of `NetworkAction`s.
struct TestNetworkHandle {
	action_rx: metered::UnboundedMeteredReceiver<NetworkAction>,
	net_tx: metered::MeteredSender<NetworkEvent>,
}

fn new_test_network() -> (TestNetwork, TestNetworkHandle, TestAuthorityDiscovery) {
	let (net_tx, net_rx) = metered::channel(10);
	let (action_tx, action_rx) = metered::unbounded();

	(
		TestNetwork {
			net_events: Arc::new(Mutex::new(Some(net_rx))),
			action_tx: Arc::new(Mutex::new(action_tx)),
		},
		TestNetworkHandle { action_rx, net_tx },
		TestAuthorityDiscovery,
	)
}

#[async_trait]
impl Network for TestNetwork {
	fn event_stream(&mut self) -> BoxStream<'static, NetworkEvent> {
		self.net_events
			.lock()
			.take()
			.expect("Subsystem made more than one call to `event_stream`")
			.boxed()
	}

	async fn set_reserved_peers(
		&mut self,
		_protocol: Cow<'static, str>,
		_: HashSet<Multiaddr>,
	) -> Result<(), String> {
		Ok(())
	}

	async fn remove_from_peers_set(&mut self, _protocol: Cow<'static, str>, _: Vec<PeerId>) {}

	async fn start_request<AD: AuthorityDiscovery>(
		&self,
		_: &mut AD,
		_: Requests,
		_: IfDisconnected,
	) {
	}

	fn report_peer(&self, who: PeerId, cost_benefit: Rep) {
		self.action_tx
			.lock()
			.unbounded_send(NetworkAction::ReputationChange(who, cost_benefit))
			.unwrap();
	}

	fn disconnect_peer(&self, who: PeerId, peer_set: PeerSet) {
		self.action_tx
			.lock()
			.unbounded_send(NetworkAction::DisconnectPeer(who, peer_set))
			.unwrap();
	}

	fn write_notification(&self, who: PeerId, peer_set: PeerSet, message: Vec<u8>) {
		self.action_tx
			.lock()
			.unbounded_send(NetworkAction::WriteNotification(who, peer_set, message))
			.unwrap();
	}
}

#[async_trait]
impl validator_discovery::AuthorityDiscovery for TestAuthorityDiscovery {
	async fn get_addresses_by_authority_id(
		&mut self,
		_authority: AuthorityDiscoveryId,
	) -> Option<HashSet<Multiaddr>> {
		None
	}

	async fn get_authority_ids_by_peer_id(
		&mut self,
		_peer_id: PeerId,
	) -> Option<HashSet<AuthorityDiscoveryId>> {
		None
	}
}

impl TestNetworkHandle {
	// Get the next network action.
	async fn next_network_action(&mut self) -> NetworkAction {
		self.action_rx.next().await.expect("subsystem concluded early")
	}

	async fn connect_peer(&mut self, peer: PeerId, peer_set: PeerSet, role: ObservedRole) {
		self.send_network_event(NetworkEvent::NotificationStreamOpened {
			remote: peer,
			protocol: peer_set.into_default_protocol_name(),
			negotiated_fallback: None,
			role: role.into(),
		})
		.await;
	}

	async fn send_network_event(&mut self, event: NetworkEvent) {
		self.net_tx.send(event).await.expect("subsystem concluded early");
	}
}

#[derive(Clone)]
struct TestSyncOracle {
	flag: Arc<AtomicBool>,
	done_syncing_sender: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl SyncOracle for TestSyncOracle {
	fn is_major_syncing(&mut self) -> bool {
		let is_major_syncing = self.flag.load(Ordering::SeqCst);

		if !is_major_syncing {
			if let Some(sender) = self.done_syncing_sender.lock().take() {
				let _ = sender.send(());
			}
		}

		is_major_syncing
	}

	fn is_offline(&mut self) -> bool {
		unimplemented!("not used in network bridge")
	}
}

// val - result of `is_major_syncing`.
fn done_syncing_oracle() -> Box<dyn SyncOracle + Send> {
	let is_major_syncing = Arc::new(AtomicBool::new(false));
	let oracle = TestSyncOracle {
		flag: is_major_syncing.clone(),
		done_syncing_sender: Arc::new(Mutex::new(None)),
	};
	Box::new(oracle)
}

type VirtualOverseer = TestSubsystemContextHandle<NetworkBridgeMessage>;

struct TestHarness {
	network_handle: TestNetworkHandle,
	virtual_overseer: VirtualOverseer,
}

fn test_harness<T: Future<Output = VirtualOverseer>>(
	sync_oracle: Box<dyn SyncOracle + Send>,
	test: impl FnOnce(TestHarness) -> T,
) {
	let pool = sp_core::testing::TaskExecutor::new();
	let (network, network_handle, discovery) = new_test_network();

	let (context, virtual_overseer) =
		polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	let bridge_out = NetworkBridgeOut::new(network, discovery, sync_oracle, Metrics(None));

	let network_bridge_out_fut = run_network_out(bridge_out, context)
		.map_err(|e| panic!("bridge-out subsystem execution failed {:?}", e))
		.map(|_| ());

	let test_fut = test(TestHarness { network_handle, virtual_overseer });

	futures::pin_mut!(test_fut);
	futures::pin_mut!(network_bridge_out_fut);

	let _ = executor::block_on(future::join(
		async move {
			let mut virtual_overseer = test_fut.await;
			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
		},
		network_bridge_out_fut,
	));
}

#[test]
fn send_messages_to_peers() {
	test_harness(done_syncing_oracle(), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer = PeerId::random();

		network_handle
			.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full)
			.timeout(TIMEOUT)
			.await
			.expect("Timeout does not occur");

		// the outgoing side does not consume network messages
		// so the single item sink has to be free explicitly

		network_handle
			.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full)
			.timeout(TIMEOUT)
			.await
			.expect("Timeout does not occur");

		// send a validation protocol message.

		{
			let approval_distribution_message =
				protocol_v1::ApprovalDistributionMessage::Approvals(Vec::new());

			let message_v1 = protocol_v1::ValidationProtocol::ApprovalDistribution(
				approval_distribution_message.clone(),
			);

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: NetworkBridgeMessage::SendValidationMessage(
						vec![peer.clone()],
						Versioned::V1(message_v1.clone()),
					),
				})
				.timeout(TIMEOUT)
				.await
				.expect("Timeout does not occur");

			assert_eq!(
				network_handle
					.next_network_action()
					.timeout(TIMEOUT)
					.await
					.expect("Timeout does not occur"),
				NetworkAction::WriteNotification(
					peer.clone(),
					PeerSet::Validation,
					WireMessage::ProtocolMessage(message_v1).encode(),
				)
			);
		}

		// send a collation protocol message.

		{
			let collator_protocol_message = protocol_v1::CollatorProtocolMessage::Declare(
				Sr25519Keyring::Alice.public().into(),
				0_u32.into(),
				dummy_collator_signature(),
			);

			let message_v1 =
				protocol_v1::CollationProtocol::CollatorProtocol(collator_protocol_message.clone());

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: NetworkBridgeMessage::SendCollationMessage(
						vec![peer.clone()],
						Versioned::V1(message_v1.clone()),
					),
				})
				.await;

			assert_eq!(
				network_handle
					.next_network_action()
					.timeout(TIMEOUT)
					.await
					.expect("Timeout does not occur"),
				NetworkAction::WriteNotification(
					peer.clone(),
					PeerSet::Collation,
					WireMessage::ProtocolMessage(message_v1).encode(),
				)
			);
		}
		virtual_overseer
	});
}
