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

//! Polkadot service. Specialized wrapper over substrate service.

pub mod chain_spec;

use client::LongestChain;
use consensus_common::SelectChain;
use std::sync::Arc;
use std::time::Duration;
use polkadot_primitives::{parachain, Block, Hash, BlockId};
use polkadot_runtime::{GenesisConfig, RuntimeApi};
use polkadot_network::gossip::{self as network_gossip, Known};
use primitives::{ed25519, Pair};
use tokio::runtime::TaskExecutor;
use service::{FactoryFullConfiguration, FullBackend, LightBackend, FullExecutor, LightExecutor};
use transaction_pool::txpool::{Pool as TransactionPool};
use aura::{import_queue, start_aura, AuraImportQueue, SlotDuration};
use inherents::InherentDataProviders;
use log::info;
pub use service::{
	Roles, PruningMode, TransactionPoolOptions, ComponentClient,
	Error, ComponentBlock, LightComponents, FullComponents,
	FullClient, LightClient, Components, Service, ServiceFactory
};
pub use service::config::full_version_from_strs;
pub use client::{backend::Backend, runtime_api::Core as CoreApi, ExecutionStrategy};
pub use polkadot_network::{PolkadotProtocol, NetworkService};
pub use polkadot_primitives::parachain::{CollatorId, ParachainHost};
pub use primitives::{Blake2Hasher};
pub use sr_primitives::traits::ProvideRuntimeApi;
pub use chain_spec::ChainSpec;

/// All configuration for the polkadot node.
pub type Configuration = FactoryFullConfiguration<Factory>;

/// Polkadot-specific configuration.
pub struct CustomConfiguration {
	/// Set to `Some` with a collator `CollatorId` and desired parachain
	/// if the network protocol should be started in collator mode.
	pub collating_for: Option<(CollatorId, parachain::Id)>,

	/// Intermediate state during setup. Will be removed in future. Set to `None`.
	// FIXME: rather than putting this on the config, let's have an actual intermediate setup state
	// https://github.com/paritytech/substrate/issues/1134
	pub grandpa_import_setup: Option<(
		Arc<grandpa::BlockImportForService<Factory>>,
		grandpa::LinkHalfForService<Factory>
	)>,

	/// Maximal `block_data` size.
	pub max_block_data_size: Option<u64>,

	inherent_data_providers: InherentDataProviders,
}

impl Default for CustomConfiguration {
	fn default() -> Self {
		Self {
			collating_for: None,
			grandpa_import_setup: None,
			inherent_data_providers: InherentDataProviders::new(),
			max_block_data_size: None,
		}
	}
}

/// Chain API type for the transaction pool.
pub type TxChainApi<Backend, Executor> = transaction_pool::ChainApi<
	client::Client<Backend, Executor, Block, RuntimeApi>,
	Block,
>;

/// Provides polkadot types.
pub trait PolkadotService {
	/// The client's backend type.
	type Backend: 'static + client::backend::Backend<Block, Blake2Hasher>;
	/// The client's call executor type.
	type Executor: 'static + client::CallExecutor<Block, Blake2Hasher> + Send + Sync + Clone;

	/// Get a handle to the client.
	fn client(&self) -> Arc<client::Client<Self::Backend, Self::Executor, Block, RuntimeApi>>;

	fn select_chain(&self) -> Option<client::LongestChain<Self::Backend, Block>>;

	/// Get a handle to the network.
	fn network(&self) -> Arc<NetworkService>;

	/// Get a handle to the transaction pool.
	fn transaction_pool(&self) -> Arc<TransactionPool<TxChainApi<Self::Backend, Self::Executor>>>;
}

impl PolkadotService for Service<FullComponents<Factory>> {
	type Backend = <FullComponents<Factory> as Components>::Backend;
	type Executor = <FullComponents<Factory> as Components>::Executor;

	fn client(&self) -> Arc<client::Client<Self::Backend, Self::Executor, Block, RuntimeApi>> {
		Service::client(self)
	}

	fn select_chain(&self) -> Option<client::LongestChain<Self::Backend, Block>> {
		Service::select_chain(self)
	}

	fn network(&self) -> Arc<NetworkService> {
		Service::network(self)
	}

	fn transaction_pool(&self) -> Arc<TransactionPool<TxChainApi<Self::Backend, Self::Executor>>> {
		Service::transaction_pool(self)
	}
}

impl PolkadotService for Service<LightComponents<Factory>> {
	type Backend = <LightComponents<Factory> as Components>::Backend;
	type Executor = <LightComponents<Factory> as Components>::Executor;

	fn client(&self) -> Arc<client::Client<Self::Backend, Self::Executor, Block, RuntimeApi>> {
		Service::client(self)
	}

	fn select_chain(&self) -> Option<client::LongestChain<Self::Backend, Block>> {
		None
	}

	fn network(&self) -> Arc<NetworkService> {
		Service::network(self)
	}

	fn transaction_pool(&self) -> Arc<TransactionPool<TxChainApi<Self::Backend, Self::Executor>>> {
		Service::transaction_pool(self)
	}
}

service::construct_service_factory! {
	struct Factory {
		Block = Block,
		RuntimeApi = RuntimeApi,
		NetworkProtocol = PolkadotProtocol {
			|config: &Configuration| Ok(PolkadotProtocol::new(config.custom.collating_for.clone()))
		},
		RuntimeDispatch = polkadot_executor::Executor,
		FullTransactionPoolApi = TxChainApi<FullBackend<Self>, FullExecutor<Self>>
			{ |config, client| Ok(TransactionPool::new(config, TxChainApi::new(client))) },
		LightTransactionPoolApi = TxChainApi<LightBackend<Self>, LightExecutor<Self>>
			{ |config, client| Ok(TransactionPool::new(config, TxChainApi::new(client))) },
		Genesis = GenesisConfig,
		Configuration = CustomConfiguration,
		FullService = FullComponents<Self>
			{ |config: FactoryFullConfiguration<Self>, executor: TaskExecutor| {
				FullComponents::<Factory>::new(config, executor)
			} },
		AuthoritySetup = { |mut service: Self::FullService, executor: TaskExecutor, key: Option<Arc<ed25519::Pair>>| {
				use polkadot_network::validation::ValidationNetwork;

				let (block_import, link_half) = service.config.custom.grandpa_import_setup.take()
					.expect("Link Half and Block Import are present for Full Services or setup failed before. qed");

				// always run GRANDPA in order to sync.
				{
					let local_key = if service.config.disable_grandpa {
						None
					} else {
						key.clone()
					};

					let config = grandpa::Config {
						local_key,
						// FIXME #1578 make this available through chainspec
						gossip_duration: Duration::from_millis(333),
						justification_period: 4096,
						name: Some(service.config.name.clone())
					};

					match config.local_key {
						None => {
							executor.spawn(grandpa::run_grandpa_observer(
								config,
								link_half,
								service.network(),
								service.on_exit(),
							)?);
						},
						Some(_) => {
							use service::TelemetryOnConnect;

							let telemetry_on_connect = TelemetryOnConnect {
								on_exit: Box::new(service.on_exit()),
								telemetry_connection_sinks: service.telemetry_on_connect_stream(),
								executor: &executor,
							};

							let grandpa_config = grandpa::GrandpaParams {
								config: config,
								link: link_half,
								network: service.network(),
								inherent_data_providers: service.config.custom.inherent_data_providers.clone(),
								on_exit: service.on_exit(),
								telemetry_on_connect: Some(telemetry_on_connect),
							};
							executor.spawn(grandpa::run_grandpa_voter(grandpa_config)?);
						},
					}
				}

				let extrinsic_store = {
					use std::path::PathBuf;

					let mut path = PathBuf::from(service.config.database_path.clone());
					path.push("availability");

					::av_store::Store::new(::av_store::Config {
						cache_size: None,
						path,
					})?
				};

				// run authorship only if authority.
				let key = match key {
					Some(key) => key,
					None => return Ok(service),
				};

				if service.config.custom.collating_for.is_some() {
					info!("The node cannot start as an authority because it is also configured\
						to run as a collator.");

					return Ok(service);
				}

				let client = service.client();
				let known_oracle = client.clone();
				let select_chain = if let Some(select_chain) = service.select_chain() {
					select_chain
				} else {
					info!("The node cannot start as an authority because it can't select chain.");
					return Ok(service);
				};

				let gossip_validator_select_chain = select_chain.clone();
				let gossip_validator = network_gossip::register_validator(
					service.network(),
					move |block_hash: &Hash| {
						use client::BlockStatus;

						match known_oracle.block_status(&BlockId::hash(*block_hash)) {
							Err(_) | Ok(BlockStatus::Unknown) | Ok(BlockStatus::Queued) => None,
							Ok(BlockStatus::KnownBad) => Some(Known::Bad),
							Ok(BlockStatus::InChainWithState) | Ok(BlockStatus::InChainPruned) => {
								match gossip_validator_select_chain.leaves() {
									Err(_) => None,
									Ok(leaves) => if leaves.contains(block_hash) {
										Some(Known::Leaf)
									} else {
										Some(Known::Old)
									},
								}
							}
						}
					},
				);

				// collator connections and validation network both fulfilled by this
				let validation_network = ValidationNetwork::new(
					service.network(),
					service.on_exit(),
					gossip_validator,
					service.client(),
					executor.clone(),
				);
				let proposer_factory = ::consensus::ProposerFactory::new(
					client.clone(),
					select_chain.clone(),
					validation_network.clone(),
					validation_network,
					service.transaction_pool(),
					executor.clone(),
					key.clone(),
					extrinsic_store,
					SlotDuration::get_or_compute(&*client)?,
					service.config.custom.max_block_data_size,
				);

				info!("Using authority key {}", key.public());
				let task = start_aura(
					SlotDuration::get_or_compute(&*client)?,
					key,
					client.clone(),
					select_chain,
					block_import,
					Arc::new(proposer_factory),
					service.network(),
					service.config.custom.inherent_data_providers.clone(),
					service.config.force_authoring,
				)?;

				executor.spawn(task);
				Ok(service)
			}},
		LightService = LightComponents<Self>
			{ |config, executor| <LightComponents<Factory>>::new(config, executor) },
		FullImportQueue = AuraImportQueue<
			Self::Block,
		>
			{ |config: &mut FactoryFullConfiguration<Self>, client: Arc<FullClient<Self>>, select_chain: Self::SelectChain| {
				let slot_duration = SlotDuration::get_or_compute(&*client)?;

				let (block_import, link_half) =
					grandpa::block_import::<_, _, _, RuntimeApi, FullClient<Self>, _>(
						client.clone(), client.clone(), select_chain
					)?;
				let block_import = Arc::new(block_import);
				let justification_import = block_import.clone();

				config.custom.grandpa_import_setup = Some((block_import.clone(), link_half));
				import_queue::<_, _, ed25519::Pair>(
					slot_duration,
					block_import,
					Some(justification_import),
					None,
					None,
					client,
					config.custom.inherent_data_providers.clone(),
				).map_err(Into::into)
			}},
		LightImportQueue = AuraImportQueue<
			Self::Block,
		>
			{ |config: &mut FactoryFullConfiguration<Self>, client: Arc<LightClient<Self>>| {
				#[allow(deprecated)]
				let fetch_checker = client.backend().blockchain().fetcher()
					.upgrade()
					.map(|fetcher| fetcher.checker().clone())
					.ok_or_else(|| "Trying to start light import queue without active fetch checker")?;
				let block_import = grandpa::light_block_import::<_, _, _, RuntimeApi, LightClient<Self>>(
					client.clone(), Arc::new(fetch_checker), client.clone()
				)?;
				let block_import = Arc::new(block_import);
				let finality_proof_import = block_import.clone();
				let finality_proof_request_builder = finality_proof_import.create_finality_proof_request_builder();

				import_queue::<_, _, ed25519::Pair>(
					SlotDuration::get_or_compute(&*client)?,
					block_import,
					None,
					Some(finality_proof_import),
					Some(finality_proof_request_builder),
					client,
					config.custom.inherent_data_providers.clone(),
				).map_err(Into::into)
			}},
		SelectChain = LongestChain<FullBackend<Self>, Self::Block>
			{ |config: &FactoryFullConfiguration<Self>, client: Arc<FullClient<Self>>| {
				#[allow(deprecated)]
				Ok(LongestChain::new(client.backend().clone()))
			}
		},
		FinalityProofProvider = { |client: Arc<FullClient<Self>>| {
			Ok(Some(Arc::new(grandpa::FinalityProofProvider::new(client.clone(), client)) as _))
		}},
	}
}
