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
use std::sync::Arc;
use std::time::Duration;
use polkadot_primitives::{parachain, Hash, BlockId};
use polkadot_runtime::GenesisConfig;
use polkadot_network::{gossip::{self as network_gossip, Known}, validation::ValidationNetwork};
use service::{error::{Error as ServiceError}, Configuration, ServiceBuilder};
use transaction_pool::txpool::{Pool as TransactionPool};
use babe::{import_queue, start_babe, Config};
use grandpa::{self, FinalityProofProvider as GrandpaFinalityProofProvider};
use inherents::InherentDataProviders;
use log::info;
pub use service::{AbstractService, Roles, PruningMode, TransactionPoolOptions, Error};
pub use service::{ServiceBuilderExport, ServiceBuilderImport, ServiceBuilderRevert};
pub use service::config::full_version_from_strs;
pub use client::{backend::Backend, runtime_api::{Core as CoreApi, ConstructRuntimeApi}, ExecutionStrategy, CallExecutor};
pub use consensus_common::SelectChain;
pub use polkadot_network::{PolkadotProtocol};
pub use polkadot_primitives::parachain::{CollatorId, ParachainHost};
pub use polkadot_primitives::Block;
pub use polkadot_runtime::RuntimeApi;
pub use primitives::Blake2Hasher;
pub use sr_primitives::traits::ProvideRuntimeApi;
pub use substrate_network::specialization::NetworkSpecialization;
pub use chain_spec::ChainSpec;
pub use consensus::run_validation_worker;

/// Polkadot-specific configuration.
pub struct CustomConfiguration {
	/// Set to `Some` with a collator `CollatorId` and desired parachain
	/// if the network protocol should be started in collator mode.
	pub collating_for: Option<(CollatorId, parachain::Id)>,

	/// Maximal `block_data` size.
	pub max_block_data_size: Option<u64>,
}

impl Default for CustomConfiguration {
	fn default() -> Self {
		Self {
			collating_for: None,
			max_block_data_size: None,
		}
	}
}

/// Chain API type for the transaction pool.
pub type TxChainApi<Backend, Executor> = transaction_pool::ChainApi<
	client::Client<Backend, Executor, Block, RuntimeApi>,
	Block,
>;

/// Starts a `ServiceBuilder` for a full service.
///
/// Use this macro if you don't actually need the full service, but just the builder in order to
/// be able to perform chain operations.
macro_rules! new_full_start {
	($config:expr) => {{
		let mut import_setup = None;
		let inherent_data_providers = inherents::InherentDataProviders::new();
		let mut tasks_to_spawn = None;

		let builder = service::ServiceBuilder::new_full::<
			Block, RuntimeApi, polkadot_executor::Executor
		>($config)?
			.with_select_chain(|_, backend| {
				Ok(client::LongestChain::new(backend.clone()))
			})?
			.with_transaction_pool(|config, client|
				Ok(transaction_pool::txpool::Pool::new(config, transaction_pool::ChainApi::new(client)))
			)?
			.with_import_queue(|_config, client, mut select_chain, transaction_pool| {
				let select_chain = select_chain.take()
					.ok_or_else(|| service::Error::SelectChainRequired)?;
				let (block_import, link_half) =
					grandpa::block_import::<_, _, _, RuntimeApi, _, _>(
						client.clone(), client.clone(), select_chain
					)?;
				let justification_import = block_import.clone();

				let (import_queue, babe_link, babe_block_import, pruning_task) = babe::import_queue(
					babe::Config::get_or_compute(&*client)?,
					block_import,
					Some(Box::new(justification_import)),
					None,
					client.clone(),
					client,
					inherent_data_providers.clone(),
					Some(transaction_pool)
				)?;

				import_setup = Some((babe_block_import.clone(), link_half, babe_link));
				tasks_to_spawn = Some(vec![Box::new(pruning_task)]);

				Ok(import_queue)
			})?;

		(builder, import_setup, inherent_data_providers, tasks_to_spawn)
	}}
}

/// Builds a new object suitable for chain operations.
pub fn new_chain_ops(config: Configuration<impl Send + Default + 'static, GenesisConfig>)
	-> Result<impl ServiceBuilderExport + ServiceBuilderImport + ServiceBuilderRevert, ServiceError>
{
	Ok(new_full_start!(config).0)
}

/// Builds a new service for a full client.
pub fn new_full(config: Configuration<CustomConfiguration, GenesisConfig>)
	-> Result<impl AbstractService<
		Block = Block, RuntimeApi = RuntimeApi, NetworkSpecialization = PolkadotProtocol,
		Backend = impl Backend<Block, Blake2Hasher> + 'static,
		SelectChain = impl SelectChain<Block>,
		CallExecutor = impl CallExecutor<Block, Blake2Hasher> + Clone + Send + Sync + 'static,
	>, ServiceError>
{
	let is_authority = config.roles.is_authority();
	let is_collator = config.custom.collating_for.is_some();
	let force_authoring = config.force_authoring;
	let max_block_data_size = config.custom.max_block_data_size;
	let db_path = config.database_path.clone();
	let disable_grandpa = config.disable_grandpa;
	let name = config.name.clone();

	let (builder, mut import_setup, inherent_data_providers, mut tasks_to_spawn) = new_full_start!(config);

	let service = builder
		.with_network_protocol(|config| Ok(PolkadotProtocol::new(config.custom.collating_for.clone())))?
		.with_finality_proof_provider(|client, backend|
			Ok(Arc::new(GrandpaFinalityProofProvider::new(backend, client)) as _)
		)?
		.build()?;

	let (block_import, link_half, babe_link) = import_setup.take()
		.expect("Link Half and Block Import are present for Full Services or setup failed before. qed");

	// spawn any futures that were created in the previous setup steps
	if let Some(tasks) = tasks_to_spawn.take() {
		for task in tasks {
			service.spawn_task(
				task.select(service.on_exit())
					.map(|_| ())
					.map_err(|_| ())
			);
		}
	}

	if is_collator {
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

	let is_known = move |block_hash: &Hash| {
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
	};

	let gossip_validator = network_gossip::register_validator(
		service.network(),
		(is_known, client.clone()),
	);

	if is_authority {
		let availability_store = {
			use std::path::PathBuf;

			let mut path = PathBuf::from(db_path);
			path.push("availability");

			av_store::Store::new(::av_store::Config {
				cache_size: None,
				path,
			})?
		};

		{
			let availability_store = availability_store.clone();
			service.network().with_spec(
				|spec, _ctx| spec.register_availability_store(availability_store)
			);
		}

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
			availability_store,
			polkadot_runtime::constants::time::SLOT_DURATION,
			max_block_data_size,
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
			inherent_data_providers: inherent_data_providers.clone(),
			force_authoring: force_authoring,
			time_source: babe_link,
		};

		let babe = start_babe(babe_config)?;
		let select = babe.select(service.on_exit()).then(|_| Ok(()));
		service.spawn_essential_task(Box::new(select));
	} else {
		network_gossip::register_non_authority_validator(service.network());
	}

	let config = grandpa::Config {
		// FIXME substrate#1578 make this available through chainspec
		gossip_duration: Duration::from_millis(333),
		justification_period: 4096,
		name: Some(name),
		keystore: Some(service.keystore()),
	};

	match (is_authority, disable_grandpa) {
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
			let grandpa_config = grandpa::GrandpaParams {
				config: config,
				link: link_half,
				network: service.network(),
				inherent_data_providers: inherent_data_providers.clone(),
				on_exit: service.on_exit(),
				telemetry_on_connect: Some(service.telemetry_on_connect_stream()),
			};
			service.spawn_essential_task(Box::new(grandpa::run_grandpa_voter(grandpa_config)?));
		},
		(_, true) => {
			grandpa::setup_disabled_grandpa(
				service.client(),
				&inherent_data_providers,
				service.network(),
			)?;
		},
	}

	Ok(service)
}

/// Builds a new service for a light client.
pub fn new_light(config: Configuration<CustomConfiguration, GenesisConfig>)
	-> Result<impl AbstractService<
		Block = Block, RuntimeApi = RuntimeApi, NetworkSpecialization = PolkadotProtocol,
		Backend = impl Backend<Block, Blake2Hasher> + 'static,
		SelectChain = impl SelectChain<Block>,
		CallExecutor = impl CallExecutor<Block, Blake2Hasher> + Clone + Send + Sync + 'static,
	>, ServiceError>
{
	let inherent_data_providers = InherentDataProviders::new();

	ServiceBuilder::new_light::<Block, RuntimeApi, polkadot_executor::Executor>(config)?
		.with_select_chain(|_, backend| {
			Ok(LongestChain::new(backend.clone()))
		})?
		.with_transaction_pool(|config, client|
			Ok(TransactionPool::new(config, transaction_pool::ChainApi::new(client)))
		)?
		.with_import_queue_and_fprb(|_config, client, backend, _select_chain, transaction_pool| {
			let fetch_checker = backend.blockchain().fetcher()
				.upgrade()
				.map(|fetcher| fetcher.checker().clone())
				.ok_or_else(|| "Trying to start light import queue without active fetch checker")?;
			let block_import = grandpa::light_block_import::<_, _, _, RuntimeApi, _>(
				client.clone(), backend, Arc::new(fetch_checker), client.clone()
			)?;

			let finality_proof_import = block_import.clone();
			let finality_proof_request_builder =
				finality_proof_import.create_finality_proof_request_builder();

			// FIXME: pruning task isn't started since light client doesn't do `AuthoritySetup`.
			let (import_queue, ..) = import_queue(
				Config::get_or_compute(&*client)?,
				block_import,
				None,
				Some(Box::new(finality_proof_import)),
				client.clone(),
				client,
				inherent_data_providers.clone(),
				Some(transaction_pool)
			)?;

			Ok((import_queue, finality_proof_request_builder))
		})?
		.with_network_protocol(|config| Ok(PolkadotProtocol::new(config.custom.collating_for.clone())))?
		.with_finality_proof_provider(|client, backend|
			Ok(Arc::new(GrandpaFinalityProofProvider::new(backend, client)) as _)
		)?
		.build()
}
