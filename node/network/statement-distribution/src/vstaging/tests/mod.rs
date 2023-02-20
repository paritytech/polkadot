// Copyright 2023 Parity Technologies (UK) Ltd.
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

// TODO [now]: Remove once some tests are written.
#![allow(unused)]

use super::*;
use crate::*;
use polkadot_node_network_protocol::request_response::ReqProtocolNames;
use polkadot_node_subsystem::messages::{
	network_bridge_event::NewGossipTopology,
	AllMessages, ChainApiMessage, ProspectiveParachainsMessage, RuntimeApiMessage,
	RuntimeApiRequest, NetworkBridgeEvent,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_subsystem_types::{jaeger, ActivatedLeaf, LeafStatus};
use polkadot_primitives::vstaging::{
	AssignmentId, AssignmentPair, AsyncBackingParameters, BlockNumber, CoreState,
	GroupRotationInfo, HeadData, Header, IndexedVec, ScheduledCore, SessionInfo,
	ValidationCodeHash, ValidatorPair, SessionIndex,
};
use sc_keystore::LocalKeystore;
use sp_application_crypto::Pair as PairT;
use sp_authority_discovery::AuthorityPair as AuthorityDiscoveryPair;
use sp_keyring::Sr25519Keyring;

use assert_matches::assert_matches;
use futures::Future;
use rand::{Rng, SeedableRng};

use std::sync::Arc;

mod cluster;
mod grid;
mod requests;

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<StatementDistributionMessage>;

const ASYNC_BACKING_PARAMETERS: AsyncBackingParameters =
	AsyncBackingParameters { max_candidate_depth: 4, allowed_ancestry_len: 3 };

// Some deterministic genesis hash for req/res protocol names
const GENESIS_HASH: Hash = Hash::repeat_byte(0xff);

struct TestConfig {
	validator_count: usize,
	// how many validators to place in each group.
	group_size: usize,
	// whether the local node should be a validator
	local_validator: bool,
	rng_seed: u64,
}

struct TestLocalValidator {
	validator_id: ValidatorId,
	validator_index: ValidatorIndex,
	group_index: GroupIndex,
}

struct TestState {
	config: TestConfig,
	local: Option<TestLocalValidator>,
	validators: Vec<ValidatorPair>,
	session_info: SessionInfo,
}

impl TestState {
	fn from_config(config: TestConfig, rng: &mut impl Rng) -> Self {
		if config.group_size == 0 {
			panic!("group size cannot be 0");
		}

		let mut validators = Vec::new();
		let mut discovery_keys = Vec::new();
		let mut assignment_keys = Vec::new();
		let mut validator_groups = Vec::new();

		let local_validator_pos = if config.local_validator {
			Some(rng.gen_range(0..config.validator_count))
		} else {
			None
		};

		for i in 0..config.validator_count {
			let validator_pair = if Some(i) == local_validator_pos {
				// Note: the specific key is used to ensure the keystore holds
				// this key and the subsystem can detect that it is a validator.
				Sr25519Keyring::Ferdie.pair().into()
			} else {
				ValidatorPair::generate().0
			};
			let assignment_id = AssignmentPair::generate().0.public();
			let discovery_id = AuthorityDiscoveryPair::generate().0.public();

			let group_index = i / config.group_size;
			validators.push(validator_pair);
			discovery_keys.push(discovery_id);
			assignment_keys.push(assignment_id);
			if validator_groups.len() == group_index {
				validator_groups.push(vec![ValidatorIndex(i as _)]);
			} else {
				validator_groups.last_mut().unwrap().push(ValidatorIndex(i as _));
			}
		}

		let local = if let Some(local_pos) = local_validator_pos {
			Some(TestLocalValidator {
				validator_id: validators[local_pos].public().clone(),
				validator_index: ValidatorIndex(local_pos as _),
				group_index: GroupIndex((local_pos / config.group_size) as _),
			})
		} else {
			None
		};

		let validator_public = validator_pubkeys(&validators);
		let session_info = SessionInfo {
			validators: validator_public,
			discovery_keys,
			validator_groups: IndexedVec::from(validator_groups),
			assignment_keys,
			n_cores: 0,
			zeroth_delay_tranche_width: 0,
			relay_vrf_modulo_samples: 0,
			n_delay_tranches: 0,
			no_show_slots: 0,
			needed_approvals: 0,
			active_validator_indices: vec![],
			dispute_period: 6,
			random_seed: [0u8; 32],
		};

		TestState {
			config,
			local,
			validators,
			session_info,
		}
	}

	fn sign_statement(
		&self,
		validator_index: ValidatorIndex,
		statement: CompactStatement,
		context: &SigningContext,
	) -> SignedStatement {
		let payload = statement.signing_payload(context);
		let pair = &self.validators[validator_index.0 as usize];
		let signature = pair.sign(&payload[..]);

		SignedStatement::new(statement, validator_index, signature, context, &pair.public()).unwrap()
	}
}

fn test_harness<T: Future<Output = VirtualOverseer>>(
	config: TestConfig,
	test: impl FnOnce(TestState, VirtualOverseer) -> T,
) {
	let pool = sp_core::testing::TaskExecutor::new();
	let keystore = if config.local_validator {
		test_helpers::mock::make_ferdie_keystore()
	} else {
		Arc::new(LocalKeystore::in_memory()) as SyncCryptoStorePtr
	};
	let req_protocol_names = ReqProtocolNames::new(&GENESIS_HASH, None);
	let (statement_req_receiver, _) = IncomingRequest::get_config_receiver(&req_protocol_names);
	let (candidate_req_receiver, _) = IncomingRequest::get_config_receiver(&req_protocol_names);
	let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(config.rng_seed);

	let test_state = TestState::from_config(config, &mut rng);

	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());
	let subsystem = async move {
		let subsystem = crate::StatementDistributionSubsystem::new(
			keystore,
			statement_req_receiver,
			candidate_req_receiver,
			Metrics::default(),
			rng,
		);

		if let Err(e) = subsystem.run(context).await {
			panic!("Fatal error: {:?}", e);
		}
	};

	let test_fut = test(test_state, virtual_overseer);

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);
	futures::executor::block_on(future::join(
		async move {
			let mut virtual_overseer = test_fut.await;
			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
		},
		subsystem,
	));
}

struct PerParaData {
	min_relay_parent: BlockNumber,
	head_data: HeadData,
}

impl PerParaData {
	pub fn new(min_relay_parent: BlockNumber, head_data: HeadData) -> Self {
		Self { min_relay_parent, head_data }
	}
}

struct TestLeaf {
	number: BlockNumber,
	hash: Hash,
	session: SessionIndex,
	availability_cores: Vec<CoreState>,
	para_data: Vec<(ParaId, PerParaData)>,
}

impl TestLeaf {
	pub fn para_data(&self, para_id: ParaId) -> &PerParaData {
		self.para_data
			.iter()
			.find_map(|(p_id, data)| if *p_id == para_id { Some(data) } else { None })
			.unwrap()
	}
}

async fn activate_leaf(
	virtual_overseer: &mut VirtualOverseer,
	para_id: ParaId,
	leaf: &TestLeaf,
	test_state: &TestState,
) {
	let activated = ActivatedLeaf {
		hash: leaf.hash,
		number: leaf.number,
		status: LeafStatus::Fresh,
		span: Arc::new(jaeger::Span::Disabled),
	};

	virtual_overseer
		.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			activated,
		))))
		.await;

	handle_leaf_activation(virtual_overseer, para_id, leaf, test_state).await;
}

async fn handle_leaf_activation(
	virtual_overseer: &mut VirtualOverseer,
	para_id: ParaId,
	leaf: &TestLeaf,
	test_state: &TestState,
) {
	let TestLeaf { number, hash, para_data, session, availability_cores } = leaf;
	let PerParaData { min_relay_parent, head_data } = leaf.para_data(para_id);

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::StagingAsyncBackingParameters(tx))
		) if parent == *hash => {
			tx.send(Ok(ASYNC_BACKING_PARAMETERS)).unwrap();
		}
	);

	let mrp_response: Vec<(ParaId, BlockNumber)> = para_data
		.iter()
		.map(|(para_id, data)| (*para_id, data.min_relay_parent))
		.collect();
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::ProspectiveParachains(
			ProspectiveParachainsMessage::GetMinimumRelayParents(parent, tx)
		) if parent == *hash => {
			tx.send(mrp_response).unwrap();
		}
	);

	let header = Header {
		parent_hash: get_parent_hash(*hash),
		number: *number,
		state_root: Hash::zero(),
		extrinsics_root: Hash::zero(),
		digest: Default::default(),
	};
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::ChainApi(
			ChainApiMessage::BlockHeader(parent, tx)
		) if parent == *hash => {
			tx.send(Ok(Some(header))).unwrap();
		}
	);

	// TODO: Can we remove this REDUNDANT request?
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::StagingAsyncBackingParameters(tx))
		) if parent == *hash => {
			tx.send(Ok(ASYNC_BACKING_PARAMETERS)).unwrap();
		}
	);

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::SessionIndexForChild(tx))) if parent == *hash => {
			tx.send(Ok(*session)).unwrap();
		}
	);

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::AvailabilityCores(tx))) if parent == *hash => {
			tx.send(Ok(availability_cores.clone())).unwrap();
		}
	);

	let validator_groups = test_state.session_info.validator_groups.to_vec();
	let group_rotation_info =
		GroupRotationInfo { session_start_block: 1, group_rotation_frequency: 12, now: 1 };
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidatorGroups(tx))) if parent == *hash => {
			tx.send(Ok((validator_groups, group_rotation_info))).unwrap();
		}
	);

	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::SessionInfo(s, tx))) if s == *session => {
			tx.send(Ok(Some(test_state.session_info.clone()))).unwrap();
		}
	);
}

fn get_parent_hash(hash: Hash) -> Hash {
	Hash::from_low_u64_be(hash.to_low_u64_be() + 1)
}

fn validator_pubkeys(val_ids: &[ValidatorPair]) -> IndexedVec<ValidatorIndex, ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

async fn provide_topology(
	virtual_overseer: &mut VirtualOverseer,
	topology: NewGossipTopology,
) {
	virtual_overseer.send(FromOrchestra::Communication {
		msg: StatementDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::NewGossipTopology(topology))
	}).await;
}
