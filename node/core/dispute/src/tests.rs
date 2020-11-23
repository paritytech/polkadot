
	use super::*;
	use bitvec::bitvec;
	use futures::executor;
	use maplit::hashmap;
	use polkadot_primitives::v1::{Signed, ValidatorPair, AvailabilityBitfield};
	use polkadot_node_subsystem_test_helpers::make_subsystem_context;
	use polkadot_node_subsystem_util::TimeoutExt;
	use sp_core::crypto::Pair;
	use std::time::Duration;
	use assert_matches::assert_matches;
	use polkadot_node_network_protocol::ObservedRole;

	macro_rules! view {
		( $( $hash:expr ),* $(,)? ) => [
			View(vec![ $( $hash.clone() ),* ])
		];
	}

	macro_rules! peers {
		( $( $peer:expr ),* $(,)? ) => [
			vec![ $( $peer.clone() ),* ]
		];
	}

	macro_rules! launch {
		($fut:expr) => {
			$fut
			.timeout(Duration::from_millis(10))
			.await
			.expect("10ms is more than enough for sending messages.")
			.expect("Error values should really never occur.")
		};
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
		let relay_parent = known_message.relay_parent.clone();
		ProtocolState {
			per_relay_parent: hashmap! {
				relay_parent.clone() =>
					PerRelayParentData {
						signing_context,
						validator_set: vec![validator.clone()],
						one_per_validator: hashmap! {
							validator.clone() => known_message.clone(),
						},
						message_received_from_peer: hashmap!{},
						message_sent_to_peer: hashmap!{},
					},
			},
			peer_views: peers
				.into_iter()
				.map(|peer| (peer, view!(relay_parent)))
				.collect(),
			view: view!(relay_parent),
		}
	}

	fn state_with_view(view: View, relay_parent: Hash) -> (ProtocolState, SigningContext, ValidatorPair) {
		let mut state = ProtocolState::default();

		let (validator_pair, _seed) = ValidatorPair::generate();
		let validator = validator_pair.public();

		let signing_context = SigningContext {
			session_index: 1,
			parent_hash: relay_parent.clone(),
		};

		state.per_relay_parent = view.0.iter().map(|relay_parent| {(
				relay_parent.clone(),
				PerRelayParentData {
					signing_context: signing_context.clone(),
					validator_set: vec![validator.clone()],
					one_per_validator: hashmap!{},
					message_received_from_peer: hashmap!{},
					message_sent_to_peer: hashmap!{},
				})
			}).collect();

		state.view = view;

		(state, signing_context, validator_pair)
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

		let signing_context = SigningContext {
			session_index: 1,
			parent_hash: hash_a.clone(),
		};

		// validator 0 key pair
		let (validator_pair, _seed) = ValidatorPair::generate();
		let validator = validator_pair.public();

		// another validator not part of the validatorset
		let (mallicious, _seed) = ValidatorPair::generate();

		let payload = AvailabilityBitfield(bitvec![bitvec::order::Lsb0, u8; 1u8; 32]);
		let signed =
			Signed::<AvailabilityBitfield>::sign(payload, &signing_context, 0, &mallicious);

		let msg = BitfieldGossipMessage {
			relay_parent: hash_a.clone(),
			signed_availability: signed.clone(),
		};

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) =
			make_subsystem_context::<BitfieldDistributionMessage, _>(pool);

		let mut state = prewarmed_state(
			validator.clone(),
			signing_context.clone(),
			msg.clone(),
			vec![peer_b.clone()],
		);

		executor::block_on(async move {
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(peer_b.clone(), msg.into_network_message()),
			));

			// reputation change due to invalid validator index
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_SIGNATURE_INVALID)
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
		let (mut state, signing_context, validator_pair) =
			state_with_view(view![hash_a, hash_b], hash_a.clone());

		state.peer_views.insert(peer_b.clone(), view![hash_a]);

		let payload = AvailabilityBitfield(bitvec![bitvec::order::Lsb0, u8; 1u8; 32]);
		let signed =
			Signed::<AvailabilityBitfield>::sign(payload, &signing_context, 42, &validator_pair);

		let msg = BitfieldGossipMessage {
			relay_parent: hash_a.clone(),
			signed_availability: signed.clone(),
		};

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) =
			make_subsystem_context::<BitfieldDistributionMessage, _>(pool);

		executor::block_on(async move {
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(peer_b.clone(), msg.into_network_message()),
			));

			// reputation change due to invalid validator index
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_VALIDATOR_INDEX_INVALID)
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
		let (mut state, signing_context, validator_pair) =
			state_with_view(view![hash_a, hash_b], hash_a.clone());

		// create a signed message by validator 0
		let payload = AvailabilityBitfield(bitvec![bitvec::order::Lsb0, u8; 1u8; 32]);
		let signed_bitfield =
			Signed::<AvailabilityBitfield>::sign(payload, &signing_context, 0, &validator_pair);

		let msg = BitfieldGossipMessage {
			relay_parent: hash_a.clone(),
			signed_availability: signed_bitfield.clone(),
		};

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) =
			make_subsystem_context::<BitfieldDistributionMessage, _>(pool);

		executor::block_on(async move {
			// send a first message
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					msg.clone().into_network_message(),
				),
			));

			// none of our peers has any interest in any messages
			// so we do not receive a network send type message here
			// but only the one for the next subsystem
			assert_matches!(
				handle.recv().await,
				AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
					ProvisionableData::Bitfield(hash, signed)
				)) => {
					assert_eq!(hash, hash_a);
					assert_eq!(signed, signed_bitfield)
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, BENEFIT_VALID_MESSAGE_FIRST)
				}
			);

			// let peer A send the same message again
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					msg.clone().into_network_message(),
				),
			));

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, BENEFIT_VALID_MESSAGE)
				}
			);

			// let peer B send the initial message again
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					msg.clone().into_network_message(),
				),
			));

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_PEER_DUPLICATE_MESSAGE)
				}
			);
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
		let (mut state, signing_context, validator_pair) = state_with_view(view![hash_a, hash_b], hash_a.clone());

		// create a signed message by validator 0
		let payload = AvailabilityBitfield(bitvec![bitvec::order::Lsb0, u8; 1u8; 32]);
		let signed_bitfield =
			Signed::<AvailabilityBitfield>::sign(payload, &signing_context, 0, &validator_pair);

		let msg = BitfieldGossipMessage {
			relay_parent: hash_a.clone(),
			signed_availability: signed_bitfield.clone(),
		};

		let pool = sp_core::testing::TaskExecutor::new();
		let (mut ctx, mut handle) =
			make_subsystem_context::<BitfieldDistributionMessage, _>(pool);

		executor::block_on(async move {
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerConnected(peer_b.clone(), ObservedRole::Full),
			));

			// make peer b interested
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![hash_a, hash_b]),
			));

			assert!(state.peer_views.contains_key(&peer_b));

			// recv a first message from the network
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					msg.clone().into_network_message(),
				),
			));

			// gossip to the overseer
			assert_matches!(
				handle.recv().await,
				AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
					ProvisionableData::Bitfield(hash, signed)
				)) => {
					assert_eq!(hash, hash_a);
					assert_eq!(signed, signed_bitfield)
				}
			);

			// gossip to the network
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage (
					peers, out_msg,
				)) => {
					assert_eq!(peers, peers![peer_b]);
					assert_eq!(out_msg, msg.clone().into_validation_protocol());
				}
			);

			// reputation change for peer B
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, BENEFIT_VALID_MESSAGE_FIRST)
				}
			);

			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![]),
			));

			assert!(state.peer_views.contains_key(&peer_b));
			assert_eq!(
				state.peer_views.get(&peer_b).expect("Must contain value for peer B"),
				&view![]
			);

			// on rx of the same message, since we are not interested,
			// should give penalty
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					msg.clone().into_network_message(),
				),
			));

			// reputation change for peer B
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_PEER_DUPLICATE_MESSAGE)
				}
			);

			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerDisconnected(peer_b.clone()),
			));

			// we are not interested in any peers at all anymore
			state.view = view![];

			// on rx of the same message, since we are not interested,
			// should give penalty
			launch!(handle_network_msg(
				&mut ctx,
				&mut state,
				&Default::default(),
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					msg.clone().into_network_message(),
				),
			));

			// reputation change for peer B
			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, COST_NOT_IN_VIEW)
				}
			);

		});
	}
