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

use super::*;
use crate::*;
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_node_network_protocol::request_response::ReqProtocolNames;
use sc_keystore::LocalKeystore;

use futures::Future;
use rand::{Rng, SeedableRng};

use std::sync::Arc;

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<StatementDistributionMessage>;

// Some deterministic genesis hash for req/res protocol names
const GENESIS_HASH: Hash = Hash::repeat_byte(0xff);

struct TestConfig {
	validator_count: usize,
	group_size: usize,
	max_groups: Option<usize>,
	// whether the local node should be a validator
	local_validator: bool,
	rng_seed: u64,
}

fn test_harness<T: Future<Output = VirtualOverseer>>(
	config: TestConfig,
	test: impl FnOnce(VirtualOverseer) -> T,
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
	let rng = rand_chacha::ChaCha8Rng::seed_from_u64(config.rng_seed);

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

	let test_fut = test(virtual_overseer);

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
