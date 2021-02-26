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

use std::{collections::{HashMap, HashSet}, sync::Arc};

use bitvec::vec::BitVec;
use polkadot_subsystem_testhelpers::TestSubsystemContextHandle;
use smallvec::SmallVec;

use futures::{FutureExt, channel::oneshot};

use polkadot_subsystem::{ActiveLeavesUpdate, OverseerSignal, messages::{AllMessages, AvailabilityDistributionMessage, AvailabilityStoreMessage, NetworkBridgeMessage, RuntimeApiMessage, RuntimeApiRequest}};
use sc_keystore::LocalKeystore;
use sp_application_crypto::AppKey;
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
use sp_keyring::Sr25519Keyring;
use sp_core::{traits::SpawnNamed, testing::TaskExecutor};
use sc_network as network;
use sc_network::config as netconfig;

use polkadot_erasure_coding::{branches, obtain_chunks_v1 as obtain_chunks};
use polkadot_primitives::v1::{AvailableData, BlockData, CandidateCommitments, CandidateDescriptor, CandidateHash, CommittedCandidateReceipt, CoreState, ErasureChunk, GroupIndex, GroupRotationInfo, Hash, HeadData, Id as ParaId, OccupiedCore, PersistedValidationData, PoV, ScheduledCore, SessionInfo, ValidatorId, ValidatorIndex};
use polkadot_node_network_protocol::{jaeger, request_response::{IncomingRequest, OutgoingRequest, Requests, v1}};



#[derive(Clone)]
pub struct TestState {
	// Simulated relay chain heads:
	pub relay_chain: Vec<Hash>,
	pub chunks: HashMap<(CandidateHash, ValidatorIndex), ErasureChunk>,
	/// All chunks that are valid and should be accepted.
	pub valid_chunks: HashSet<(CandidateHash, ValidatorIndex)>,
	pub session_info: SessionInfo,

	pub chain_ids: Vec<ParaId>,
	pub validators: Vec<Sr25519Keyring>,
	pub validator_public: Vec<ValidatorId>,
	pub validator_groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo),
	pub head_data: HashMap<ParaId, HeadData>,
	pub keystore: SyncCryptoStorePtr,
	pub ancestors: Vec<Hash>,
	pub persisted_validation_data: PersistedValidationData,
	pub candidates: Vec<CommittedCandidateReceipt>,
	pub pov_blocks: Vec<PoV>,
}

impl Default for TestState {
	fn default() -> Self {
		let relay_chain: Vec<_> = (1u8..10).map(Hash::repeat_byte).collect();
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);

		let chain_ids = vec![chain_a, chain_b];

		let validators = vec![
			Sr25519Keyring::Ferdie, // <- this node, role: validator
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Eve,
			Sr25519Keyring::One,
		];

		let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());

		SyncCryptoStore::sr25519_generate_new(
			&*keystore,
			ValidatorId::ID,
			Some(&validators[0].to_seed()),
		)
		.expect("Insert key into keystore");

		let validator_public = validator_pubkeys(&validators);

		let validator_groups = vec![vec![5, 0], vec![1, 6], vec![3, 4, 2]]
			.into_iter().map(|g| g.into_iter().map(ValidatorIndex).collect()).collect();
		let group_rotation_info = GroupRotationInfo {
			session_start_block: 0,
			group_rotation_frequency: 100,
			now: 1,
		};
		let validator_groups = (validator_groups, group_rotation_info);

		let availability_cores = 
		let mut head_data = HashMap::new();
		head_data.insert(chain_a, HeadData(vec![4, 5, 6]));
		head_data.insert(chain_b, HeadData(vec![7, 8, 9]));

		let ancestors = vec![
			Hash::repeat_byte(0x44),
			Hash::repeat_byte(0x33),
			Hash::repeat_byte(0x22),
		];
		let relay_parent = Hash::repeat_byte(0x05);

		let persisted_validation_data = PersistedValidationData {
			parent_head: HeadData(vec![7, 8, 9]),
			relay_parent_number: Default::default(),
			max_pov_size: 1024,
			relay_parent_storage_root: Default::default(),
		};

		let pov_block_a = PoV {
			block_data: BlockData(vec![42, 43, 44]),
		};

		let pov_block_b = PoV {
			block_data: BlockData(vec![45, 46, 47]),
		};

		let candidates = vec![
			TestCandidateBuilder {
				para_id: chain_ids[0],
				relay_parent: relay_parent,
				pov_hash: pov_block_a.hash(),
				erasure_root: make_erasure_root(persisted_validation_data.clone(), validators.len(), pov_block_a.clone()),
				head_data: head_data.get(&chain_ids[0]).unwrap().clone(),
				..Default::default()
			}
			.build(),
			TestCandidateBuilder {
				para_id: chain_ids[1],
				relay_parent: relay_parent,
				pov_hash: pov_block_b.hash(),
				erasure_root: make_erasure_root(persisted_validation_data.clone(), validators.len(), pov_block_b.clone()),
				head_data: head_data.get(&chain_ids[1]).unwrap().clone(),
				..Default::default()
			}
			.build(),
		];

		let pov_blocks = vec![pov_block_a, pov_block_b];

		Self {
			relay_chain,
			chain_ids,
			keystore,
			validators,
			validator_public,
			validator_groups,
			head_data,
			persisted_validation_data,
			ancestors,
			candidates,
			pov_blocks,
		}
	}
}

impl TestState {

	/// Run tests with the given mock values in `TestState`.
	///
	/// This will simply advance through the simulated chain and examines whether the
	/// subsystem behaves as expected:
	///
	/// - Fetches and stores valid chunks
	/// - Delivers valid chunks
	/// - Does not store invalid chunks.
	pub async fn run(self, executor: TaskExecutor, virtual_overseer: &mut TestSubsystemContextHandle<AvailabilityDistributionMessage>) {
		// We skip genesis here (in reality ActiveLeavesUpdate can also skip a block:
		let updates = self
			.relay_chain.iter().zip(self.relay_chain.iter().next())
			.map(|(old, new)| ActiveLeavesUpdate {
				activated: SmallVec::from[(new, Arc::new(jaeger::Span::Disabled))],
				deactivated: SmallVec::from[old],
			});

		for update in updates
		{
			// Advance subsystem by one block:
			overseer_signal(
				virtual_overseer,
				OverseerSignal::ActiveLeaves(update)
			).await;
			let msg = overseer_recv(virtual_overseer).await;
			match msg {
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs)) => {
					for req in reqs {
						// Forward requests:
						let in_req = to_incoming_req(&executor, req);
						overseer_send(
							virtual_overseer,
							AvailabilityDistributionMessage::AvailabilityFetchingRequest(in_req)
						).await;
					}
				}
				AllMessages::AvailabilityStore(AvailabilityStoreMessage::QueryChunk(candidate_hash,	validator_index, tx)) => {
					let chunk = self.chunks.get(&(candidate_hash, validator_index));
					tx.send(chunk.map(Clone::clone));
				}
				AllMessages::AvailabilityStore(AvailabilityStoreMessage::StoreChunk{candidate_hash,	chunk, tx, ..}) => {
					assert!(
						self.valid_chunks.contains(&(candidate_hash, chunk.index)),
						"Only valid chunks should ever get stored."
					);
					tx.send(Ok(()));
				}
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(hash, req)) => {
					match req {
						RuntimeApiRequest::SessionIndexForChild(tx) => {
							// Always session index 1 for now:
							tx.send(Ok(1))
							.expect("Receiver should still be alive");
						}
						RuntimeApiRequest::SessionInfo(index, tx) => {
							tx.send(Ok(Some(self.session_info)))
							.expect("Receiver should be alive.");
						}
						RuntimeApiRequest::AvailabilityCores(tx) => {
							tx.send(Ok(self.get_cores(hash)))
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

	/// Get test core states for a given relay parent.
	fn get_cores(&self, hash: Hash) -> Vec<CoreState> {
		if hash == Hash::repeat_byte(1u8) {
			return vec![
				CoreState::Scheduled(ScheduledCore {
					para_id: self.chain_ids[0],
					collator: None,
				}),
				CoreState::Scheduled(ScheduledCore {
					para_id: self.chain_ids[1],
					collator: None,
				}),
			]
		}
		// chain_id 0 becomes occupied:
		if hash == Hash::repeat_byte(2u8) {
			return vec![
				CoreState::Occupied(OccupiedCoreBuilder{
						group_responsible: GroupIndex(0),
						candidate_hash:

					}
					.build()
				),
				CoreState::Scheduled(ScheduledCore {
					para_id: self.chain_ids[1],
					collator: None,
				}),
			];
		}
		// chain_id 1 becomes occupied:
		if hash == Hash::repeat_byte(3u8) {
			return vec![
				CoreState::Occupied(OccupiedCoreBuilder {
						group_responsible: GroupIndex(0),
						candidate_hash:

					}
					.build()
				),
				CoreState::Scheduled(OccupiedCoreBuilder {
					group_responsible: GroupIndex(0),
					candidate_hash:
				}),
			];
		}
	}
}

#[derive(Default)]
struct TestCandidateBuilder {
	para_id: ParaId,
	head_data: HeadData,
	pov_hash: Hash,
	relay_parent: Hash,
	erasure_root: Hash,
}

impl TestCandidateBuilder {
	fn build(self) -> CommittedCandidateReceipt {
		CommittedCandidateReceipt {
			descriptor: CandidateDescriptor {
				para_id: self.para_id,
				pov_hash: self.pov_hash,
				relay_parent: self.relay_parent,
				erasure_root: self.erasure_root,
				..Default::default()
			},
			commitments: CandidateCommitments {
				head_data: self.head_data,
				..Default::default()
			},
		}
	}
}

/// Builder for constructing occupied cores.
///
/// Takes all the values we care about and fills the rest with dummy values on `build`.
struct OccupiedCoreBuilder {
	group_responsible: GroupIndex,
	candidate_hash: CandidateHash,
	candidate_descriptor: CandidateDescriptor<Hash>,
}

impl OccupiedCoreBuilder {
	fn build(self) -> OccupiedCore {
		OccupiedCore {
			next_up_on_available: None,
			occupied_since: 0,
			time_out_at: 0,
			next_up_on_time_out: None,
			availability: BitVec::new(),
			group_responsible: self.group_responsible,
			candidate_hash: self.candidate_hash,
			candidate_descriptor: self.candidate_descriptor,
		}
	}
}

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

fn make_erasure_root(peristed: PersistedValidationData, validator_count: usize, pov: PoV) -> Hash {
	let available_data = make_available_data(peristed, pov);

	let chunks = obtain_chunks(validator_count, &available_data).unwrap();
	branches(&chunks).root()
}

fn make_available_data(validation_data: PersistedValidationData, pov: PoV) -> AvailableData {
	AvailableData {
		validation_data,
		pov: Arc::new(pov),
	}
}

async fn overseer_signal(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	msg: impl Into<OverseerSignal>,
) {
	let msg = msg.into();
	tracing::trace!(msg = ?msg, "sending message");
	overseer.send(FromOverseer::Signal(msg)).await
}

async fn overseer_send(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
	msg: impl Into<AvailabilityDistributionMessage>,
) {
	let msg = msg.into();
	tracing::trace!(msg = ?msg, "sending message");
	overseer.send(FromOverseer::Communication { msg }).await
}


async fn overseer_recv(
	overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityDistributionMessage>,
) -> AllMessages {
	tracing::trace!("waiting for message ...");
	let msg = overseer.recv().await;
	tracing::trace!(msg = ?msg, "received message");
	msg
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
				pending_response.send(payload.map_err(|_| network::RequestFailure::Refused));
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
