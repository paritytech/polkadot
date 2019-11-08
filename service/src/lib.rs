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

use futures::sync::mpsc;
use client::LongestChain;
use std::sync::Arc;
use std::time::Duration;
use polkadot_primitives::{parachain, Hash, BlockId};
use polkadot_runtime::GenesisConfig;
use polkadot_network::{gossip::{self as network_gossip, Known}, validation::ValidationNetwork};
use service::{error::{Error as ServiceError}, Configuration, ServiceBuilder};
use transaction_pool::txpool::{Pool as TransactionPool};
use grandpa::{self, FinalityProofProvider as GrandpaFinalityProofProvider};
use inherents::InherentDataProviders;
use log::info;
pub use service::{AbstractService, Roles, PruningMode, TransactionPoolOptions, Error};
pub use service::{ServiceBuilderExport, ServiceBuilderImport, ServiceBuilderRevert};
pub use service::config::{DatabaseConfig, full_version_from_strs};
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
pub type TxChainApi<Backend, Executor> = transaction_pool::FullChainApi<
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
		let builder = service::ServiceBuilder::new_full::<
			Block, RuntimeApi, polkadot_executor::Executor
		>($config)?
			.with_select_chain(|_, backend| {
				Ok(client::LongestChain::new(backend.clone()))
			})?
			.with_transaction_pool(|config, client|
				Ok(transaction_pool::txpool::Pool::new(config, transaction_pool::FullChainApi::new(client)))
			)?
			.with_import_queue(|_config, client, mut select_chain, _| {
				let select_chain = select_chain.take()
					.ok_or_else(|| service::Error::SelectChainRequired)?;
				let (grandpa_block_import, grandpa_link) =
					grandpa::block_import::<_, _, _, RuntimeApi, _>(
						client.clone(), &*client, select_chain
					)?;
				let justification_import = grandpa_block_import.clone();

				let (block_import, babe_link) = babe::block_import(
					babe::Config::get_or_compute(&*client)?,
					grandpa_block_import,
					client.clone(),
					client.clone(),
				)?;

				let import_queue = babe::import_queue(
					babe_link.clone(),
					block_import.clone(),
					Some(Box::new(justification_import)),
					None,
					client.clone(),
					client,
					inherent_data_providers.clone(),
				)?;

				import_setup = Some((block_import, grandpa_link, babe_link));
				Ok(import_queue)
			})?
			.with_rpc_extensions(|client, pool, _backend| -> polkadot_rpc::RpcExtension {
				polkadot_rpc::create(client, pool)
			})?;

		(builder, import_setup, inherent_data_providers)
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
	use substrate_network::DhtEvent;

	let is_collator = config.custom.collating_for.is_some();
	let is_authority = config.roles.is_authority() && !is_collator;
	let force_authoring = config.force_authoring;
	let max_block_data_size = config.custom.max_block_data_size;
	let db_path = if let DatabaseConfig::Path { ref path, .. } = config.database {
		path.clone()
	} else {
		return Err("Starting a Polkadot service with a custom database isn't supported".to_string().into());
	};
	let disable_grandpa = config.disable_grandpa;
	let name = config.name.clone();

	// sentry nodes announce themselves as authorities to the network
	// and should run the same protocols authorities do, but it should
	// never actively participate in any consensus process.
	let participates_in_consensus = is_authority && !config.sentry_mode;

	let (builder, mut import_setup, inherent_data_providers) = new_full_start!(config);

	// Dht event channel from the network to the authority discovery module. Use
	// bounded channel to ensure back-pressure. Authority discovery is triggering one
	// event per authority within the current authority set. This estimates the
	// authority set size to be somewhere below 10 000 thereby setting the channel
	// buffer size to 10 000.
	let (dht_event_tx, _dht_event_rx) = mpsc::channel::<DhtEvent>(10000);

	let service = builder
		.with_network_protocol(|config| Ok(PolkadotProtocol::new(config.custom.collating_for.clone())))?
		.with_finality_proof_provider(|client, backend|
			Ok(Arc::new(GrandpaFinalityProofProvider::new(backend, client)) as _)
		)?
		.with_dht_event_tx(dht_event_tx)?
		.build()?;

	let (block_import, link_half, babe_link) = import_setup.take()
		.expect("Link Half and Block Import are present for Full Services or setup failed before. qed");

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
		use consensus_common::BlockStatus;

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

	if participates_in_consensus {
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
			keystore: service.keystore(),
			client,
			select_chain,
			block_import,
			env: proposer,
			sync_oracle: service.network(),
			inherent_data_providers: inherent_data_providers.clone(),
			force_authoring: force_authoring,
			babe_link,
		};

		let babe = babe::start_babe(babe_config)?;
		service.spawn_essential_task(babe);
	}

	// if the node isn't actively participating in consensus then it doesn't
	// need a keystore, regardless of which protocol we use below.
	let keystore = if participates_in_consensus {
		Some(service.keystore())
	} else {
		None
	};

	let config = grandpa::Config {
		// FIXME substrate#1578 make this available through chainspec
		gossip_duration: Duration::from_millis(333),
		justification_period: 512,
		name: Some(name),
		observer_enabled: false,
		keystore,
		is_authority,
	};

	let enable_grandpa = !disable_grandpa;
	if enable_grandpa {
		// start the full GRANDPA voter
		// NOTE: unlike in substrate we are currently running the full
		// GRANDPA voter protocol for all full nodes (regardless of whether
		// they're validators or not). at this point the full voter should
		// provide better guarantees of block and vote data availability than
		// the observer.
		let grandpa_config = grandpa::GrandpaParams {
			config: config,
			link: link_half,
			network: service.network(),
			inherent_data_providers: inherent_data_providers.clone(),
			on_exit: service.on_exit(),
			telemetry_on_connect: Some(service.telemetry_on_connect_stream()),
			voting_rule: grandpa::VotingRulesBuilder::default().build(),
		};
		service.spawn_essential_task(grandpa::run_grandpa_voter(grandpa_config)?);
	} else {
		grandpa::setup_disabled_grandpa(
			service.client(),
			&inherent_data_providers,
			service.network(),
		)?;
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
			Ok(TransactionPool::new(config, transaction_pool::FullChainApi::new(client)))
		)?
		.with_import_queue_and_fprb(|_config, client, backend, fetcher, _select_chain, _| {
			let fetch_checker = fetcher
				.map(|fetcher| fetcher.checker().clone())
				.ok_or_else(|| "Trying to start light import queue without active fetch checker")?;
			let grandpa_block_import = grandpa::light_block_import::<_, _, _, RuntimeApi>(
				client.clone(), backend, &*client, Arc::new(fetch_checker)
			)?;

			let finality_proof_import = grandpa_block_import.clone();
			let finality_proof_request_builder =
				finality_proof_import.create_finality_proof_request_builder();

			let (babe_block_import, babe_link) = babe::block_import(
				babe::Config::get_or_compute(&*client)?,
				grandpa_block_import,
				client.clone(),
				client.clone(),
			)?;

			// FIXME: pruning task isn't started since light client doesn't do `AuthoritySetup`.
			let import_queue = babe::import_queue(
				babe_link,
				babe_block_import,
				None,
				Some(Box::new(finality_proof_import)),
				client.clone(),
				client,
				inherent_data_providers.clone(),
			)?;

			Ok((import_queue, finality_proof_request_builder))
		})?
		.with_network_protocol(|config| Ok(PolkadotProtocol::new(config.custom.collating_for.clone())))?
		.with_finality_proof_provider(|client, backend|
			Ok(Arc::new(GrandpaFinalityProofProvider::new(backend, client)) as _)
		)?
		.with_rpc_extensions(|client, pool, _backend| -> polkadot_rpc::RpcExtension {
			polkadot_rpc::create(client, pool)
		})?
		.build()
}
