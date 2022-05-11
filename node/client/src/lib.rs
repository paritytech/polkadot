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
use sp_core::Pair;
use sp_keyring::Sr25519Keyring;
use sp_runtime::{
	generic::{BlockId, SignedBlock},
	traits::{BlakeTwo256, Block as BlockT},
	Justifications, OpaqueExtrinsic,
};
use sp_storage::{ChildInfo, StorageData, StorageKey};
use std::sync::Arc;

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
	+ sp_mmr_primitives::MmrApi<Block, <Block as BlockT>::Hash>
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
		+ sp_mmr_primitives::MmrApi<Block, <Block as BlockT>::Hash>
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

macro_rules! with_client {
	{
		$self:ident,
		$client:ident,
		{
			$( $code:tt )*
		}
	} => {
		match $self {
			#[cfg(feature = "polkadot")]
			Self::Polkadot($client) => { $( $code )* },
			#[cfg(feature = "westend")]
			Self::Westend($client) => { $( $code )* },
			#[cfg(feature = "kusama")]
			Self::Kusama($client) => { $( $code )* },
			#[cfg(feature = "rococo")]
			Self::Rococo($client) => { $( $code )* },
		}
	}
}

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
		id: &BlockId<Block>,
	) -> sp_blockchain::Result<Option<Vec<<Block as BlockT>::Extrinsic>>> {
		with_client! {
			self,
			client,
			{
				client.block_body(id)
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

	fn justifications(&self, id: &BlockId<Block>) -> sp_blockchain::Result<Option<Justifications>> {
		with_client! {
			self,
			client,
			{
				client.justifications(id)
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
		id: &<Block as BlockT>::Hash,
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
		id: &BlockId<Block>,
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
		id: &BlockId<Block>,
		key: &StorageKey,
	) -> sp_blockchain::Result<Option<StorageData>> {
		with_client! {
			self,
			client,
			{
				client.storage(id, key)
			}
		}
	}

	fn storage_keys(
		&self,
		id: &BlockId<Block>,
		key_prefix: &StorageKey,
	) -> sp_blockchain::Result<Vec<StorageKey>> {
		with_client! {
			self,
			client,
			{
				client.storage_keys(id, key_prefix)
			}
		}
	}

	fn storage_hash(
		&self,
		id: &BlockId<Block>,
		key: &StorageKey,
	) -> sp_blockchain::Result<Option<<Block as BlockT>::Hash>> {
		with_client! {
			self,
			client,
			{
				client.storage_hash(id, key)
			}
		}
	}

	fn storage_pairs(
		&self,
		id: &BlockId<Block>,
		key_prefix: &StorageKey,
	) -> sp_blockchain::Result<Vec<(StorageKey, StorageData)>> {
		with_client! {
			self,
			client,
			{
				client.storage_pairs(id, key_prefix)
			}
		}
	}

	fn storage_keys_iter<'a>(
		&self,
		id: &BlockId<Block>,
		prefix: Option<&'a StorageKey>,
		start_key: Option<&StorageKey>,
	) -> sp_blockchain::Result<
		KeyIterator<'a, <crate::FullBackend as sc_client_api::Backend<Block>>::State, Block>,
	> {
		with_client! {
			self,
			client,
			{
				client.storage_keys_iter(id, prefix, start_key)
			}
		}
	}

	fn child_storage(
		&self,
		id: &BlockId<Block>,
		child_info: &ChildInfo,
		key: &StorageKey,
	) -> sp_blockchain::Result<Option<StorageData>> {
		with_client! {
			self,
			client,
			{
				client.child_storage(id, child_info, key)
			}
		}
	}

	fn child_storage_keys(
		&self,
		id: &BlockId<Block>,
		child_info: &ChildInfo,
		key_prefix: &StorageKey,
	) -> sp_blockchain::Result<Vec<StorageKey>> {
		with_client! {
			self,
			client,
			{
				client.child_storage_keys(id, child_info, key_prefix)
			}
		}
	}

	fn child_storage_keys_iter<'a>(
		&self,
		id: &BlockId<Block>,
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
				client.child_storage_keys_iter(id, child_info, prefix, start_key)
			}
		}
	}

	fn child_storage_hash(
		&self,
		id: &BlockId<Block>,
		child_info: &ChildInfo,
		key: &StorageKey,
	) -> sp_blockchain::Result<Option<<Block as BlockT>::Hash>> {
		with_client! {
			self,
			client,
			{
				client.child_storage_hash(id, child_info, key)
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

/// Provides a `SignedPayload` for any runtime.
///
/// Should only be used for benchmarking as it is not tested for regular usage.
///
/// The first code block should set up all variables that are needed to create the
/// `SignedPayload`. The second block can make use of the `SignedPayload`.
///
/// This is not done as a trait function since the return type depends on the runtime.
/// This macro therefore uses the same approach as [`with_client!`].
macro_rules! with_signed_payload {
	{
		$self:ident,
		{
			$extra:ident,
			$client:ident,
			$raw_payload:ident
		},
		{
			$( $setup:tt )*
		},
		(
			$period:expr,
			$current_block:expr,
			$nonce:expr,
			$tip:expr,
			$call:expr,
			$genesis:expr
		),
		{
			$( $usage:tt )*
		}
	} => {
		match $self {
			#[cfg(feature = "polkadot")]
			Self::Polkadot($client) => {
				use polkadot_runtime as runtime;

				$( $setup )*

				let $extra: runtime::SignedExtra = (
					frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
					frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
					frame_system::CheckTxVersion::<runtime::Runtime>::new(),
					frame_system::CheckGenesis::<runtime::Runtime>::new(),
					frame_system::CheckMortality::<runtime::Runtime>::from(sp_runtime::generic::Era::mortal(
						$period,
						$current_block,
					)),
					frame_system::CheckNonce::<runtime::Runtime>::from($nonce),
					frame_system::CheckWeight::<runtime::Runtime>::new(),
					pallet_transaction_payment::ChargeTransactionPayment::<runtime::Runtime>::from($tip),
					polkadot_runtime_common::claims::PrevalidateAttests::<runtime::Runtime>::new(),
				);

				let $raw_payload = runtime::SignedPayload::from_raw(
					$call.clone(),
					$extra.clone(),
					(
						(),
						runtime::VERSION.spec_version,
						runtime::VERSION.transaction_version,
						$genesis.clone(),
						$genesis,
						(),
						(),
						(),
						(),
					),
				);

				$( $usage )*
			},
			#[cfg(feature = "westend")]
			Self::Westend($client) => {
				use westend_runtime as runtime;

				$( $setup )*

				signed_payload!($extra, $raw_payload,
					($period, $current_block, $nonce, $tip, $call, $genesis));

				$( $usage )*
			},
			#[cfg(feature = "kusama")]
			Self::Kusama($client) => {
				use kusama_runtime as runtime;

				$( $setup )*

				signed_payload!($extra, $raw_payload,
					($period, $current_block, $nonce, $tip, $call, $genesis));

				$( $usage )*
			},
			#[cfg(feature = "rococo")]
			Self::Rococo($client) => {
				use rococo_runtime as runtime;

				$( $setup )*

				signed_payload!($extra, $raw_payload,
					($period, $current_block, $nonce, $tip, $call, $genesis));

				$( $usage )*
			},
		}
	}
}

/// Generates a `SignedPayload` for the Kusama, Westend and Rococo runtime.
///
/// Should only be used for benchmarking as it is not tested for regular usage.
#[allow(unused_macros)]
macro_rules! signed_payload {
	(
	$extra:ident, $raw_payload:ident,
	(
		$period:expr,
		$current_block:expr,
		$nonce:expr,
		$tip:expr,
		$call:expr,
		$genesis:expr
	)
	) => {
		let $extra: runtime::SignedExtra = (
			frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
			frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
			frame_system::CheckTxVersion::<runtime::Runtime>::new(),
			frame_system::CheckGenesis::<runtime::Runtime>::new(),
			frame_system::CheckMortality::<runtime::Runtime>::from(
				sp_runtime::generic::Era::mortal($period, $current_block),
			),
			frame_system::CheckNonce::<runtime::Runtime>::from($nonce),
			frame_system::CheckWeight::<runtime::Runtime>::new(),
			pallet_transaction_payment::ChargeTransactionPayment::<runtime::Runtime>::from($tip),
		);

		let $raw_payload = runtime::SignedPayload::from_raw(
			$call.clone(),
			$extra.clone(),
			(
				(),
				runtime::VERSION.spec_version,
				runtime::VERSION.transaction_version,
				$genesis.clone(),
				$genesis,
				(),
				(),
				(),
			),
		);
	};
}

impl frame_benchmarking_cli::ExtrinsicBuilder for Client {
	fn remark(&self, nonce: u32) -> std::result::Result<OpaqueExtrinsic, &'static str> {
		with_signed_payload! {
			self,
			{extra, client, raw_payload},
			{
				// First the setup code to init all the variables that are needed
				// to build the signed extras.
				use runtime::{Call, SystemCall};

				let call = Call::System(SystemCall::remark { remark: vec![] });
				let bob = Sr25519Keyring::Bob.pair();

				let period = polkadot_runtime_common::BlockHashCount::get().checked_next_power_of_two().map(|c| c / 2).unwrap_or(2) as u64;

				let current_block = 0;
				let tip = 0;
				let genesis = client.usage_info().chain.best_hash;
			},
			(period, current_block, nonce, tip, call, genesis),
			/* The SignedPayload is generated here */
			{
				// Use the payload to generate a signature.
				let signature = raw_payload.using_encoded(|payload| bob.sign(payload));

				let ext = runtime::UncheckedExtrinsic::new_signed(
					call,
					sp_runtime::AccountId32::from(bob.public()).into(),
					polkadot_core_primitives::Signature::Sr25519(signature.clone()),
					extra,
				);
				Ok(ext.into())
			}
		}
	}
}

/// Generates inherent data for benchmarking Polkadot, Kusama, Westend and Rococo.
///
/// Not to be used outside of benchmarking since it returns mocked values.
pub fn benchmark_inherent_data(
	header: polkadot_core_primitives::Header,
) -> std::result::Result<sp_inherents::InherentData, sp_inherents::Error> {
	use sp_inherents::InherentDataProvider;
	let mut inherent_data = sp_inherents::InherentData::new();

	// Assume that all runtimes have the `timestamp` pallet.
	let d = std::time::Duration::from_millis(0);
	let timestamp = sp_timestamp::InherentDataProvider::new(d.into());
	timestamp.provide_inherent_data(&mut inherent_data)?;

	let para_data = polkadot_primitives::v2::InherentData {
		bitfields: Vec::new(),
		backed_candidates: Vec::new(),
		disputes: Vec::new(),
		parent_header: header,
	};

	polkadot_node_core_parachains_inherent::ParachainsInherentDataProvider::from_data(para_data)
		.provide_inherent_data(&mut inherent_data)?;

	Ok(inherent_data)
}
