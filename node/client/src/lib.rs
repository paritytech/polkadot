// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Polkadot Client
//!
//! Provides the [`AbstractClient`] trait that is a super trait that combines all the traits the client implements.
//! There is also the [`Client`] enum that combines all the different clients into one common structure.

use polkadot_primitives::{
	runtime_api::ParachainHost,
	v2::{AccountId, Balance, Block, BlockNumber, Hash, Header, Nonce},
};
use sc_client_api::{AuxStore, Backend as BackendT, BlockchainEvents, KeyIterator, UsageProvider};
use sc_executor::NativeElseWasmExecutor;
use sp_api::{CallApiAt, Encode, NumberFor, ProvideRuntimeApi};
use sp_blockchain::{HeaderBackend, HeaderMetadata};
use sp_consensus::BlockStatus;
use sp_runtime::{
	generic::{BlockId, SignedBlock},
	traits::{BlakeTwo256, Block as BlockT},
	Justifications,
};
use sp_storage::{ChildInfo, StorageData, StorageKey};
use std::sync::Arc;

pub mod benchmarking;

pub type FullBackend = sc_service::TFullBackend<Block>;

pub type FullClient<RuntimeApi, ExecutorDispatch> =
	sc_service::TFullClient<Block, RuntimeApi, NativeElseWasmExecutor<ExecutorDispatch>>;

#[cfg(not(any(
	feature = "rococo",
	feature = "kusama",
	feature = "westend",
	feature = "polkadot"
)))]
compile_error!("at least one runtime feature must be enabled");

/// The native executor instance for Polkadot.
#[cfg(feature = "polkadot")]
pub struct PolkadotExecutorDispatch;

#[cfg(feature = "polkadot")]
impl sc_executor::NativeExecutionDispatch for PolkadotExecutorDispatch {
	type ExtendHostFunctions = frame_benchmarking::benchmarking::HostFunctions;

	fn dispatch(method: &str, data: &[u8]) -> Option<Vec<u8>> {
		polkadot_runtime::api::dispatch(method, data)
	}

	fn native_version() -> sc_executor::NativeVersion {
		polkadot_runtime::native_version()
	}
}

#[cfg(feature = "kusama")]
/// The native executor instance for Kusama.
pub struct KusamaExecutorDispatch;

#[cfg(feature = "kusama")]
impl sc_executor::NativeExecutionDispatch for KusamaExecutorDispatch {
	type ExtendHostFunctions = frame_benchmarking::benchmarking::HostFunctions;

	fn dispatch(method: &str, data: &[u8]) -> Option<Vec<u8>> {
		kusama_runtime::api::dispatch(method, data)
	}

	fn native_version() -> sc_executor::NativeVersion {
		kusama_runtime::native_version()
	}
}

#[cfg(feature = "westend")]
/// The native executor instance for Westend.
pub struct WestendExecutorDispatch;

#[cfg(feature = "westend")]
impl sc_executor::NativeExecutionDispatch for WestendExecutorDispatch {
	type ExtendHostFunctions = frame_benchmarking::benchmarking::HostFunctions;

	fn dispatch(method: &str, data: &[u8]) -> Option<Vec<u8>> {
		westend_runtime::api::dispatch(method, data)
	}

	fn native_version() -> sc_executor::NativeVersion {
		westend_runtime::native_version()
	}
}

#[cfg(feature = "rococo")]
/// The native executor instance for Rococo.
pub struct RococoExecutorDispatch;

#[cfg(feature = "rococo")]
impl sc_executor::NativeExecutionDispatch for RococoExecutorDispatch {
	type ExtendHostFunctions = frame_benchmarking::benchmarking::HostFunctions;

	fn dispatch(method: &str, data: &[u8]) -> Option<Vec<u8>> {
		rococo_runtime::api::dispatch(method, data)
	}

	fn native_version() -> sc_executor::NativeVersion {
		rococo_runtime::native_version()
	}
}

/// A set of APIs that polkadot-like runtimes must implement.
pub trait RuntimeApiCollection:
	sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block>
	+ sp_api::ApiExt<Block>
	+ sp_consensus_babe::BabeApi<Block>
	+ sp_finality_grandpa::GrandpaApi<Block>
	+ ParachainHost<Block>
	+ sp_block_builder::BlockBuilder<Block>
	+ frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce>
	+ sp_mmr_primitives::MmrApi<Block, <Block as BlockT>::Hash, BlockNumber>
	+ pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance>
	+ sp_api::Metadata<Block>
	+ sp_offchain::OffchainWorkerApi<Block>
	+ sp_session::SessionKeys<Block>
	+ sp_authority_discovery::AuthorityDiscoveryApi<Block>
	+ beefy_primitives::BeefyApi<Block>
where
	<Self as sp_api::ApiExt<Block>>::StateBackend: sp_api::StateBackend<BlakeTwo256>,
{
}

impl<Api> RuntimeApiCollection for Api
where
	Api: sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block>
		+ sp_api::ApiExt<Block>
		+ sp_consensus_babe::BabeApi<Block>
		+ sp_finality_grandpa::GrandpaApi<Block>
		+ ParachainHost<Block>
		+ sp_block_builder::BlockBuilder<Block>
		+ frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce>
		+ sp_mmr_primitives::MmrApi<Block, <Block as BlockT>::Hash, BlockNumber>
		+ pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance>
		+ sp_api::Metadata<Block>
		+ sp_offchain::OffchainWorkerApi<Block>
		+ sp_session::SessionKeys<Block>
		+ sp_authority_discovery::AuthorityDiscoveryApi<Block>
		+ beefy_primitives::BeefyApi<Block>,
	<Self as sp_api::ApiExt<Block>>::StateBackend: sp_api::StateBackend<BlakeTwo256>,
{
}

/// Trait that abstracts over all available client implementations.
///
/// For a concrete type there exists [`Client`].
pub trait AbstractClient<Block, Backend>:
	BlockchainEvents<Block>
	+ Sized
	+ Send
	+ Sync
	+ ProvideRuntimeApi<Block>
	+ HeaderBackend<Block>
	+ CallApiAt<Block, StateBackend = Backend::State>
	+ AuxStore
	+ UsageProvider<Block>
	+ HeaderMetadata<Block, Error = sp_blockchain::Error>
where
	Block: BlockT,
	Backend: BackendT<Block>,
	Backend::State: sp_api::StateBackend<BlakeTwo256>,
	Self::Api: RuntimeApiCollection<StateBackend = Backend::State>,
{
}

impl<Block, Backend, Client> AbstractClient<Block, Backend> for Client
where
	Block: BlockT,
	Backend: BackendT<Block>,
	Backend::State: sp_api::StateBackend<BlakeTwo256>,
	Client: BlockchainEvents<Block>
		+ ProvideRuntimeApi<Block>
		+ HeaderBackend<Block>
		+ AuxStore
		+ UsageProvider<Block>
		+ Sized
		+ Send
		+ Sync
		+ CallApiAt<Block, StateBackend = Backend::State>
		+ HeaderMetadata<Block, Error = sp_blockchain::Error>,
	Client::Api: RuntimeApiCollection<StateBackend = Backend::State>,
{
}

/// Execute something with the client instance.
///
/// As there exist multiple chains inside Polkadot, like Polkadot itself, Kusama, Westend etc,
/// there can exist different kinds of client types. As these client types differ in the generics
/// that are being used, we can not easily return them from a function. For returning them from a
/// function there exists [`Client`]. However, the problem on how to use this client instance still
/// exists. This trait "solves" it in a dirty way. It requires a type to implement this trait and
/// than the [`execute_with_client`](ExecuteWithClient::execute_with_client) function can be called
/// with any possible client instance.
///
/// In a perfect world, we could make a closure work in this way.
pub trait ExecuteWithClient {
	/// The return type when calling this instance.
	type Output;

	/// Execute whatever should be executed with the given client instance.
	fn execute_with_client<Client, Api, Backend>(self, client: Arc<Client>) -> Self::Output
	where
		<Api as sp_api::ApiExt<Block>>::StateBackend: sp_api::StateBackend<BlakeTwo256>,
		Backend: sc_client_api::Backend<Block> + 'static,
		Backend::State: sp_api::StateBackend<BlakeTwo256>,
		Api: crate::RuntimeApiCollection<StateBackend = Backend::State>,
		Client: AbstractClient<Block, Backend, Api = Api> + 'static;
}

/// A handle to a Polkadot client instance.
///
/// The Polkadot service supports multiple different runtimes (Westend, Polkadot itself, etc). As each runtime has a
/// specialized client, we need to hide them behind a trait. This is this trait.
///
/// When wanting to work with the inner client, you need to use `execute_with`.
///
/// See [`ExecuteWithClient`](trait.ExecuteWithClient.html) for more information.
pub trait ClientHandle {
	/// Execute the given something with the client.
	fn execute_with<T: ExecuteWithClient>(&self, t: T) -> T::Output;
}

/// Unwraps a [`Client`] into the concrete client type and
/// provides the concrete runtime as `runtime`.
macro_rules! with_client {
	{
		// The client instance that should be unwrapped.
		$self:expr,
		// The name that the unwrapped client will have.
		$client:ident,
		// NOTE: Using an expression here is fine since blocks are also expressions.
		$code:expr
	} => {
		match $self {
			#[cfg(feature = "polkadot")]
			Client::Polkadot($client) => {
				#[allow(unused_imports)]
				use polkadot_runtime as runtime;

				$code
			},
			#[cfg(feature = "westend")]
			Client::Westend($client) => {
				#[allow(unused_imports)]
				use westend_runtime as runtime;

				$code
			},
			#[cfg(feature = "kusama")]
			Client::Kusama($client) => {
				#[allow(unused_imports)]
				use kusama_runtime as runtime;

				$code
			},
			#[cfg(feature = "rococo")]
			Client::Rococo($client) => {
				#[allow(unused_imports)]
				use rococo_runtime as runtime;

				$code
			},
		}
	}
}
// Make the macro available only within this crate.
pub(crate) use with_client;

/// A client instance of Polkadot.
///
/// See [`ExecuteWithClient`] for more information.
#[derive(Clone)]
pub enum Client {
	#[cfg(feature = "polkadot")]
	Polkadot(Arc<FullClient<polkadot_runtime::RuntimeApi, PolkadotExecutorDispatch>>),
	#[cfg(feature = "westend")]
	Westend(Arc<FullClient<westend_runtime::RuntimeApi, WestendExecutorDispatch>>),
	#[cfg(feature = "kusama")]
	Kusama(Arc<FullClient<kusama_runtime::RuntimeApi, KusamaExecutorDispatch>>),
	#[cfg(feature = "rococo")]
	Rococo(Arc<FullClient<rococo_runtime::RuntimeApi, RococoExecutorDispatch>>),
}

impl ClientHandle for Client {
	fn execute_with<T: ExecuteWithClient>(&self, t: T) -> T::Output {
		with_client! {
			self,
			client,
			{
				T::execute_with_client::<_, _, FullBackend>(t, client.clone())
			}
		}
	}
}

impl UsageProvider<Block> for Client {
	fn usage_info(&self) -> sc_client_api::ClientInfo<Block> {
		with_client! {
			self,
			client,
			{
				client.usage_info()
			}
		}
	}
}

impl sc_client_api::BlockBackend<Block> for Client {
	fn block_body(
		&self,
		hash: <Block as BlockT>::Hash,
	) -> sp_blockchain::Result<Option<Vec<<Block as BlockT>::Extrinsic>>> {
		with_client! {
			self,
			client,
			{
				client.block_body(hash)
			}
		}
	}

	fn block(&self, id: &BlockId<Block>) -> sp_blockchain::Result<Option<SignedBlock<Block>>> {
		with_client! {
			self,
			client,
			{
				client.block(id)
			}
		}
	}

	fn block_status(&self, id: &BlockId<Block>) -> sp_blockchain::Result<BlockStatus> {
		with_client! {
			self,
			client,
			{
				client.block_status(id)
			}
		}
	}

	fn justifications(
		&self,
		hash: <Block as BlockT>::Hash,
	) -> sp_blockchain::Result<Option<Justifications>> {
		with_client! {
			self,
			client,
			{
				client.justifications(hash)
			}
		}
	}

	fn block_hash(
		&self,
		number: NumberFor<Block>,
	) -> sp_blockchain::Result<Option<<Block as BlockT>::Hash>> {
		with_client! {
			self,
			client,
			{
				client.block_hash(number)
			}
		}
	}

	fn indexed_transaction(
		&self,
		id: <Block as BlockT>::Hash,
	) -> sp_blockchain::Result<Option<Vec<u8>>> {
		with_client! {
			self,
			client,
			{
				client.indexed_transaction(id)
			}
		}
	}

	fn block_indexed_body(
		&self,
		id: <Block as BlockT>::Hash,
	) -> sp_blockchain::Result<Option<Vec<Vec<u8>>>> {
		with_client! {
			self,
			client,
			{
				client.block_indexed_body(id)
			}
		}
	}

	fn requires_full_sync(&self) -> bool {
		with_client! {
			self,
			client,
			{
				client.requires_full_sync()
			}
		}
	}
}

impl sc_client_api::StorageProvider<Block, crate::FullBackend> for Client {
	fn storage(
		&self,
		hash: <Block as BlockT>::Hash,
		key: &StorageKey,
	) -> sp_blockchain::Result<Option<StorageData>> {
		with_client! {
			self,
			client,
			{
				client.storage(hash, key)
			}
		}
	}

	fn storage_keys(
		&self,
		hash: <Block as BlockT>::Hash,
		key_prefix: &StorageKey,
	) -> sp_blockchain::Result<Vec<StorageKey>> {
		with_client! {
			self,
			client,
			{
				client.storage_keys(hash, key_prefix)
			}
		}
	}

	fn storage_hash(
		&self,
		hash: <Block as BlockT>::Hash,
		key: &StorageKey,
	) -> sp_blockchain::Result<Option<<Block as BlockT>::Hash>> {
		with_client! {
			self,
			client,
			{
				client.storage_hash(hash, key)
			}
		}
	}

	fn storage_pairs(
		&self,
		hash: <Block as BlockT>::Hash,
		key_prefix: &StorageKey,
	) -> sp_blockchain::Result<Vec<(StorageKey, StorageData)>> {
		with_client! {
			self,
			client,
			{
				client.storage_pairs(hash, key_prefix)
			}
		}
	}

	fn storage_keys_iter<'a>(
		&self,
		hash: <Block as BlockT>::Hash,
		prefix: Option<&'a StorageKey>,
		start_key: Option<&StorageKey>,
	) -> sp_blockchain::Result<
		KeyIterator<'a, <crate::FullBackend as sc_client_api::Backend<Block>>::State, Block>,
	> {
		with_client! {
			self,
			client,
			{
				client.storage_keys_iter(hash, prefix, start_key)
			}
		}
	}

	fn child_storage(
		&self,
		hash: <Block as BlockT>::Hash,
		child_info: &ChildInfo,
		key: &StorageKey,
	) -> sp_blockchain::Result<Option<StorageData>> {
		with_client! {
			self,
			client,
			{
				client.child_storage(hash, child_info, key)
			}
		}
	}

	fn child_storage_keys(
		&self,
		hash: <Block as BlockT>::Hash,
		child_info: &ChildInfo,
		key_prefix: &StorageKey,
	) -> sp_blockchain::Result<Vec<StorageKey>> {
		with_client! {
			self,
			client,
			{
				client.child_storage_keys(hash, child_info, key_prefix)
			}
		}
	}

	fn child_storage_keys_iter<'a>(
		&self,
		hash: <Block as BlockT>::Hash,
		child_info: ChildInfo,
		prefix: Option<&'a StorageKey>,
		start_key: Option<&StorageKey>,
	) -> sp_blockchain::Result<
		KeyIterator<'a, <crate::FullBackend as sc_client_api::Backend<Block>>::State, Block>,
	> {
		with_client! {
			self,
			client,
			{
				client.child_storage_keys_iter(hash, child_info, prefix, start_key)
			}
		}
	}

	fn child_storage_hash(
		&self,
		hash: <Block as BlockT>::Hash,
		child_info: &ChildInfo,
		key: &StorageKey,
	) -> sp_blockchain::Result<Option<<Block as BlockT>::Hash>> {
		with_client! {
			self,
			client,
			{
				client.child_storage_hash(hash, child_info, key)
			}
		}
	}
}

impl sp_blockchain::HeaderBackend<Block> for Client {
	fn header(&self, id: BlockId<Block>) -> sp_blockchain::Result<Option<Header>> {
		with_client! {
			self,
			client,
			{
				client.header(&id)
			}
		}
	}

	fn info(&self) -> sp_blockchain::Info<Block> {
		with_client! {
			self,
			client,
			{
				client.info()
			}
		}
	}

	fn status(&self, id: BlockId<Block>) -> sp_blockchain::Result<sp_blockchain::BlockStatus> {
		with_client! {
			self,
			client,
			{
				client.status(id)
			}
		}
	}

	fn number(&self, hash: Hash) -> sp_blockchain::Result<Option<BlockNumber>> {
		with_client! {
			self,
			client,
			{
				client.number(hash)
			}
		}
	}

	fn hash(&self, number: BlockNumber) -> sp_blockchain::Result<Option<Hash>> {
		with_client! {
			self,
			client,
			{
				client.hash(number)
			}
		}
	}
}
