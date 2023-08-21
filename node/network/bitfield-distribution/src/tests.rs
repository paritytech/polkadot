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

use super::*;
use assert_matches::assert_matches;
use bitvec::bitvec;
use futures::executor;
use maplit::hashmap;
use polkadot_node_network_protocol::{
	grid_topology::{SessionBoundGridTopologyStorage, SessionGridTopology, TopologyPeerInfo},
	our_view,
	peer_set::ValidationVersion,
	view, ObservedRole,
};
use polkadot_node_subsystem::{
	jaeger,
	jaeger::{PerLeafSpan, Span},
	messages::ReportPeerMessage,
};
use polkadot_node_subsystem_test_helpers::make_subsystem_context;
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_primitives::{AvailabilityBitfield, Signed, ValidatorIndex};
use rand_chacha::ChaCha12Rng;
use sp_application_crypto::AppCrypto;
use sp_authority_discovery::AuthorityPair as AuthorityDiscoveryPair;
use sp_core::Pair as PairT;
use sp_keyring::Sr25519Keyring;
use sp_keystore::{testing::MemoryKeystore, Keystore, KeystorePtr};

use std::{iter::FromIterator as _, sync::Arc, time::Duration};

const TIMEOUT: Duration = Duration::from_millis(50);
macro_rules! launch {
	($fut:expr) => {
		$fut.timeout(TIMEOUT).await.unwrap_or_else(|| {
			panic!("{}ms is more than enough for sending messages.", TIMEOUT.as_millis())
		});
	};
}

/// Pre-seeded `crypto` random numbers generator for testing purposes
fn dummy_rng() -> ChaCha12Rng {
	rand_chacha::ChaCha12Rng::seed_from_u64(12345)
}

fn peer_data_v1(view: View) -> PeerData {
	PeerData { view, version: ValidationVersion::V1.into() }
}

/// A very limited state, only interested in the relay parent of the
/// given message, which must be signed by `validator` and a set of peers
/// which are also only interested in that relay parent.
fn prewarmed_state(
	validator: ValidatorId,
	signing_context: SigningContext,
	known_message: BitfieldGossipMessage,
	peers: Vec<PeerId>,
) -> ProtocolState {
	let relay_parent = known_message.relay_parent;
	let mut topologies = SessionBoundGridTopologyStorage::default();
	topologies.update_topology(0_u32, SessionGridTopology::new(Vec::new(), Vec::new()), None);
	topologies.get_current_topology_mut().local_grid_neighbors_mut().peers_x =
		peers.iter().cloned().collect();

	ProtocolState {
		per_relay_parent: hashmap! {
			relay_parent =>
				PerRelayParentData {
					signing_context,
					validator_set: vec![validator.clone()],
					one_per_validator: hashmap! {
						validator.clone() => known_message.clone(),
					},
					message_received_from_peer: hashmap!{},
					message_sent_to_peer: hashmap!{},
					span: PerLeafSpan::new(Arc::new(jaeger::Span::Disabled), "test"),
				},
		},
		peer_data: peers
			.iter()
			.cloned()
			.map(|peer| (peer, peer_data_v1(view![relay_parent])))
			.collect(),
		topologies,
		view: our_view!(relay_parent),
		reputation: ReputationAggregator::new(|_| true),
	}
}

fn state_with_view(
	view: OurView,
	relay_parent: Hash,
	reputation: ReputationAggregator,
) -> (ProtocolState, SigningContext, KeystorePtr, ValidatorId) {
	let mut state = ProtocolState { reputation, ..Default::default() };

	let signing_context = SigningContext { session_index: 1, parent_hash: relay_parent };

	let keystore: KeystorePtr = Arc::new(MemoryKeystore::new());
	let validator = Keystore::sr25519_generate_new(&*keystore, ValidatorId::ID, None)
		.expect("generating sr25519 key not to fail");

	state.per_relay_parent = view
		.iter()
		.map(|relay_parent| {
			(
				*relay_parent,
				PerRelayParentData {
					signing_context: signing_context.clone(),
					validator_set: vec![validator.into()],
					one_per_validator: hashmap! {},
					message_received_from_peer: hashmap! {},
					message_sent_to_peer: hashmap! {},
					span: PerLeafSpan::new(Arc::new(jaeger::Span::Disabled), "test"),
				},
			)
		})
		.collect();

	state.view = view;

	(state, signing_context, keystore, validator.into())
}

#[test]
fn receive_invalid_signature() {
	let _ = env_logger::builder()
		.filter(None, log::LevelFilter::Trace)
		.is_test(true)
		.try_init();

	let hash_a: Hash = [0; 32].into();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(peer_a, peer_b);

	let signing_context = SigningContext { session_index: 1, parent_hash: hash_a };

	// another validator not part of the validatorset
	let keystore: KeystorePtr = Arc::new(MemoryKeystore::new());
	let malicious = Keystore::sr25519_generate_new(&*keystore, ValidatorId::ID, None)
		.expect("Malicious key created");
	let validator_0 =
		Keystore::sr25519_generate_new(&*keystore, ValidatorId::ID, None).expect("key created");
	let validator_1 =
		Keystore::sr25519_generate_new(&*keystore, ValidatorId::ID, None).expect("key created");

	let payload = AvailabilityBitfield(bitvec![u8, bitvec::order::Lsb0; 1u8; 32]);
	let invalid_signed = Signed::<AvailabilityBitfield>::sign(
		&keystore,
		payload.clone(),
		&signing_context,
		ValidatorIndex(0),
		&malicious.into(),
	)
	.ok()
	.flatten()
	.expect("should be signed");
	let invalid_signed_2 = Signed::<AvailabilityBitfield>::sign(
		&keystore,
		payload.clone(),
		&signing_context,
		ValidatorIndex(1),
		&malicious.into(),
	)
	.ok()
	.flatten()
	.expect("should be signed");

	let valid_signed = Signed::<AvailabilityBitfield>::sign(
		&keystore,
		payload,
		&signing_context,
		ValidatorIndex(0),
		&validator_0.into(),
	)
	.ok()
	.flatten()
	.expect("should be signed");

	let invalid_msg =
		BitfieldGossipMessage { relay_parent: hash_a, signed_availability: invalid_signed.clone() };
	let invalid_msg_2 = BitfieldGossipMessage {
		relay_parent: hash_a,
		signed_availability: invalid_signed_2.clone(),
	};
	let valid_msg =
		BitfieldGossipMessage { relay_parent: hash_a, signed_availability: valid_signed.clone() };

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = make_subsystem_context::<BitfieldDistributionMessage, _>(pool);

	let mut state =
		prewarmed_state(validator_0.into(), signing_context.clone(), valid_msg, vec![peer_b]);
	state
		.per_relay_parent
		.get_mut(&hash_a)
		.unwrap()
		.validator_set
		.push(validator_1.into());
	let mut rng = dummy_rng();

	executor::block_on(async move {
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peer_b,
				invalid_msg.into_network_message(ValidationVersion::V1.into())
			),
			&mut rng,
		));

		// reputation doesn't change due to one_job_per_validator check
		assert!(handle.recv().timeout(TIMEOUT).await.is_none());

		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peer_b,
				invalid_msg_2.into_network_message(ValidationVersion::V1.into())
			),
			&mut rng,
		));
		// reputation change due to invalid signature
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(peer, rep))
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep.value, COST_SIGNATURE_INVALID.cost_or_benefit())
			}
		);
	});
}

#[test]
fn receive_invalid_validator_index() {
	let _ = env_logger::builder()
		.filter(None, log::LevelFilter::Trace)
		.is_test(true)
		.try_init();

	let hash_a: Hash = [0; 32].into();
	let hash_b: Hash = [1; 32].into(); // other

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(peer_a, peer_b);

	// validator 0 key pair
	let (mut state, signing_context, keystore, validator) =
		state_with_view(our_view![hash_a, hash_b], hash_a, ReputationAggregator::new(|_| true));

	state.peer_data.insert(peer_b, peer_data_v1(view![hash_a]));

	let payload = AvailabilityBitfield(bitvec![u8, bitvec::order::Lsb0; 1u8; 32]);
	let signed = Signed::<AvailabilityBitfield>::sign(
		&keystore,
		payload,
		&signing_context,
		ValidatorIndex(42),
		&validator,
	)
	.ok()
	.flatten()
	.expect("should be signed");

	let msg = BitfieldGossipMessage { relay_parent: hash_a, signed_availability: signed.clone() };

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = make_subsystem_context::<BitfieldDistributionMessage, _>(pool);
	let mut rng = dummy_rng();

	executor::block_on(async move {
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peer_b,
				msg.into_network_message(ValidationVersion::V1.into())
			),
			&mut rng,
		));

		// reputation change due to invalid validator index
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(peer, rep))
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep.value, COST_VALIDATOR_INDEX_INVALID.cost_or_benefit())
			}
		);
	});
}

#[test]
fn receive_duplicate_messages() {
	let _ = env_logger::builder()
		.filter(None, log::LevelFilter::Trace)
		.is_test(true)
		.try_init();

	let hash_a: Hash = [0; 32].into();
	let hash_b: Hash = [1; 32].into();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(peer_a, peer_b);

	// validator 0 key pair
	let (mut state, signing_context, keystore, validator) =
		state_with_view(our_view![hash_a, hash_b], hash_a, ReputationAggregator::new(|_| true));

	// create a signed message by validator 0
	let payload = AvailabilityBitfield(bitvec![u8, bitvec::order::Lsb0; 1u8; 32]);
	let signed_bitfield = Signed::<AvailabilityBitfield>::sign(
		&keystore,
		payload,
		&signing_context,
		ValidatorIndex(0),
		&validator,
	)
	.ok()
	.flatten()
	.expect("should be signed");

	let msg = BitfieldGossipMessage {
		relay_parent: hash_a,
		signed_availability: signed_bitfield.clone(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = make_subsystem_context::<BitfieldDistributionMessage, _>(pool);
	let mut rng = dummy_rng();

	executor::block_on(async move {
		// send a first message
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peer_b,
				msg.clone().into_network_message(ValidationVersion::V1.into()),
			),
			&mut rng,
		));

		// none of our peers has any interest in any messages
		// so we do not receive a network send type message here
		// but only the one for the next subsystem
		assert_matches!(
			handle.recv().await,
			AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
				_,
				ProvisionableData::Bitfield(hash, signed)
			)) => {
				assert_eq!(hash, hash_a);
				assert_eq!(signed, signed_bitfield)
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(peer, rep))
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep.value, BENEFIT_VALID_MESSAGE_FIRST.cost_or_benefit())
			}
		);

		// let peer A send the same message again
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peer_a,
				msg.clone().into_network_message(ValidationVersion::V1.into()),
			),
			&mut rng,
		));

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(peer, rep))
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep.value, BENEFIT_VALID_MESSAGE.cost_or_benefit())
			}
		);

		// let peer B send the initial message again
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peer_b,
				msg.clone().into_network_message(ValidationVersion::V1.into()),
			),
			&mut rng,
		));

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(peer, rep))
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep.value, COST_PEER_DUPLICATE_MESSAGE.cost_or_benefit())
			}
		);
	});
}

#[test]
fn delay_reputation_change() {
	use polkadot_node_subsystem_util::reputation::add_reputation;

	let _ = env_logger::builder()
		.filter(None, log::LevelFilter::Trace)
		.is_test(true)
		.try_init();

	let hash_a: Hash = [0; 32].into();
	let hash_b: Hash = [1; 32].into();

	let peer = PeerId::random();

	// validator 0 key pair
	let (mut state, signing_context, keystore, validator) =
		state_with_view(our_view![hash_a, hash_b], hash_a, ReputationAggregator::new(|_| false));

	// create a signed message by validator 0
	let payload = AvailabilityBitfield(bitvec![u8, bitvec::order::Lsb0; 1u8; 32]);
	let signed_bitfield = Signed::<AvailabilityBitfield>::sign(
		&keystore,
		payload,
		&signing_context,
		ValidatorIndex(0),
		&validator,
	)
	.ok()
	.flatten()
	.expect("should be signed");

	let msg = BitfieldGossipMessage {
		relay_parent: hash_a,
		signed_availability: signed_bitfield.clone(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (ctx, mut handle) = make_subsystem_context::<BitfieldDistributionMessage, _>(pool);
	let mut rng = dummy_rng();
	let reputation_interval = Duration::from_millis(100);

	let bg = async move {
		let subsystem = BitfieldDistribution::new(Default::default());
		subsystem.run_inner(ctx, &mut state, reputation_interval, &mut rng).await;
	};

	let test_fut = async move {
		// send a first message
		handle
			.send(FromOrchestra::Communication {
				msg: BitfieldDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(
						peer,
						msg.clone().into_network_message(ValidationVersion::V1.into()),
					),
				),
			})
			.await;

		// none of our peers has any interest in any messages
		// so we do not receive a network send type message here
		// but only the one for the next subsystem
		assert_matches!(
			handle.recv().await,
			AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
				_,
				ProvisionableData::Bitfield(hash, signed)
			)) => {
				assert_eq!(hash, hash_a);
				assert_eq!(signed, signed_bitfield)
			}
		);

		// let peer send the initial message again
		handle
			.send(FromOrchestra::Communication {
				msg: BitfieldDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(
						peer,
						msg.clone().into_network_message(ValidationVersion::V1.into()),
					),
				),
			})
			.await;

		// Wait enough to fire reputation delay
		futures_timer::Delay::new(reputation_interval).await;

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Batch(v))
			) => {
				let mut expected_change = HashMap::new();
				for rep in vec![BENEFIT_VALID_MESSAGE_FIRST, COST_PEER_DUPLICATE_MESSAGE] {
					add_reputation(&mut expected_change, peer, rep)
				}
				assert_eq!(v, expected_change)
			}
		);

		handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::pin_mut!(bg);
	futures::pin_mut!(test_fut);

	executor::block_on(futures::future::join(bg, test_fut));
}

#[test]
fn do_not_relay_message_twice() {
	let _ = env_logger::builder()
		.filter(None, log::LevelFilter::Trace)
		.is_test(true)
		.try_init();

	let hash = Hash::random();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(peer_a, peer_b);

	// validator 0 key pair
	let (mut state, signing_context, keystore, validator) =
		state_with_view(our_view![hash], hash, ReputationAggregator::new(|_| true));

	// create a signed message by validator 0
	let payload = AvailabilityBitfield(bitvec![u8, bitvec::order::Lsb0; 1u8; 32]);
	let signed_bitfield = Signed::<AvailabilityBitfield>::sign(
		&keystore,
		payload,
		&signing_context,
		ValidatorIndex(0),
		&validator,
	)
	.ok()
	.flatten()
	.expect("should be signed");

	state.peer_data.insert(peer_b, peer_data_v1(view![hash]));
	state.peer_data.insert(peer_a, peer_data_v1(view![hash]));

	let msg =
		BitfieldGossipMessage { relay_parent: hash, signed_availability: signed_bitfield.clone() };

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = make_subsystem_context::<BitfieldDistributionMessage, _>(pool);
	let mut rng = dummy_rng();

	executor::block_on(async move {
		let mut gossip_peers = GridNeighbors::empty();
		gossip_peers.peers_x = HashSet::from_iter(vec![peer_a, peer_b].into_iter());

		relay_message(
			&mut ctx,
			state.per_relay_parent.get_mut(&hash).unwrap(),
			&gossip_peers,
			&mut state.peer_data,
			validator.clone(),
			msg.clone(),
			RequiredRouting::GridXY,
			&mut rng,
		)
		.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
				_,
				ProvisionableData::Bitfield(h, signed)
			)) => {
				assert_eq!(h, hash);
				assert_eq!(signed, signed_bitfield)
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendValidationMessage(peers, send_msg),
			) => {
				assert_eq!(2, peers.len());
				assert!(peers.contains(&peer_a));
				assert!(peers.contains(&peer_b));
				assert_eq!(send_msg, msg.clone().into_validation_protocol(ValidationVersion::V1.into()));
			}
		);

		// Relaying the message a second time shouldn't work.
		relay_message(
			&mut ctx,
			state.per_relay_parent.get_mut(&hash).unwrap(),
			&gossip_peers,
			&mut state.peer_data,
			validator.clone(),
			msg.clone(),
			RequiredRouting::GridXY,
			&mut rng,
		)
		.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
				_,
				ProvisionableData::Bitfield(h, signed)
			)) => {
				assert_eq!(h, hash);
				assert_eq!(signed, signed_bitfield)
			}
		);

		// There shouldn't be any other message
		assert!(handle.recv().timeout(TIMEOUT).await.is_none());
	});
}

#[test]
fn changing_view() {
	let _ = env_logger::builder()
		.filter(None, log::LevelFilter::Trace)
		.is_test(true)
		.try_init();

	let hash_a: Hash = [0; 32].into();
	let hash_b: Hash = [1; 32].into();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(peer_a, peer_b);

	// validator 0 key pair
	let (mut state, signing_context, keystore, validator) =
		state_with_view(our_view![hash_a, hash_b], hash_a, ReputationAggregator::new(|_| true));

	// create a signed message by validator 0
	let payload = AvailabilityBitfield(bitvec![u8, bitvec::order::Lsb0; 1u8; 32]);
	let signed_bitfield = Signed::<AvailabilityBitfield>::sign(
		&keystore,
		payload,
		&signing_context,
		ValidatorIndex(0),
		&validator,
	)
	.ok()
	.flatten()
	.expect("should be signed");

	let msg = BitfieldGossipMessage {
		relay_parent: hash_a,
		signed_availability: signed_bitfield.clone(),
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = make_subsystem_context::<BitfieldDistributionMessage, _>(pool);
	let mut rng = dummy_rng();

	executor::block_on(async move {
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerConnected(
				peer_b,
				ObservedRole::Full,
				ValidationVersion::V1.into(),
				None
			),
			&mut rng,
		));

		// make peer b interested
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerViewChange(peer_b, view![hash_a, hash_b]),
			&mut rng,
		));

		assert!(state.peer_data.contains_key(&peer_b));

		// recv a first message from the network
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peer_b,
				msg.clone().into_network_message(ValidationVersion::V1.into()),
			),
			&mut rng,
		));

		// gossip to the overseer
		assert_matches!(
			handle.recv().await,
			AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
				_,
				ProvisionableData::Bitfield(hash, signed)
			)) => {
				assert_eq!(hash, hash_a);
				assert_eq!(signed, signed_bitfield)
			}
		);

		// reputation change for peer B
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(peer, rep))
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep.value, BENEFIT_VALID_MESSAGE_FIRST.cost_or_benefit())
			}
		);

		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerViewChange(peer_b, view![]),
			&mut rng,
		));

		assert!(state.peer_data.contains_key(&peer_b));
		assert_eq!(
			&state.peer_data.get(&peer_b).expect("Must contain value for peer B").view,
			&view![]
		);

		// on rx of the same message, since we are not interested,
		// should give penalty
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peer_b,
				msg.clone().into_network_message(ValidationVersion::V1.into()),
			),
			&mut rng,
		));

		// reputation change for peer B
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(peer, rep))
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep.value, COST_PEER_DUPLICATE_MESSAGE.cost_or_benefit())
			}
		);

		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerDisconnected(peer_b),
			&mut rng,
		));

		// we are not interested in any peers at all anymore
		state.view = our_view![];

		// on rx of the same message, since we are not interested,
		// should give penalty
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peer_a,
				msg.clone().into_network_message(ValidationVersion::V1.into()),
			),
			&mut rng,
		));

		// reputation change for peer B
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(peer, rep))
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep.value, COST_NOT_IN_VIEW.cost_or_benefit())
			}
		);
	});
}

#[test]
fn do_not_send_message_back_to_origin() {
	let _ = env_logger::builder()
		.filter(None, log::LevelFilter::Trace)
		.is_test(true)
		.try_init();

	let hash: Hash = [0; 32].into();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(peer_a, peer_b);

	// validator 0 key pair
	let (mut state, signing_context, keystore, validator) =
		state_with_view(our_view![hash], hash, ReputationAggregator::new(|_| true));

	// create a signed message by validator 0
	let payload = AvailabilityBitfield(bitvec![u8, bitvec::order::Lsb0; 1u8; 32]);
	let signed_bitfield = Signed::<AvailabilityBitfield>::sign(
		&keystore,
		payload,
		&signing_context,
		ValidatorIndex(0),
		&validator,
	)
	.ok()
	.flatten()
	.expect("should be signed");

	state.peer_data.insert(peer_b, peer_data_v1(view![hash]));
	state.peer_data.insert(peer_a, peer_data_v1(view![hash]));

	let msg =
		BitfieldGossipMessage { relay_parent: hash, signed_availability: signed_bitfield.clone() };

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = make_subsystem_context::<BitfieldDistributionMessage, _>(pool);
	let mut rng = dummy_rng();

	executor::block_on(async move {
		// send a first message
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peer_b,
				msg.clone().into_network_message(ValidationVersion::V1.into()),
			),
			&mut rng,
		));

		assert_matches!(
			handle.recv().await,
			AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
				_,
				ProvisionableData::Bitfield(hash, signed)
			)) => {
				assert_eq!(hash, hash);
				assert_eq!(signed, signed_bitfield)
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendValidationMessage(peers, send_msg),
			) => {
				assert_eq!(1, peers.len());
				assert!(peers.contains(&peer_a));
				assert_eq!(send_msg, msg.clone().into_validation_protocol(ValidationVersion::V1.into()));
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(peer, rep))
			) => {
				assert_eq!(peer, peer_b);
				assert_eq!(rep.value, BENEFIT_VALID_MESSAGE_FIRST.cost_or_benefit())
			}
		);
	});
}

#[test]
fn topology_test() {
	let _ = env_logger::builder()
		.filter(None, log::LevelFilter::Trace)
		.is_test(true)
		.try_init();

	let hash: Hash = [0; 32].into();

	// validator 0 key pair
	let (mut state, signing_context, keystore, validator) =
		state_with_view(our_view![hash], hash, ReputationAggregator::new(|_| true));

	// Create a simple grid without any shuffling. We occupy position 1.
	let topology_peer_info: Vec<_> = (0..49)
		.map(|i| TopologyPeerInfo {
			peer_ids: vec![PeerId::random()],
			validator_index: ValidatorIndex(i as _),
			discovery_id: AuthorityDiscoveryPair::generate().0.public(),
		})
		.collect();

	let topology = SessionGridTopology::new((0usize..49).collect(), topology_peer_info.clone());
	state.topologies.update_topology(0_u32, topology, Some(ValidatorIndex(1)));

	let peers_x: Vec<_> = [0, 2, 3, 4, 5, 6]
		.iter()
		.cloned()
		.map(|i| topology_peer_info[i].peer_ids[0])
		.collect();

	let peers_y: Vec<_> = [8, 15, 22, 29, 36, 43]
		.iter()
		.cloned()
		.map(|i| topology_peer_info[i].peer_ids[0])
		.collect();

	{
		let t = state.topologies.get_current_topology().local_grid_neighbors();
		for p_x in &peers_x {
			assert!(t.peers_x.contains(p_x));
		}
		for p_y in &peers_y {
			assert!(t.peers_y.contains(p_y));
		}
	}

	// create a signed message by validator 0
	let payload = AvailabilityBitfield(bitvec![u8, bitvec::order::Lsb0; 1u8; 32]);
	let signed_bitfield = Signed::<AvailabilityBitfield>::sign(
		&keystore,
		payload,
		&signing_context,
		ValidatorIndex(0),
		&validator,
	)
	.ok()
	.flatten()
	.expect("should be signed");

	peers_x.iter().chain(peers_y.iter()).for_each(|peer| {
		state.peer_data.insert(*peer, peer_data_v1(view![hash]));
	});

	let msg =
		BitfieldGossipMessage { relay_parent: hash, signed_availability: signed_bitfield.clone() };

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = make_subsystem_context::<BitfieldDistributionMessage, _>(pool);
	let mut rng = dummy_rng();

	executor::block_on(async move {
		// send a first message
		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peers_x[0],
				msg.clone().into_network_message(ValidationVersion::V1.into()),
			),
			&mut rng,
		));

		assert_matches!(
			handle.recv().await,
			AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
				_,
				ProvisionableData::Bitfield(hash, signed)
			)) => {
				assert_eq!(hash, hash);
				assert_eq!(signed, signed_bitfield)
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendValidationMessage(peers, send_msg),
			) => {
				let topology = state.topologies.get_current_topology().local_grid_neighbors();
				// It should send message to all peers in y direction and to 4 random peers in x direction
				assert_eq!(peers_y.len() + 4, peers.len());
				assert!(topology.peers_y.iter().all(|peer| peers.contains(&peer)));
				assert!(topology.peers_x.iter().filter(|peer| peers.contains(&peer)).count() == 4);
				// Must never include originator
				assert!(!peers.contains(&peers_x[0]));
				assert_eq!(send_msg, msg.clone().into_validation_protocol(ValidationVersion::V1.into()));
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(peer, rep))
			) => {
				assert_eq!(peer, peers_x[0]);
				assert_eq!(rep.value, BENEFIT_VALID_MESSAGE_FIRST.cost_or_benefit())
			}
		);
	});
}

#[test]
fn need_message_works() {
	let validators = vec![Sr25519Keyring::Alice.pair(), Sr25519Keyring::Bob.pair()];

	let validator_set = Vec::from_iter(validators.iter().map(|k| ValidatorId::from(k.public())));

	let signing_context = SigningContext { session_index: 1, parent_hash: Hash::repeat_byte(0x00) };
	let mut state = PerRelayParentData::new(
		signing_context,
		validator_set.clone(),
		PerLeafSpan::new(Arc::new(Span::Disabled), "foo"),
	);

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(peer_a, peer_b);

	let pretend_send =
		|state: &mut PerRelayParentData, dest_peer: PeerId, signed_by: &ValidatorId| -> bool {
			if state.message_from_validator_needed_by_peer(&dest_peer, signed_by) {
				state
					.message_sent_to_peer
					.entry(dest_peer)
					.or_default()
					.insert(signed_by.clone());
				true
			} else {
				false
			}
		};

	let pretend_receive =
		|state: &mut PerRelayParentData, source_peer: PeerId, signed_by: &ValidatorId| {
			state
				.message_received_from_peer
				.entry(source_peer)
				.or_default()
				.insert(signed_by.clone());
		};

	assert!(pretend_send(&mut state, peer_a, &validator_set[0]));
	assert!(pretend_send(&mut state, peer_b, &validator_set[1]));
	// sending the same thing must not be allowed
	assert!(!pretend_send(&mut state, peer_a, &validator_set[0]));

	// receive by Alice
	pretend_receive(&mut state, peer_a, &validator_set[0]);
	// must be marked as not needed by Alice, so attempt to send to Alice must be false
	assert!(!pretend_send(&mut state, peer_a, &validator_set[0]));
	// but ok for Bob
	assert!(!pretend_send(&mut state, peer_b, &validator_set[1]));

	// receive by Bob
	pretend_receive(&mut state, peer_a, &validator_set[0]);
	// not ok for Alice
	assert!(!pretend_send(&mut state, peer_a, &validator_set[0]));
	// also not ok for Bob
	assert!(!pretend_send(&mut state, peer_b, &validator_set[1]));
}

#[test]
fn network_protocol_versioning() {
	let hash_a: Hash = [0; 32].into();
	let hash_b: Hash = [1; 32].into();

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();

	let peers = [
		(peer_a, ValidationVersion::VStaging),
		(peer_b, ValidationVersion::V1),
		(peer_c, ValidationVersion::VStaging),
	];

	// validator 0 key pair
	let (mut state, signing_context, keystore, validator) =
		state_with_view(our_view![hash_a, hash_b], hash_a, ReputationAggregator::new(|_| true));

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = make_subsystem_context::<BitfieldDistributionMessage, _>(pool);
	let mut rng = dummy_rng();

	executor::block_on(async move {
		// create a signed message by validator 0
		let payload = AvailabilityBitfield(bitvec![u8, bitvec::order::Lsb0; 1u8; 32]);
		let signed_bitfield = Signed::<AvailabilityBitfield>::sign(
			&keystore,
			payload,
			&signing_context,
			ValidatorIndex(0),
			&validator,
		)
		.ok()
		.flatten()
		.expect("should be signed");
		let msg = BitfieldGossipMessage {
			relay_parent: hash_a,
			signed_availability: signed_bitfield.clone(),
		};

		for (peer, protocol_version) in peers {
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerConnected(
					peer,
					ObservedRole::Full,
					protocol_version.into(),
					None
				),
				&mut rng,
			));

			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerViewChange(peer, view![hash_a, hash_b]),
				&mut rng,
			));

			assert!(state.peer_data.contains_key(&peer));
		}

		launch!(handle_network_msg(
			&mut ctx,
			&mut state,
			&Default::default(),
			NetworkBridgeEvent::PeerMessage(
				peer_a,
				msg.clone().into_network_message(ValidationVersion::VStaging.into()),
			),
			&mut rng,
		));

		// gossip to the overseer
		assert_matches!(
			handle.recv().await,
			AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
				_,
				ProvisionableData::Bitfield(hash, signed)
			)) => {
				assert_eq!(hash, hash_a);
				assert_eq!(signed, signed_bitfield)
			}
		);

		// v1 gossip
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendValidationMessage(peers, send_msg),
			) => {
				assert_eq!(peers, vec![peer_b]);
				assert_eq!(send_msg, msg.clone().into_validation_protocol(ValidationVersion::V1.into()));
			}
		);

		// vstaging gossip
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendValidationMessage(peers, send_msg),
			) => {
				assert_eq!(peers, vec![peer_c]);
				assert_eq!(send_msg, msg.clone().into_validation_protocol(ValidationVersion::VStaging.into()));
			}
		);

		// reputation change
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(ReportPeerMessage::Single(peer, rep))
			) => {
				assert_eq!(peer, peer_a);
				assert_eq!(rep, BENEFIT_VALID_MESSAGE_FIRST.into())
			}
		);
	});
}
