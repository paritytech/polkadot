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

use super::{metrics::Metrics, *};
use assert_matches::assert_matches;
use futures::executor::{self, block_on};
use futures_timer::Delay;
use parity_scale_codec::{Decode, Encode};
use polkadot_node_network_protocol::{
	grid_topology::{SessionGridTopology, TopologyPeerInfo},
	peer_set::ValidationVersion,
	request_response::{
		v1::{StatementFetchingRequest, StatementFetchingResponse},
		IncomingRequest, Recipient, ReqProtocolNames, Requests,
	},
	view, ObservedRole,
};
use polkadot_node_primitives::{Statement, UncheckedSignedFullStatement};
use polkadot_node_subsystem::{
	jaeger,
	messages::{network_bridge_event, AllMessages, RuntimeApiMessage, RuntimeApiRequest},
	ActivatedLeaf, LeafStatus,
};
use polkadot_node_subsystem_test_helpers::mock::make_ferdie_keystore;
use polkadot_primitives::v2::{
	GroupIndex, Hash, Id as ParaId, IndexedVec, SessionInfo, ValidationCode, ValidatorId,
};
use polkadot_primitives_test_helpers::{
	dummy_committed_candidate_receipt, dummy_hash, AlwaysZeroRng,
};
use sc_keystore::LocalKeystore;
use sp_application_crypto::{sr25519::Pair, AppKey, Pair as TraitPair};
use sp_authority_discovery::AuthorityPair;
use sp_keyring::Sr25519Keyring;
use sp_keystore::{CryptoStore, SyncCryptoStore, SyncCryptoStorePtr};
use std::{iter::FromIterator as _, sync::Arc, time::Duration};

// Some deterministic genesis hash for protocol names
const GENESIS_HASH: Hash = Hash::repeat_byte(0xff);

#[test]
fn active_head_accepts_only_2_seconded_per_validator() {
	let validators = vec![
		Sr25519Keyring::Alice.public().into(),
		Sr25519Keyring::Bob.public().into(),
		Sr25519Keyring::Charlie.public().into(),
	];
	let parent_hash: Hash = [1; 32].into();

	let session_index = 1;
	let signing_context = SigningContext { parent_hash, session_index };

	let candidate_a = {
		let mut c = dummy_committed_candidate_receipt(dummy_hash());
		c.descriptor.relay_parent = parent_hash;
		c.descriptor.para_id = 1.into();
		c
	};

	let candidate_b = {
		let mut c = dummy_committed_candidate_receipt(dummy_hash());
		c.descriptor.relay_parent = parent_hash;
		c.descriptor.para_id = 2.into();
		c
	};

	let candidate_c = {
		let mut c = dummy_committed_candidate_receipt(dummy_hash());
		c.descriptor.relay_parent = parent_hash;
		c.descriptor.para_id = 3.into();
		c
	};

	let mut head_data = ActiveHeadData::new(
		IndexedVec::<ValidatorIndex, ValidatorId>::from(validators),
		session_index,
		PerLeafSpan::new(Arc::new(jaeger::Span::Disabled), "test"),
	);

	let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
	let alice_public = SyncCryptoStore::sr25519_generate_new(
		&*keystore,
		ValidatorId::ID,
		Some(&Sr25519Keyring::Alice.to_seed()),
	)
	.unwrap();
	let bob_public = SyncCryptoStore::sr25519_generate_new(
		&*keystore,
		ValidatorId::ID,
		Some(&Sr25519Keyring::Bob.to_seed()),
	)
	.unwrap();

	// note A
	let a_seconded_val_0 = block_on(SignedFullStatement::sign(
		&keystore,
		Statement::Seconded(candidate_a.clone()),
		&signing_context,
		ValidatorIndex(0),
		&alice_public.into(),
	))
	.ok()
	.flatten()
	.expect("should be signed");
	assert!(head_data
		.check_useful_or_unknown(&a_seconded_val_0.clone().convert_payload().into())
		.is_ok());
	let noted = head_data.note_statement(a_seconded_val_0.clone());

	assert_matches!(noted, NotedStatement::Fresh(_));

	// note A (duplicate)
	assert_eq!(
		head_data.check_useful_or_unknown(&a_seconded_val_0.clone().convert_payload().into()),
		Err(DeniedStatement::UsefulButKnown),
	);
	let noted = head_data.note_statement(a_seconded_val_0);

	assert_matches!(noted, NotedStatement::UsefulButKnown);

	// note B
	let statement = block_on(SignedFullStatement::sign(
		&keystore,
		Statement::Seconded(candidate_b.clone()),
		&signing_context,
		ValidatorIndex(0),
		&alice_public.into(),
	))
	.ok()
	.flatten()
	.expect("should be signed");
	assert!(head_data
		.check_useful_or_unknown(&statement.clone().convert_payload().into())
		.is_ok());
	let noted = head_data.note_statement(statement);
	assert_matches!(noted, NotedStatement::Fresh(_));

	// note C (beyond 2 - ignored)
	let statement = block_on(SignedFullStatement::sign(
		&keystore,
		Statement::Seconded(candidate_c.clone()),
		&signing_context,
		ValidatorIndex(0),
		&alice_public.into(),
	))
	.ok()
	.flatten()
	.expect("should be signed");
	assert_eq!(
		head_data.check_useful_or_unknown(&statement.clone().convert_payload().into()),
		Err(DeniedStatement::NotUseful),
	);
	let noted = head_data.note_statement(statement);
	assert_matches!(noted, NotedStatement::NotUseful);

	// note B (new validator)
	let statement = block_on(SignedFullStatement::sign(
		&keystore,
		Statement::Seconded(candidate_b.clone()),
		&signing_context,
		ValidatorIndex(1),
		&bob_public.into(),
	))
	.ok()
	.flatten()
	.expect("should be signed");
	assert!(head_data
		.check_useful_or_unknown(&statement.clone().convert_payload().into())
		.is_ok());
	let noted = head_data.note_statement(statement);
	assert_matches!(noted, NotedStatement::Fresh(_));

	// note C (new validator)
	let statement = block_on(SignedFullStatement::sign(
		&keystore,
		Statement::Seconded(candidate_c.clone()),
		&signing_context,
		ValidatorIndex(1),
		&bob_public.into(),
	))
	.ok()
	.flatten()
	.expect("should be signed");
	assert!(head_data
		.check_useful_or_unknown(&statement.clone().convert_payload().into())
		.is_ok());
	let noted = head_data.note_statement(statement);
	assert_matches!(noted, NotedStatement::Fresh(_));
}

#[test]
fn note_local_works() {
	let hash_a = CandidateHash([1; 32].into());
	let hash_b = CandidateHash([2; 32].into());

	let mut per_peer_tracker = VcPerPeerTracker::default();
	per_peer_tracker.note_local(hash_a.clone());
	per_peer_tracker.note_local(hash_b.clone());

	assert!(per_peer_tracker.local_observed.contains(&hash_a));
	assert!(per_peer_tracker.local_observed.contains(&hash_b));

	assert!(!per_peer_tracker.remote_observed.contains(&hash_a));
	assert!(!per_peer_tracker.remote_observed.contains(&hash_b));
}

#[test]
fn note_remote_works() {
	let hash_a = CandidateHash([1; 32].into());
	let hash_b = CandidateHash([2; 32].into());
	let hash_c = CandidateHash([3; 32].into());

	let mut per_peer_tracker = VcPerPeerTracker::default();
	assert!(per_peer_tracker.note_remote(hash_a.clone()));
	assert!(per_peer_tracker.note_remote(hash_b.clone()));
	assert!(!per_peer_tracker.note_remote(hash_c.clone()));

	assert!(per_peer_tracker.remote_observed.contains(&hash_a));
	assert!(per_peer_tracker.remote_observed.contains(&hash_b));
	assert!(!per_peer_tracker.remote_observed.contains(&hash_c));

	assert!(!per_peer_tracker.local_observed.contains(&hash_a));
	assert!(!per_peer_tracker.local_observed.contains(&hash_b));
	assert!(!per_peer_tracker.local_observed.contains(&hash_c));
}

#[test]
fn per_peer_relay_parent_knowledge_send() {
	let mut knowledge = PeerRelayParentKnowledge::default();

	let hash_a = CandidateHash([1; 32].into());

	// Sending an un-pinned statement should not work and should have no effect.
	assert!(!knowledge.can_send(&(CompactStatement::Valid(hash_a), ValidatorIndex(0))));
	assert!(!knowledge.is_known_candidate(&hash_a));
	assert!(knowledge.sent_statements.is_empty());
	assert!(knowledge.received_statements.is_empty());
	assert!(knowledge.seconded_counts.is_empty());
	assert!(knowledge.received_message_count.is_empty());

	// Make the peer aware of the candidate.
	assert_eq!(knowledge.send(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0))), true);
	assert_eq!(knowledge.send(&(CompactStatement::Seconded(hash_a), ValidatorIndex(1))), false);
	assert!(knowledge.is_known_candidate(&hash_a));
	assert_eq!(knowledge.sent_statements.len(), 2);
	assert!(knowledge.received_statements.is_empty());
	assert_eq!(knowledge.seconded_counts.len(), 2);
	assert!(knowledge.received_message_count.get(&hash_a).is_none());

	// And now it should accept the dependent message.
	assert_eq!(knowledge.send(&(CompactStatement::Valid(hash_a), ValidatorIndex(0))), false);
	assert!(knowledge.is_known_candidate(&hash_a));
	assert_eq!(knowledge.sent_statements.len(), 3);
	assert!(knowledge.received_statements.is_empty());
	assert_eq!(knowledge.seconded_counts.len(), 2);
	assert!(knowledge.received_message_count.get(&hash_a).is_none());
}

#[test]
fn cant_send_after_receiving() {
	let mut knowledge = PeerRelayParentKnowledge::default();

	let hash_a = CandidateHash([1; 32].into());
	assert!(knowledge
		.check_can_receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3)
		.is_ok());
	assert!(knowledge
		.receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3)
		.unwrap());
	assert!(!knowledge.can_send(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0))));
}

#[test]
fn per_peer_relay_parent_knowledge_receive() {
	let mut knowledge = PeerRelayParentKnowledge::default();

	let hash_a = CandidateHash([1; 32].into());

	assert_eq!(
		knowledge.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(0)), 3),
		Err(COST_UNEXPECTED_STATEMENT_UNKNOWN_CANDIDATE),
	);
	assert_eq!(
		knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(0)), 3),
		Err(COST_UNEXPECTED_STATEMENT_UNKNOWN_CANDIDATE),
	);

	assert!(knowledge
		.check_can_receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3)
		.is_ok());
	assert_eq!(
		knowledge.receive(&(CompactStatement::Seconded(hash_a), ValidatorIndex(0)), 3),
		Ok(true),
	);

	// Push statements up to the flood limit.
	assert!(knowledge
		.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(1)), 3)
		.is_ok());
	assert_eq!(
		knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(1)), 3),
		Ok(false),
	);

	assert!(knowledge.is_known_candidate(&hash_a));
	assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 2);

	assert!(knowledge
		.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3)
		.is_ok());
	assert_eq!(
		knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3),
		Ok(false),
	);

	assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 3);

	assert_eq!(
		knowledge.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(7)), 3),
		Err(COST_APPARENT_FLOOD),
	);
	assert_eq!(
		knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(7)), 3),
		Err(COST_APPARENT_FLOOD),
	);

	assert_eq!(*knowledge.received_message_count.get(&hash_a).unwrap(), 3);
	assert_eq!(knowledge.received_statements.len(), 3); // number of prior `Ok`s.

	// Now make sure that the seconding limit is respected.
	let hash_b = CandidateHash([2; 32].into());
	let hash_c = CandidateHash([3; 32].into());

	assert!(knowledge
		.check_can_receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3)
		.is_ok());
	assert_eq!(
		knowledge.receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3),
		Ok(true),
	);

	assert_eq!(
		knowledge.check_can_receive(&(CompactStatement::Seconded(hash_c), ValidatorIndex(0)), 3),
		Err(COST_UNEXPECTED_STATEMENT_REMOTE),
	);
	assert_eq!(
		knowledge.receive(&(CompactStatement::Seconded(hash_c), ValidatorIndex(0)), 3),
		Err(COST_UNEXPECTED_STATEMENT_REMOTE),
	);

	// Last, make sure that already-known statements are disregarded.
	assert_eq!(
		knowledge.check_can_receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3),
		Err(COST_DUPLICATE_STATEMENT),
	);
	assert_eq!(
		knowledge.receive(&(CompactStatement::Valid(hash_a), ValidatorIndex(2)), 3),
		Err(COST_DUPLICATE_STATEMENT),
	);

	assert_eq!(
		knowledge.check_can_receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3),
		Err(COST_DUPLICATE_STATEMENT),
	);
	assert_eq!(
		knowledge.receive(&(CompactStatement::Seconded(hash_b), ValidatorIndex(0)), 3),
		Err(COST_DUPLICATE_STATEMENT),
	);
}

#[test]
fn peer_view_update_sends_messages() {
	let hash_a = Hash::repeat_byte(1);
	let hash_b = Hash::repeat_byte(2);
	let hash_c = Hash::repeat_byte(3);

	let candidate = {
		let mut c = dummy_committed_candidate_receipt(dummy_hash());
		c.descriptor.relay_parent = hash_c;
		c.descriptor.para_id = ParaId::from(1_u32);
		c
	};
	let candidate_hash = candidate.hash();

	let old_view = view![hash_a, hash_b];
	let new_view = view![hash_b, hash_c];

	let mut active_heads = HashMap::new();
	let validators = vec![
		Sr25519Keyring::Alice.public().into(),
		Sr25519Keyring::Bob.public().into(),
		Sr25519Keyring::Charlie.public().into(),
	];

	let session_index = 1;
	let signing_context = SigningContext { parent_hash: hash_c, session_index };

	let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());

	let alice_public = SyncCryptoStore::sr25519_generate_new(
		&*keystore,
		ValidatorId::ID,
		Some(&Sr25519Keyring::Alice.to_seed()),
	)
	.unwrap();
	let bob_public = SyncCryptoStore::sr25519_generate_new(
		&*keystore,
		ValidatorId::ID,
		Some(&Sr25519Keyring::Bob.to_seed()),
	)
	.unwrap();
	let charlie_public = SyncCryptoStore::sr25519_generate_new(
		&*keystore,
		ValidatorId::ID,
		Some(&Sr25519Keyring::Charlie.to_seed()),
	)
	.unwrap();

	let new_head_data = {
		let mut data = ActiveHeadData::new(
			IndexedVec::<ValidatorIndex, ValidatorId>::from(validators),
			session_index,
			PerLeafSpan::new(Arc::new(jaeger::Span::Disabled), "test"),
		);

		let statement = block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate.clone()),
			&signing_context,
			ValidatorIndex(0),
			&alice_public.into(),
		))
		.ok()
		.flatten()
		.expect("should be signed");
		assert!(data
			.check_useful_or_unknown(&statement.clone().convert_payload().into())
			.is_ok());
		let noted = data.note_statement(statement);

		assert_matches!(noted, NotedStatement::Fresh(_));

		let statement = block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Valid(candidate_hash),
			&signing_context,
			ValidatorIndex(1),
			&bob_public.into(),
		))
		.ok()
		.flatten()
		.expect("should be signed");
		assert!(data
			.check_useful_or_unknown(&statement.clone().convert_payload().into())
			.is_ok());
		let noted = data.note_statement(statement);

		assert_matches!(noted, NotedStatement::Fresh(_));

		let statement = block_on(SignedFullStatement::sign(
			&keystore,
			Statement::Valid(candidate_hash),
			&signing_context,
			ValidatorIndex(2),
			&charlie_public.into(),
		))
		.ok()
		.flatten()
		.expect("should be signed");
		assert!(data
			.check_useful_or_unknown(&statement.clone().convert_payload().into())
			.is_ok());
		let noted = data.note_statement(statement);
		assert_matches!(noted, NotedStatement::Fresh(_));

		data
	};

	active_heads.insert(hash_c, new_head_data);

	let mut peer_data = PeerData {
		view: old_view,
		view_knowledge: {
			let mut k = HashMap::new();

			k.insert(hash_a, Default::default());
			k.insert(hash_b, Default::default());

			k
		},
		maybe_authority: None,
	};

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context::<
		StatementDistributionMessage,
		_,
	>(pool);
	let peer = PeerId::random();

	executor::block_on(async move {
		let mut topology = GridNeighbors::empty();
		topology.peers_x = HashSet::from_iter(vec![peer.clone()].into_iter());
		update_peer_view_and_maybe_send_unlocked(
			peer.clone(),
			&topology,
			&mut peer_data,
			&mut ctx,
			&active_heads,
			new_view.clone(),
			&Default::default(),
			&mut AlwaysZeroRng,
		)
		.await;

		assert_eq!(peer_data.view, new_view);
		assert!(!peer_data.view_knowledge.contains_key(&hash_a));
		assert!(peer_data.view_knowledge.contains_key(&hash_b));

		let c_knowledge = peer_data.view_knowledge.get(&hash_c).unwrap();

		assert!(c_knowledge.is_known_candidate(&candidate_hash));
		assert!(c_knowledge
			.sent_statements
			.contains(&(CompactStatement::Seconded(candidate_hash), ValidatorIndex(0))));
		assert!(c_knowledge
			.sent_statements
			.contains(&(CompactStatement::Valid(candidate_hash), ValidatorIndex(1))));
		assert!(c_knowledge
			.sent_statements
			.contains(&(CompactStatement::Valid(candidate_hash), ValidatorIndex(2))));

		// now see if we got the 3 messages from the active head data.
		let active_head = active_heads.get(&hash_c).unwrap();

		// semi-fragile because hashmap iterator ordering is undefined, but in practice
		// it will not change between runs of the program.
		for statement in active_head.statements_about(candidate_hash) {
			let message = handle.recv().await;
			let expected_to = vec![peer.clone()];
			let expected_payload =
				statement_message(hash_c, statement.statement.clone(), &Metrics::default());

			assert_matches!(
				message,
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
					to,
					payload,
				)) => {
					assert_eq!(to, expected_to);
					assert_eq!(payload, expected_payload)
				}
			)
		}
	});
}

#[test]
fn circulated_statement_goes_to_all_peers_with_view() {
	let hash_a = Hash::repeat_byte(1);
	let hash_b = Hash::repeat_byte(2);
	let hash_c = Hash::repeat_byte(3);

	let candidate = {
		let mut c = dummy_committed_candidate_receipt(dummy_hash());
		c.descriptor.relay_parent = hash_b;
		c.descriptor.para_id = ParaId::from(1_u32);
		c
	};

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	let peer_c = PeerId::random();

	let peer_a_view = view![hash_a];
	let peer_b_view = view![hash_a, hash_b];
	let peer_c_view = view![hash_b, hash_c];

	let session_index = 1;

	let peer_data_from_view = |view: View| PeerData {
		view: view.clone(),
		view_knowledge: view.iter().map(|v| (v.clone(), Default::default())).collect(),
		maybe_authority: None,
	};

	let mut peer_data: HashMap<_, _> = vec![
		(peer_a.clone(), peer_data_from_view(peer_a_view)),
		(peer_b.clone(), peer_data_from_view(peer_b_view)),
		(peer_c.clone(), peer_data_from_view(peer_c_view)),
	]
	.into_iter()
	.collect();

	let pool = sp_core::testing::TaskExecutor::new();
	let (mut ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context::<
		StatementDistributionMessage,
		_,
	>(pool);

	executor::block_on(async move {
		let signing_context = SigningContext { parent_hash: hash_b, session_index };

		let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
		let alice_public = CryptoStore::sr25519_generate_new(
			&*keystore,
			ValidatorId::ID,
			Some(&Sr25519Keyring::Alice.to_seed()),
		)
		.await
		.unwrap();

		let statement = SignedFullStatement::sign(
			&keystore,
			Statement::Seconded(candidate),
			&signing_context,
			ValidatorIndex(0),
			&alice_public.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let comparator = StoredStatementComparator {
			compact: statement.payload().to_compact(),
			validator_index: ValidatorIndex(0),
			signature: statement.signature().clone(),
		};
		let statement = StoredStatement { comparator: &comparator, statement: &statement };

		let mut topology = GridNeighbors::empty();
		topology.peers_x =
			HashSet::from_iter(vec![peer_a.clone(), peer_b.clone(), peer_c.clone()].into_iter());
		let needs_dependents = circulate_statement(
			RequiredRouting::GridXY,
			&topology,
			&mut peer_data,
			&mut ctx,
			hash_b,
			statement,
			Vec::new(),
			&Metrics::default(),
			&mut AlwaysZeroRng,
		)
		.await;

		{
			assert_eq!(needs_dependents.len(), 2);
			assert!(needs_dependents.contains(&peer_b));
			assert!(needs_dependents.contains(&peer_c));
		}

		let fingerprint = (statement.compact().clone(), ValidatorIndex(0));

		assert!(peer_data
			.get(&peer_b)
			.unwrap()
			.view_knowledge
			.get(&hash_b)
			.unwrap()
			.sent_statements
			.contains(&fingerprint));

		assert!(peer_data
			.get(&peer_c)
			.unwrap()
			.view_knowledge
			.get(&hash_b)
			.unwrap()
			.sent_statements
			.contains(&fingerprint));

		let message = handle.recv().await;
		assert_matches!(
			message,
			AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendValidationMessage(
				to,
				payload,
			)) => {
				assert_eq!(to.len(), 2);
				assert!(to.contains(&peer_b));
				assert!(to.contains(&peer_c));

				assert_eq!(
					payload,
					statement_message(hash_b, statement.statement.clone(), &Metrics::default()),
				);
			}
		)
	});
}

#[test]
fn receiving_from_one_sends_to_another_and_to_candidate_backing() {
	let hash_a = Hash::repeat_byte(1);

	let candidate = {
		let mut c = dummy_committed_candidate_receipt(dummy_hash());
		c.descriptor.relay_parent = hash_a;
		c.descriptor.para_id = 1.into();
		c
	};

	let peer_a = PeerId::random();
	let peer_b = PeerId::random();

	let validators = vec![
		Sr25519Keyring::Alice.pair(),
		Sr25519Keyring::Bob.pair(),
		Sr25519Keyring::Charlie.pair(),
	];

	let session_info = make_session_info(validators, vec![]);

	let session_index = 1;

	let pool = sp_core::testing::TaskExecutor::new();
	let (ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	let req_protocol_names = ReqProtocolNames::new(&GENESIS_HASH, None);
	let (statement_req_receiver, _) = IncomingRequest::get_config_receiver(&req_protocol_names);

	let bg = async move {
		let s = StatementDistributionSubsystem::new(
			Arc::new(LocalKeystore::in_memory()),
			statement_req_receiver,
			Default::default(),
			AlwaysZeroRng,
		);
		s.run(ctx).await.unwrap();
	};

	let test_fut = async move {
		// register our active heads.
		handle
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: hash_a,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionIndexForChild(tx))
			)
				if r == hash_a
			=> {
				let _ = tx.send(Ok(session_index));
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionInfo(sess_index, tx))
			)
				if r == hash_a && sess_index == session_index
			=> {
				let _ = tx.send(Ok(Some(session_info)));
			}
		);

		// notify of peers and view
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_a.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						None,
					),
				),
			})
			.await;

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						None,
					),
				),
			})
			.await;

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_a.clone(), view![hash_a]),
				),
			})
			.await;

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![hash_a]),
				),
			})
			.await;

		// receive a seconded statement from peer A. it should be propagated onwards to peer B and to
		// candidate backing.
		let statement = {
			let signing_context = SigningContext { parent_hash: hash_a, session_index };

			let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
			let alice_public = CryptoStore::sr25519_generate_new(
				&*keystore,
				ValidatorId::ID,
				Some(&Sr25519Keyring::Alice.to_seed()),
			)
			.await
			.unwrap();

			SignedFullStatement::sign(
				&keystore,
				Statement::Seconded(candidate),
				&signing_context,
				ValidatorIndex(0),
				&alice_public.into(),
			)
			.await
			.ok()
			.flatten()
			.expect("should be signed")
		};

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(
						peer_a.clone(),
						Versioned::V1(protocol_v1::StatementDistributionMessage::Statement(
							hash_a,
							statement.clone().into(),
						)),
					),
				),
			})
			.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(p, r)
			) if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST => {}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::CandidateBacking(
				CandidateBackingMessage::Statement(r, s)
			) if r == hash_a && s == statement => {}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendValidationMessage(
					recipients,
					Versioned::V1(protocol_v1::ValidationProtocol::StatementDistribution(
						protocol_v1::StatementDistributionMessage::Statement(r, s)
					)),
				)
			) => {
				assert_eq!(recipients, vec![peer_b.clone()]);
				assert_eq!(r, hash_a);
				assert_eq!(s, statement.into());
			}
		);
		handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::pin_mut!(test_fut);
	futures::pin_mut!(bg);

	executor::block_on(future::join(test_fut, bg));
}

#[test]
fn receiving_large_statement_from_one_sends_to_another_and_to_candidate_backing() {
	sp_tracing::try_init_simple();
	let hash_a = Hash::repeat_byte(1);
	let hash_b = Hash::repeat_byte(2);

	let candidate = {
		let mut c = dummy_committed_candidate_receipt(dummy_hash());
		c.descriptor.relay_parent = hash_a;
		c.descriptor.para_id = 1.into();
		c.commitments.new_validation_code = Some(ValidationCode(vec![1, 2, 3]));
		c
	};

	let peer_a = PeerId::random(); // Alice
	let peer_b = PeerId::random(); // Bob
	let peer_c = PeerId::random(); // Charlie
	let peer_bad = PeerId::random(); // No validator

	let validators = vec![
		Sr25519Keyring::Alice.pair(),
		Sr25519Keyring::Bob.pair(),
		Sr25519Keyring::Charlie.pair(),
		// We:
		Sr25519Keyring::Ferdie.pair(),
	];

	let session_info = make_session_info(validators, vec![vec![0, 1, 2, 4], vec![3]]);

	let session_index = 1;

	let pool = sp_core::testing::TaskExecutor::new();
	let (ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	let req_protocol_names = ReqProtocolNames::new(&GENESIS_HASH, None);
	let (statement_req_receiver, mut req_cfg) =
		IncomingRequest::get_config_receiver(&req_protocol_names);

	let bg = async move {
		let s = StatementDistributionSubsystem::new(
			make_ferdie_keystore(),
			statement_req_receiver,
			Default::default(),
			AlwaysZeroRng,
		);
		s.run(ctx).await.unwrap();
	};

	let test_fut = async move {
		// register our active heads.
		handle
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: hash_a,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionIndexForChild(tx))
			)
				if r == hash_a
			=> {
				let _ = tx.send(Ok(session_index));
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionInfo(sess_index, tx))
			)
				if r == hash_a && sess_index == session_index
			=> {
				let _ = tx.send(Ok(Some(session_info)));
			}
		);

		// notify of peers and view
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_a.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						Some(HashSet::from([Sr25519Keyring::Alice.public().into()])),
					),
				),
			})
			.await;

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						Some(HashSet::from([Sr25519Keyring::Bob.public().into()])),
					),
				),
			})
			.await;
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_c.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						Some(HashSet::from([Sr25519Keyring::Charlie.public().into()])),
					),
				),
			})
			.await;
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_bad.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						None,
					),
				),
			})
			.await;

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_a.clone(), view![hash_a]),
				),
			})
			.await;

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![hash_a]),
				),
			})
			.await;
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_c.clone(), view![hash_a]),
				),
			})
			.await;
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_bad.clone(), view![hash_a]),
				),
			})
			.await;

		// receive a seconded statement from peer A, which does not provide the request data,
		// then get that data from peer C. It should be propagated onwards to peer B and to
		// candidate backing.
		let statement = {
			let signing_context = SigningContext { parent_hash: hash_a, session_index };

			let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
			let alice_public = CryptoStore::sr25519_generate_new(
				&*keystore,
				ValidatorId::ID,
				Some(&Sr25519Keyring::Alice.to_seed()),
			)
			.await
			.unwrap();

			SignedFullStatement::sign(
				&keystore,
				Statement::Seconded(candidate.clone()),
				&signing_context,
				ValidatorIndex(0),
				&alice_public.into(),
			)
			.await
			.ok()
			.flatten()
			.expect("should be signed")
		};

		let metadata = derive_metadata_assuming_seconded(hash_a, statement.clone().into());

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(
						peer_a.clone(),
						Versioned::V1(protocol_v1::StatementDistributionMessage::LargeStatement(
							metadata.clone(),
						)),
					),
				),
			})
			.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendRequests(
					mut reqs, IfDisconnected::ImmediateError
				)
			) => {
				let reqs = reqs.pop().unwrap();
				let outgoing = match reqs {
					Requests::StatementFetchingV1(outgoing) => outgoing,
					_ => panic!("Unexpected request"),
				};
				let req = outgoing.payload;
				assert_eq!(req.relay_parent, metadata.relay_parent);
				assert_eq!(req.candidate_hash, metadata.candidate_hash);
				assert_eq!(outgoing.peer, Recipient::Peer(peer_a));
				// Just drop request - should trigger error.
			}
		);

		// There is a race between request handler asking for more peers and processing of the
		// coming `PeerMessage`s, we want the request handler to ask first here for better test
		// coverage:
		Delay::new(Duration::from_millis(20)).await;

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(
						peer_c.clone(),
						Versioned::V1(protocol_v1::StatementDistributionMessage::LargeStatement(
							metadata.clone(),
						)),
					),
				),
			})
			.await;

		// Malicious peer:
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(
						peer_bad.clone(),
						Versioned::V1(protocol_v1::StatementDistributionMessage::LargeStatement(
							metadata.clone(),
						)),
					),
				),
			})
			.await;

		// Let c fail once too:
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendRequests(
					mut reqs, IfDisconnected::ImmediateError
				)
			) => {
				let reqs = reqs.pop().unwrap();
				let outgoing = match reqs {
					Requests::StatementFetchingV1(outgoing) => outgoing,
					_ => panic!("Unexpected request"),
				};
				let req = outgoing.payload;
				assert_eq!(req.relay_parent, metadata.relay_parent);
				assert_eq!(req.candidate_hash, metadata.candidate_hash);
				assert_eq!(outgoing.peer, Recipient::Peer(peer_c));
			}
		);

		// a fails again:
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendRequests(
					mut reqs, IfDisconnected::ImmediateError
				)
			) => {
				let reqs = reqs.pop().unwrap();
				let outgoing = match reqs {
					Requests::StatementFetchingV1(outgoing) => outgoing,
					_ => panic!("Unexpected request"),
				};
				let req = outgoing.payload;
				assert_eq!(req.relay_parent, metadata.relay_parent);
				assert_eq!(req.candidate_hash, metadata.candidate_hash);
				// On retry, we should have reverse order:
				assert_eq!(outgoing.peer, Recipient::Peer(peer_a));
			}
		);

		// Send invalid response (all other peers have been tried now):
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendRequests(
					mut reqs, IfDisconnected::ImmediateError
				)
			) => {
				let reqs = reqs.pop().unwrap();
				let outgoing = match reqs {
					Requests::StatementFetchingV1(outgoing) => outgoing,
					_ => panic!("Unexpected request"),
				};
				let req = outgoing.payload;
				assert_eq!(req.relay_parent, metadata.relay_parent);
				assert_eq!(req.candidate_hash, metadata.candidate_hash);
				assert_eq!(outgoing.peer, Recipient::Peer(peer_bad));
				let bad_candidate = {
					let mut bad = candidate.clone();
					bad.descriptor.para_id = 0xeadbeaf.into();
					bad
				};
				let response = StatementFetchingResponse::Statement(bad_candidate);
				outgoing.pending_response.send(Ok(response.encode())).unwrap();
			}
		);

		// Should get punished and never tried again:
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(p, r)
			) if p == peer_bad && r == COST_WRONG_HASH => {}
		);

		// a is tried again (retried in reverse order):
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendRequests(
					mut reqs, IfDisconnected::ImmediateError
				)
			) => {
				let reqs = reqs.pop().unwrap();
				let outgoing = match reqs {
					Requests::StatementFetchingV1(outgoing) => outgoing,
					_ => panic!("Unexpected request"),
				};
				let req = outgoing.payload;
				assert_eq!(req.relay_parent, metadata.relay_parent);
				assert_eq!(req.candidate_hash, metadata.candidate_hash);
				// On retry, we should have reverse order:
				assert_eq!(outgoing.peer, Recipient::Peer(peer_a));
			}
		);

		// c succeeds now:
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendRequests(
					mut reqs, IfDisconnected::ImmediateError
				)
			) => {
				let reqs = reqs.pop().unwrap();
				let outgoing = match reqs {
					Requests::StatementFetchingV1(outgoing) => outgoing,
					_ => panic!("Unexpected request"),
				};
				let req = outgoing.payload;
				assert_eq!(req.relay_parent, metadata.relay_parent);
				assert_eq!(req.candidate_hash, metadata.candidate_hash);
				// On retry, we should have reverse order:
				assert_eq!(outgoing.peer, Recipient::Peer(peer_c));
				let response = StatementFetchingResponse::Statement(candidate.clone());
				outgoing.pending_response.send(Ok(response.encode())).unwrap();
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(p, r)
			) if p == peer_a && r == COST_FETCH_FAIL => {}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(p, r)
			) if p == peer_c && r == BENEFIT_VALID_RESPONSE => {}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(p, r)
			) if p == peer_a && r == BENEFIT_VALID_STATEMENT_FIRST => {}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::CandidateBacking(
				CandidateBackingMessage::Statement(r, s)
			) if r == hash_a && s == statement => {}
		);

		// Now messages should go out:
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendValidationMessage(
					mut recipients,
					Versioned::V1(protocol_v1::ValidationProtocol::StatementDistribution(
						protocol_v1::StatementDistributionMessage::LargeStatement(meta)
					)),
				)
			) => {
				gum::debug!(
					target: LOG_TARGET,
					?recipients,
					"Recipients received"
				);
				recipients.sort();
				let mut expected = vec![peer_b, peer_c, peer_bad];
				expected.sort();
				assert_eq!(recipients, expected);
				assert_eq!(meta.relay_parent, hash_a);
				assert_eq!(meta.candidate_hash, statement.payload().candidate_hash());
				assert_eq!(meta.signed_by, statement.validator_index());
				assert_eq!(&meta.signature, statement.signature());
			}
		);

		// Now that it has the candidate it should answer requests accordingly (even after a
		// failed request):

		// Failing request first (wrong relay parent hash):
		let (pending_response, response_rx) = oneshot::channel();
		let inner_req = StatementFetchingRequest {
			relay_parent: hash_b,
			candidate_hash: metadata.candidate_hash,
		};
		let req = sc_network::config::IncomingRequest {
			peer: peer_b,
			payload: inner_req.encode(),
			pending_response,
		};
		req_cfg.inbound_queue.as_mut().unwrap().send(req).await.unwrap();
		assert_matches!(
			response_rx.await.unwrap().result,
			Err(()) => {}
		);

		// Another failing request (peer_a never received a statement from us, so it is not
		// allowed to request the data):
		let (pending_response, response_rx) = oneshot::channel();
		let inner_req = StatementFetchingRequest {
			relay_parent: metadata.relay_parent,
			candidate_hash: metadata.candidate_hash,
		};
		let req = sc_network::config::IncomingRequest {
			peer: peer_a,
			payload: inner_req.encode(),
			pending_response,
		};
		req_cfg.inbound_queue.as_mut().unwrap().send(req).await.unwrap();
		assert_matches!(
			response_rx.await.unwrap().result,
			Err(()) => {}
		);

		// And now the succeding request from peer_b:
		let (pending_response, response_rx) = oneshot::channel();
		let inner_req = StatementFetchingRequest {
			relay_parent: metadata.relay_parent,
			candidate_hash: metadata.candidate_hash,
		};
		let req = sc_network::config::IncomingRequest {
			peer: peer_b,
			payload: inner_req.encode(),
			pending_response,
		};
		req_cfg.inbound_queue.as_mut().unwrap().send(req).await.unwrap();
		let StatementFetchingResponse::Statement(committed) =
			Decode::decode(&mut response_rx.await.unwrap().result.unwrap().as_ref()).unwrap();
		assert_eq!(committed, candidate);

		handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::pin_mut!(test_fut);
	futures::pin_mut!(bg);

	executor::block_on(future::join(test_fut, bg));
}

#[test]
fn share_prioritizes_backing_group() {
	sp_tracing::try_init_simple();
	let hash_a = Hash::repeat_byte(1);

	let candidate = {
		let mut c = dummy_committed_candidate_receipt(dummy_hash());
		c.descriptor.relay_parent = hash_a;
		c.descriptor.para_id = 1.into();
		c.commitments.new_validation_code = Some(ValidationCode(vec![1, 2, 3]));
		c
	};

	let peer_a = PeerId::random(); // Alice
	let peer_b = PeerId::random(); // Bob
	let peer_c = PeerId::random(); // Charlie
	let peer_bad = PeerId::random(); // No validator
	let peer_other_group = PeerId::random(); //Ferdie

	let mut validators = vec![
		Sr25519Keyring::Alice.pair(),
		Sr25519Keyring::Bob.pair(),
		Sr25519Keyring::Charlie.pair(),
		// other group
		Sr25519Keyring::Dave.pair(),
		// We:
		Sr25519Keyring::Ferdie.pair(),
	];

	// Strictly speaking we only need MIN_GOSSIP_PEERS - 3 to make sure only priority peers
	// will be served, but by using a larger value we test for overflow errors:
	let dummy_count = MIN_GOSSIP_PEERS;

	// We artificially inflate our group, so there won't be any free slots for other peers. (We
	// want to test that our group is prioritized):
	let dummy_pairs: Vec<_> =
		std::iter::repeat_with(|| Pair::generate().0).take(dummy_count).collect();
	let dummy_peers: Vec<_> =
		std::iter::repeat_with(|| PeerId::random()).take(dummy_count).collect();

	validators = validators.into_iter().chain(dummy_pairs.clone()).collect();

	let mut first_group = vec![0, 1, 2, 4];
	first_group.append(&mut (0..dummy_count as u32).map(|v| v + 5).collect());
	let session_info = make_session_info(validators, vec![first_group, vec![3]]);

	let session_index = 1;

	let pool = sp_core::testing::TaskExecutor::new();
	let (ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	let req_protocol_names = ReqProtocolNames::new(&GENESIS_HASH, None);
	let (statement_req_receiver, mut req_cfg) =
		IncomingRequest::get_config_receiver(&req_protocol_names);

	let bg = async move {
		let s = StatementDistributionSubsystem::new(
			make_ferdie_keystore(),
			statement_req_receiver,
			Default::default(),
			AlwaysZeroRng,
		);
		s.run(ctx).await.unwrap();
	};

	let test_fut = async move {
		// register our active heads.
		handle
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: hash_a,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionIndexForChild(tx))
			)
				if r == hash_a
			=> {
				let _ = tx.send(Ok(session_index));
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionInfo(sess_index, tx))
			)
				if r == hash_a && sess_index == session_index
			=> {
				let _ = tx.send(Ok(Some(session_info)));
			}
		);

		// notify of dummy peers and view
		for (peer, pair) in dummy_peers.clone().into_iter().zip(dummy_pairs) {
			handle
				.send(FromOrchestra::Communication {
					msg: StatementDistributionMessage::NetworkBridgeUpdate(
						NetworkBridgeEvent::PeerConnected(
							peer,
							ObservedRole::Full,
							ValidationVersion::V1.into(),
							Some(HashSet::from([pair.public().into()])),
						),
					),
				})
				.await;

			handle
				.send(FromOrchestra::Communication {
					msg: StatementDistributionMessage::NetworkBridgeUpdate(
						NetworkBridgeEvent::PeerViewChange(peer, view![hash_a]),
					),
				})
				.await;
		}

		// notify of peers and view
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_a.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						Some(HashSet::from([Sr25519Keyring::Alice.public().into()])),
					),
				),
			})
			.await;
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_b.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						Some(HashSet::from([Sr25519Keyring::Bob.public().into()])),
					),
				),
			})
			.await;
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_c.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						Some(HashSet::from([Sr25519Keyring::Charlie.public().into()])),
					),
				),
			})
			.await;
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_bad.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						None,
					),
				),
			})
			.await;
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_other_group.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						Some(HashSet::from([Sr25519Keyring::Dave.public().into()])),
					),
				),
			})
			.await;

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_a.clone(), view![hash_a]),
				),
			})
			.await;

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![hash_a]),
				),
			})
			.await;
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_c.clone(), view![hash_a]),
				),
			})
			.await;
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_bad.clone(), view![hash_a]),
				),
			})
			.await;
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_other_group.clone(), view![hash_a]),
				),
			})
			.await;

		// receive a seconded statement from peer A, which does not provide the request data,
		// then get that data from peer C. It should be propagated onwards to peer B and to
		// candidate backing.
		let statement = {
			let signing_context = SigningContext { parent_hash: hash_a, session_index };

			let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
			let ferdie_public = CryptoStore::sr25519_generate_new(
				&*keystore,
				ValidatorId::ID,
				Some(&Sr25519Keyring::Ferdie.to_seed()),
			)
			.await
			.unwrap();

			SignedFullStatement::sign(
				&keystore,
				Statement::Seconded(candidate.clone()),
				&signing_context,
				ValidatorIndex(4),
				&ferdie_public.into(),
			)
			.await
			.ok()
			.flatten()
			.expect("should be signed")
		};

		let metadata = derive_metadata_assuming_seconded(hash_a, statement.clone().into());

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::Share(hash_a, statement.clone()),
			})
			.await;

		// Messages should go out:
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendValidationMessage(
					mut recipients,
					Versioned::V1(protocol_v1::ValidationProtocol::StatementDistribution(
						protocol_v1::StatementDistributionMessage::LargeStatement(meta)
					)),
				)
			) => {
				gum::debug!(
					target: LOG_TARGET,
					?recipients,
					"Recipients received"
				);
				recipients.sort();
				// We expect only our backing group to be the recipients, du to the inflated
				// test group above:
				let mut expected: Vec<_> = vec![peer_a, peer_b, peer_c].into_iter().chain(dummy_peers).collect();
				expected.sort();
				assert_eq!(recipients.len(), expected.len());
				assert_eq!(recipients, expected);
				assert_eq!(meta.relay_parent, hash_a);
				assert_eq!(meta.candidate_hash, statement.payload().candidate_hash());
				assert_eq!(meta.signed_by, statement.validator_index());
				assert_eq!(&meta.signature, statement.signature());
			}
		);

		// Now that it has the candidate it should answer requests accordingly:

		let (pending_response, response_rx) = oneshot::channel();
		let inner_req = StatementFetchingRequest {
			relay_parent: metadata.relay_parent,
			candidate_hash: metadata.candidate_hash,
		};
		let req = sc_network::config::IncomingRequest {
			peer: peer_b,
			payload: inner_req.encode(),
			pending_response,
		};
		req_cfg.inbound_queue.as_mut().unwrap().send(req).await.unwrap();
		let StatementFetchingResponse::Statement(committed) =
			Decode::decode(&mut response_rx.await.unwrap().result.unwrap().as_ref()).unwrap();
		assert_eq!(committed, candidate);

		handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::pin_mut!(test_fut);
	futures::pin_mut!(bg);

	executor::block_on(future::join(test_fut, bg));
}

#[test]
fn peer_cant_flood_with_large_statements() {
	sp_tracing::try_init_simple();
	let hash_a = Hash::repeat_byte(1);

	let candidate = {
		let mut c = dummy_committed_candidate_receipt(dummy_hash());
		c.descriptor.relay_parent = hash_a;
		c.descriptor.para_id = 1.into();
		c.commitments.new_validation_code = Some(ValidationCode(vec![1, 2, 3]));
		c
	};

	let peer_a = PeerId::random(); // Alice

	let validators = vec![
		Sr25519Keyring::Alice.pair(),
		Sr25519Keyring::Bob.pair(),
		Sr25519Keyring::Charlie.pair(),
		// other group
		Sr25519Keyring::Dave.pair(),
		// We:
		Sr25519Keyring::Ferdie.pair(),
	];

	let first_group = vec![0, 1, 2, 4];
	let session_info = make_session_info(validators, vec![first_group, vec![3]]);

	let session_index = 1;

	let pool = sp_core::testing::TaskExecutor::new();
	let (ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	let req_protocol_names = ReqProtocolNames::new(&GENESIS_HASH, None);
	let (statement_req_receiver, _) = IncomingRequest::get_config_receiver(&req_protocol_names);
	let bg = async move {
		let s = StatementDistributionSubsystem::new(
			make_ferdie_keystore(),
			statement_req_receiver,
			Default::default(),
			AlwaysZeroRng,
		);
		s.run(ctx).await.unwrap();
	};

	let test_fut = async move {
		// register our active heads.
		handle
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: hash_a,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionIndexForChild(tx))
			)
				if r == hash_a
			=> {
				let _ = tx.send(Ok(session_index));
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionInfo(sess_index, tx))
			)
				if r == hash_a && sess_index == session_index
			=> {
				let _ = tx.send(Ok(Some(session_info)));
			}
		);

		// notify of peers and view
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerConnected(
						peer_a.clone(),
						ObservedRole::Full,
						ValidationVersion::V1.into(),
						Some(HashSet::from([Sr25519Keyring::Alice.public().into()])),
					),
				),
			})
			.await;

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerViewChange(peer_a.clone(), view![hash_a]),
				),
			})
			.await;

		// receive a seconded statement from peer A.
		let statement = {
			let signing_context = SigningContext { parent_hash: hash_a, session_index };

			let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
			let alice_public = CryptoStore::sr25519_generate_new(
				&*keystore,
				ValidatorId::ID,
				Some(&Sr25519Keyring::Alice.to_seed()),
			)
			.await
			.unwrap();

			SignedFullStatement::sign(
				&keystore,
				Statement::Seconded(candidate.clone()),
				&signing_context,
				ValidatorIndex(0),
				&alice_public.into(),
			)
			.await
			.ok()
			.flatten()
			.expect("should be signed")
		};

		let metadata = derive_metadata_assuming_seconded(hash_a, statement.clone().into());

		for _ in 0..MAX_LARGE_STATEMENTS_PER_SENDER + 1 {
			handle
				.send(FromOrchestra::Communication {
					msg: StatementDistributionMessage::NetworkBridgeUpdate(
						NetworkBridgeEvent::PeerMessage(
							peer_a.clone(),
							Versioned::V1(
								protocol_v1::StatementDistributionMessage::LargeStatement(
									metadata.clone(),
								),
							),
						),
					),
				})
				.await;
		}

		// We should try to fetch the data and punish the peer (but we don't know what comes
		// first):
		let mut requested = false;
		let mut punished = false;
		for _ in 0..2 {
			match handle.recv().await {
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(
					mut reqs,
					IfDisconnected::ImmediateError,
				)) => {
					let reqs = reqs.pop().unwrap();
					let outgoing = match reqs {
						Requests::StatementFetchingV1(outgoing) => outgoing,
						_ => panic!("Unexpected request"),
					};
					let req = outgoing.payload;
					assert_eq!(req.relay_parent, metadata.relay_parent);
					assert_eq!(req.candidate_hash, metadata.candidate_hash);
					assert_eq!(outgoing.peer, Recipient::Peer(peer_a));
					// Just drop request - should trigger error.
					requested = true;
				},

				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::ReportPeer(p, r))
					if p == peer_a && r == COST_APPARENT_FLOOD =>
				{
					punished = true;
				},

				m => panic!("Unexpected message: {:?}", m),
			}
		}
		assert!(requested, "large data has not been requested.");
		assert!(punished, "Peer should have been punished for flooding.");

		handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::pin_mut!(test_fut);
	futures::pin_mut!(bg);

	executor::block_on(future::join(test_fut, bg));
}

// This test addresses an issue when received knowledge is not updated on a
// subsequent `Seconded` statements
// See https://github.com/paritytech/polkadot/pull/5177
#[test]
fn handle_multiple_seconded_statements() {
	let relay_parent_hash = Hash::repeat_byte(1);

	let candidate = dummy_committed_candidate_receipt(relay_parent_hash);
	let candidate_hash = candidate.hash();

	// We want to ensure that our peers are not lucky
	let mut all_peers: Vec<PeerId> = Vec::with_capacity(MIN_GOSSIP_PEERS + 4);
	let peer_a = PeerId::random();
	let peer_b = PeerId::random();
	assert_ne!(peer_a, peer_b);

	for _ in 0..MIN_GOSSIP_PEERS + 2 {
		all_peers.push(PeerId::random());
	}
	all_peers.push(peer_a.clone());
	all_peers.push(peer_b.clone());

	let mut lucky_peers = all_peers.clone();
	util::choose_random_subset_with_rng(
		|_| false,
		&mut lucky_peers,
		&mut AlwaysZeroRng,
		MIN_GOSSIP_PEERS,
	);
	lucky_peers.sort();
	assert_eq!(lucky_peers.len(), MIN_GOSSIP_PEERS);
	assert!(!lucky_peers.contains(&peer_a));
	assert!(!lucky_peers.contains(&peer_b));

	let validators = vec![
		Sr25519Keyring::Alice.pair(),
		Sr25519Keyring::Bob.pair(),
		Sr25519Keyring::Charlie.pair(),
	];

	let session_info = make_session_info(validators, vec![]);

	let session_index = 1;

	let pool = sp_core::testing::TaskExecutor::new();
	let (ctx, mut handle) = polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);

	let req_protocol_names = ReqProtocolNames::new(&GENESIS_HASH, None);
	let (statement_req_receiver, _) = IncomingRequest::get_config_receiver(&req_protocol_names);

	let virtual_overseer_fut = async move {
		let s = StatementDistributionSubsystem::new(
			Arc::new(LocalKeystore::in_memory()),
			statement_req_receiver,
			Default::default(),
			AlwaysZeroRng,
		);
		s.run(ctx).await.unwrap();
	};

	let test_fut = async move {
		// register our active heads.
		handle
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: relay_parent_hash,
					number: 1,
					status: LeafStatus::Fresh,
					span: Arc::new(jaeger::Span::Disabled),
				}),
			)))
			.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionIndexForChild(tx))
			)
				if r == relay_parent_hash
			=> {
				let _ = tx.send(Ok(session_index));
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::RuntimeApi(
				RuntimeApiMessage::Request(r, RuntimeApiRequest::SessionInfo(sess_index, tx))
			)
				if r == relay_parent_hash && sess_index == session_index
			=> {
				let _ = tx.send(Ok(Some(session_info)));
			}
		);

		// notify of peers and view
		for peer in all_peers.iter() {
			handle
				.send(FromOrchestra::Communication {
					msg: StatementDistributionMessage::NetworkBridgeUpdate(
						NetworkBridgeEvent::PeerConnected(
							peer.clone(),
							ObservedRole::Full,
							ValidationVersion::V1.into(),
							None,
						),
					),
				})
				.await;
			handle
				.send(FromOrchestra::Communication {
					msg: StatementDistributionMessage::NetworkBridgeUpdate(
						NetworkBridgeEvent::PeerViewChange(peer.clone(), view![relay_parent_hash]),
					),
				})
				.await;
		}

		// Set up a topology which puts peers a & b in a column together.
		let gossip_topology = {
			// create a lucky_peers+1 * lucky_peers+1 grid topology where we are at index 2, sharing
			// a row with peer_a (0) and peer_b (1) and a column with all the lucky peers.
			// the rest is filled with junk.
			// This is an absolute garbage hack depending on quirks of the implementation
			// and not on sound architecture.

			let n_lucky = lucky_peers.len();
			let dim = n_lucky + 1;
			let grid_size = dim * dim;
			let topology_peer_info: Vec<_> = (0..grid_size)
				.map(|i| {
					if i == 0 {
						TopologyPeerInfo {
							peer_ids: vec![peer_a.clone()],
							validator_index: ValidatorIndex(0),
							discovery_id: AuthorityPair::generate().0.public(),
						}
					} else if i == 1 {
						TopologyPeerInfo {
							peer_ids: vec![peer_b.clone()],
							validator_index: ValidatorIndex(1),
							discovery_id: AuthorityPair::generate().0.public(),
						}
					} else if i == 2 {
						TopologyPeerInfo {
							peer_ids: vec![],
							validator_index: ValidatorIndex(2),
							discovery_id: AuthorityPair::generate().0.public(),
						}
					} else if (i - 2) % dim == 0 {
						let lucky_index = ((i - 2) / dim) - 1;
						TopologyPeerInfo {
							peer_ids: vec![lucky_peers[lucky_index].clone()],
							validator_index: ValidatorIndex(i as _),
							discovery_id: AuthorityPair::generate().0.public(),
						}
					} else {
						TopologyPeerInfo {
							peer_ids: vec![PeerId::random()],
							validator_index: ValidatorIndex(i as _),
							discovery_id: AuthorityPair::generate().0.public(),
						}
					}
				})
				.collect();

			// also a hack: this is only required to be accurate for
			// the validator indices we compute grid neighbors for.
			let mut shuffled_indices = vec![0; grid_size];
			shuffled_indices[2] = 2;

			// Some sanity checking to make sure this hack is set up correctly.
			let topology = SessionGridTopology::new(shuffled_indices, topology_peer_info);
			let grid_neighbors = topology.compute_grid_neighbors_for(ValidatorIndex(2)).unwrap();
			assert_eq!(grid_neighbors.peers_x.len(), 25);
			assert!(grid_neighbors.peers_x.contains(&peer_a));
			assert!(grid_neighbors.peers_x.contains(&peer_b));
			assert!(!grid_neighbors.peers_y.contains(&peer_b));
			assert!(!grid_neighbors.route_to_peer(RequiredRouting::GridY, &peer_b));
			assert_eq!(grid_neighbors.peers_y.len(), lucky_peers.len());
			for lucky in &lucky_peers {
				assert!(grid_neighbors.peers_y.contains(lucky));
			}

			network_bridge_event::NewGossipTopology {
				session: 1,
				topology,
				local_index: Some(ValidatorIndex(2)),
			}
		};

		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::NewGossipTopology(gossip_topology),
				),
			})
			.await;

		// receive a seconded statement from peer A. it should be propagated onwards to peer B and to
		// candidate backing.
		let statement = {
			let signing_context = SigningContext { parent_hash: relay_parent_hash, session_index };

			let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
			let alice_public = CryptoStore::sr25519_generate_new(
				&*keystore,
				ValidatorId::ID,
				Some(&Sr25519Keyring::Alice.to_seed()),
			)
			.await
			.unwrap();

			SignedFullStatement::sign(
				&keystore,
				Statement::Seconded(candidate.clone()),
				&signing_context,
				ValidatorIndex(0),
				&alice_public.into(),
			)
			.await
			.ok()
			.flatten()
			.expect("should be signed")
		};

		// `PeerA` sends a `Seconded` message
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(
						peer_a.clone(),
						Versioned::V1(protocol_v1::StatementDistributionMessage::Statement(
							relay_parent_hash,
							statement.clone().into(),
						)),
					),
				),
			})
			.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(p, r)
			) => {
				assert_eq!(p, peer_a);
				assert_eq!(r, BENEFIT_VALID_STATEMENT_FIRST);
			}
		);

		// After the first valid statement, we expect messages to be circulated
		assert_matches!(
			handle.recv().await,
			AllMessages::CandidateBacking(
				CandidateBackingMessage::Statement(r, s)
			) => {
				assert_eq!(r, relay_parent_hash);
				assert_eq!(s, statement);
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendValidationMessage(
					recipients,
					Versioned::V1(protocol_v1::ValidationProtocol::StatementDistribution(
						protocol_v1::StatementDistributionMessage::Statement(r, s)
					)),
				)
			) => {
				assert!(!recipients.contains(&peer_b));
				assert_eq!(r, relay_parent_hash);
				assert_eq!(s, statement.clone().into());
			}
		);

		// `PeerB` sends a `Seconded` message: valid but known
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						Versioned::V1(protocol_v1::StatementDistributionMessage::Statement(
							relay_parent_hash,
							statement.clone().into(),
						)),
					),
				),
			})
			.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(p, r)
			) => {
				assert_eq!(p, peer_b);
				assert_eq!(r, BENEFIT_VALID_STATEMENT);
			}
		);

		// Create a `Valid` statement
		let statement = {
			let signing_context = SigningContext { parent_hash: relay_parent_hash, session_index };

			let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
			let alice_public = CryptoStore::sr25519_generate_new(
				&*keystore,
				ValidatorId::ID,
				Some(&Sr25519Keyring::Alice.to_seed()),
			)
			.await
			.unwrap();

			SignedFullStatement::sign(
				&keystore,
				Statement::Valid(candidate_hash),
				&signing_context,
				ValidatorIndex(0),
				&alice_public.into(),
			)
			.await
			.ok()
			.flatten()
			.expect("should be signed")
		};

		// `PeerA` sends a `Valid` message
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(
						peer_a.clone(),
						Versioned::V1(protocol_v1::StatementDistributionMessage::Statement(
							relay_parent_hash,
							statement.clone().into(),
						)),
					),
				),
			})
			.await;

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(p, r)
			) => {
				assert_eq!(p, peer_a);
				assert_eq!(r, BENEFIT_VALID_STATEMENT_FIRST);
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::CandidateBacking(
				CandidateBackingMessage::Statement(r, s)
			) => {
				assert_eq!(r, relay_parent_hash);
				assert_eq!(s, statement);
			}
		);

		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::SendValidationMessage(
					recipients,
					Versioned::V1(protocol_v1::ValidationProtocol::StatementDistribution(
						protocol_v1::StatementDistributionMessage::Statement(r, s)
					)),
				)
			) => {
				assert!(!recipients.contains(&peer_b));
				assert_eq!(r, relay_parent_hash);
				assert_eq!(s, statement.clone().into());
			}
		);

		// `PeerB` sends a `Valid` message
		handle
			.send(FromOrchestra::Communication {
				msg: StatementDistributionMessage::NetworkBridgeUpdate(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						Versioned::V1(protocol_v1::StatementDistributionMessage::Statement(
							relay_parent_hash,
							statement.clone().into(),
						)),
					),
				),
			})
			.await;

		// We expect that this is still valid despite the fact that `PeerB` was not
		// the first when sending `Seconded`
		assert_matches!(
			handle.recv().await,
			AllMessages::NetworkBridgeTx(
				NetworkBridgeTxMessage::ReportPeer(p, r)
			) => {
				assert_eq!(p, peer_b);
				assert_eq!(r, BENEFIT_VALID_STATEMENT);
			}
		);

		handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::pin_mut!(test_fut);
	futures::pin_mut!(virtual_overseer_fut);

	executor::block_on(future::join(test_fut, virtual_overseer_fut));
}

fn make_session_info(validators: Vec<Pair>, groups: Vec<Vec<u32>>) -> SessionInfo {
	let validator_groups: IndexedVec<GroupIndex, Vec<ValidatorIndex>> = groups
		.iter()
		.map(|g| g.into_iter().map(|v| ValidatorIndex(*v)).collect())
		.collect();

	SessionInfo {
		discovery_keys: validators.iter().map(|k| k.public().into()).collect(),
		// Not used:
		n_cores: validator_groups.len() as u32,
		validator_groups,
		validators: validators.iter().map(|k| k.public().into()).collect(),
		// Not used values:
		assignment_keys: Vec::new(),
		zeroth_delay_tranche_width: 0,
		relay_vrf_modulo_samples: 0,
		n_delay_tranches: 0,
		no_show_slots: 0,
		needed_approvals: 0,
		active_validator_indices: Vec::new(),
		dispute_period: 6,
		random_seed: [0u8; 32],
	}
}

fn derive_metadata_assuming_seconded(
	hash: Hash,
	statement: UncheckedSignedFullStatement,
) -> protocol_v1::StatementMetadata {
	protocol_v1::StatementMetadata {
		relay_parent: hash,
		candidate_hash: statement.unchecked_payload().candidate_hash(),
		signed_by: statement.unchecked_validator_index(),
		signature: statement.unchecked_signature().clone(),
	}
}
