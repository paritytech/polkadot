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

//! Polkadot Client abstractions.

use std::sync::Arc;
use sp_api::{ProvideRuntimeApi, CallApiAt};
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::{Block as BlockT, BlakeTwo256};
use sc_client_api::{Backend as BackendT, BlockchainEvents};
use polkadot_primitives::v0::{Block, ParachainHost, AccountId, Nonce, Balance};

/// A set of APIs that polkadot-like runtimes must implement.
pub trait RuntimeApiCollection:
	sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block>
	+ sp_api::ApiExt<Block, Error = sp_blockchain::Error>
	+ babe_primitives::BabeApi<Block>
	+ grandpa_primitives::GrandpaApi<Block>
	+ ParachainHost<Block>
	+ sp_block_builder::BlockBuilder<Block>
	+ system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce>
	+ pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance>
	+ sp_api::Metadata<Block>
	+ sp_offchain::OffchainWorkerApi<Block>
	+ sp_session::SessionKeys<Block>
	+ authority_discovery_primitives::AuthorityDiscoveryApi<Block>
where
	<Self as sp_api::ApiExt<Block>>::StateBackend: sp_api::StateBackend<BlakeTwo256>,
{}

impl<Api> RuntimeApiCollection for Api
where
	Api:
	sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block>
	+ sp_api::ApiExt<Block, Error = sp_blockchain::Error>
	+ babe_primitives::BabeApi<Block>
	+ grandpa_primitives::GrandpaApi<Block>
	+ ParachainHost<Block>
	+ sp_block_builder::BlockBuilder<Block>
	+ system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce>
	+ pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance>
	+ sp_api::Metadata<Block>
	+ sp_offchain::OffchainWorkerApi<Block>
	+ sp_session::SessionKeys<Block>
	+ authority_discovery_primitives::AuthorityDiscoveryApi<Block>,
	<Self as sp_api::ApiExt<Block>>::StateBackend: sp_api::StateBackend<BlakeTwo256>,
{}

/// Trait that abstracts over all available client implementations.
///
/// For a concrete type there exists [`Client`].
pub trait AbstractClient<Block, Backend>:
	BlockchainEvents<Block> + Sized + Send + Sync
	+ ProvideRuntimeApi<Block>
	+ HeaderBackend<Block>
	+ CallApiAt<
		Block,
		Error = sp_blockchain::Error,
		StateBackend = Backend::State
	>
	where
		Block: BlockT,
		Backend: BackendT<Block>,
		Backend::State: sp_api::StateBackend<BlakeTwo256>,
		Self::Api: RuntimeApiCollection<StateBackend = Backend::State>,
{}

impl<Block, Backend, Client> AbstractClient<Block, Backend> for Client
	where
		Block: BlockT,
		Backend: BackendT<Block>,
		Backend::State: sp_api::StateBackend<BlakeTwo256>,
		Client: BlockchainEvents<Block> + ProvideRuntimeApi<Block> + HeaderBackend<Block>
			+ Sized + Send + Sync
			+ CallApiAt<
				Block,
				Error = sp_blockchain::Error,
				StateBackend = Backend::State
			>,
		Client::Api: RuntimeApiCollection<StateBackend = Backend::State>,
{}

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
			Backend: sc_client_api::Backend<Block>,
			Backend::State: sp_api::StateBackend<BlakeTwo256>,
			Api: crate::RuntimeApiCollection<StateBackend = Backend::State>,
			Client: AbstractClient<Block, Backend, Api = Api> + 'static;
}

/// A client instance of Polkadot.
///
/// See [`ExecuteWithClient`] for more information.
#[derive(Clone)]
pub enum Client {
	Polkadot(Arc<crate::FullClient<polkadot_runtime::RuntimeApi, crate::PolkadotExecutor>>),
	Westend(Arc<crate::FullClient<westend_runtime::RuntimeApi, crate::WestendExecutor>>),
	Kusama(Arc<crate::FullClient<kusama_runtime::RuntimeApi, crate::KusamaExecutor>>),
	Rococo(Arc<crate::FullClient<rococo_runtime::RuntimeApi, crate::RococoExecutor>>),
}

impl Client {
	/// Execute the given something with the client.
	pub fn execute_with<T: ExecuteWithClient>(&self, t: T) -> T::Output {
		match self {
			Self::Polkadot(client) => {
				T::execute_with_client::<_, _, crate::FullBackend>(t, client.clone())
			},
			Self::Westend(client) => {
				T::execute_with_client::<_, _, crate::FullBackend>(t, client.clone())
			},
			Self::Kusama(client) => {
				T::execute_with_client::<_, _, crate::FullBackend>(t, client.clone())
			},
			Self::Rococo(client) => {
				T::execute_with_client::<_, _, crate::FullBackend>(t, client.clone())
			}
		}
	}
}
