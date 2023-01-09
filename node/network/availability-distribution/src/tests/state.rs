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

use std::{
	collections::{HashMap, HashSet},
	sync::Arc,
	time::Duration,
};

use polkadot_node_subsystem_test_helpers::TestSubsystemContextHandle;
use polkadot_node_subsystem_util::TimeoutExt;

use futures::{
	channel::{mpsc, oneshot},
	FutureExt, SinkExt, StreamExt,
};
use futures_timer::Delay;

use sc_network as network;
use sc_network::{config as netconfig, config::RequestResponseConfig, IfDisconnected};
use sp_core::{testing::TaskExecutor, traits::SpawnNamed};
use sp_keystore::SyncCryptoStorePtr;

use polkadot_node_network_protocol::{
	jaeger,
	request_response::{v1, IncomingRequest, OutgoingRequest, Requests},
};
use polkadot_node_primitives::ErasureChunk;
use polkadot_node_subsystem::{
	messages::{
		AllMessages, AvailabilityDistributionMessage, AvailabilityStoreMessage, ChainApiMessage,
		NetworkBridgeTxMessage, RuntimeApiMessage, RuntimeApiRequest,
	},
	ActivatedLeaf, ActiveLeavesUpdate, FromOrchestra, LeafStatus, OverseerSignal,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_primitives::v2::{
	CandidateHash, CoreState, GroupIndex, Hash, Id as ParaId, ScheduledCore, SessionInfo,
	ValidatorIndex,
};
use test_helpers::mock::make_ferdie_keystore;

use super::mock::{make_session_info, OccupiedCoreBuilder};
use crate::LOG_TARGET;

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>;
pub struct TestHarness {
	pub virtual_overseer: VirtualOverseer,
	pub pov_req_cfg: RequestResponseConfig,
	pub chunk_req_cfg: RequestResponseConfig,
	pub pool: TaskExecutor,
}

/// `TestState` for mocking execution of this subsystem.
///
/// The `Default` instance provides data, which makes the system succeed by providing a couple of
/// valid occupied cores. You can tune the data before calling `TestState::run`. E.g. modify some
/// chunks to be invalid, the test will then still pass if you remove that chunk from
/// `valid_chunks`.
#[derive(Clone)]
pub struct TestState {
	/// Simulated relay chain heads:
	pub relay_chain: Vec<Hash>,
	/// Whenever the subsystem tries to fetch an erasure chunk one item of the given vec will be
	/// popped. So you can experiment with serving invalid chunks or no chunks on request and see
	/// whether the subsystem still succeeds with its goal.
	pub chunks: HashMap<(CandidateHash, ValidatorIndex), Vec<Option<ErasureChunk>>>,
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

		let keystore = make_ferdie_keystore();

		let session_info = make_session_info();

		let (cores, chunks) = {
			let mut cores = HashMap::new();
			let mut chunks = HashMap::new();

			cores.insert(
				relay_chain[0],
				vec![
					CoreState::Scheduled(ScheduledCore { para_id: chain_ids[0], collator: None }),
					CoreState::Scheduled(ScheduledCore { para_id: chain_ids[1], collator: None }),
				],
			);

			let heads = {
				let mut advanced = relay_chain.iter();
				advanced.next();
				relay_chain.iter().zip(advanced)
			};
			for (relay_parent, relay_child) in heads {
				let (p_cores, p_chunks): (Vec<_>, Vec<_>) = chain_ids
					.iter()
					.enumerate()
					.map(|(i, para_id)| {
						let (core, chunk) = OccupiedCoreBuilder {
							group_responsible: GroupIndex(i as _),
							para_id: *para_id,
							relay_parent: relay_parent.clone(),
						}
						.build();
						(CoreState::Occupied(core), chunk)
					})
					.unzip();
				cores.insert(relay_child.clone(), p_cores);
				// Skip chunks for our own group (won't get fetched):
				let mut chunks_other_groups = p_chunks.into_iter();
				chunks_other_groups.next();
				for (validator_index, chunk) in chunks_other_groups {
					chunks.insert((validator_index, chunk.index), vec![Some(chunk)]);
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
		let f = self.run_inner(harness).timeout(Duration::from_secs(10));
		assert!(f.await.is_some(), "Test ran into timeout");
	}

	/// Run tests with the given mock values in `TestState`.
	///
	/// This will simply advance through the simulated chain and examines whether the subsystem
	/// behaves as expected: It will succeed if all valid chunks of other backing groups get stored
	/// and no other.
	///
	/// We try to be as agnostic about details as possible, how the subsystem achieves those goals
	/// should not be a matter to this test suite.
	async fn run_inner(mut self, mut harness: TestHarness) {
		// We skip genesis here (in reality ActiveLeavesUpdate can also skip a block):
		let updates = {
			let mut advanced = self.relay_chain.iter();
			advanced.next();
			self.relay_chain
				.iter()
				.zip(advanced)
				.map(|(old, new)| ActiveLeavesUpdate {
					activated: Some(ActivatedLeaf {
						hash: new.clone(),
						number: 1,
						status: LeafStatus::Fresh,
						span: Arc::new(jaeger::Span::Disabled),
					}),
					deactivated: vec![old.clone()].into(),
				})
				.collect::<Vec<_>>()
		};

		// We should be storing all valid chunks during execution:
		//
		// Test will fail if this does not happen until timeout.
		let mut remaining_stores = self.valid_chunks.len();

		let TestSubsystemContextHandle { tx, mut rx } = harness.virtual_overseer;

		// Spawning necessary as incoming queue can only hold a single item, we don't want to dead
		// lock ;-)
		let update_tx = tx.clone();
		harness.pool.spawn(
			"sending-active-leaves-updates",
			None,
			async move {
				for update in updates {
					overseer_signal(update_tx.clone(), OverseerSignal::ActiveLeaves(update)).await;
					// We need to give the subsystem a little time to do its job, otherwise it will
					// cancel jobs as obsolete:
					Delay::new(Duration::from_millis(100)).await;
				}
			}
			.boxed(),
		);

		while remaining_stores > 0 {
			gum::trace!(target: LOG_TARGET, remaining_stores, "Stores left to go");
			let msg = overseer_recv(&mut rx).await;
			match msg {
				AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(
					reqs,
					IfDisconnected::ImmediateError,
				)) => {
					for req in reqs {
						// Forward requests:
						let in_req = to_incoming_req(&harness.pool, req);
						harness
							.chunk_req_cfg
							.inbound_queue
							.as_mut()
							.unwrap()
							.send(in_req.into_raw())
							.await
							.unwrap();
					}
				},
				AllMessages::AvailabilityStore(AvailabilityStoreMessage::QueryChunk(
					candidate_hash,
					validator_index,
					tx,
				)) => {
					let chunk = self
						.chunks
						.get_mut(&(candidate_hash, validator_index))
						.map(Vec::pop)
						.flatten()
						.flatten();
					tx.send(chunk).expect("Receiver is expected to be alive");
				},
				AllMessages::AvailabilityStore(AvailabilityStoreMessage::StoreChunk {
					candidate_hash,
					chunk,
					tx,
					..
				}) => {
					assert!(
						self.valid_chunks.contains(&(candidate_hash, chunk.index)),
						"Only valid chunks should ever get stored."
					);
					tx.send(Ok(())).expect("Receiver is expected to be alive");
					gum::trace!(target: LOG_TARGET, "'Stored' fetched chunk.");
					remaining_stores -= 1;
				},
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(hash, req)) => {
					match req {
						RuntimeApiRequest::SessionIndexForChild(tx) => {
							// Always session index 1 for now:
							tx.send(Ok(1)).expect("Receiver should still be alive");
						},
						RuntimeApiRequest::SessionInfo(_, tx) => {
							tx.send(Ok(Some(self.session_info.clone())))
								.expect("Receiver should be alive.");
						},
						RuntimeApiRequest::AvailabilityCores(tx) => {
							gum::trace!(target: LOG_TARGET, cores= ?self.cores[&hash], hash = ?hash, "Sending out cores for hash");
							tx.send(Ok(self.cores[&hash].clone()))
								.expect("Receiver should still be alive");
						},
						_ => {
							panic!("Unexpected runtime request: {:?}", req);
						},
					}
				},
				AllMessages::ChainApi(ChainApiMessage::Ancestors { hash, k, response_channel }) => {
					let chain = &self.relay_chain;
					let maybe_block_position = chain.iter().position(|h| *h == hash);
					let ancestors = maybe_block_position
						.map(|idx| chain[..idx].iter().rev().take(k).copied().collect())
						.unwrap_or_default();
					response_channel.send(Ok(ancestors)).expect("Receiver is expected to be alive");
				},
				_ => {},
			}
		}

		overseer_signal(tx, OverseerSignal::Conclude).await;
	}
}

async fn overseer_signal(
	mut tx: mpsc::Sender<FromOrchestra<AvailabilityDistributionMessage>>,
	msg: impl Into<OverseerSignal>,
) {
	let msg = msg.into();
	gum::trace!(target: LOG_TARGET, msg = ?msg, "sending message");
	tx.send(FromOrchestra::Signal(msg))
		.await
		.expect("Test subsystem no longer live");
}

async fn overseer_recv(rx: &mut mpsc::UnboundedReceiver<AllMessages>) -> AllMessages {
	gum::trace!(target: LOG_TARGET, "waiting for message ...");
	rx.next().await.expect("Test subsystem no longer live")
}

fn to_incoming_req(
	executor: &TaskExecutor,
	outgoing: Requests,
) -> IncomingRequest<v1::ChunkFetchingRequest> {
	match outgoing {
		Requests::ChunkFetchingV1(OutgoingRequest { payload, pending_response, .. }) => {
			let (tx, rx): (oneshot::Sender<netconfig::OutgoingResponse>, oneshot::Receiver<_>) =
				oneshot::channel();
			executor.spawn(
				"message-forwarding",
				None,
				async {
					let response = rx.await;
					let payload = response.expect("Unexpected canceled request").result;
					pending_response
						.send(payload.map_err(|_| network::RequestFailure::Refused))
						.expect("Sending response is expected to work");
				}
				.boxed(),
			);

			IncomingRequest::new(
				// We don't really care:
				network::PeerId::random(),
				payload,
				tx,
			)
		},
		_ => panic!("Unexpected request!"),
	}
}
