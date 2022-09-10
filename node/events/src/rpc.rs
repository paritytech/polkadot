// This file is part of Substrate.

// Copyright (C) 2021-2022 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! RPC API for Node Events.

#![warn(missing_docs)]

use sc_rpc::SubscriptionTaskExecutor;

use futures::{task::SpawnError, FutureExt, StreamExt};
use jsonrpsee::{
	core::{async_trait, Error as JsonRpseeError, RpcResult},
	proc_macros::rpc,
	types::{error::CallError, ErrorObject, SubscriptionResult},
	SubscriptionSink,
};
// use log::warn;

use super::*;

#[derive(Debug, thiserror::Error)]
/// Top-level error type for the RPC handler
pub enum Error {
	/// The Node Events RPC endpoint is not ready.
	#[error("Node Events RPC endpoint not ready")]
	EndpointNotReady,
	/// The Node Events RPC background task failed to spawn.
	#[error("Node Events RPC background task failed to spawn")]
	RpcTaskFailure(#[from] SpawnError),
}

/// The error codes returned by jsonrpc.
pub enum ErrorCode {
	/// Returned when Node Events RPC endpoint is not ready.
	NotReady = 1,
	/// Returned on Node Events RPC background task failure.
	TaskFailure = 2,
}

impl From<Error> for ErrorCode {
	fn from(error: Error) -> Self {
		match error {
			Error::EndpointNotReady => ErrorCode::NotReady,
			Error::RpcTaskFailure(_) => ErrorCode::TaskFailure,
		}
	}
}

impl From<Error> for JsonRpseeError {
	fn from(error: Error) -> Self {
		let message = error.to_string();
		let code = ErrorCode::from(error);
		JsonRpseeError::Call(CallError::Custom(ErrorObject::owned(
			code as i32,
			message,
			None::<()>,
		)))
	}
}

// Provides RPC methods.
#[rpc(client, server)]
pub trait NodeEventsApi<Notification> {
	/// Returns the node events as they happen.
	#[subscription(
		name = "node_subscribeEvents" => "node_events",
		unsubscribe = "node_unsubscribeEvents",
		item = Notification,
	)]
	fn subscribe_node_events(&self);
}

/// Implements the BeefyApi RPC trait for interacting with BEEFY.
pub struct NodeEventsBackend {
	event_stream: NodeEventsStream,
	executor: SubscriptionTaskExecutor,
}

impl NodeEventsBackend {
	/// Creates a new Beefy Rpc handler instance.
	pub fn new(
		event_stream: NodeEventsStream,
		executor: SubscriptionTaskExecutor,
	) -> Result<Self, Error> {
		Ok(Self { event_stream, executor })
	}
}

#[async_trait]
impl<NodeEvents> NodeEventsApiServer<NodeEvents> for NodeEventsBackend {
	fn subscribe_node_events(&self, mut sink: SubscriptionSink) -> SubscriptionResult {
		let stream = self.event_stream.subscribe().map(|a| a);

		let fut = async move {
			let _ = sink.pipe_from_stream(stream).await;
		};

		self.executor
			.spawn("substrate-rpc-node-events-subscription", Some("rpc"), fut.boxed());
		Ok(())
	}
}

// #[cfg(test)]
// mod tests {
// 	use super::*;

// 	use beefy_gadget::{
// 		justification::BeefyVersionedFinalityProof,
// 		notification::{BeefyBestBlockStream, BeefyVersionedFinalityProofSender},
// 	};
// 	use beefy_primitives::{known_payload_ids, Payload, SignedCommitment};
// 	use codec::{Decode, Encode};
// 	use jsonrpsee::{types::EmptyParams, RpcModule};
// 	use sp_runtime::traits::{BlakeTwo256, Hash};
// 	use substrate_test_runtime_client::runtime::Block;

// 	fn setup_io_handler() -> (RpcModule<Beefy<Block>>, BeefyVersionedFinalityProofSender<Block>) {
// 		let (_, stream) = BeefyBestBlockStream::<Block>::channel();
// 		setup_io_handler_with_best_block_stream(stream)
// 	}

// 	fn setup_io_handler_with_best_block_stream(
// 		best_block_stream: BeefyBestBlockStream<Block>,
// 	) -> (RpcModule<Beefy<Block>>, BeefyVersionedFinalityProofSender<Block>) {
// 		let (finality_proof_sender, finality_proof_stream) =
// 			BeefyVersionedFinalityProofStream::<Block>::channel();

// 		let handler =
// 			Beefy::new(finality_proof_stream, best_block_stream, sc_rpc::testing::test_executor())
// 				.expect("Setting up the BEEFY RPC handler works");

// 		(handler.into_rpc(), finality_proof_sender)
// 	}

// 	#[tokio::test]
// 	async fn uninitialized_rpc_handler() {
// 		let (rpc, _) = setup_io_handler();
// 		let request = r#"{"jsonrpc":"2.0","method":"beefy_getFinalizedHead","params":[],"id":1}"#;
// 		let expected_response = r#"{"jsonrpc":"2.0","error":{"code":1,"message":"BEEFY RPC endpoint not ready"},"id":1}"#.to_string();
// 		let (response, _) = rpc.raw_json_request(&request).await.unwrap();

// 		assert_eq!(expected_response, response.result);
// 	}

// 	#[tokio::test]
// 	async fn latest_finalized_rpc() {
// 		let (sender, stream) = BeefyBestBlockStream::<Block>::channel();
// 		let (io, _) = setup_io_handler_with_best_block_stream(stream);

// 		let hash = BlakeTwo256::hash(b"42");
// 		let r: Result<(), ()> = sender.notify(|| Ok(hash));
// 		r.unwrap();

// 		// Verify RPC `beefy_getFinalizedHead` returns expected hash.
// 		let request = r#"{"jsonrpc":"2.0","method":"beefy_getFinalizedHead","params":[],"id":1}"#;
// 		let expected = "{\
// 			\"jsonrpc\":\"2.0\",\
// 			\"result\":\"0x2f0039e93a27221fcf657fb877a1d4f60307106113e885096cb44a461cd0afbf\",\
// 			\"id\":1\
// 		}"
// 		.to_string();
// 		let not_ready = "{\
// 			\"jsonrpc\":\"2.0\",\
// 			\"error\":{\"code\":1,\"message\":\"BEEFY RPC endpoint not ready\"},\
// 			\"id\":1\
// 		}"
// 		.to_string();

// 		let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
// 		while std::time::Instant::now() < deadline {
// 			let (response, _) = io.raw_json_request(request).await.expect("RPC requests work");
// 			if response.result != not_ready {
// 				assert_eq!(response.result, expected);
// 				// Success
// 				return
// 			}
// 			std::thread::sleep(std::time::Duration::from_millis(50))
// 		}

// 		panic!(
// 			"Deadline reached while waiting for best BEEFY block to update. Perhaps the background task is broken?"
// 		);
// 	}

// 	#[tokio::test]
// 	async fn subscribe_and_unsubscribe_with_wrong_id() {
// 		let (rpc, _) = setup_io_handler();
// 		// Subscribe call.
// 		let _sub = rpc
// 			.subscribe("beefy_subscribeJustifications", EmptyParams::new())
// 			.await
// 			.unwrap();

// 		// Unsubscribe with wrong ID
// 		let (response, _) = rpc
// 			.raw_json_request(
// 				r#"{"jsonrpc":"2.0","method":"beefy_unsubscribeJustifications","params":["FOO"],"id":1}"#,
// 			)
// 			.await
// 			.unwrap();
// 		let expected = r#"{"jsonrpc":"2.0","result":false,"id":1}"#;

// 		assert_eq!(response.result, expected);
// 	}

// 	fn create_finality_proof() -> BeefyVersionedFinalityProof<Block> {
// 		let payload = Payload::new(known_payload_ids::MMR_ROOT_ID, "Hello World!".encode());
// 		BeefyVersionedFinalityProof::<Block>::V1(SignedCommitment {
// 			commitment: beefy_primitives::Commitment {
// 				payload,
// 				block_number: 5,
// 				validator_set_id: 0,
// 			},
// 			signatures: vec![],
// 		})
// 	}

// 	#[tokio::test]
// 	async fn subscribe_and_listen_to_one_justification() {
// 		let (rpc, finality_proof_sender) = setup_io_handler();

// 		// Subscribe
// 		let mut sub = rpc
// 			.subscribe("beefy_subscribeJustifications", EmptyParams::new())
// 			.await
// 			.unwrap();

// 		// Notify with finality_proof
// 		let finality_proof = create_finality_proof();
// 		let r: Result<(), ()> = finality_proof_sender.notify(|| Ok(finality_proof.clone()));
// 		r.unwrap();

// 		// Inspect what we received
// 		let (bytes, recv_sub_id) = sub.next::<sp_core::Bytes>().await.unwrap().unwrap();
// 		let recv_finality_proof: BeefyVersionedFinalityProof<Block> =
// 			Decode::decode(&mut &bytes[..]).unwrap();
// 		assert_eq!(&recv_sub_id, sub.subscription_id());
// 		assert_eq!(recv_finality_proof, finality_proof);
// 	}
// }
