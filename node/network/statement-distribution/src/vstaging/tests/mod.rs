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
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_primitives::vstaging::{AssignmentId, AssignmentPair, IndexedVec, ValidatorPair};
use sc_keystore::LocalKeystore;
use sp_application_crypto::Pair as PairT;
use sp_authority_discovery::AuthorityPair as AuthorityDiscoveryPair;
use sp_keyring::Sr25519Keyring;

use futures::Future;
use rand::{Rng, SeedableRng};

use std::sync::Arc;

mod cluster;
mod grid;
mod requests;

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<StatementDistributionMessage>;

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
	discovery_keys: Vec<AuthorityDiscoveryId>,
	assignment_keys: Vec<AssignmentId>,
	validator_groups: IndexedVec<GroupIndex, Vec<ValidatorIndex>>,
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

		TestState {
			config,
			local,
			validators,
			discovery_keys,
			assignment_keys,
			validator_groups: IndexedVec::from(validator_groups),
		}
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
