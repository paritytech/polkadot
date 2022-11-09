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

//! JSON-RPC related types and helpers.

use super::*;
use jsonrpsee::{
	core::{Error as RpcError, RpcResult},
	proc_macros::rpc,
};
use pallet_transaction_payment::RuntimeDispatchInfo;
use sc_transaction_pool_api::TransactionStatus;
use sp_core::{storage::StorageKey, Bytes};
use sp_version::RuntimeVersion;
use std::{future::Future, time::Duration};

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
	/// Fetch system name.
	#[method(name = "system_chain")]
	async fn system_chain(&self) -> RpcResult<String>;

	/// Fetch a storage key.
	#[method(name = "state_getStorage")]
	async fn storage(&self, key: &StorageKey, hash: Option<Hash>) -> RpcResult<Option<Bytes>>;

	/// Fetch the runtime version.
	#[method(name = "state_getRuntimeVersion")]
	async fn runtime_version(&self, at: Option<Hash>) -> RpcResult<RuntimeVersion>;

	/// Fetch the payment query info.
	#[method(name = "payment_queryInfo")]
	async fn payment_query_info(
		&self,
		encoded_xt: &Bytes,
		at: Option<&Hash>,
	) -> RpcResult<RuntimeDispatchInfo<Balance>>;

	/// Dry run an extrinsic at a given block. Return SCALE encoded [`sp_runtime::ApplyExtrinsicResult`].
	#[method(name = "system_dryRun")]
	async fn dry_run(&self, extrinsic: &Bytes, at: Option<Hash>) -> RpcResult<Bytes>;

	/// Get hash of the n-th block in the canon chain.
	///
	/// By default returns latest block hash.
	#[method(name = "chain_getBlockHash", aliases = ["chain_getHead"], blocking)]
	fn block_hash(&self, hash: Option<Hash>) -> RpcResult<Option<Hash>>;

	/// Get hash of the last finalized block in the canon chain.
	#[method(name = "chain_getFinalizedHead", aliases = ["chain_getFinalisedHead"], blocking)]
	fn finalized_head(&self) -> RpcResult<Hash>;

	/// Submit an extrinsic to watch.
	///
	/// See [`TransactionStatus`](sc_transaction_pool_api::TransactionStatus) for details on
	/// transaction life cycle.
	#[subscription(
		name = "author_submitAndWatchExtrinsic" => "author_extrinsicUpdate",
		unsubscribe = "author_unwatchExtrinsic",
		item = TransactionStatus<Hash, Hash>
	)]
	fn watch_extrinsic(&self, bytes: &Bytes);

	/// New head subscription.
	#[subscription(
		name = "chain_subscribeNewHeads" => "newHead",
		unsubscribe = "chain_unsubscribeNewHeads",
		item = Header
	)]
	fn subscribe_new_heads(&self);

	/// Finalized head subscription.
	#[subscription(
		name = "chain_subscribeFinalizedHeads" => "chain_finalizedHead",
		unsubscribe = "chain_unsubscribeFinalizedHeads",
		item = Header
	)]
	fn subscribe_finalized_heads(&self);
}

/// Wraps a shared web-socket JSON-RPC client that can be cloned.
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

	/// Create a new shared JSON-RPC web-socket client.
	pub(crate) async fn new(
		uri: &str,
		connection_timeout: Duration,
		request_timeout: Duration,
	) -> Result<Self, RpcError> {
		let client = WsClientBuilder::default()
			.connection_timeout(connection_timeout)
			.max_request_body_size(u32::MAX)
			.request_timeout(request_timeout)
			.build(uri)
			.await?;
		Ok(Self(Arc::new(client)))
	}

	/// Get a storage item and decode it as `T`.
	///
	/// # Return value:
	///
	/// The function returns:
	///
	/// * `Ok(Some(val))` if successful.
	/// * `Ok(None)` if the storage item was not found.
	/// * `Err(e)` if the JSON-RPC call failed.
	pub(crate) async fn get_storage_and_decode<'a, T: codec::Decode>(
		&self,
		key: &StorageKey,
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
}

/// Takes a future that returns `Bytes` and tries to decode those bytes into the type `Dec`.
/// Warning: don't use for storage, it will fail for non-existent storage items.
///
/// # Return value:
///
/// The function returns:
///
/// * `Ok(val)` if successful.
/// * `Err(RpcHelperError::JsonRpsee)` if the JSON-RPC call failed.
/// * `Err(RpcHelperError::Codec)` if `Bytes` could not be decoded.
pub(crate) async fn await_request_and_decode<'a, Dec: codec::Decode>(
	req: impl Future<Output = Result<Bytes, RpcError>>,
) -> Result<Dec, RpcHelperError> {
	let bytes = req.await?;
	Dec::decode(&mut &*bytes.0).map_err::<RpcHelperError, _>(Into::into)
}
