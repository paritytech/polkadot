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

//! Helper method for RPC.

use super::*;
use jsonrpsee::core::{client::ClientT, Error as RpcError, RpcResult};
use jsonrpsee::proc_macros::rpc;
pub(crate) use jsonrpsee::types::ParamsSer;
use pallet_transaction_payment::RuntimeDispatchInfo;
use sc_transaction_pool_api::TransactionStatus;
use sp_core::storage::StorageKey;
use sp_core::Bytes;
use sp_version::RuntimeVersion;
use std::time::Duration;

#[derive(frame_support::DebugNoBound, thiserror::Error)]
pub(crate) enum RpcHelperError {
	JsonRpsee(#[from] jsonrpsee::core::Error),
	Codec(#[from] codec::Error),
}

impl std::fmt::Display for RpcHelperError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		<RpcHelperError as std::fmt::Debug>::fmt(self, f)
	}
}

#[rpc(client)]
pub trait RpcApi {
	/// Fetch system name
	#[method(name = "system_chain")]
	async fn system_chain(&self) -> RpcResult<String>;

	/// Fetch a storage key
	#[method(name = "state_getStorage")]
	async fn storage(&self, key: StorageKey, hash: Option<Hash>) -> RpcResult<Option<Bytes>>;

	/// Fetch the runtime version
	#[method(name = "state_getRuntimeVersion")]
	async fn runtime_version(&self, at: Option<Hash>) -> RpcResult<RuntimeVersion>;

	#[method(name = "payment_queryInfo")]
	async fn payment_query_info(
		&self,
		encoded_xt: Bytes,
		at: Option<Hash>,
	) -> RpcResult<RuntimeDispatchInfo<Balance>>;

	/// Submit an extrinsic to watch.
	///
	/// See [`TransactionStatus`](sc_transaction_pool_api::TransactionStatus) for details on
	/// transaction life cycle.
	#[subscription(
		name = "author_submitAndWatchExtrinsic" => "extrinsicUpdate",
		unsubscribe_aliases = ["author_unwatchExtrinsic"],
		item = TransactionStatus<Hash, Hash>,
	)]
	fn watch_extrinsic(&self, bytes: Bytes) -> RpcResult<()>;

	/// New head subscription.
	#[subscription(
		name = "chain_subscribeNewHeads" => "newHead",
		item = Header
	)]
	fn subscribe_new_heads(&self) -> RpcResult<()>;

	/// Finalized head subscription.
	#[subscription(
		name = "chain_subscribeFinalizedHeads" => "finalizedHead",
		item = Header
	)]
	fn subscribe_finalized_heads(&self) -> RpcResult<()>;
}

/// Wraps a shared websocket JSON-RPC client that can be cloned.
#[derive(Clone, Debug)]
pub(crate) struct SharedRpcClient(Arc<WsClient>);

impl Deref for SharedRpcClient {
	type Target = WsClient;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl SharedRpcClient {
	/// Consume and extract the inner client.
	pub fn into_inner(self) -> Arc<WsClient> {
		self.0
	}

	/// Create a new shared RPC client.
	pub(crate) async fn new(uri: &str) -> Result<Self, RpcError> {
		let client = WsClientBuilder::default()
			.connection_timeout(Duration::from_secs(20))
			.max_request_body_size(u32::MAX)
			.request_timeout(Duration::from_secs(10 * 60))
			.build(uri)
			.await?;
		Ok(Self(Arc::new(client)))
	}

	/// Get the storage item.
	pub(crate) async fn get_storage<'a, T: codec::Decode>(
		&self,
		key: StorageKey,
		hash: Option<Hash>,
	) -> Result<Option<T>, RpcHelperError> {
		if let Some(bytes) = self.storage(key, hash).await? {
			let decoded = <T as codec::Decode>::decode(&mut &*bytes.0)
				.map_err::<RpcHelperError, _>(Into::into)?;
			Ok(Some(decoded))
		} else {
			Ok(None)
		}
	}

	/// Make the rpc request, decode the outcome into `Dec`. Don't use for storage, it will fail for
	/// non-existent storage items.
	pub(crate) async fn request_and_decode<'a, Dec: codec::Decode>(
		&self,
		method: &'a str,
		params: Option<ParamsSer<'a>>,
	) -> Result<Dec, RpcHelperError> {
		let bytes = self
			.request::<sp_core::Bytes>(method, params)
			.await
			.map_err::<RpcHelperError, _>(Into::into)?;
		<Dec as codec::Decode>::decode(&mut &*bytes.0).map_err::<RpcHelperError, _>(Into::into)
	}
}
