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
use polkadot_node_network_protocol::{self as net_protocol, OurView};
use polkadot_node_subsystem::{messages::NetworkBridgeEvent, ActivatedLeaf};
use polkadot_node_subsystem_util::TimeoutExt;

use assert_matches::assert_matches;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::{
	borrow::Cow,
	collections::HashSet,
	sync::atomic::{AtomicBool, Ordering},
};

use sc_network::{Event as NetworkEvent, IfDisconnected};

use polkadot_node_network_protocol::{
	request_response::outgoing::Requests, view, ObservedRole, Versioned,
};
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		AllMessages, ApprovalDistributionMessage, BitfieldDistributionMessage,
		CollatorProtocolMessage, GossipSupportMessage, StatementDistributionMessage,
	},
	ActiveLeavesUpdate, FromOrchestra, LeafStatus, OverseerSignal,
};
use polkadot_node_subsystem_test_helpers::{
	SingleItemSink, SingleItemStream, TestSubsystemContextHandle,
};
use polkadot_node_subsystem_util::metered;
use polkadot_primitives::v2::{AuthorityDiscoveryId, Hash};
use polkadot_primitives_test_helpers::dummy_collator_signature;
use sc_network::Multiaddr;
use sp_keyring::Sr25519Keyring;

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
	net_events: Arc<Mutex<Option<SingleItemStream<NetworkEvent>>>>,
	action_tx: Arc<Mutex<metered::UnboundedMeteredSender<NetworkAction>>>,
}

#[derive(Clone, Debug)]
struct TestAuthorityDiscovery;

// The test's view of the network. This receives updates from the subsystem in the form
// of `NetworkAction`s.
struct TestNetworkHandle {
	action_rx: metered::UnboundedMeteredReceiver<NetworkAction>,
	net_tx: SingleItemSink<NetworkEvent>,
}

fn new_test_network() -> (TestNetwork, TestNetworkHandle, TestAuthorityDiscovery) {
	let (net_tx, net_rx) = polkadot_node_subsystem_test_helpers::single_item_sink();
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

	// Wait for the next N network actions.
	async fn next_network_actions(&mut self, n: usize) -> Vec<NetworkAction> {
		let mut v = Vec::with_capacity(n);
		for _ in 0..n {
			v.push(self.next_network_action().await);
		}

		v
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

	async fn disconnect_peer(&mut self, peer: PeerId, peer_set: PeerSet) {
		self.send_network_event(NetworkEvent::NotificationStreamClosed {
			remote: peer,
			protocol: peer_set.into_default_protocol_name(),
		})
		.await;
	}

	async fn peer_message(&mut self, peer: PeerId, peer_set: PeerSet, message: Vec<u8>) {
		self.send_network_event(NetworkEvent::NotificationsReceived {
			remote: peer,
			messages: vec![(peer_set.into_default_protocol_name(), message.into())],
		})
		.await;
	}

	async fn send_network_event(&mut self, event: NetworkEvent) {
		self.net_tx.send(event).await.expect("subsystem concluded early");
	}
}

/// Assert that the given actions contain the given `action`.
fn assert_network_actions_contains(actions: &[NetworkAction], action: &NetworkAction) {
	if !actions.iter().any(|x| x == action) {
		panic!("Could not find `{:?}` in `{:?}`", action, actions);
	}
}

#[derive(Clone)]
struct TestSyncOracle {
	flag: Arc<AtomicBool>,
	done_syncing_sender: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

struct TestSyncOracleHandle {
	done_syncing_receiver: oneshot::Receiver<()>,
	flag: Arc<AtomicBool>,
}

impl TestSyncOracleHandle {
	fn set_done(&self) {
		self.flag.store(false, Ordering::SeqCst);
	}

	async fn await_mode_switch(self) {
		let _ = self.done_syncing_receiver.await;
	}
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
fn make_sync_oracle(val: bool) -> (TestSyncOracle, TestSyncOracleHandle) {
	let (tx, rx) = oneshot::channel();
	let flag = Arc::new(AtomicBool::new(val));

	(
		TestSyncOracle { flag: flag.clone(), done_syncing_sender: Arc::new(Mutex::new(Some(tx))) },
		TestSyncOracleHandle { flag, done_syncing_receiver: rx },
	)
}

fn done_syncing_oracle() -> Box<dyn SyncOracle + Send> {
	let (oracle, _) = make_sync_oracle(false);
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
	let (mut network, network_handle, discovery) = new_test_network();
	let (context, virtual_overseer) =
		polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);
	let network_stream = network.event_stream();

	let bridge_out = NetworkBridgeOut::new(network, discovery, sync_oracle, Metrics(None));

	let shared = Shared::default();
	let network_bridge_out_fut = run_network_out(bridge_out, context, shared.clone())
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

async fn assert_sends_validation_event_to_all(
	event: NetworkBridgeEvent<net_protocol::VersionedValidationProtocol>,
	virtual_overseer: &mut TestSubsystemContextHandle<NetworkBridgeMessage>,
) {
	// Ordering must be consistent across:
	// `fn dispatch_validation_event_to_all_unbounded`
	// `dispatch_validation_events_to_all`
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::StatementDistribution(
			StatementDistributionMessage::NetworkBridgeUpdate(e)
		) if e == event.focus().expect("could not focus message")
	);

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::BitfieldDistribution(
			BitfieldDistributionMessage::NetworkBridgeUpdate(e)
		) if e == event.focus().expect("could not focus message")
	);

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::ApprovalDistribution(
			ApprovalDistributionMessage::NetworkBridgeUpdate(e)
		) if e == event.focus().expect("could not focus message")
	);

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::GossipSupport(
			GossipSupportMessage::NetworkBridgeUpdate(e)
		) if e == event.focus().expect("could not focus message")
	);
}

async fn assert_sends_collation_event_to_all(
	event: NetworkBridgeEvent<net_protocol::VersionedCollationProtocol>,
	virtual_overseer: &mut TestSubsystemContextHandle<NetworkBridgeMessage>,
) {
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::CollatorProtocol(
			CollatorProtocolMessage::NetworkBridgeUpdate(e)
		) if e == event.focus().expect("could not focus message")
	)
}

#[test]
fn send_messages_to_peers() {
	test_harness(done_syncing_oracle(), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer = PeerId::random();

		network_handle
			.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full)
			.timeout(std::time::Duration::from_secs(1))
			.await
			.expect("Timeout does not occur");
		network_handle
			.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full)
			.timeout(std::time::Duration::from_secs(1))
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
				.timeout(std::time::Duration::from_secs(1))
				.await
				.expect("Timeout does not occur");

			assert_eq!(
				network_handle
					.next_network_action()
					.timeout(std::time::Duration::from_secs(1))
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
					.timeout(std::time::Duration::from_secs(1))
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
