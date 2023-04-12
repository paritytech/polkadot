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

use std::collections::HashSet;

use futures::{executor, future, Future};

use polkadot_node_network_protocol::request_response::{IncomingRequest, ReqProtocolNames};
use polkadot_primitives::{CoreState, Hash};
use sp_keystore::KeystorePtr;

use polkadot_node_subsystem_test_helpers as test_helpers;

use super::*;

mod state;
/// State for test harnesses.
use state::{TestHarness, TestState};

/// Mock data useful for testing.
pub(crate) mod mock;

fn test_harness<T: Future<Output = ()>>(
	keystore: KeystorePtr,
	test_fx: impl FnOnce(TestHarness) -> T,
) {
	sp_tracing::try_init_simple();

	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());
	let genesis_hash = Hash::repeat_byte(0xff);
	let req_protocol_names = ReqProtocolNames::new(&genesis_hash, None);

	let (pov_req_receiver, pov_req_cfg) = IncomingRequest::get_config_receiver(&req_protocol_names);
	let (chunk_req_receiver, chunk_req_cfg) =
		IncomingRequest::get_config_receiver(&req_protocol_names);
	let subsystem = AvailabilityDistributionSubsystem::new(
		keystore,
		IncomingRequestReceivers { pov_req_receiver, chunk_req_receiver },
		Default::default(),
	);
	let subsystem = subsystem.run(context);

	let test_fut = test_fx(TestHarness { virtual_overseer, pov_req_cfg, chunk_req_cfg, pool });

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	executor::block_on(future::join(test_fut, subsystem)).1.unwrap();
}

/// Simple basic check, whether the subsystem works as expected.
///
/// Exceptional cases are tested as unit tests in `fetch_task`.
#[test]
fn check_basic() {
	let state = TestState::default();
	test_harness(state.keystore.clone(), move |harness| state.run(harness));
}

/// Check whether requester tries all validators in group.
#[test]
fn check_fetch_tries_all() {
	let mut state = TestState::default();
	for (_, v) in state.chunks.iter_mut() {
		// 4 validators in group, so this should still succeed:
		v.push(None);
		v.push(None);
		v.push(None);
	}
	test_harness(state.keystore.clone(), move |harness| state.run(harness));
}

/// Check whether requester tries all validators in group
///
/// Check that requester will retry the fetch on error on the next block still pending
/// availability.
#[test]
fn check_fetch_retry() {
	let mut state = TestState::default();
	state
		.cores
		.insert(state.relay_chain[2], state.cores.get(&state.relay_chain[1]).unwrap().clone());
	// We only care about the first three blocks.
	// 1. scheduled
	// 2. occupied
	// 3. still occupied
	state.relay_chain.truncate(3);

	// Get rid of unused valid chunks:
	let valid_candidate_hashes: HashSet<_> = state
		.cores
		.get(&state.relay_chain[1])
		.iter()
		.flat_map(|v| v.iter())
		.filter_map(|c| match c {
			CoreState::Occupied(core) => Some(core.candidate_hash),
			_ => None,
		})
		.collect();
	state.valid_chunks.retain(|(ch, _)| valid_candidate_hashes.contains(ch));

	for (_, v) in state.chunks.iter_mut() {
		// This should still succeed as cores are still pending availability on next block.
		v.push(None);
		v.push(None);
		v.push(None);
		v.push(None);
		v.push(None);
	}
	test_harness(state.keystore.clone(), move |harness| state.run(harness));
}
