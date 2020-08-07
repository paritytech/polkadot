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

//! Polkadot service. Specialized wrapper over substrate service.

pub mod chain_spec;
pub mod grandpa_support;
mod client;

use std::sync::Arc;
use std::time::Duration;
use polkadot_primitives::v0::{self as parachain, Hash, BlockId};
#[cfg(feature = "full-node")]
use polkadot_network::{legacy::gossip::Known, protocol as network_protocol};
use service::{error::Error as ServiceError};
use grandpa::{self, FinalityProofProvider as GrandpaFinalityProofProvider};
use sc_executor::native_executor_instance;
use log::info;
use sp_trie::PrefixedMemoryDB;
pub use service::{
	Role, PruningMode, TransactionPoolOptions, Error, RuntimeGenesis, RpcHandlers,
	TFullClient, TLightClient, TFullBackend, TLightBackend, TFullCallExecutor, TLightCallExecutor,
	Configuration, ChainSpec, TaskManager,
};
pub use service::config::{DatabaseConfig, PrometheusConfig};
pub use sc_executor::NativeExecutionDispatch;
pub use sc_client_api::{Backend, ExecutionStrategy, CallExecutor};
pub use sc_consensus::LongestChain;
pub use sp_api::{Core as CoreApi, ConstructRuntimeApi, ProvideRuntimeApi, StateBackend};
pub use sp_runtime::traits::{HashFor, NumberFor};
pub use consensus_common::{SelectChain, BlockImport, block_validation::Chain};
pub use polkadot_primitives::v0::{Block, CollatorId, ParachainHost};
pub use sp_runtime::traits::{Block as BlockT, self as runtime_traits, BlakeTwo256};
pub use chain_spec::{PolkadotChainSpec, KusamaChainSpec, WestendChainSpec};
#[cfg(feature = "full-node")]
pub use consensus::run_validation_worker;
pub use codec::Codec;
pub use polkadot_runtime;
pub use kusama_runtime;
pub use westend_runtime;
use prometheus_endpoint::Registry;
pub use self::client::*;

native_executor_instance!(
	pub PolkadotExecutor,
	polkadot_runtime::api::dispatch,
	polkadot_runtime::native_version,
	frame_benchmarking::benchmarking::HostFunctions,
);

native_executor_instance!(
	pub KusamaExecutor,
	kusama_runtime::api::dispatch,
	kusama_runtime::native_version,
	frame_benchmarking::benchmarking::HostFunctions,
);

native_executor_instance!(
	pub WestendExecutor,
	westend_runtime::api::dispatch,
	westend_runtime::native_version,
	frame_benchmarking::benchmarking::HostFunctions,
);

native_executor_instance!(
	pub RococoExecutor,
	rococo_runtime::api::dispatch,
	rococo_runtime::native_version,
	frame_benchmarking::benchmarking::HostFunctions,
);

/// Can be called for a `Configuration` to check if it is a configuration for the `Kusama` network.
pub trait IdentifyVariant {
	/// Returns if this is a configuration for the `Kusama` network.
	fn is_kusama(&self) -> bool;

	/// Returns if this is a configuration for the `Westend` network.
	fn is_westend(&self) -> bool;

	/// Returns if this is a configuration for the `Rococo` network.
	fn is_rococo(&self) -> bool;
}

impl IdentifyVariant for Box<dyn ChainSpec> {
	fn is_kusama(&self) -> bool {
		self.id().starts_with("kusama") || self.id().starts_with("ksm")
	}

	fn is_westend(&self) -> bool {
		self.id().starts_with("westend") || self.id().starts_with("wnd")
	}

	fn is_rococo(&self) -> bool {
		self.id().starts_with("rococo") || self.id().starts_with("roc")
	}
}

type FullBackend = service::TFullBackend<Block>;
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;
type FullClient<RuntimeApi, Executor> = service::TFullClient<Block, RuntimeApi, Executor>;
type FullGrandpaBlockImport<RuntimeApi, Executor> = grandpa::GrandpaBlockImport<
	FullBackend, Block, FullClient<RuntimeApi, Executor>, FullSelectChain
>;

type LightBackend = service::TLightBackendWithHash<Block, sp_runtime::traits::BlakeTwo256>;

type LightClient<RuntimeApi, Executor> =
	service::TLightClientWithBackend<Block, RuntimeApi, Executor, LightBackend>;

#[cfg(feature = "full-node")]
pub fn new_partial<RuntimeApi, Executor>(config: &mut Configuration, test: bool) -> Result<
	service::PartialComponents<
		FullClient<RuntimeApi, Executor>, FullBackend, FullSelectChain,
		consensus_common::DefaultImportQueue<Block, FullClient<RuntimeApi, Executor>>,
		sc_transaction_pool::FullPool<Block, FullClient<RuntimeApi, Executor>>,
		(
			impl Fn(polkadot_rpc::DenyUnsafe, polkadot_rpc::SubscriptionManager) -> polkadot_rpc::RpcExtension,
			(
				babe::BabeBlockImport<
					Block, FullClient<RuntimeApi, Executor>, FullGrandpaBlockImport<RuntimeApi, Executor>
				>,
				grandpa::LinkHalf<Block, FullClient<RuntimeApi, Executor>, FullSelectChain>,
				babe::BabeLink<Block>
			),
			grandpa::SharedVoterState,
		)
	>,
	Error
>
	where
		RuntimeApi: ConstructRuntimeApi<Block, FullClient<RuntimeApi, Executor>> + Send + Sync + 'static,
		RuntimeApi::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
		Executor: NativeExecutionDispatch + 'static,
{
	if !test {
		// If we're using prometheus, use a registry with a prefix of `polkadot`.
		if let Some(PrometheusConfig { registry, .. }) = config.prometheus_config.as_mut() {
			*registry = Registry::new_custom(Some("polkadot".into()), None)?;
		}
	}

	let inherent_data_providers = inherents::InherentDataProviders::new();

	let (client, backend, keystore, task_manager) =
		service::new_full_parts::<Block, RuntimeApi, Executor>(&config)?;
	let client = Arc::new(client);

	let select_chain = sc_consensus::LongestChain::new(backend.clone());

	let transaction_pool = sc_transaction_pool::BasicPool::new_full(
		config.transaction_pool.clone(),
		config.prometheus_registry(),
		task_manager.spawn_handle(),
		client.clone(),
	);

	let grandpa_hard_forks = if config.chain_spec.is_kusama() && !test {
		crate::grandpa_support::kusama_hard_forks()
	} else {
		Vec::new()
	};

	let (grandpa_block_import, grandpa_link) =
		grandpa::block_import_with_authority_set_hard_forks(
			client.clone(),
			&(client.clone() as Arc<_>),
			select_chain.clone(),
			grandpa_hard_forks,
		)?;

	let justification_import = grandpa_block_import.clone();

	let (block_import, babe_link) = babe::block_import(
		babe::Config::get_or_compute(&*client)?,
		grandpa_block_import,
		client.clone(),
	)?;

	let import_queue = babe::import_queue(
		babe_link.clone(),
		block_import.clone(),
		Some(Box::new(justification_import)),
		None,
		client.clone(),
		select_chain.clone(),
		inherent_data_providers.clone(),
		&task_manager.spawn_handle(),
		config.prometheus_registry(),
	)?;

	let justification_stream = grandpa_link.justification_stream();
	let shared_authority_set = grandpa_link.shared_authority_set().clone();
	let shared_voter_state = grandpa::SharedVoterState::empty();

	let import_setup = (block_import.clone(), grandpa_link, babe_link.clone());
	let rpc_setup = shared_voter_state.clone();

	let babe_config = babe_link.config().clone();
	let shared_epoch_changes = babe_link.epoch_changes().clone();

	let rpc_extensions_builder = {
		let client = client.clone();
		let keystore = keystore.clone();
		let transaction_pool = transaction_pool.clone();
		let select_chain = select_chain.clone();

		move |deny_unsafe, subscriptions| -> polkadot_rpc::RpcExtension {
			let deps = polkadot_rpc::FullDeps {
				client: client.clone(),
				pool: transaction_pool.clone(),
				select_chain: select_chain.clone(),
				deny_unsafe,
				babe: polkadot_rpc::BabeDeps {
					babe_config: babe_config.clone(),
					shared_epoch_changes: shared_epoch_changes.clone(),
					keystore: keystore.clone(),
				},
				grandpa: polkadot_rpc::GrandpaDeps {
					shared_voter_state: shared_voter_state.clone(),
					shared_authority_set: shared_authority_set.clone(),
					justification_stream: justification_stream.clone(),
					subscriptions,
				},
			};

			polkadot_rpc::create_full(deps)
		}
	};

	Ok(service::PartialComponents {
		client, backend, task_manager, keystore, select_chain, import_queue, transaction_pool,
		inherent_data_providers,
		other: (rpc_extensions_builder, import_setup, rpc_setup)
	})
}

#[cfg(feature = "full-node")]
pub fn new_full<RuntimeApi, Executor>(
	mut config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
	test: bool,
) -> Result<(
	TaskManager,
	Arc<FullClient<RuntimeApi, Executor>>,
	FullNodeHandles,
	Arc<sc_network::NetworkService<Block, <Block as BlockT>::Hash>>,
	Arc<RpcHandlers>,
), Error>
	where
		RuntimeApi: ConstructRuntimeApi<Block, FullClient<RuntimeApi, Executor>> + Send + Sync + 'static,
		RuntimeApi::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
		Executor: NativeExecutionDispatch + 'static,
{
	use sc_network::Event;
	use sc_client_api::ExecutorProvider;
	use futures::stream::StreamExt;
	use sp_core::traits::BareCryptoStorePtr;

	let is_collator = collating_for.is_some();
	let role = config.role.clone();
	let is_authority = role.is_authority() && !is_collator;
	let force_authoring = config.force_authoring;
	let db_path = match config.database.path() {
		Some(path) => std::path::PathBuf::from(path),
		None => return Err("Starting a Polkadot service with a custom database isn't supported".to_string().into()),
	};
	let disable_grandpa = config.disable_grandpa;
	let name = config.network.node_name.clone();

	let service::PartialComponents {
		client, backend, mut task_manager, keystore, select_chain, import_queue, transaction_pool,
		inherent_data_providers,
		other: (rpc_extensions_builder, import_setup, rpc_setup)
	} = new_partial::<RuntimeApi, Executor>(&mut config, test)?;

	let prometheus_registry = config.prometheus_registry().cloned();

	let finality_proof_provider =
		GrandpaFinalityProofProvider::new_for_service(backend.clone(), client.clone());

	let (network, network_status_sinks, system_rpc_tx, network_starter) =
		service::build_network(service::BuildNetworkParams {
			config: &config,
			client: client.clone(),
			transaction_pool: transaction_pool.clone(),
			spawn_handle: task_manager.spawn_handle(),
			import_queue,
			on_demand: None,
			block_announce_validator_builder: None,
			finality_proof_request_builder: None,
			finality_proof_provider: Some(finality_proof_provider.clone()),
		})?;

	if config.offchain_worker.enabled {
		service::build_offchain_workers(
			&config, backend.clone(), task_manager.spawn_handle(), client.clone(), network.clone(),
		);
	}

	let telemetry_connection_sinks = service::TelemetryConnectionSinks::default();

	let rpc_handlers = service::spawn_tasks(service::SpawnTasksParams {
		config,
		backend: backend.clone(),
		client: client.clone(),
		keystore: keystore.clone(),
		network: network.clone(),
		rpc_extensions_builder: Box::new(rpc_extensions_builder),
		transaction_pool: transaction_pool.clone(),
		task_manager: &mut task_manager,
		on_demand: None,
		remote_blockchain: None,
		telemetry_connection_sinks: telemetry_connection_sinks.clone(),
		network_status_sinks, system_rpc_tx,
	})?;

	let (block_import, link_half, babe_link) = import_setup;

	let shared_voter_state = rpc_setup;

	let known_oracle = client.clone();

	let mut handles = FullNodeHandles::default();
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

	let polkadot_network_service = network_protocol::start(
		network.clone(),
		network_protocol::Config {
			collating_for: collating_for,
		},
		(is_known, client.clone()),
		client.clone(),
		task_manager.spawn_handle(),
	).map_err(|e| format!("Could not spawn network worker: {:?}", e))?;

	let authority_handles = if is_collator || role.is_authority() {
		let availability_store = {
			use std::path::PathBuf;

			let mut path = PathBuf::from(db_path);
			path.push("availability");

			#[cfg(not(target_os = "unknown"))]
			{
				av_store::Store::new(
					::av_store::Config {
						cache_size: None,
						path,
					},
					polkadot_network_service.clone(),
				)?
			}

			#[cfg(target_os = "unknown")]
			av_store::Store::new_in_memory(gossip)
		};

		polkadot_network_service.register_availability_store(availability_store.clone());

		let (validation_service_handle, validation_service) = consensus::ServiceBuilder {
			client: client.clone(),
			network: polkadot_network_service.clone(),
			collators: polkadot_network_service.clone(),
			spawner: task_manager.spawn_handle(),
			availability_store: availability_store.clone(),
			select_chain: select_chain.clone(),
			keystore: keystore.clone(),
			max_block_data_size,
		}.build();

		task_manager.spawn_essential_handle().spawn("validation-service", Box::pin(validation_service));

		handles.validation_service_handle = Some(validation_service_handle.clone());

		Some((validation_service_handle, availability_store))
	} else {
		None
	};

	if role.is_authority() {
		let (validation_service_handle, availability_store) = authority_handles
			.clone()
			.expect("Authority handles are set for authority nodes; qed");

		let proposer = consensus::ProposerFactory::new(
			client.clone(),
			transaction_pool,
			validation_service_handle,
			slot_duration,
			prometheus_registry.as_ref(),
		);

		let can_author_with =
			consensus_common::CanAuthorWithNativeVersion::new(client.executor().clone());

		let block_import = availability_store.block_import(
			block_import,
			client.clone(),
			task_manager.spawn_handle(),
			keystore.clone(),
		)?;

		let babe_config = babe::BabeParams {
			keystore: keystore.clone(),
			client: client.clone(),
			select_chain,
			block_import,
			env: proposer,
			sync_oracle: network.clone(),
			inherent_data_providers: inherent_data_providers.clone(),
			force_authoring,
			babe_link,
			can_author_with,
		};

		let babe = babe::start_babe(babe_config)?;
		task_manager.spawn_essential_handle().spawn_blocking("babe", babe);
	}

	if matches!(role, Role::Authority{..} | Role::Sentry{..}) {
		if authority_discovery_enabled {
			let (sentries, authority_discovery_role) = match role {
				Role::Authority { ref sentry_nodes } => (
					sentry_nodes.clone(),
					authority_discovery::Role::Authority (
						keystore.clone(),
					),
				),
				Role::Sentry {..} => (
					vec![],
					authority_discovery::Role::Sentry,
				),
				_ => unreachable!("Due to outer matches! constraint; qed."),
			};

			let network_event_stream = network.event_stream("authority-discovery");
			let dht_event_stream = network_event_stream.filter_map(|e| async move { match e {
				Event::Dht(e) => Some(e),
				_ => None,
			}}).boxed();
			let authority_discovery = authority_discovery::AuthorityDiscovery::new(
				client.clone(),
				network.clone(),
				sentries,
				dht_event_stream,
				authority_discovery_role,
				prometheus_registry.clone(),
			);

			task_manager.spawn_handle().spawn("authority-discovery", authority_discovery);
		}
	}

	// if the node isn't actively participating in consensus then it doesn't
	// need a keystore, regardless of which protocol we use below.
	let keystore = if is_authority {
		Some(keystore as BareCryptoStorePtr)
	} else {
		None
	};

	let config = grandpa::Config {
		// FIXME substrate#1578 make this available through chainspec
		gossip_duration: Duration::from_millis(1000),
		justification_period: 512,
		name: Some(name),
		observer_enabled: false,
		keystore,
		is_authority: role.is_network_authority(),
	};

	let enable_grandpa = !disable_grandpa;
	if enable_grandpa {
		// start the full GRANDPA voter
		// NOTE: unlike in substrate we are currently running the full
		// GRANDPA voter protocol for all full nodes (regardless of whether
		// they're validators or not). at this point the full voter should
		// provide better guarantees of block and vote data availability than
		// the observer.

		// add a custom voting rule to temporarily stop voting for new blocks
		// after the given pause block is finalized and restarting after the
		// given delay.
		let voting_rule = match grandpa_pause {
			Some((block, delay)) => {
				info!("GRANDPA scheduled voting pause set for block #{} with a duration of {} blocks.",
					block,
					delay,
				);

				grandpa::VotingRulesBuilder::default()
					.add(crate::grandpa_support::PauseAfterBlockFor(block, delay))
					.build()
			},
			None =>
				grandpa::VotingRulesBuilder::default()
					.build(),
		};

		let grandpa_config = grandpa::GrandpaParams {
			config,
			link: link_half,
			network: network.clone(),
			inherent_data_providers: inherent_data_providers.clone(),
			telemetry_on_connect: Some(telemetry_connection_sinks.on_connect_stream()),
			voting_rule,
			prometheus_registry: prometheus_registry.clone(),
			shared_voter_state,
		};

		task_manager.spawn_essential_handle().spawn_blocking(
			"grandpa-voter",
			grandpa::run_grandpa_voter(grandpa_config)?
		);
	} else {
		grandpa::setup_disabled_grandpa(
			client.clone(),
			&inherent_data_providers,
			network.clone(),
		)?;
	}

	network_starter.start_network();

	handles.polkadot_network = Some(polkadot_network_service);
	Ok((task_manager, client, handles, network, rpc_handlers))
}

/// Builds a new service for a light client.
fn new_light<Runtime, Dispatch>(mut config: Configuration) -> Result<(TaskManager, Arc<RpcHandlers>), Error>
	where
		Runtime: 'static + Send + Sync + ConstructRuntimeApi<Block, LightClient<Runtime, Dispatch>>,
		<Runtime as ConstructRuntimeApi<Block, LightClient<Runtime, Dispatch>>>::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<LightBackend, Block>>,
		Dispatch: NativeExecutionDispatch + 'static,
{
	use sc_client_api::backend::RemoteBackend;

	// If we're using prometheus, use a registry with a prefix of `polkadot`.
	if let Some(PrometheusConfig { registry, .. }) = config.prometheus_config.as_mut() {
		*registry = Registry::new_custom(Some("polkadot".into()), None)?;
	}

	let (client, backend, keystore, mut task_manager, on_demand) =
		service::new_light_parts::<Block, Runtime, Dispatch>(&config)?;

	let select_chain = sc_consensus::LongestChain::new(backend.clone());

	let transaction_pool = Arc::new(sc_transaction_pool::BasicPool::new_light(
		config.transaction_pool.clone(),
		config.prometheus_registry(),
		task_manager.spawn_handle(),
		client.clone(),
		on_demand.clone(),
	));

	let grandpa_block_import = grandpa::light_block_import(
		client.clone(), backend.clone(), &(client.clone() as Arc<_>),
		Arc::new(on_demand.checker().clone()),
	)?;

	let finality_proof_import = grandpa_block_import.clone();
	let finality_proof_request_builder =
		finality_proof_import.create_finality_proof_request_builder();

	let (babe_block_import, babe_link) = babe::block_import(
		babe::Config::get_or_compute(&*client)?,
		grandpa_block_import,
		client.clone(),
	)?;

	let inherent_data_providers = inherents::InherentDataProviders::new();

	// FIXME: pruning task isn't started since light client doesn't do `AuthoritySetup`.
	let import_queue = babe::import_queue(
		babe_link,
		babe_block_import,
		None,
		Some(Box::new(finality_proof_import)),
		client.clone(),
		select_chain.clone(),
		inherent_data_providers.clone(),
		&task_manager.spawn_handle(),
		config.prometheus_registry(),
	)?;

	let finality_proof_provider =
		GrandpaFinalityProofProvider::new_for_service(backend.clone(), client.clone());

	let (network, network_status_sinks, system_rpc_tx, network_starter) =
		service::build_network(service::BuildNetworkParams {
			config: &config,
			client: client.clone(),
			transaction_pool: transaction_pool.clone(),
			spawn_handle: task_manager.spawn_handle(),
			import_queue,
			on_demand: Some(on_demand.clone()),
			block_announce_validator_builder: None,
			finality_proof_request_builder: Some(finality_proof_request_builder),
			finality_proof_provider: Some(finality_proof_provider),
		})?;

	if config.offchain_worker.enabled {
		service::build_offchain_workers(
			&config, backend.clone(), task_manager.spawn_handle(), client.clone(), network.clone(),
		);
	}

	let light_deps = polkadot_rpc::LightDeps {
		remote_blockchain: backend.remote_blockchain(),
		fetcher: on_demand.clone(),
		client: client.clone(),
		pool: transaction_pool.clone(),
	};

	let rpc_extensions = polkadot_rpc::create_light(light_deps);

	let rpc_handlers = service::spawn_tasks(service::SpawnTasksParams {
		on_demand: Some(on_demand),
		remote_blockchain: Some(backend.remote_blockchain()),
		rpc_extensions_builder: Box::new(service::NoopRpcExtensionBuilder(rpc_extensions)),
		task_manager: &mut task_manager,
		telemetry_connection_sinks: service::TelemetryConnectionSinks::default(),
		config, keystore, backend, transaction_pool, client, network, network_status_sinks,
		system_rpc_tx,
	})?;

	network_starter.start_network();

	Ok((task_manager, rpc_handlers))
}

/// Builds a new object suitable for chain operations.
#[cfg(feature = "full-node")]
pub fn new_chain_ops<Runtime, Dispatch>(mut config: Configuration) -> Result<
	(
		Arc<FullClient<Runtime, Dispatch>>,
		Arc<FullBackend>,
		consensus_common::import_queue::BasicQueue<Block, PrefixedMemoryDB<BlakeTwo256>>,
		TaskManager,
	),
	ServiceError
>
where
	Runtime: ConstructRuntimeApi<Block, FullClient<Runtime, Dispatch>> + Send + Sync + 'static,
	Runtime::RuntimeApi:
	RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
	Dispatch: NativeExecutionDispatch + 'static,
{
	config.keystore = service::config::KeystoreConfig::InMemory;
	let service::PartialComponents { client, backend, import_queue, task_manager, .. }
		= new_partial::<Runtime, Dispatch>(&mut config, false)?;
	Ok((client, backend, import_queue, task_manager))
}

/// Create a new Polkadot service for a full node.
#[cfg(feature = "full-node")]
pub fn polkadot_new_full(
	config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
)
	-> Result<(
		TaskManager,
		Arc<impl AbstractClient<Block, FullBackend>>,
		FullNodeHandles,
	), ServiceError>
{
	let (service, client, handles, _, _) = new_full::<polkadot_runtime::RuntimeApi, PolkadotExecutor>(
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		grandpa_pause,
		false,
	)?;

	Ok((service, client, handles))
}

/// Create a new Kusama service for a full node.
#[cfg(feature = "full-node")]
pub fn kusama_new_full(
	config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
) -> Result<(
		TaskManager,
		Arc<impl AbstractClient<Block, FullBackend>>,
		FullNodeHandles
	), ServiceError>
{
	let (service, client, handles, _, _) = new_full::<kusama_runtime::RuntimeApi, KusamaExecutor>(
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		grandpa_pause,
		false,
	)?;

	Ok((service, client, handles))
}

/// Create a new Westend service for a full node.
#[cfg(feature = "full-node")]
pub fn westend_new_full(
	config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
)
	-> Result<(
		TaskManager,
		Arc<impl AbstractClient<Block, FullBackend>>,
		FullNodeHandles,
	), ServiceError>
{
	let (service, client, handles, _, _) = new_full::<westend_runtime::RuntimeApi, WestendExecutor>(
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		grandpa_pause,
		false,
	)?;

	Ok((service, client, handles))
}

/// Create a new Rococo service for a full node.
#[cfg(feature = "full-node")]
pub fn rococo_new_full(
	config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
)
	-> Result<(
		TaskManager,
		Arc<impl AbstractClient<Block, TFullBackend<Block>>>,
		FullNodeHandles,
	), ServiceError>
{
	let (service, client, handles, _, _) = new_full::<rococo_runtime::RuntimeApi, RococoExecutor>(
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		grandpa_pause,
		false,
	)?;

	Ok((service, client, handles))
}

/// Handles to other sub-services that full nodes instantiate, which consumers
/// of the node may use.
#[cfg(feature = "full-node")]
#[derive(Default)]
pub struct FullNodeHandles {
	/// A handle to the Polkadot networking protocol.
	pub polkadot_network: Option<network_protocol::Service>,
	/// A handle to the validation service.
	pub validation_service_handle: Option<consensus::ServiceHandle>,
}

/// Build a new light node.
pub fn build_light(config: Configuration) -> Result<(TaskManager, Arc<RpcHandlers>), ServiceError> {
	if config.chain_spec.is_kusama() {
		new_light::<kusama_runtime::RuntimeApi, KusamaExecutor>(config)
	} else if config.chain_spec.is_westend() {
		new_light::<westend_runtime::RuntimeApi, WestendExecutor>(config)
	} else if config.chain_spec.is_rococo() {
		new_light::<rococo_runtime::RuntimeApi, RococoExecutor>(config)
	} else {
		new_light::<polkadot_runtime::RuntimeApi, PolkadotExecutor>(config)
	}
}

/// Build a new full node.
#[cfg(feature = "full-node")]
pub fn build_full(
	config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
) -> Result<(TaskManager, Client, FullNodeHandles), ServiceError> {
	if config.chain_spec.is_kusama() {
		new_full::<kusama_runtime::RuntimeApi, KusamaExecutor>(
			config,
			collating_for,
			max_block_data_size,
			authority_discovery_enabled,
			slot_duration,
			grandpa_pause,
			false,
		).map(|(task_manager, client, handles, _, _)| (task_manager, Client::Kusama(client), handles))
	} else if config.chain_spec.is_westend() {
		new_full::<westend_runtime::RuntimeApi, WestendExecutor>(
			config,
			collating_for,
			max_block_data_size,
			authority_discovery_enabled,
			slot_duration,
			grandpa_pause,
			false,
		).map(|(task_manager, client, handles, _, _)| (task_manager, Client::Westend(client), handles))
	} else if config.chain_spec.is_rococo() {
		new_full::<rococo_runtime::RuntimeApi, RococoExecutor>(
			config,
			collating_for,
			max_block_data_size,
			authority_discovery_enabled,
			slot_duration,
			grandpa_pause,
			false,
		).map(|(task_manager, client, handles, _, _)| (task_manager, Client::Rococo(client), handles))
	} else {
		new_full::<polkadot_runtime::RuntimeApi, PolkadotExecutor>(
			config,
			collating_for,
			max_block_data_size,
			authority_discovery_enabled,
			slot_duration,
			grandpa_pause,
			false,
		).map(|(task_manager, client, handles, _, _)| (task_manager, Client::Polkadot(client), handles))
	}
}
