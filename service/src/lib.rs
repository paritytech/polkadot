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

use futures::prelude::*;
use client::LongestChain;
use consensus_common::SelectChain;
use std::sync::Arc;
use std::time::Duration;
use polkadot_primitives::{parachain, Block, Hash, BlockId};
use polkadot_runtime::{GenesisConfig, RuntimeApi};
use polkadot_network::gossip::{self as network_gossip, Known};
use service::{
	FactoryFullConfiguration, FullBackend, LightBackend, FullExecutor, LightExecutor,
	error::Error as ServiceError, TelemetryOnConnect
};
use transaction_pool::txpool::{Pool as TransactionPool};
use babe::{import_queue, start_babe, BabeImportQueue, Config};
use grandpa::{self, FinalityProofProvider as GrandpaFinalityProofProvider};
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
pub use primitives::Blake2Hasher;
pub use sr_primitives::traits::ProvideRuntimeApi;
pub use chain_spec::ChainSpec;
pub use consensus::run_validation_worker;

/// All configuration for the polkadot node.
pub type Configuration = FactoryFullConfiguration<Factory>;

type BabeBlockImportForService<F> = babe::BabeBlockImport<
	FullBackend<F>,
	FullExecutor<F>,
	<F as crate::ServiceFactory>::Block,
	grandpa::BlockImportForService<F>,
	<F as crate::ServiceFactory>::RuntimeApi,
	client::Client<
		FullBackend<F>,
		FullExecutor<F>,
		<F as crate::ServiceFactory>::Block,
		<F as crate::ServiceFactory>::RuntimeApi
	>,
>;

/// Polkadot-specific configuration.
pub struct CustomConfiguration {
	/// Set to `Some` with a collator `CollatorId` and desired parachain
	/// if the network protocol should be started in collator mode.
	pub collating_for: Option<(CollatorId, parachain::Id)>,

	/// Intermediate state during setup. Will be removed in future. Set to `None`.
	// FIXME: rather than putting this on the config, let's have an actual intermediate setup state
	// https://github.com/paritytech/substrate/issues/1134
	pub import_setup: Option<(
		BabeBlockImportForService<Factory>,
		grandpa::LinkHalfForService<Factory>,
		babe::BabeLink,
	)>,

	/// Tasks that were created by previous setup steps and should be spawned.
	pub tasks_to_spawn: Option<Vec<Box<dyn Future<Item = (), Error = ()> + Send>>>,

	/// Maximal `block_data` size.
	pub max_block_data_size: Option<u64>,

	inherent_data_providers: InherentDataProviders,
}

impl Default for CustomConfiguration {
	fn default() -> Self {
		Self {
			collating_for: None,
			import_setup: None,
			inherent_data_providers: InherentDataProviders::new(),
			tasks_to_spawn: None,
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
		NetworkProtocol = PolkadotProtocol { |config: &Configuration| Ok(PolkadotProtocol::new(config.custom.collating_for.clone())) },
		RuntimeDispatch = polkadot_executor::Executor,
		FullTransactionPoolApi = transaction_pool::ChainApi<client::Client<FullBackend<Self>, FullExecutor<Self>, Block, RuntimeApi>, Block>
			{ |config, client| Ok(TransactionPool::new(config, transaction_pool::ChainApi::new(client))) },
		LightTransactionPoolApi = transaction_pool::ChainApi<client::Client<LightBackend<Self>, LightExecutor<Self>, Block, RuntimeApi>, Block>
			{ |config, client| Ok(TransactionPool::new(config, transaction_pool::ChainApi::new(client))) },
		Genesis = GenesisConfig,
		Configuration = CustomConfiguration,
		FullService = FullComponents<Self> { |config: FactoryFullConfiguration<Self>| FullComponents::<Factory>::new(config) },
		AuthoritySetup = {
			|mut service: Self::FullService| {
				let (block_import, link_half, babe_link) = service.config_mut().custom.import_setup.take()
					.expect("Link Half and Block Import are present for Full Services or setup failed before. qed");

				// spawn any futures that were created in the previous setup steps
				if let Some(tasks) = service.config_mut().custom.tasks_to_spawn.take() {
					for task in tasks {
						service.spawn_task(
							task.select(service.on_exit())
								.map(|_| ())
								.map_err(|_| ())
						);
					}
				}

				if service.config().roles != service::Roles::AUTHORITY {
					return Ok(service);
				}

				if service.config().custom.collating_for.is_some() {
					info!(
						"The node cannot start as an authority because it is also configured to run as a collator."
					);
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

				use polkadot_network::validation::ValidationNetwork;
				let extrinsic_store = {
					use std::path::PathBuf;

					let mut path = PathBuf::from(service.config().database_path.clone());
					path.push("availability");

					av_store::Store::new(::av_store::Config {
						cache_size: None,
						path,
					})?
				};

				// collator connections and validation network both fulfilled by this
				let validation_network = ValidationNetwork::new(
					service.network(),
					service.on_exit(),
					gossip_validator,
					service.client(),
					polkadot_network::validation::WrappedExecutor(service.spawn_task_handle()),
				);
				let proposer = consensus::ProposerFactory::new(
					client.clone(),
					select_chain.clone(),
					validation_network.clone(),
					validation_network,
					service.transaction_pool(),
					Arc::new(service.spawn_task_handle()),
					service.keystore(),
					extrinsic_store,
					polkadot_runtime::constants::time::SLOT_DURATION,
					service.config().custom.max_block_data_size,
				);

				let client = service.client();
				let select_chain = service.select_chain().ok_or(ServiceError::SelectChainRequired)?;

				let babe_config = babe::BabeParams {
					config: Config::get_or_compute(&*client)?,
					keystore: service.keystore(),
					client,
					select_chain,
					block_import,
					env: proposer,
					sync_oracle: service.network(),
					inherent_data_providers: service.config().custom.inherent_data_providers.clone(),
					force_authoring: service.config().force_authoring,
					time_source: babe_link,
				};

				let babe = start_babe(babe_config)?;
				let select = babe.select(service.on_exit()).then(|_| Ok(()));
				service.spawn_task(Box::new(select));

				let config = grandpa::Config {
					// FIXME substrate#1578 make this available through chainspec
					gossip_duration: Duration::from_millis(333),
					justification_period: 4096,
					name: Some(service.config().name.clone()),
					keystore: Some(service.keystore()),
				};

				match (service.config().roles.is_authority(), service.config().disable_grandpa) {
					(false, false) => {
						// start the lightweight GRANDPA observer
						service.spawn_task(Box::new(grandpa::run_grandpa_observer(
							config,
							link_half,
							service.network(),
							service.on_exit(),
						)?));
					},
					(true, false) => {
						// start the full GRANDPA voter
						let telemetry_on_connect = TelemetryOnConnect {
							telemetry_connection_sinks: service.telemetry_on_connect_stream(),
						};
						let grandpa_config = grandpa::GrandpaParams {
							config: config,
							link: link_half,
							network: service.network(),
							inherent_data_providers: service.config().custom.inherent_data_providers.clone(),
							on_exit: service.on_exit(),
							telemetry_on_connect: Some(telemetry_on_connect),
						};
						service.spawn_task(Box::new(grandpa::run_grandpa_voter(grandpa_config)?));
					},
					(_, true) => {
						grandpa::setup_disabled_grandpa(
							service.client(),
							&service.config().custom.inherent_data_providers,
							service.network(),
						)?;
					},
				}
				// let config = grandpa::Config {
				// 	// FIXME #1578 make this available through chainspec
				// 	gossip_duration: Duration::from_millis(333),
				// 	justification_period: 4096,
				// 	name: Some(service.config().name.clone()),
				// 	keystore: Some(service.keystore()),
				// };

				// if !service.config().disable_grandpa {
				// 	if service.config().roles.is_authority() {
				// 		let telemetry_on_connect = TelemetryOnConnect {
				// 			telemetry_connection_sinks: service.telemetry_on_connect_stream(),
				// 		};
				// 		let grandpa_config = grandpa::GrandpaParams {
				// 			config: config,
				// 			link: link_half,
				// 			network: service.network(),
				// 			inherent_data_providers: service.config().custom.inherent_data_providers.clone(),
				// 			on_exit: service.on_exit(),
				// 			telemetry_on_connect: Some(telemetry_on_connect),
				// 		};
				// 		service.spawn_task(Box::new(grandpa::run_grandpa_voter(grandpa_config)?));
				// 	} else {
				// 		service.spawn_task(Box::new(grandpa::run_grandpa_observer(
				// 			config,
				// 			link_half,
				// 			service.network(),
				// 			service.on_exit(),
				// 		)?));
				// 	}
				// }

				// // regardless of whether grandpa is started or not, when
				// // authoring blocks we expect inherent data regarding what our
				// // last finalized block is, to be available.
				// grandpa::register_finality_tracker_inherent_data_provider(
				// 	service.client(),
				// 	&service.config().custom.inherent_data_providers,
				// )?;

				Ok(service)
			}
		},
		LightService = LightComponents<Self>
			{ |config| <LightComponents<Factory>>::new(config) },
		FullImportQueue = BabeImportQueue<Self::Block>
			{ |config: &mut FactoryFullConfiguration<Self>, client: Arc<FullClient<Self>>, select_chain: Self::SelectChain| {
				let (block_import, link_half) =
					grandpa::block_import::<_, _, _, RuntimeApi, FullClient<Self>, _>(
						client.clone(), client.clone(), select_chain
					)?;
				let justification_import = block_import.clone();

				let (import_queue, babe_link, babe_block_import, pruning_task) = import_queue(
					Config::get_or_compute(&*client)?,
					block_import,
					Some(Box::new(justification_import)),
					None,
					client.clone(),
					client,
					config.custom.inherent_data_providers.clone(),
				)?;

				config.custom.import_setup = Some((babe_block_import.clone(), link_half, babe_link));
				config.custom.tasks_to_spawn = Some(vec![Box::new(pruning_task)]);

				Ok(import_queue)
			}},
		LightImportQueue = BabeImportQueue<Self::Block>
			{ |config: &FactoryFullConfiguration<Self>, client: Arc<LightClient<Self>>| {
				#[allow(deprecated)]
				let fetch_checker = client.backend().blockchain().fetcher()
					.upgrade()
					.map(|fetcher| fetcher.checker().clone())
					.ok_or_else(|| "Trying to start light import queue without active fetch checker")?;
				let block_import = grandpa::light_block_import::<_, _, _, RuntimeApi, LightClient<Self>>(
					client.clone(), Arc::new(fetch_checker), client.clone()
				)?;

				let finality_proof_import = block_import.clone();
				let finality_proof_request_builder = finality_proof_import.create_finality_proof_request_builder();

				// FIXME: pruning task isn't started since light client doesn't do `AuthoritySetup`.
				let (import_queue, ..) = import_queue(
					Config::get_or_compute(&*client)?,
					block_import,
					None,
					Some(Box::new(finality_proof_import)),
					client.clone(),
					client,
					config.custom.inherent_data_providers.clone(),
				)?;

				Ok((import_queue, finality_proof_request_builder))
			}},
		SelectChain = LongestChain<FullBackend<Self>, Self::Block>
			{ |config: &FactoryFullConfiguration<Self>, client: Arc<FullClient<Self>>| {
				#[allow(deprecated)]
				Ok(LongestChain::new(client.backend().clone()))
			}
		},
		FinalityProofProvider = { |client: Arc<FullClient<Self>>| {
			Ok(Some(Arc::new(GrandpaFinalityProofProvider::new(client.clone(), client)) as _))
		}},
	}
}
