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

use assert_matches::assert_matches;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::{
	collections::HashSet,
	sync::atomic::{AtomicBool, Ordering},
};

use sc_network::{Event as NetworkEvent, IfDisconnected, ProtocolName};

use polkadot_node_network_protocol::{
	peer_set::PeerSetProtocolNames,
	request_response::{outgoing::Requests, ReqProtocolNames},
	view, ObservedRole, Versioned,
};
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		AllMessages, ApprovalDistributionMessage, BitfieldDistributionMessage,
		GossipSupportMessage, StatementDistributionMessage,
	},
	ActiveLeavesUpdate, FromOrchestra, LeafStatus, OverseerSignal,
};
use polkadot_node_subsystem_test_helpers::{
	SingleItemSink, SingleItemStream, TestSubsystemContextHandle,
};
use polkadot_node_subsystem_util::metered;
use polkadot_primitives::v2::{AuthorityDiscoveryId, Hash};

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
	protocol_names: Arc<PeerSetProtocolNames>,
}

#[derive(Clone, Debug)]
struct TestAuthorityDiscovery;

// The test's view of the network. This receives updates from the subsystem in the form
// of `NetworkAction`s.
struct TestNetworkHandle {
	action_rx: metered::UnboundedMeteredReceiver<NetworkAction>,
	net_tx: SingleItemSink<NetworkEvent>,
	protocol_names: PeerSetProtocolNames,
}

fn new_test_network(
	protocol_names: PeerSetProtocolNames,
) -> (TestNetwork, TestNetworkHandle, TestAuthorityDiscovery) {
	let (net_tx, net_rx) = polkadot_node_subsystem_test_helpers::single_item_sink();
	let (action_tx, action_rx) = metered::unbounded();

	(
		TestNetwork {
			net_events: Arc::new(Mutex::new(Some(net_rx))),
			action_tx: Arc::new(Mutex::new(action_tx)),
			protocol_names: Arc::new(protocol_names.clone()),
		},
		TestNetworkHandle { action_rx, net_tx, protocol_names },
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
		_protocol: ProtocolName,
		_: HashSet<Multiaddr>,
	) -> Result<(), String> {
		Ok(())
	}

	async fn remove_from_peers_set(&mut self, _protocol: ProtocolName, _: Vec<PeerId>) {}

	async fn start_request<AD: AuthorityDiscovery>(
		&self,
		_: &mut AD,
		_: Requests,
		_: &ReqProtocolNames,
		_: IfDisconnected,
	) {
	}

	fn report_peer(&self, who: PeerId, cost_benefit: Rep) {
		self.action_tx
			.lock()
			.unbounded_send(NetworkAction::ReputationChange(who, cost_benefit))
			.unwrap();
	}

	fn disconnect_peer(&self, who: PeerId, protocol: ProtocolName) {
		let (peer_set, version) = self.protocol_names.try_get_protocol(&protocol).unwrap();
		assert_eq!(version, peer_set.get_main_version());

		self.action_tx
			.lock()
			.unbounded_send(NetworkAction::DisconnectPeer(who, peer_set))
			.unwrap();
	}

	fn write_notification(&self, who: PeerId, protocol: ProtocolName, message: Vec<u8>) {
		let (peer_set, version) = self.protocol_names.try_get_protocol(&protocol).unwrap();
		assert_eq!(version, peer_set.get_main_version());

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
			protocol: self.protocol_names.get_main_name(peer_set),
			negotiated_fallback: None,
			role: role.into(),
		})
		.await;
	}

	async fn disconnect_peer(&mut self, peer: PeerId, peer_set: PeerSet) {
		self.send_network_event(NetworkEvent::NotificationStreamClosed {
			remote: peer,
			protocol: self.protocol_names.get_main_name(peer_set),
		})
		.await;
	}

	async fn peer_message(&mut self, peer: PeerId, peer_set: PeerSet, message: Vec<u8>) {
		self.send_network_event(NetworkEvent::NotificationsReceived {
			remote: peer,
			messages: vec![(self.protocol_names.get_main_name(peer_set), message.into())],
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
	is_major_syncing: Arc<AtomicBool>,
	done_syncing_sender: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

struct TestSyncOracleHandle {
	done_syncing_receiver: oneshot::Receiver<()>,
	is_major_syncing: Arc<AtomicBool>,
}

impl TestSyncOracleHandle {
	fn set_done(&self) {
		self.is_major_syncing.store(false, Ordering::SeqCst);
	}

	async fn await_mode_switch(self) {
		let _ = self.done_syncing_receiver.await;
	}
}

impl SyncOracle for TestSyncOracle {
	fn is_major_syncing(&self) -> bool {
		let is_major_syncing = self.is_major_syncing.load(Ordering::SeqCst);

		if !is_major_syncing {
			if let Some(sender) = self.done_syncing_sender.lock().take() {
				let _ = sender.send(());
			}
		}

		is_major_syncing
	}

	fn is_offline(&self) -> bool {
		unimplemented!("not used in network bridge")
	}
}

// val - result of `is_major_syncing`.
fn make_sync_oracle(is_major_syncing: bool) -> (TestSyncOracle, TestSyncOracleHandle) {
	let (tx, rx) = oneshot::channel();
	let is_major_syncing = Arc::new(AtomicBool::new(is_major_syncing));

	(
		TestSyncOracle {
			is_major_syncing: is_major_syncing.clone(),
			done_syncing_sender: Arc::new(Mutex::new(Some(tx))),
		},
		TestSyncOracleHandle { is_major_syncing, done_syncing_receiver: rx },
	)
}

fn done_syncing_oracle() -> Box<dyn SyncOracle + Send> {
	let (oracle, _) = make_sync_oracle(false);
	Box::new(oracle)
}

type VirtualOverseer = TestSubsystemContextHandle<NetworkBridgeRxMessage>;

struct TestHarness {
	network_handle: TestNetworkHandle,
	virtual_overseer: VirtualOverseer,
}

fn test_harness<T: Future<Output = VirtualOverseer>>(
	sync_oracle: Box<dyn SyncOracle + Send>,
	test: impl FnOnce(TestHarness) -> T,
) {
	let genesis_hash = Hash::repeat_byte(0xff);
	let fork_id = None;
	let peerset_protocol_names = PeerSetProtocolNames::new(genesis_hash, fork_id);

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut network, network_handle, discovery) = new_test_network(peerset_protocol_names.clone());
	let (context, virtual_overseer) =
		polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);
	let network_stream = network.event_stream();

	let bridge = NetworkBridgeRx {
		network_service: network,
		authority_discovery_service: discovery,
		metrics: Metrics(None),
		sync_oracle,
		shared: Shared::default(),
		peerset_protocol_names,
	};

	let network_bridge = run_network_in(bridge, context, network_stream)
		.map_err(|_| panic!("subsystem execution failed"))
		.map(|_| ());

	let test_fut = test(TestHarness { network_handle, virtual_overseer });

	futures::pin_mut!(test_fut);
	futures::pin_mut!(network_bridge);

	let _ = executor::block_on(future::join(
		async move {
			let mut virtual_overseer = test_fut.await;
			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
		},
		network_bridge,
	));
}

async fn assert_sends_validation_event_to_all(
	event: NetworkBridgeEvent<net_protocol::VersionedValidationProtocol>,
	virtual_overseer: &mut TestSubsystemContextHandle<NetworkBridgeRxMessage>,
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
	virtual_overseer: &mut TestSubsystemContextHandle<NetworkBridgeRxMessage>,
) {
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::CollatorProtocol(
			CollatorProtocolMessage::NetworkBridgeUpdate(e)
		) if e == event.focus().expect("could not focus message")
	)
}

#[test]
fn send_our_view_upon_connection() {
	let (oracle, handle) = make_sync_oracle(false);
	test_harness(Box::new(oracle), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer = PeerId::random();

		let head = Hash::repeat_byte(1);
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: head,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		handle.await_mode_switch().await;

		network_handle
			.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;
		network_handle
			.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full)
			.await;

		let view = view![head];
		let actions = network_handle.next_network_actions(2).await;
		assert_network_actions_contains(
			&actions,
			&NetworkAction::WriteNotification(
				peer.clone(),
				PeerSet::Validation,
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(view.clone()).encode(),
			),
		);
		assert_network_actions_contains(
			&actions,
			&NetworkAction::WriteNotification(
				peer.clone(),
				PeerSet::Collation,
				WireMessage::<protocol_v1::CollationProtocol>::ViewUpdate(view.clone()).encode(),
			),
		);
		virtual_overseer
	});
}

#[test]
fn sends_view_updates_to_peers() {
	let (oracle, handle) = make_sync_oracle(false);
	test_harness(Box::new(oracle), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated: Default::default(),
				deactivated: Default::default(),
			})))
			.await;

		handle.await_mode_switch().await;

		network_handle
			.connect_peer(peer_a.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;
		network_handle
			.connect_peer(peer_b.clone(), PeerSet::Collation, ObservedRole::Full)
			.await;

		let actions = network_handle.next_network_actions(2).await;
		let wire_message =
			WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(View::default()).encode();

		assert_network_actions_contains(
			&actions,
			&NetworkAction::WriteNotification(peer_a, PeerSet::Validation, wire_message.clone()),
		);

		assert_network_actions_contains(
			&actions,
			&NetworkAction::WriteNotification(peer_b, PeerSet::Collation, wire_message.clone()),
		);

		let hash_a = Hash::repeat_byte(1);

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: hash_a,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		let actions = network_handle.next_network_actions(2).await;
		let wire_message =
			WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(view![hash_a]).encode();

		assert_network_actions_contains(
			&actions,
			&NetworkAction::WriteNotification(peer_a, PeerSet::Validation, wire_message.clone()),
		);

		assert_network_actions_contains(
			&actions,
			&NetworkAction::WriteNotification(peer_b, PeerSet::Collation, wire_message.clone()),
		);
		virtual_overseer
	});
}

#[test]
fn do_not_send_view_update_until_synced() {
	let (oracle, handle) = make_sync_oracle(true);
	test_harness(Box::new(oracle), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();
		assert_ne!(peer_a, peer_b);

		network_handle
			.connect_peer(peer_a.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;
		network_handle
			.connect_peer(peer_b.clone(), PeerSet::Collation, ObservedRole::Full)
			.await;

		{
			let actions = network_handle.next_network_actions(2).await;
			let wire_message =
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(View::default())
					.encode();

			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(
					peer_a,
					PeerSet::Validation,
					wire_message.clone(),
				),
			);

			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(peer_b, PeerSet::Collation, wire_message.clone()),
			);
		}

		let hash_a = Hash::repeat_byte(1);
		let hash_b = Hash::repeat_byte(1);

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: hash_a,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		// delay until the previous update has certainly been processed.
		futures_timer::Delay::new(std::time::Duration::from_millis(100)).await;

		handle.set_done();

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: hash_b,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		handle.await_mode_switch().await;

		// There should be a mode switch only for the second view update.
		{
			let actions = network_handle.next_network_actions(2).await;
			let wire_message =
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(view![hash_a, hash_b])
					.encode();

			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(
					peer_a,
					PeerSet::Validation,
					wire_message.clone(),
				),
			);

			assert_network_actions_contains(
				&actions,
				&NetworkAction::WriteNotification(peer_b, PeerSet::Collation, wire_message.clone()),
			);
		}
		virtual_overseer
	});
}

#[test]
fn do_not_send_view_update_when_only_finalized_block_changed() {
	test_harness(done_syncing_oracle(), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();

		network_handle
			.connect_peer(peer_a.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;
		network_handle
			.connect_peer(peer_b.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;

		let hash_a = Hash::repeat_byte(1);

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::BlockFinalized(Hash::random(), 5)))
			.await;

		// Send some empty active leaves update
		//
		// This should not trigger a view update to our peers.
		virtual_overseer
			.send(FromOrchestra::Signal(
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::default()),
			))
			.await;

		// This should trigger the view update to our peers.
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: hash_a,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		let actions = network_handle.next_network_actions(4).await;
		let wire_message =
			WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(View::new(vec![hash_a], 5))
				.encode();

		assert_network_actions_contains(
			&actions,
			&NetworkAction::WriteNotification(peer_a, PeerSet::Validation, wire_message.clone()),
		);

		assert_network_actions_contains(
			&actions,
			&NetworkAction::WriteNotification(peer_b, PeerSet::Validation, wire_message.clone()),
		);
		virtual_overseer
	});
}

#[test]
fn peer_view_updates_sent_via_overseer() {
	test_harness(done_syncing_oracle(), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer = PeerId::random();

		network_handle
			.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;

		let view = view![Hash::repeat_byte(1)];

		// bridge will inform about all connected peers.
		{
			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerConnected(
					peer.clone(),
					ObservedRole::Full,
					ValidationVersion::V1.into(),
					None,
				),
				&mut virtual_overseer,
			)
			.await;

			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
				&mut virtual_overseer,
			)
			.await;
		}

		network_handle
			.peer_message(
				peer.clone(),
				PeerSet::Validation,
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(view.clone()).encode(),
			)
			.await;

		assert_sends_validation_event_to_all(
			NetworkBridgeEvent::PeerViewChange(peer.clone(), view),
			&mut virtual_overseer,
		)
		.await;
		virtual_overseer
	});
}

#[test]
fn peer_messages_sent_via_overseer() {
	test_harness(done_syncing_oracle(), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer = PeerId::random();

		network_handle
			.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;

		// bridge will inform about all connected peers.
		{
			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerConnected(
					peer.clone(),
					ObservedRole::Full,
					ValidationVersion::V1.into(),
					None,
				),
				&mut virtual_overseer,
			)
			.await;

			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
				&mut virtual_overseer,
			)
			.await;
		}

		let approval_distribution_message =
			protocol_v1::ApprovalDistributionMessage::Approvals(Vec::new());

		let message_v1 = protocol_v1::ValidationProtocol::ApprovalDistribution(
			approval_distribution_message.clone(),
		);

		network_handle
			.peer_message(
				peer.clone(),
				PeerSet::Validation,
				WireMessage::ProtocolMessage(message_v1.clone()).encode(),
			)
			.await;

		network_handle.disconnect_peer(peer.clone(), PeerSet::Validation).await;

		// Approval distribution message comes first, and the message is only sent to that subsystem.
		// then a disconnection event arises that is sent to all validation networking subsystems.

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::ApprovalDistribution(
				ApprovalDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(p, Versioned::V1(m))
				)
			) => {
				assert_eq!(p, peer);
				assert_eq!(m, approval_distribution_message);
			}
		);

		assert_sends_validation_event_to_all(
			NetworkBridgeEvent::PeerDisconnected(peer),
			&mut virtual_overseer,
		)
		.await;
		virtual_overseer
	});
}

#[test]
fn peer_disconnect_from_just_one_peerset() {
	test_harness(done_syncing_oracle(), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer = PeerId::random();

		network_handle
			.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;
		network_handle
			.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full)
			.await;

		// bridge will inform about all connected peers.
		{
			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerConnected(
					peer.clone(),
					ObservedRole::Full,
					ValidationVersion::V1.into(),
					None,
				),
				&mut virtual_overseer,
			)
			.await;

			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
				&mut virtual_overseer,
			)
			.await;
		}

		{
			assert_sends_collation_event_to_all(
				NetworkBridgeEvent::PeerConnected(
					peer.clone(),
					ObservedRole::Full,
					ValidationVersion::V1.into(),
					None,
				),
				&mut virtual_overseer,
			)
			.await;

			assert_sends_collation_event_to_all(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
				&mut virtual_overseer,
			)
			.await;
		}

		network_handle.disconnect_peer(peer.clone(), PeerSet::Validation).await;

		assert_sends_validation_event_to_all(
			NetworkBridgeEvent::PeerDisconnected(peer.clone()),
			&mut virtual_overseer,
		)
		.await;

		// to show that we're still connected on the collation protocol, send a view update.

		let hash_a = Hash::repeat_byte(1);

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: hash_a,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		let actions = network_handle.next_network_actions(3).await;
		let wire_message =
			WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(view![hash_a]).encode();

		assert_network_actions_contains(
			&actions,
			&NetworkAction::WriteNotification(
				peer.clone(),
				PeerSet::Collation,
				wire_message.clone(),
			),
		);
		virtual_overseer
	});
}

#[test]
fn relays_collation_protocol_messages() {
	test_harness(done_syncing_oracle(), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();

		network_handle
			.connect_peer(peer_a.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;
		network_handle
			.connect_peer(peer_b.clone(), PeerSet::Collation, ObservedRole::Full)
			.await;

		// bridge will inform about all connected peers.
		{
			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerConnected(
					peer_a.clone(),
					ObservedRole::Full,
					ValidationVersion::V1.into(),
					None,
				),
				&mut virtual_overseer,
			)
			.await;

			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerViewChange(peer_a.clone(), View::default()),
				&mut virtual_overseer,
			)
			.await;
		}

		{
			assert_sends_collation_event_to_all(
				NetworkBridgeEvent::PeerConnected(
					peer_b.clone(),
					ObservedRole::Full,
					ValidationVersion::V1.into(),
					None,
				),
				&mut virtual_overseer,
			)
			.await;

			assert_sends_collation_event_to_all(
				NetworkBridgeEvent::PeerViewChange(peer_b.clone(), View::default()),
				&mut virtual_overseer,
			)
			.await;
		}

		// peer A gets reported for sending a collation message.

		let collator_protocol_message = protocol_v1::CollatorProtocolMessage::Declare(
			Sr25519Keyring::Alice.public().into(),
			Default::default(),
			sp_core::crypto::UncheckedFrom::unchecked_from([1u8; 64]),
		);

		let message_v1 =
			protocol_v1::CollationProtocol::CollatorProtocol(collator_protocol_message.clone());

		network_handle
			.peer_message(
				peer_a.clone(),
				PeerSet::Collation,
				WireMessage::ProtocolMessage(message_v1.clone()).encode(),
			)
			.await;

		let actions = network_handle.next_network_actions(3).await;
		assert_network_actions_contains(
			&actions,
			&NetworkAction::ReputationChange(peer_a.clone(), UNCONNECTED_PEERSET_COST),
		);

		// peer B has the message relayed.

		network_handle
			.peer_message(
				peer_b.clone(),
				PeerSet::Collation,
				WireMessage::ProtocolMessage(message_v1.clone()).encode(),
			)
			.await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CollatorProtocol(
				CollatorProtocolMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(p, Versioned::V1(m))
				)
			) => {
				assert_eq!(p, peer_b);
				assert_eq!(m, collator_protocol_message);
			}
		);
		virtual_overseer
	});
}

#[test]
fn different_views_on_different_peer_sets() {
	test_harness(done_syncing_oracle(), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer = PeerId::random();

		network_handle
			.connect_peer(peer.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;
		network_handle
			.connect_peer(peer.clone(), PeerSet::Collation, ObservedRole::Full)
			.await;

		// bridge will inform about all connected peers.
		{
			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerConnected(
					peer.clone(),
					ObservedRole::Full,
					ValidationVersion::V1.into(),
					None,
				),
				&mut virtual_overseer,
			)
			.await;

			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
				&mut virtual_overseer,
			)
			.await;
		}

		{
			assert_sends_collation_event_to_all(
				NetworkBridgeEvent::PeerConnected(
					peer.clone(),
					ObservedRole::Full,
					ValidationVersion::V1.into(),
					None,
				),
				&mut virtual_overseer,
			)
			.await;

			assert_sends_collation_event_to_all(
				NetworkBridgeEvent::PeerViewChange(peer.clone(), View::default()),
				&mut virtual_overseer,
			)
			.await;
		}

		let view_a = view![Hash::repeat_byte(1)];
		let view_b = view![Hash::repeat_byte(2)];

		network_handle
			.peer_message(
				peer.clone(),
				PeerSet::Validation,
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(view_a.clone()).encode(),
			)
			.await;

		network_handle
			.peer_message(
				peer.clone(),
				PeerSet::Collation,
				WireMessage::<protocol_v1::CollationProtocol>::ViewUpdate(view_b.clone()).encode(),
			)
			.await;

		assert_sends_validation_event_to_all(
			NetworkBridgeEvent::PeerViewChange(peer.clone(), view_a.clone()),
			&mut virtual_overseer,
		)
		.await;

		assert_sends_collation_event_to_all(
			NetworkBridgeEvent::PeerViewChange(peer.clone(), view_b.clone()),
			&mut virtual_overseer,
		)
		.await;
		virtual_overseer
	});
}

#[test]
fn sent_views_include_finalized_number_update() {
	test_harness(done_syncing_oracle(), |test_harness| async move {
		let TestHarness { mut network_handle, mut virtual_overseer } = test_harness;

		let peer_a = PeerId::random();

		network_handle
			.connect_peer(peer_a.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;

		let hash_a = Hash::repeat_byte(1);
		let hash_b = Hash::repeat_byte(2);

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::BlockFinalized(hash_a, 1)))
			.await;
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: hash_b,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		let actions = network_handle.next_network_actions(2).await;
		let wire_message =
			WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(View::new(vec![hash_b], 1))
				.encode();

		assert_network_actions_contains(
			&actions,
			&NetworkAction::WriteNotification(
				peer_a.clone(),
				PeerSet::Validation,
				wire_message.clone(),
			),
		);
		virtual_overseer
	});
}

#[test]
fn view_finalized_number_can_not_go_down() {
	test_harness(done_syncing_oracle(), |test_harness| async move {
		let TestHarness { mut network_handle, virtual_overseer } = test_harness;

		let peer_a = PeerId::random();

		network_handle
			.connect_peer(peer_a.clone(), PeerSet::Validation, ObservedRole::Full)
			.await;

		network_handle
			.peer_message(
				peer_a.clone(),
				PeerSet::Validation,
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(View::new(
					vec![Hash::repeat_byte(0x01)],
					1,
				))
				.encode(),
			)
			.await;

		network_handle
			.peer_message(
				peer_a.clone(),
				PeerSet::Validation,
				WireMessage::<protocol_v1::ValidationProtocol>::ViewUpdate(View::new(vec![], 0))
					.encode(),
			)
			.await;

		let actions = network_handle.next_network_actions(2).await;
		assert_network_actions_contains(
			&actions,
			&NetworkAction::ReputationChange(peer_a.clone(), MALFORMED_VIEW_COST),
		);
		virtual_overseer
	});
}

#[test]
fn our_view_updates_decreasing_order_and_limited_to_max() {
	test_harness(done_syncing_oracle(), |test_harness| async move {
		let TestHarness { mut virtual_overseer, .. } = test_harness;

		// to show that we're still connected on the collation protocol, send a view update.

		let hashes = (0..MAX_VIEW_HEADS + 1).map(|i| Hash::repeat_byte(i as u8));

		for (i, hash) in hashes.enumerate().rev() {
			// These are in reverse order, so the subsystem must sort internally to
			// get the correct view.
			virtual_overseer
				.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(ActivatedLeaf {
						hash,
						number: i as _,
						status: LeafStatus::Fresh,
						span: Arc::new(jaeger::Span::Disabled),
					}),
				)))
				.await;
		}

		let our_views = (1..=MAX_VIEW_HEADS).rev().map(|start| {
			OurView::new(
				(start..=MAX_VIEW_HEADS)
					.rev()
					.map(|i| (Hash::repeat_byte(i as u8), Arc::new(jaeger::Span::Disabled))),
				0,
			)
		});

		for our_view in our_views {
			assert_sends_validation_event_to_all(
				NetworkBridgeEvent::OurViewChange(our_view.clone()),
				&mut virtual_overseer,
			)
			.await;

			assert_sends_collation_event_to_all(
				NetworkBridgeEvent::OurViewChange(our_view),
				&mut virtual_overseer,
			)
			.await;
		}

		virtual_overseer
	});
}
