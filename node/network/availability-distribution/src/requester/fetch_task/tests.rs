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

use std::collections::HashMap;


use parity_scale_codec::Encode;

use futures::channel::{mpsc, oneshot};
use futures::{executor, Future, FutureExt, StreamExt, select};
use futures::task::{Poll, Context, noop_waker};

use sc_network as network;
use sp_keyring::Sr25519Keyring;

use polkadot_primitives::v1::{BlockData, CandidateHash, PoV, ValidatorIndex};
use polkadot_node_network_protocol::request_response::v1;
use polkadot_subsystem::messages::AllMessages;

use crate::metrics::Metrics;
use crate::tests::mock::get_valid_chunk_data;
use super::*;

#[test]
fn task_can_be_canceled() {
	let (task, _rx) = get_test_running_task();
	let (handle, kill) = oneshot::channel();
	std::mem::drop(handle);
	let running_task = task.run(kill);
	futures::pin_mut!(running_task);
	let waker = noop_waker();
	let mut ctx = Context::from_waker(&waker);
	assert!(running_task.poll(&mut ctx) == Poll::Ready(()), "Task is immediately finished");
}

/// Make sure task won't accept a chunk that has is invalid.
#[test]
fn task_does_not_accept_invalid_chunk() {
	let (mut task, rx) = get_test_running_task();
	let validators = vec![Sr25519Keyring::Alice.public().into()];
	task.group = validators;
	let test = TestRun {
		chunk_responses:  {
			let mut m = HashMap::new();
			m.insert(
				Sr25519Keyring::Alice.public().into(),
				AvailabilityFetchingResponse::Chunk(
					v1::ChunkResponse {
						chunk: vec![1,2,3],
						proof: vec![vec![9,8,2], vec![2,3,4]],
					}
				)
			);
			m
		},
		valid_chunks: HashSet::new(),
	};
	test.run(task, rx);
}

#[test]
fn task_stores_valid_chunk() {
	let (mut task, rx) = get_test_running_task();
	let pov = PoV {
		block_data: BlockData(vec![45, 46, 47]),
	};
	let (root_hash, chunk) = get_valid_chunk_data(pov);
	task.erasure_root = root_hash;
	task.request.index = chunk.index;

	let validators = vec![Sr25519Keyring::Alice.public().into()];
	task.group = validators;

	let test = TestRun {
		chunk_responses:  {
			let mut m = HashMap::new();
			m.insert(
				Sr25519Keyring::Alice.public().into(),
				AvailabilityFetchingResponse::Chunk(
					v1::ChunkResponse {
						chunk: chunk.chunk.clone(),
						proof: chunk.proof,
					}
				)
			);
			m
		},
		valid_chunks: {
			let mut s = HashSet::new();
			s.insert(chunk.chunk);
			s
		},
	};
	test.run(task, rx);
}

#[test]
fn task_does_not_accept_wrongly_indexed_chunk() {
	let (mut task, rx) = get_test_running_task();
	let pov = PoV {
		block_data: BlockData(vec![45, 46, 47]),
	};
	let (root_hash, chunk) = get_valid_chunk_data(pov);
	task.erasure_root = root_hash;
	task.request.index = ValidatorIndex(chunk.index.0+1);

	let validators = vec![Sr25519Keyring::Alice.public().into()];
	task.group = validators;

	let test = TestRun {
		chunk_responses:  {
			let mut m = HashMap::new();
			m.insert(
				Sr25519Keyring::Alice.public().into(),
				AvailabilityFetchingResponse::Chunk(
					v1::ChunkResponse {
						chunk: chunk.chunk.clone(),
						proof: chunk.proof,
					}
				)
			);
			m
		},
		valid_chunks: HashSet::new(),
	};
	test.run(task, rx);
}

/// Task stores chunk, if there is at least one validator having a valid chunk.
#[test]
fn task_stores_valid_chunk_if_there_is_one() {
	let (mut task, rx) = get_test_running_task();
	let pov = PoV {
		block_data: BlockData(vec![45, 46, 47]),
	};
	let (root_hash, chunk) = get_valid_chunk_data(pov);
	task.erasure_root = root_hash;
	task.request.index = chunk.index;

	let validators = [
			// Only Alice has valid chunk - should succeed, even though she is tried last.
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob, Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave, Sr25519Keyring::Eve,
		]
		.iter().map(|v| v.public().into()).collect::<Vec<_>>();
	task.group = validators;

	let test = TestRun {
		chunk_responses:  {
			let mut m = HashMap::new();
			m.insert(
				Sr25519Keyring::Alice.public().into(),
				AvailabilityFetchingResponse::Chunk(
					v1::ChunkResponse {
						chunk: chunk.chunk.clone(),
						proof: chunk.proof,
					}
				)
			);
			m.insert(
				Sr25519Keyring::Bob.public().into(),
				AvailabilityFetchingResponse::NoSuchChunk
			);
			m.insert(
				Sr25519Keyring::Charlie.public().into(),
				AvailabilityFetchingResponse::Chunk(
					v1::ChunkResponse {
						chunk: vec![1,2,3],
						proof: vec![vec![9,8,2], vec![2,3,4]],
					}
				)
			);

			m
		},
		valid_chunks: {
			let mut s = HashSet::new();
			s.insert(chunk.chunk);
			s
		},
	};
	test.run(task, rx);
}

struct TestRun {
	/// Response to deliver for a given validator index.
	/// None means, answer with NetworkError.
	chunk_responses: HashMap<AuthorityDiscoveryId, AvailabilityFetchingResponse>,
	/// Set of chunks that should be considered valid:
	valid_chunks: HashSet<Vec<u8>>,
}


impl TestRun {
	fn run(self, task: RunningTask, rx: mpsc::Receiver<FromFetchTask>) {
		sp_tracing::try_init_simple();
		let mut rx = rx.fuse();
		let task = task.run_inner().fuse();
		futures::pin_mut!(task);
		executor::block_on(async {
			let mut end_ok = false;
			loop {
				let msg = select!(
					from_task = rx.next() => {
						match from_task {
							Some(msg) => msg,
							None => break,
						}
					},
					() = task =>
						break,
				);
				match msg {
					FromFetchTask::Concluded(_) => break,
					FromFetchTask::Message(msg) => 
						end_ok = self.handle_message(msg).await,
				}
			}
			if !end_ok {
				panic!("Task ended prematurely (failed to store valid chunk)!");
			}
		});
	}

	/// Returns true, if after processing of the given message it would be ok for the stream to
	/// end.
	async fn handle_message(&self, msg: AllMessages) -> bool {
		match msg {
			AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(reqs)) => {
				let mut valid_responses = 0;
				for req in reqs {
					let req = match req {
						Requests::AvailabilityFetching(req) => req,
					};
					let response = self.chunk_responses.get(&req.peer)
						.ok_or(network::RequestFailure::Refused);

					if let Ok(AvailabilityFetchingResponse::Chunk(resp)) = &response {
						if self.valid_chunks.contains(&resp.chunk) {
							valid_responses += 1;
						}
					}
					req.pending_response.send(response.map(Encode::encode))
						.expect("Sending response should succeed");
				}
				return (valid_responses == 0) && self.valid_chunks.is_empty()
			}
			AllMessages::AvailabilityStore(
				AvailabilityStoreMessage::StoreChunk { chunk, tx, .. }
			) => {
				assert!(self.valid_chunks.contains(&chunk.chunk));
				tx.send(Ok(())).expect("Answering fetching task should work");
				return true
			}
			_ => {
				tracing::debug!(target: LOG_TARGET, "Unexpected message");
				return false
			}
		}
	}
}

/// Get a `RunningTask` filled with dummy values.
fn get_test_running_task() -> (RunningTask, mpsc::Receiver<FromFetchTask>) {
	let (tx,rx) = mpsc::channel(0);

	(
		RunningTask {
			session_index: 0,
			group_index: GroupIndex(0),
			group: Vec::new(),
			request: AvailabilityFetchingRequest {
				candidate_hash: CandidateHash([43u8;32].into()),
				index: ValidatorIndex(0),
			},
			erasure_root: Hash::repeat_byte(99),
			relay_parent: Hash::repeat_byte(71),
			sender: tx,
			metrics: Metrics::new_dummy(),
			span: jaeger::Span::Disabled,
		},
		rx
	)
}

