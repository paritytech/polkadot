// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Substrate node client.

use crate::{
	chain::{Chain, ChainWithBalances, TransactionStatusOf},
	rpc::SubstrateClient,
	AccountIdOf, BlockNumberOf, ConnectionParams, Error, HashOf, HeaderIdOf, HeaderOf, IndexOf,
	Result,
};

use async_std::sync::{Arc, Mutex};
use async_trait::async_trait;
use codec::{Decode, Encode};
use frame_system::AccountInfo;
use futures::{SinkExt, StreamExt};
use jsonrpsee::{
	core::{client::SubscriptionClientT, DeserializeOwned},
	types::params::ParamsSer,
	ws_client::{WsClient as RpcClient, WsClientBuilder as RpcClientBuilder},
};
use num_traits::{Bounded, CheckedSub, One, Zero};
use pallet_balances::AccountData;
use pallet_transaction_payment::InclusionFee;
use relay_utils::{relay_loop::RECONNECT_DELAY, HeaderId};
use sp_core::{
	storage::{StorageData, StorageKey},
	Bytes, Hasher,
};
use sp_runtime::{
	traits::Header as HeaderT,
	transaction_validity::{TransactionSource, TransactionValidity},
};
use sp_trie::StorageProof;
use sp_version::RuntimeVersion;
use std::future::Future;

const SUB_API_GRANDPA_AUTHORITIES: &str = "GrandpaApi_grandpa_authorities";
const SUB_API_TXPOOL_VALIDATE_TRANSACTION: &str = "TaggedTransactionQueue_validate_transaction";
const MAX_SUBSCRIPTION_CAPACITY: usize = 4096;

/// Opaque justifications subscription type.
pub struct Subscription<T>(Mutex<futures::channel::mpsc::Receiver<Option<T>>>);

/// Opaque GRANDPA authorities set.
pub type OpaqueGrandpaAuthoritiesSet = Vec<u8>;

/// Chain runtime version in client
#[derive(Clone, Debug)]
pub enum ChainRuntimeVersion {
	/// Auto query from chain.
	Auto,
	/// Custom runtime version, defined by user.
	/// the first is `spec_version`
	/// the second is `transaction_version`
	Custom(u32, u32),
}

/// Substrate client type.
///
/// Cloning `Client` is a cheap operation.
pub struct Client<C: Chain> {
	/// Tokio runtime handle.
	tokio: Arc<tokio::runtime::Runtime>,
	/// Client connection params.
	params: ConnectionParams,
	/// Substrate RPC client.
	client: Arc<RpcClient>,
	/// Genesis block hash.
	genesis_hash: HashOf<C>,
	/// If several tasks are submitting their transactions simultaneously using
	/// `submit_signed_extrinsic` method, they may get the same transaction nonce. So one of
	/// transactions will be rejected from the pool. This lock is here to prevent situations like
	/// that.
	submit_signed_extrinsic_lock: Arc<Mutex<()>>,
	/// Saved chain runtime version
	chain_runtime_version: ChainRuntimeVersion,
}

#[async_trait]
impl<C: Chain> relay_utils::relay_loop::Client for Client<C> {
	type Error = Error;

	async fn reconnect(&mut self) -> Result<()> {
		let (tokio, client) = Self::build_client(self.params.clone()).await?;
		self.tokio = tokio;
		self.client = client;
		Ok(())
	}
}

impl<C: Chain> Clone for Client<C> {
	fn clone(&self) -> Self {
		Client {
			tokio: self.tokio.clone(),
			params: self.params.clone(),
			client: self.client.clone(),
			genesis_hash: self.genesis_hash,
			submit_signed_extrinsic_lock: self.submit_signed_extrinsic_lock.clone(),
			chain_runtime_version: self.chain_runtime_version.clone(),
		}
	}
}

impl<C: Chain> std::fmt::Debug for Client<C> {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		fmt.debug_struct("Client").field("genesis_hash", &self.genesis_hash).finish()
	}
}

impl<C: Chain> Client<C> {
	/// Returns client that is able to call RPCs on Substrate node over websocket connection.
	///
	/// This function will keep connecting to given Substrate node until connection is established
	/// and is functional. If attempt fail, it will wait for `RECONNECT_DELAY` and retry again.
	pub async fn new(params: ConnectionParams) -> Self {
		loop {
			match Self::try_connect(params.clone()).await {
				Ok(client) => return client,
				Err(error) => log::error!(
					target: "bridge",
					"Failed to connect to {} node: {:?}. Going to retry in {}s",
					C::NAME,
					error,
					RECONNECT_DELAY.as_secs(),
				),
			}

			async_std::task::sleep(RECONNECT_DELAY).await;
		}
	}

	/// Try to connect to Substrate node over websocket. Returns Substrate RPC client if connection
	/// has been established or error otherwise.
	pub async fn try_connect(params: ConnectionParams) -> Result<Self> {
		let (tokio, client) = Self::build_client(params.clone()).await?;

		let number: C::BlockNumber = Zero::zero();
		let genesis_hash_client = client.clone();
		let genesis_hash = tokio
			.spawn(async move {
				SubstrateClient::<
					AccountIdOf<C>,
					BlockNumberOf<C>,
					HashOf<C>,
					HeaderOf<C>,
					IndexOf<C>,
					C::SignedBlock,
				>::chain_get_block_hash(&*genesis_hash_client, Some(number))
				.await
			})
			.await??;

		let chain_runtime_version = params.chain_runtime_version.clone();
		Ok(Self {
			tokio,
			params,
			client,
			genesis_hash,
			submit_signed_extrinsic_lock: Arc::new(Mutex::new(())),
			chain_runtime_version,
		})
	}

	/// Build client to use in connection.
	async fn build_client(
		params: ConnectionParams,
	) -> Result<(Arc<tokio::runtime::Runtime>, Arc<RpcClient>)> {
		let tokio = tokio::runtime::Runtime::new()?;
		let uri = format!(
			"{}://{}:{}",
			if params.secure { "wss" } else { "ws" },
			params.host,
			params.port,
		);
		let client = tokio
			.spawn(async move {
				RpcClientBuilder::default()
					.max_notifs_per_subscription(MAX_SUBSCRIPTION_CAPACITY)
					.build(&uri)
					.await
			})
			.await??;

		Ok((Arc::new(tokio), Arc::new(client)))
	}
}

impl<C: Chain> Client<C> {
	/// Return simple runtime version, only include `spec_version` and `transaction_version`.
	pub async fn simple_runtime_version(&self) -> Result<(u32, u32)> {
		let (spec_version, transaction_version) = match self.chain_runtime_version {
			ChainRuntimeVersion::Auto => {
				let runtime_version = self.runtime_version().await?;
				(runtime_version.spec_version, runtime_version.transaction_version)
			},
			ChainRuntimeVersion::Custom(spec_version, transaction_version) =>
				(spec_version, transaction_version),
		};
		Ok((spec_version, transaction_version))
	}

	/// Returns true if client is connected to at least one peer and is in synced state.
	pub async fn ensure_synced(&self) -> Result<()> {
		self.jsonrpsee_execute(|client| async move {
			let health = SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::system_health(&*client)
			.await?;
			let is_synced = !health.is_syncing && (!health.should_have_peers || health.peers > 0);
			if is_synced {
				Ok(())
			} else {
				Err(Error::ClientNotSynced(health))
			}
		})
		.await
	}

	/// Return hash of the genesis block.
	pub fn genesis_hash(&self) -> &C::Hash {
		&self.genesis_hash
	}

	/// Return hash of the best finalized block.
	pub async fn best_finalized_header_hash(&self) -> Result<C::Hash> {
		self.jsonrpsee_execute(|client| async move {
			Ok(SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::chain_get_finalized_head(&*client)
			.await?)
		})
		.await
	}

	/// Return number of the best finalized block.
	pub async fn best_finalized_header_number(&self) -> Result<C::BlockNumber> {
		Ok(*self.header_by_hash(self.best_finalized_header_hash().await?).await?.number())
	}

	/// Returns the best Substrate header.
	pub async fn best_header(&self) -> Result<C::Header>
	where
		C::Header: DeserializeOwned,
	{
		self.jsonrpsee_execute(|client| async move {
			Ok(SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::chain_get_header(&*client, None)
			.await?)
		})
		.await
	}

	/// Get a Substrate block from its hash.
	pub async fn get_block(&self, block_hash: Option<C::Hash>) -> Result<C::SignedBlock> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::chain_get_block(&*client, block_hash)
			.await?)
		})
		.await
	}

	/// Get a Substrate header by its hash.
	pub async fn header_by_hash(&self, block_hash: C::Hash) -> Result<C::Header>
	where
		C::Header: DeserializeOwned,
	{
		self.jsonrpsee_execute(move |client| async move {
			Ok(SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::chain_get_header(&*client, Some(block_hash))
			.await?)
		})
		.await
	}

	/// Get a Substrate block hash by its number.
	pub async fn block_hash_by_number(&self, number: C::BlockNumber) -> Result<C::Hash> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::chain_get_block_hash(&*client, Some(number))
			.await?)
		})
		.await
	}

	/// Get a Substrate header by its number.
	pub async fn header_by_number(&self, block_number: C::BlockNumber) -> Result<C::Header>
	where
		C::Header: DeserializeOwned,
	{
		let block_hash = Self::block_hash_by_number(self, block_number).await?;
		let header_by_hash = Self::header_by_hash(self, block_hash).await?;
		Ok(header_by_hash)
	}

	/// Return runtime version.
	pub async fn runtime_version(&self) -> Result<RuntimeVersion> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::state_runtime_version(&*client)
			.await?)
		})
		.await
	}

	/// Read value from runtime storage.
	pub async fn storage_value<T: Send + Decode + 'static>(
		&self,
		storage_key: StorageKey,
		block_hash: Option<C::Hash>,
	) -> Result<Option<T>> {
		self.raw_storage_value(storage_key, block_hash)
			.await?
			.map(|encoded_value| {
				T::decode(&mut &encoded_value.0[..]).map_err(Error::ResponseParseFailed)
			})
			.transpose()
	}

	/// Read raw value from runtime storage.
	pub async fn raw_storage_value(
		&self,
		storage_key: StorageKey,
		block_hash: Option<C::Hash>,
	) -> Result<Option<StorageData>> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::state_get_storage(&*client, storage_key, block_hash)
			.await?)
		})
		.await
	}

	/// Return native tokens balance of the account.
	pub async fn free_native_balance(&self, account: C::AccountId) -> Result<C::Balance>
	where
		C: ChainWithBalances,
	{
		self.jsonrpsee_execute(move |client| async move {
			let storage_key = C::account_info_storage_key(&account);
			let encoded_account_data = SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::state_get_storage(&*client, storage_key, None)
			.await?
			.ok_or(Error::AccountDoesNotExist)?;
			let decoded_account_data = AccountInfo::<C::Index, AccountData<C::Balance>>::decode(
				&mut &encoded_account_data.0[..],
			)
			.map_err(Error::ResponseParseFailed)?;
			Ok(decoded_account_data.data.free)
		})
		.await
	}

	/// Get the nonce of the given Substrate account.
	///
	/// Note: It's the caller's responsibility to make sure `account` is a valid SS58 address.
	pub async fn next_account_index(&self, account: C::AccountId) -> Result<C::Index> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::system_account_next_index(&*client, account)
			.await?)
		})
		.await
	}

	/// Submit unsigned extrinsic for inclusion in a block.
	///
	/// Note: The given transaction needs to be SCALE encoded beforehand.
	pub async fn submit_unsigned_extrinsic(&self, transaction: Bytes) -> Result<C::Hash> {
		self.jsonrpsee_execute(move |client| async move {
			let tx_hash = SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::author_submit_extrinsic(&*client, transaction)
			.await?;
			log::trace!(target: "bridge", "Sent transaction to Substrate node: {:?}", tx_hash);
			Ok(tx_hash)
		})
		.await
	}

	/// Submit an extrinsic signed by given account.
	///
	/// All calls of this method are synchronized, so there can't be more than one active
	/// `submit_signed_extrinsic()` call. This guarantees that no nonces collision may happen
	/// if all client instances are clones of the same initial `Client`.
	///
	/// Note: The given transaction needs to be SCALE encoded beforehand.
	pub async fn submit_signed_extrinsic(
		&self,
		extrinsic_signer: C::AccountId,
		prepare_extrinsic: impl FnOnce(HeaderIdOf<C>, C::Index) -> Result<Bytes> + Send + 'static,
	) -> Result<C::Hash> {
		let _guard = self.submit_signed_extrinsic_lock.lock().await;
		let transaction_nonce = self.next_account_index(extrinsic_signer).await?;
		let best_header = self.best_header().await?;

		// By using parent of best block here, we are protecing again best-block reorganizations.
		// E.g. transaction my have been submitted when the best block was `A[num=100]`. Then it has
		// been changed to `B[num=100]`. Hash of `A` has been included into transaction signature
		// payload. So when signature will be checked, the check will fail and transaction will be
		// dropped from the pool.
		let best_header_id = match best_header.number().checked_sub(&One::one()) {
			Some(parent_block_number) => HeaderId(parent_block_number, *best_header.parent_hash()),
			None => HeaderId(*best_header.number(), best_header.hash()),
		};

		self.jsonrpsee_execute(move |client| async move {
			let extrinsic = prepare_extrinsic(best_header_id, transaction_nonce)?;
			let tx_hash = SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::author_submit_extrinsic(&*client, extrinsic)
			.await?;
			log::trace!(target: "bridge", "Sent transaction to {} node: {:?}", C::NAME, tx_hash);
			Ok(tx_hash)
		})
		.await
	}

	/// Does exactly the same as `submit_signed_extrinsic`, but keeps watching for extrinsic status
	/// after submission.
	pub async fn submit_and_watch_signed_extrinsic(
		&self,
		extrinsic_signer: C::AccountId,
		prepare_extrinsic: impl FnOnce(HeaderIdOf<C>, C::Index) -> Result<Bytes> + Send + 'static,
	) -> Result<Subscription<TransactionStatusOf<C>>> {
		let _guard = self.submit_signed_extrinsic_lock.lock().await;
		let transaction_nonce = self.next_account_index(extrinsic_signer).await?;
		let best_header = self.best_header().await?;
		let best_header_id = HeaderId(*best_header.number(), best_header.hash());
		let subscription = self
			.jsonrpsee_execute(move |client| async move {
				let extrinsic = prepare_extrinsic(best_header_id, transaction_nonce)?;
				let tx_hash = C::Hasher::hash(&extrinsic.0);
				let subscription = client
					.subscribe(
						"author_submitAndWatchExtrinsic",
						Some(ParamsSer::Array(vec![jsonrpsee::core::to_json_value(extrinsic)
							.map_err(|e| Error::RpcError(e.into()))?])),
						"author_unwatchExtrinsic",
					)
					.await?;
				log::trace!(target: "bridge", "Sent transaction to {} node: {:?}", C::NAME, tx_hash);
				Ok(subscription)
			})
			.await?;
		let (sender, receiver) = futures::channel::mpsc::channel(MAX_SUBSCRIPTION_CAPACITY);
		self.tokio.spawn(Subscription::background_worker(
			C::NAME.into(),
			"extrinsic".into(),
			subscription,
			sender,
		));
		Ok(Subscription(Mutex::new(receiver)))
	}

	/// Returns pending extrinsics from transaction pool.
	pub async fn pending_extrinsics(&self) -> Result<Vec<Bytes>> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::author_pending_extrinsics(&*client)
			.await?)
		})
		.await
	}

	/// Validate transaction at given block state.
	pub async fn validate_transaction<SignedTransaction: Encode + Send + 'static>(
		&self,
		at_block: C::Hash,
		transaction: SignedTransaction,
	) -> Result<TransactionValidity> {
		self.jsonrpsee_execute(move |client| async move {
			let call = SUB_API_TXPOOL_VALIDATE_TRANSACTION.to_string();
			let data = Bytes((TransactionSource::External, transaction, at_block).encode());

			let encoded_response = SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::state_call(&*client, call, data, Some(at_block))
			.await?;
			let validity = TransactionValidity::decode(&mut &encoded_response.0[..])
				.map_err(Error::ResponseParseFailed)?;

			Ok(validity)
		})
		.await
	}

	/// Estimate fee that will be spent on given extrinsic.
	pub async fn estimate_extrinsic_fee(
		&self,
		transaction: Bytes,
	) -> Result<InclusionFee<C::Balance>> {
		self.jsonrpsee_execute(move |client| async move {
			let fee_details = SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::payment_query_fee_details(&*client, transaction, None)
			.await?;
			let inclusion_fee = fee_details
				.inclusion_fee
				.map(|inclusion_fee| InclusionFee {
					base_fee: C::Balance::try_from(inclusion_fee.base_fee.into_u256())
						.unwrap_or_else(|_| C::Balance::max_value()),
					len_fee: C::Balance::try_from(inclusion_fee.len_fee.into_u256())
						.unwrap_or_else(|_| C::Balance::max_value()),
					adjusted_weight_fee: C::Balance::try_from(
						inclusion_fee.adjusted_weight_fee.into_u256(),
					)
					.unwrap_or_else(|_| C::Balance::max_value()),
				})
				.unwrap_or_else(|| InclusionFee {
					base_fee: Zero::zero(),
					len_fee: Zero::zero(),
					adjusted_weight_fee: Zero::zero(),
				});
			Ok(inclusion_fee)
		})
		.await
	}

	/// Get the GRANDPA authority set at given block.
	pub async fn grandpa_authorities_set(
		&self,
		block: C::Hash,
	) -> Result<OpaqueGrandpaAuthoritiesSet> {
		self.jsonrpsee_execute(move |client| async move {
			let call = SUB_API_GRANDPA_AUTHORITIES.to_string();
			let data = Bytes(Vec::new());

			let encoded_response = SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::state_call(&*client, call, data, Some(block))
			.await?;
			let authority_list = encoded_response.0;

			Ok(authority_list)
		})
		.await
	}

	/// Execute runtime call at given block.
	pub async fn state_call(
		&self,
		method: String,
		data: Bytes,
		at_block: Option<C::Hash>,
	) -> Result<Bytes> {
		self.jsonrpsee_execute(move |client| async move {
			SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::state_call(&*client, method, data, at_block)
			.await
			.map_err(Into::into)
		})
		.await
	}

	/// Returns storage proof of given storage keys.
	pub async fn prove_storage(
		&self,
		keys: Vec<StorageKey>,
		at_block: C::Hash,
	) -> Result<StorageProof> {
		self.jsonrpsee_execute(move |client| async move {
			SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::state_prove_storage(&*client, keys, Some(at_block))
			.await
			.map(|proof| {
				StorageProof::new(proof.proof.into_iter().map(|b| b.0).collect::<Vec<_>>())
			})
			.map_err(Into::into)
		})
		.await
	}

	/// Return `tokenDecimals` property from the set of chain properties.
	pub async fn token_decimals(&self) -> Result<Option<u64>> {
		self.jsonrpsee_execute(move |client| async move {
			let system_properties = SubstrateClient::<
				AccountIdOf<C>,
				BlockNumberOf<C>,
				HashOf<C>,
				HeaderOf<C>,
				IndexOf<C>,
				C::SignedBlock,
			>::system_properties(&*client)
			.await?;
			Ok(system_properties.get("tokenDecimals").and_then(|v| v.as_u64()))
		})
		.await
	}

	/// Return new justifications stream.
	pub async fn subscribe_justifications(&self) -> Result<Subscription<Bytes>> {
		let subscription = self
			.jsonrpsee_execute(move |client| async move {
				Ok(client
					.subscribe(
						"grandpa_subscribeJustifications",
						None,
						"grandpa_unsubscribeJustifications",
					)
					.await?)
			})
			.await?;
		let (sender, receiver) = futures::channel::mpsc::channel(MAX_SUBSCRIPTION_CAPACITY);
		self.tokio.spawn(Subscription::background_worker(
			C::NAME.into(),
			"justification".into(),
			subscription,
			sender,
		));
		Ok(Subscription(Mutex::new(receiver)))
	}

	/// Execute jsonrpsee future in tokio context.
	async fn jsonrpsee_execute<MF, F, T>(&self, make_jsonrpsee_future: MF) -> Result<T>
	where
		MF: FnOnce(Arc<RpcClient>) -> F + Send + 'static,
		F: Future<Output = Result<T>> + Send,
		T: Send + 'static,
	{
		let client = self.client.clone();
		self.tokio.spawn(async move { make_jsonrpsee_future(client).await }).await?
	}
}

impl<T: DeserializeOwned> Subscription<T> {
	/// Return next item from the subscription.
	pub async fn next(&self) -> Result<Option<T>> {
		let mut receiver = self.0.lock().await;
		let item = receiver.next().await;
		Ok(item.unwrap_or(None))
	}

	/// Background worker that is executed in tokio context as `jsonrpsee` requires.
	async fn background_worker(
		chain_name: String,
		item_type: String,
		mut subscription: jsonrpsee::core::client::Subscription<T>,
		mut sender: futures::channel::mpsc::Sender<Option<T>>,
	) {
		loop {
			match subscription.next().await {
				Some(Ok(item)) =>
					if sender.send(Some(item)).await.is_err() {
						break
					},
				Some(Err(e)) => {
					log::trace!(
						target: "bridge",
						"{} {} subscription stream has returned '{:?}'. Stream needs to be restarted.",
						chain_name,
						item_type,
						e,
					);
					let _ = sender.send(None).await;
					break
				},
				None => {
					log::trace!(
						target: "bridge",
						"{} {} subscription stream has returned None. Stream needs to be restarted.",
						chain_name,
						item_type,
					);
					let _ = sender.send(None).await;
					break
				},
			}
		}
	}
}
