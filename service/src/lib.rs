// Copyright 2017 Parity Technologies (UK) Ltd.
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

#![warn(unused_extern_crates)]

//! Polkadot service. Specialized wrapper over substrate service.

extern crate ed25519;
extern crate polkadot_availability_store as av_store;
extern crate polkadot_primitives;
extern crate polkadot_runtime;
extern crate polkadot_executor;
extern crate polkadot_api;
extern crate polkadot_consensus as consensus;
extern crate polkadot_transaction_pool as transaction_pool;
extern crate polkadot_network;
extern crate substrate_primitives as primitives;
extern crate substrate_network as network;
extern crate substrate_client as client;
extern crate substrate_service as service;
extern crate tokio;

#[macro_use]
extern crate log;
#[macro_use]
extern crate hex_literal;

pub mod chain_spec;

use std::sync::Arc;
use tokio::prelude::{Stream, Future};
use transaction_pool::TransactionPool;
use polkadot_api::{PolkadotApi, light::RemotePolkadotApiWrapper};
use polkadot_primitives::{parachain, AccountId, Block, BlockId, Hash};
use polkadot_runtime::GenesisConfig;
use client::{Client, BlockchainEvents};
use polkadot_network::{PolkadotProtocol, consensus::ConsensusNetwork};
use tokio::runtime::TaskExecutor;
use service::FactoryFullConfiguration;
use primitives::{KeccakHasher, RlpCodec};

pub use service::{Roles, PruningMode, ExtrinsicPoolOptions,
	ErrorKind, Error, ComponentBlock, LightComponents, FullComponents};
pub use client::ExecutionStrategy;

/// Specialised polkadot `ChainSpec`.
pub type ChainSpec = service::ChainSpec<GenesisConfig>;
/// Polkadot client type for specialised `Components`.
pub type ComponentClient<C> = Client<<C as Components>::Backend, <C as Components>::Executor, Block>;
pub type NetworkService = network::Service<Block, <Factory as service::ServiceFactory>::NetworkProtocol, Hash>;

/// A collection of type to generalise Polkadot specific components over full / light client.
pub trait Components: service::Components {
	/// Polkadot API.
	type Api: 'static + PolkadotApi + Send + Sync;
	/// Client backend.
	type Backend: 'static + client::backend::Backend<Block, KeccakHasher, RlpCodec>;
	/// Client executor.
	type Executor: 'static + client::CallExecutor<Block, KeccakHasher, RlpCodec> + Send + Sync;
}

impl Components for service::LightComponents<Factory> {
	type Api = RemotePolkadotApiWrapper<
		<service::LightComponents<Factory> as service::Components>::Backend,
		<service::LightComponents<Factory> as service::Components>::Executor,
	>;
	type Executor = service::LightExecutor<Factory>;
	type Backend = service::LightBackend<Factory>;
}

impl Components for service::FullComponents<Factory> {
	type Api = service::FullClient<Factory>;
	type Executor = service::FullExecutor<Factory>;
	type Backend = service::FullBackend<Factory>;
}

/// All configuration for the polkadot node.
pub type Configuration = FactoryFullConfiguration<Factory>;

/// Polkadot-specific configuration.
#[derive(Default)]
pub struct CustomConfiguration {
	/// Set to `Some` with a collator `AccountId` and desired parachain
	/// if the network protocol should be started in collator mode.
	pub collating_for: Option<(AccountId, parachain::Id)>,
}

/// Polkadot config for the substrate service.
pub struct Factory;

impl service::ServiceFactory for Factory {
	type Block = Block;
	type ExtrinsicHash = Hash;
	type NetworkProtocol = PolkadotProtocol;
	type RuntimeDispatch = polkadot_executor::Executor;
	type FullExtrinsicPoolApi = transaction_pool::ChainApi<service::FullClient<Self>>;
	type LightExtrinsicPoolApi = transaction_pool::ChainApi<
		RemotePolkadotApiWrapper<service::LightBackend<Self>, service::LightExecutor<Self>>
	>;
	type Genesis = GenesisConfig;
	type Configuration = CustomConfiguration;

	const NETWORK_PROTOCOL_ID: network::ProtocolId = ::polkadot_network::DOT_PROTOCOL_ID;

	fn build_full_extrinsic_pool(config: ExtrinsicPoolOptions, client: Arc<service::FullClient<Self>>)
		-> Result<TransactionPool<service::FullClient<Self>>, Error>
	{
		let api = client.clone();
		Ok(TransactionPool::new(config, transaction_pool::ChainApi::new(api)))
	}

	fn build_light_extrinsic_pool(config: ExtrinsicPoolOptions, client: Arc<service::LightClient<Self>>)
		-> Result<TransactionPool<RemotePolkadotApiWrapper<service::LightBackend<Self>, service::LightExecutor<Self>>>, Error>
	{
		let api = Arc::new(RemotePolkadotApiWrapper(client.clone()));
		Ok(TransactionPool::new(config, transaction_pool::ChainApi::new(api)))
	}

	fn build_network_protocol(config: &Configuration)
		-> Result<PolkadotProtocol, Error>
	{
		if let Some((_, ref para_id)) = config.custom.collating_for {
			info!("Starting network in Collator mode for parachain {:?}", para_id);
		}
		Ok(PolkadotProtocol::new(config.custom.collating_for))
	}
}

/// Polkadot service.
pub struct Service<C: Components> {
	inner: service::Service<C>,
	client: Arc<ComponentClient<C>>,
	network: Arc<NetworkService>,
	api: Arc<<C as Components>::Api>,
	_consensus: Option<consensus::Service>,
}

impl <C: Components> Service<C> {
	pub fn client(&self) -> Arc<ComponentClient<C>> {
		self.client.clone()
	}

	pub fn network(&self) -> Arc<NetworkService> {
		self.network.clone()
	}

	pub fn api(&self) -> Arc<<C as Components>::Api> {
		self.api.clone()
	}
}

/// Creates light client and register protocol with the network service
pub fn new_light(config: Configuration, executor: TaskExecutor)
	-> Result<Service<LightComponents<Factory>>, Error>
{
	let service = service::Service::<LightComponents<Factory>>::new(config, executor.clone())?;
	let api = Arc::new(RemotePolkadotApiWrapper(service.client()));
	let pool  = service.extrinsic_pool();
	let events = service.client().import_notification_stream()
		.for_each(move |notification| {
			// re-verify all transactions without the sender.
			pool.retry_verification(&BlockId::hash(notification.hash), None)
				.map_err(|e| warn!("Error re-verifying transactions: {:?}", e))?;
			Ok(())
		})
		.then(|_| Ok(()));
	executor.spawn(events);
	Ok(Service {
		client: service.client(),
		network: service.network(),
		api: api,
		inner: service,
		_consensus: None,
	})
}

/// Creates full client and register protocol with the network service
pub fn new_full(config: Configuration, executor: TaskExecutor)
	-> Result<Service<FullComponents<Factory>>, Error>
{
	// open availability store.
	let av_store = {
		use std::path::PathBuf;

		let mut path = PathBuf::from(config.database_path.clone());
		path.push("availability");

		::av_store::Store::new(::av_store::Config {
			cache_size: None,
			path,
		})?
	};

	let is_validator = (config.roles & Roles::AUTHORITY) == Roles::AUTHORITY;
	let service = service::Service::<FullComponents<Factory>>::new(config, executor.clone())?;
	let pool  = service.extrinsic_pool();
	let events = service.client().import_notification_stream()
		.for_each(move |notification| {
			// re-verify all transactions without the sender.
			pool.retry_verification(&BlockId::hash(notification.hash), None)
				.map_err(|e| warn!("Error re-verifying transactions: {:?}", e))?;
			Ok(())
		})
		.then(|_| Ok(()));
	executor.spawn(events);
	// Spin consensus service if configured
	let consensus = if is_validator {
		// Load the first available key
		let key = service.keystore().load(&service.keystore().contents()?[0], "")?;
		info!("Using authority key {}", key.public());

		let client = service.client();

		let consensus_net = ConsensusNetwork::new(service.network(), client.clone());
		Some(consensus::Service::new(
			client.clone(),
			client.clone(),
			consensus_net,
			service.extrinsic_pool(),
			executor,
			::std::time::Duration::from_secs(4), // TODO: dynamic
			key,
			av_store.clone(),
		))
	} else {
		None
	};

	service.network().with_spec(|spec, _| spec.register_availability_store(av_store));

	Ok(Service {
		client: service.client(),
		network: service.network(),
		api: service.client(),
		inner: service,
		_consensus: consensus,
	})
}

/// Creates bare client without any networking.
pub fn new_client(config: Configuration)
-> Result<Arc<service::ComponentClient<FullComponents<Factory>>>, Error>
{
	service::new_client::<Factory>(config)
}

impl<C: Components> ::std::ops::Deref for Service<C> {
	type Target = service::Service<C>;

	fn deref(&self) -> &Self::Target {
		&self.inner
	}
}
