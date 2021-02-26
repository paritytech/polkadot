// Copyright 2021 Parity Technologies (UK) Ltd.
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


use std::{collections::HashMap, sync::Arc, time::Duration};

use futures::{executor, future, Future};
use assert_matches::assert_matches;
use maplit::hashmap;
use smallvec::SmallVec;

use sc_keystore::LocalKeystore;
use sp_application_crypto::AppKey;
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
use sp_keyring::Sr25519Keyring;
use sc_network::PeerId;


use polkadot_erasure_coding::{branches, obtain_chunks_v1 as obtain_chunks};
use polkadot_node_network_protocol::{view, ObservedRole, our_view};
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_node_jaeger as jaeger;
use polkadot_primitives::v1::{AvailableData, BlockData, CandidateCommitments, CandidateDescriptor, CommittedCandidateReceipt, CoreState, GroupIndex, GroupRotationInfo, HeadData, Id as ParaId, OccupiedCore, PersistedValidationData, PoV, ScheduledCore, ValidatorId, ValidatorIndex, Hash};
use polkadot_subsystem::messages::AllMessages;
use polkadot_subsystem::ActiveLeavesUpdate;
use polkadot_subsystem_testhelpers as test_helpers;

use super::*;


mod state;
/// State for test harnesses.
use state::TestState;


struct TestHarness {
	virtual_overseer: test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
}


fn test_harness<T: Future<Output = ()>>(
	keystore: SyncCryptoStorePtr,
	test_fx: impl FnOnce(TestHarness) -> T,
) {
	sp_tracing::try_init_simple();

	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = AvailabilityDistributionSubsystem::new(keystore, Default::default());
	{
		let subsystem = subsystem.run(context);

		let test_fut = test_fx(TestHarness { virtual_overseer });

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}
}

/// Tests
///
/// - No fetches from own backing group
/// - Fetch succeeds as long as at least one validator can deliver the chunk.
///
/// Test with two backing groups, our own and another one which contains a couple of
/// validators that fail in delivering the chunk for different reasons, except one. 
#[test]
fn check_chunks_are_fetched() {
	let test_state = TestState::default();

	let peer_a = PeerId::random();
	let peer_a_2 = peer_a.clone();
	let peer_b = PeerId::random();
	let peer_b_2 = peer_b.clone();
	assert_ne!(&peer_a, &peer_b);

	let keystore = test_state.keystore.clone();
	let current = test_state.relay_parent;
	let ancestors = test_state.ancestors.clone();

	let state = test_harness(keystore, move |test_harness| async move {
		let mut virtual_overseer = test_harness.virtual_overseer;

		let TestState {
			validator_public,
			relay_parent: current,
			ancestors,
			candidates,
			pov_blocks,
			..
		} = test_state.clone();

		let genesis = Hash::repeat_byte(0x1);

		send_active_leaves_update(&virtual_overseer, activate_leave(genesis)).await;


		// setup peer a with interest in current
		setup_peer_with_view(&mut virtual_overseer, peer_a.clone(), view![current]).await;

		// setup peer b with interest in ancestor
		setup_peer_with_view(&mut virtual_overseer, peer_b.clone(), view![ancestors[0]]).await;
	});

	assert_matches! {
		state,
		ProtocolState {
			peer_views,
			view,
			..
		} => {
			assert_eq!(
				peer_views,
				hashmap! {
					peer_a_2 => view![current],
					peer_b_2 => view![ancestors[0]],
				},
			);
			assert_eq!(view, our_view![current]);
		}
	};
}



/// Returns the number of chunks that the subsystem asked to store/fetched successfully.
async fn send_active_leaves_update(
	virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	update: ActiveLeavesUpdate,
	) -> u32 {
		// Get fetcher started:
		overseer_signal(
			virtual_overseer,
			OverseerSignal::ActiveLeaves(update)
		).await;

		let stored_chunks: u32 = 0;
		// Handle requests:
		loop {
			match overseer_recv(virtual_overseer).await {
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(hash, RuntimeApiRequest::AvailabilityCores(sender)) => {
				}
			}
		}
}

/// Create update with a single activated leaf.
fn activate_leave(activated: Hash) -> ActiveLeavesUpdate {
	ActiveLeavesUpdate {
		activated: SmallVec::from[(activated, Arc::new(jaeger::Span::Disabled))],
		deactivated: SmallVec::new(),
	}
}

/// Create an update with a single deactivated leaf.
fn deactivate_leave(deactivated: Hash) -> ActiveLeavesUpdate {
	ActiveLeavesUpdate {
		activated: SmallVec::new(),
		deactivated: SmallVec::from([deactivated]),
	}
}

