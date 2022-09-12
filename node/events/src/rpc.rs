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

/// Implements the NodeEventsApi RPC trait for receiving node events.
pub struct NodeEventsBackend {
	event_stream: NodeEventsStream,
	executor: SubscriptionTaskExecutor,
}

impl NodeEventsBackend {
	/// Creates a new Node event Rpc handler instance.
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
