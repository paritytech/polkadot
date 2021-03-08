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

use std::{collections::{HashMap, HashSet}, sync::Arc, time::Duration};

use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_subsystem_testhelpers::TestSubsystemContextHandle;
use smallvec::smallvec;

use futures::{FutureExt, channel::oneshot, SinkExt, channel::mpsc, StreamExt};
use futures_timer::Delay;

use sc_keystore::LocalKeystore;
use sp_application_crypto::AppKey;
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
use sp_keyring::Sr25519Keyring;
use sp_core::{traits::SpawnNamed, testing::TaskExecutor};
use sc_network as network;
use sc_network::config as netconfig;

use polkadot_subsystem::{ActiveLeavesUpdate, FromOverseer, OverseerSignal, messages::{AllMessages,
	AvailabilityDistributionMessage, AvailabilityStoreMessage, NetworkBridgeMessage, RuntimeApiMessage,
	RuntimeApiRequest}
};
use polkadot_primitives::v1::{CandidateHash, CoreState, ErasureChunk, GroupIndex, Hash, Id
	as ParaId, ScheduledCore, SessionInfo, ValidatorId,
	ValidatorIndex
};
use polkadot_node_network_protocol::{jaeger,
	request_response::{IncomingRequest, OutgoingRequest, Requests, v1}
};
use polkadot_subsystem_testhelpers as test_helpers;
use test_helpers::SingleItemSink;

use super::mock::{make_session_info, OccupiedCoreBuilder, };
use crate::LOG_TARGET;

pub struct TestHarness {
	pub virtual_overseer: test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	pub pool: TaskExecutor,
}

/// TestState for mocking execution of this subsystem.
///
/// The `Default` instance provides data, which makes the system succeed by providing a couple of
/// valid occupied cores. You can tune the data before calling `TestState::run`. E.g. modify some
/// chunks to be invalid, the test will then still pass if you remove that chunk from
/// `valid_chunks`.
#[derive(Clone)]
pub struct TestState {
	// Simulated relay chain heads:
	pub relay_chain: Vec<Hash>,
	pub chunks: HashMap<(CandidateHash, ValidatorIndex), ErasureChunk>,
	/// All chunks that are valid and should be accepted.
	pub valid_chunks: HashSet<(CandidateHash, ValidatorIndex)>,
	pub session_info: SessionInfo,
	/// Cores per relay chain block.
	pub cores: HashMap<Hash, Vec<CoreState>>,
	pub keystore: SyncCryptoStorePtr,
}

impl Default for TestState {
	fn default() -> Self {
		let relay_chain: Vec<_> = (1u8..10).map(Hash::repeat_byte).collect();
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);

		let chain_ids = vec![chain_a, chain_b];

		let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());

		let session_info = make_session_info();

		SyncCryptoStore::sr25519_generate_new(
			&*keystore,
			ValidatorId::ID,
			Some(&Sr25519Keyring::Ferdie.to_seed()),
		)
		.expect("Insert key into keystore");

		let (cores, chunks) = {
			let mut cores = HashMap::new();
			let mut chunks = HashMap::new();

			cores.insert(relay_chain[0], 
				vec![
					CoreState::Scheduled(ScheduledCore {
						para_id: chain_ids[0],
						collator: None,
					}),
					CoreState::Scheduled(ScheduledCore {
						para_id: chain_ids[1],
						collator: None,
					}),
				]
			);

			let heads =  {
				let mut advanced = relay_chain.iter();
				advanced.next();
				relay_chain.iter().zip(advanced)
			};
			for (relay_parent, relay_child) in heads {
				let (p_cores, p_chunks): (Vec<_>, Vec<_>) = chain_ids.iter().enumerate()
					.map(|(i, para_id)| {
						let (core, chunk) = OccupiedCoreBuilder {
							group_responsible: GroupIndex(i as _),
							para_id: *para_id,
							relay_parent: relay_parent.clone(),
						}.build();
						(CoreState::Occupied(core), chunk)
					}
					)
					.unzip();
				cores.insert(relay_child.clone(), p_cores);
				// Skip chunks for our own group (won't get fetched):
				let mut chunks_other_groups = p_chunks.into_iter();
				chunks_other_groups.next();
				for (validator_index, chunk) in chunks_other_groups {
					chunks.insert((validator_index, chunk.index), chunk);
				}
			}
			(cores, chunks)
		};
		Self {
			relay_chain,
			valid_chunks: chunks.clone().keys().map(Clone::clone).collect(),
			chunks,
			session_info,
			cores,
			keystore,
		}
	}
}

impl TestState {
	
	/// Run, but fail after some timeout.
	pub async fn run(self, harness: TestHarness) {
		// Make sure test won't run forever.
		let f = self.run_inner(harness.pool, harness.virtual_overseer).timeout(Duration::from_secs(10));
		assert!(f.await.is_some(), "Test ran into timeout");
	}

	/// Run tests with the given mock values in `TestState`.
	///
	/// This will simply advance through the simulated chain and examines whether the subsystem
	/// behaves as expected: It will succeed if all valid chunks of other backing groups get stored
	/// and no other.
	async fn run_inner(self, executor: TaskExecutor, virtual_overseer: TestSubsystemContextHandle<AvailabilityDistributionMessage>) {
		// We skip genesis here (in reality ActiveLeavesUpdate can also skip a block:
		let updates = {
			let mut advanced = self.relay_chain.iter();
			advanced.next();
			self
			.relay_chain.iter().zip(advanced)
			.map(|(old, new)| ActiveLeavesUpdate {
				activated: smallvec![(new.clone(), Arc::new(jaeger::Span::Disabled))],
				deactivated: smallvec![old.clone()],
			}).collect::<Vec<_>>()
		};

		// We should be storing all valid chunks during execution:
		//
		// Test will fail if this does not happen until timeout.
		let mut remaining_stores = self.valid_chunks.len();
		
		let TestSubsystemContextHandle { tx, mut rx } = virtual_overseer;

		// Spawning necessary as incoming queue can only hold a single item, we don't want to dead
		// lock ;-)
		let update_tx = tx.clone();
		executor.spawn("Sending active leaves updates", async move {
			for update in updates {
				overseer_signal(
					update_tx.clone(),
					OverseerSignal::ActiveLeaves(update)
				).await;
				// We need to give the subsystem a little time to do its job, otherwise it will
				// cancel jobs as obsolete:
				Delay::new(Duration::from_millis(20)).await;
			}
		}.boxed()
		);

		while remaining_stores > 0
		{
			tracing::trace!(target: LOG_TARGET, remaining_stores, "Stores left to go");
			let msg = overseer_recv(&mut rx).await;
			match msg {
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs)) => {
					for req in reqs {
						// Forward requests:
						let in_req = to_incoming_req(&executor, req);

						executor.spawn("Request forwarding",
									overseer_send(
										tx.clone(),
										AvailabilityDistributionMessage::AvailabilityFetchingRequest(in_req)
									).boxed()
						);
					}
				}
				AllMessages::AvailabilityStore(AvailabilityStoreMessage::QueryChunk(candidate_hash,	validator_index, tx)) => {
					let chunk = self.chunks.get(&(candidate_hash, validator_index));
					tx.send(chunk.map(Clone::clone))
						.expect("Receiver is expected to be alive");
				}
				AllMessages::AvailabilityStore(AvailabilityStoreMessage::StoreChunk{candidate_hash,	chunk, tx, ..}) => {
					assert!(
						self.valid_chunks.contains(&(candidate_hash, chunk.index)),
						"Only valid chunks should ever get stored."
					);
					tx.send(Ok(()))
						.expect("Receiver is expected to be alive");
					tracing::trace!(target: LOG_TARGET, "'Stored' fetched chunk.");
					remaining_stores -= 1;
				}
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(hash, req)) => {
					match req {
						RuntimeApiRequest::SessionIndexForChild(tx) => {
							// Always session index 1 for now:
							tx.send(Ok(1))
							.expect("Receiver should still be alive");
						}
						RuntimeApiRequest::SessionInfo(_, tx) => {
							tx.send(Ok(Some(self.session_info.clone())))
							.expect("Receiver should be alive.");
						}
						RuntimeApiRequest::AvailabilityCores(tx) => {
							tracing::trace!(target: LOG_TARGET, cores= ?self.cores[&hash], hash = ?hash, "Sending out cores for hash");
							tx.send(Ok(self.cores[&hash].clone()))
							.expect("Receiver should still be alive");
						}
						_ => {
							panic!("Unexpected runtime request: {:?}", req);
						}
					}
				}
				_ => {
					panic!("Unexpected message received: {:?}", msg);
				}
			}
		}
	}
}



async fn overseer_signal(
	mut tx: SingleItemSink<FromOverseer<AvailabilityDistributionMessage>>,
	msg: impl Into<OverseerSignal>,
) {
	let msg = msg.into();
	tracing::trace!(target: LOG_TARGET, msg = ?msg, "sending message");
	tx.send(FromOverseer::Signal(msg))
		.await
		.expect("Test subsystem no longer live");
}

async fn overseer_send(
	mut tx: SingleItemSink<FromOverseer<AvailabilityDistributionMessage>>,
	msg: impl Into<AvailabilityDistributionMessage>,
) {
	let msg = msg.into();
	tracing::trace!(target: LOG_TARGET, msg = ?msg, "sending message");
	tx.send(FromOverseer::Communication { msg }).await
		.expect("Test subsystem no longer live");
	tracing::trace!(target: LOG_TARGET, "sent message");
}


async fn overseer_recv(
	rx: &mut mpsc::UnboundedReceiver<AllMessages>,
) -> AllMessages {
	tracing::trace!(target: LOG_TARGET, "waiting for message ...");
	rx.next().await.expect("Test subsystem no longer live")
}

fn to_incoming_req(
	executor: &TaskExecutor,
	outgoing: Requests
) -> IncomingRequest<v1::AvailabilityFetchingRequest> {
	match outgoing {
		Requests::AvailabilityFetching(OutgoingRequest { payload, pending_response, .. }) => {
			let (tx, rx): (oneshot::Sender<netconfig::OutgoingResponse>, oneshot::Receiver<_>)
			   = oneshot::channel();
			executor.spawn("Message forwarding", async {
				let response = rx.await;
				let payload = response.expect("Unexpected canceled request").result;
				pending_response.send(payload.map_err(|_| network::RequestFailure::Refused))
					.expect("Sending response is expected to work");
			}.boxed()
			);

			IncomingRequest::new(
				// We don't really care:
				network::PeerId::random(),
				payload,
				tx
			)
		}
	}
}
