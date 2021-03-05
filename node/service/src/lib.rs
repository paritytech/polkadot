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

#![deny(unused_results)]

pub mod chain_spec;
mod grandpa_support;
mod client;

#[cfg(feature = "full-node")]
use {
	std::convert::TryInto,
	std::time::Duration,
	tracing::info,
	polkadot_node_core_av_store::Config as AvailabilityConfig,
	polkadot_node_core_av_store::Error as AvailabilityError,
	polkadot_node_core_approval_voting::Config as ApprovalVotingConfig,
	polkadot_node_core_proposer::ProposerFactory,
	polkadot_overseer::{AllSubsystems, BlockInfo, Overseer, OverseerHandler},
	polkadot_primitives::v1::ParachainHost,
	sc_authority_discovery::Service as AuthorityDiscoveryService,
	sp_blockchain::HeaderBackend,
	sp_trie::PrefixedMemoryDB,
	sc_client_api::{AuxStore, ExecutorProvider},
	sc_keystore::LocalKeystore,
	babe_primitives::BabeApi,
	grandpa::{self, FinalityProofProvider as GrandpaFinalityProofProvider},
};
#[cfg(feature = "real-overseer")]
use polkadot_network_bridge::RequestMultiplexer;

use sp_core::traits::SpawnNamed;


use polkadot_subsystem::jaeger;

use std::sync::Arc;

use prometheus_endpoint::Registry;
use sc_executor::native_executor_instance;
use service::RpcHandlers;
use telemetry::{TelemetryConnectionNotifier, TelemetrySpan};

pub use self::client::{AbstractClient, Client, ClientHandle, ExecuteWithClient, RuntimeApiCollection};
pub use chain_spec::{PolkadotChainSpec, KusamaChainSpec, WestendChainSpec, RococoChainSpec};
pub use consensus_common::{Proposal, SelectChain, BlockImport, block_validation::Chain};
pub use polkadot_parachain::wasm_executor::IsolationStrategy;
pub use polkadot_primitives::v1::{Block, BlockId, CollatorId, Hash, Id as ParaId};
pub use sc_client_api::{Backend, ExecutionStrategy, CallExecutor};
pub use sc_consensus::LongestChain;
pub use sc_executor::NativeExecutionDispatch;
pub use service::{
	Role, PruningMode, TransactionPoolOptions, Error as SubstrateServiceError, RuntimeGenesis,
	TFullClient, TLightClient, TFullBackend, TLightBackend, TFullCallExecutor, TLightCallExecutor,
	Configuration, ChainSpec, TaskManager,
};
pub use service::config::{DatabaseConfig, PrometheusConfig};
pub use sp_api::{ApiRef, Core as CoreApi, ConstructRuntimeApi, ProvideRuntimeApi, StateBackend};
pub use sp_runtime::traits::{DigestFor, HashFor, NumberFor, Block as BlockT, self as runtime_traits, BlakeTwo256};

pub use kusama_runtime;
pub use polkadot_runtime;
pub use rococo_runtime;
pub use westend_runtime;

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

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error(transparent)]
	Io(#[from] std::io::Error),

	#[error(transparent)]
	AddrFormatInvalid(#[from] std::net::AddrParseError),

	#[error(transparent)]
	Sub(#[from] SubstrateServiceError),

	#[error(transparent)]
	Blockchain(#[from] sp_blockchain::Error),

	#[error(transparent)]
	Consensus(#[from] consensus_common::Error),

	#[error("Failed to create an overseer")]
	Overseer(#[from] polkadot_overseer::SubsystemError),

	#[error(transparent)]
	Prometheus(#[from] prometheus_endpoint::PrometheusError),

	#[error(transparent)]
	Jaeger(#[from] polkadot_subsystem::jaeger::JaegerError),

	#[cfg(feature = "full-node")]
	#[error(transparent)]
	Availability(#[from] AvailabilityError),

	#[error("Authorities require the real overseer implementation")]
	AuthoritiesRequireRealOverseer,

	#[cfg(feature = "full-node")]
	#[error("Creating a custom database is required for validators")]
	DatabasePathRequired,
}

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
		self.id().starts_with("rococo") || self.id().starts_with("rco")
	}
}

// If we're using prometheus, use a registry with a prefix of `polkadot`.
fn set_prometheus_registry(config: &mut Configuration) -> Result<(), Error> {
	if let Some(PrometheusConfig { registry, .. }) = config.prometheus_config.as_mut() {
		*registry = Registry::new_custom(Some("polkadot".into()), None)?;
	}

	Ok(())
}

/// Initialize the `Jeager` collector. The destination must listen
/// on the given address and port for `UDP` packets.
fn jaeger_launch_collector_with_agent(spawner: impl SpawnNamed, config: &Configuration, agent: Option<std::net::SocketAddr>) -> Result<(), Error> {
	if let Some(agent) = agent {
		let cfg = jaeger::JaegerConfig::builder()
			.agent(agent)
			.named(&config.network.node_name)
			.build();

		jaeger::Jaeger::new(cfg).launch(spawner)?;
	}
	Ok(())
}

pub type FullBackend = service::TFullBackend<Block>;
#[cfg(feature = "full-node")]
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;
pub type FullClient<RuntimeApi, Executor> = service::TFullClient<Block, RuntimeApi, Executor>;
#[cfg(feature = "full-node")]
type FullGrandpaBlockImport<RuntimeApi, Executor> = grandpa::GrandpaBlockImport<
	FullBackend, Block, FullClient<RuntimeApi, Executor>, FullSelectChain
>;

type LightBackend = service::TLightBackendWithHash<Block, sp_runtime::traits::BlakeTwo256>;

type LightClient<RuntimeApi, Executor> =
	service::TLightClientWithBackend<Block, RuntimeApi, Executor, LightBackend>;

#[cfg(feature = "full-node")]
fn new_partial<RuntimeApi, Executor>(config: &mut Configuration, jaeger_agent: Option<std::net::SocketAddr>) -> Result<
	service::PartialComponents<
		FullClient<RuntimeApi, Executor>, FullBackend, FullSelectChain,
		consensus_common::DefaultImportQueue<Block, FullClient<RuntimeApi, Executor>>,
		sc_transaction_pool::FullPool<Block, FullClient<RuntimeApi, Executor>>,
		(
			impl Fn(
				polkadot_rpc::DenyUnsafe,
				polkadot_rpc::SubscriptionTaskExecutor,
			) -> polkadot_rpc::RpcExtension,
			(
				babe::BabeBlockImport<
					Block, FullClient<RuntimeApi, Executor>, FullGrandpaBlockImport<RuntimeApi, Executor>
				>,
				grandpa::LinkHalf<Block, FullClient<RuntimeApi, Executor>, FullSelectChain>,
				babe::BabeLink<Block>
			),
			grandpa::SharedVoterState,
			u64, // slot-duration
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
	set_prometheus_registry(config)?;


	let inherent_data_providers = inherents::InherentDataProviders::new();

	let (client, backend, keystore_container, task_manager) =
		service::new_full_parts::<Block, RuntimeApi, Executor>(&config)?;
	let client = Arc::new(client);

	jaeger_launch_collector_with_agent(task_manager.spawn_handle(), &*config, jaeger_agent)?;

	let select_chain = sc_consensus::LongestChain::new(backend.clone());

	let transaction_pool = sc_transaction_pool::BasicPool::new_full(
		config.transaction_pool.clone(),
		config.role.is_authority().into(),
		config.prometheus_registry(),
		task_manager.spawn_handle(),
		client.clone(),
	);

	let grandpa_hard_forks = if config.chain_spec.is_kusama() {
		grandpa_support::kusama_hard_forks()
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

	let babe_config = babe::Config::get_or_compute(&*client)?;
	let (block_import, babe_link) = babe::block_import(
		babe_config.clone(),
		grandpa_block_import,
		client.clone(),
	)?;

	let import_queue = babe::import_queue(
		babe_link.clone(),
		block_import.clone(),
		Some(Box::new(justification_import)),
		client.clone(),
		select_chain.clone(),
		inherent_data_providers.clone(),
		&task_manager.spawn_essential_handle(),
		config.prometheus_registry(),
		consensus_common::CanAuthorWithNativeVersion::new(client.executor().clone()),
	)?;

	let justification_stream = grandpa_link.justification_stream();
	let shared_authority_set = grandpa_link.shared_authority_set().clone();
	let shared_voter_state = grandpa::SharedVoterState::empty();
	let finality_proof_provider = GrandpaFinalityProofProvider::new_for_service(
		backend.clone(),
		Some(shared_authority_set.clone()),
	);

	let import_setup = (block_import.clone(), grandpa_link, babe_link.clone());
	let rpc_setup = shared_voter_state.clone();

	let shared_epoch_changes = babe_link.epoch_changes().clone();
	let slot_duration = babe_config.slot_duration();

	let rpc_extensions_builder = {
		let client = client.clone();
		let keystore = keystore_container.sync_keystore();
		let transaction_pool = transaction_pool.clone();
		let select_chain = select_chain.clone();
		let chain_spec = config.chain_spec.cloned_box();

		move |deny_unsafe, subscription_executor| -> polkadot_rpc::RpcExtension {
			let deps = polkadot_rpc::FullDeps {
				client: client.clone(),
				pool: transaction_pool.clone(),
				select_chain: select_chain.clone(),
				chain_spec: chain_spec.cloned_box(),
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
					subscription_executor,
					finality_provider: finality_proof_provider.clone(),
				},
			};

			polkadot_rpc::create_full(deps)
		}
	};

	Ok(service::PartialComponents {
		client,
		backend,
		task_manager,
		keystore_container,
		select_chain,
		import_queue,
		transaction_pool,
		inherent_data_providers,
		other: (rpc_extensions_builder, import_setup, rpc_setup, slot_duration)
	})
}

#[cfg(all(feature="full-node", not(feature = "real-overseer")))]
fn real_overseer<Spawner, RuntimeClient>(
	leaves: impl IntoIterator<Item = BlockInfo>,
	_: Arc<LocalKeystore>,
	_: Arc<RuntimeClient>,
	_: AvailabilityConfig,
	_: Arc<sc_network::NetworkService<Block, Hash>>,
	_: AuthorityDiscoveryService,
	_request_multiplexer: (),
	registry: Option<&Registry>,
	spawner: Spawner,
	_: IsCollator,
	_: IsolationStrategy,
	_: ApprovalVotingConfig,
) -> Result<(Overseer<Spawner>, OverseerHandler), Error>
where
	RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
	RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block>,
	Spawner: 'static + SpawnNamed + Clone + Unpin,
{
	Overseer::new(
		leaves,
		AllSubsystems::<()>::dummy(),
		registry,
		spawner,
	).map_err(|e| e.into())
}

#[cfg(all(feature = "full-node", feature = "real-overseer"))]
fn real_overseer<Spawner, RuntimeClient>(
	leaves: impl IntoIterator<Item = BlockInfo>,
	keystore: Arc<LocalKeystore>,
	runtime_client: Arc<RuntimeClient>,
	availability_config: AvailabilityConfig,
	network_service: Arc<sc_network::NetworkService<Block, Hash>>,
	authority_discovery: AuthorityDiscoveryService,
	request_multiplexer: RequestMultiplexer,
	registry: Option<&Registry>,
	spawner: Spawner,
	is_collator: IsCollator,
	isolation_strategy: IsolationStrategy,
	approval_voting_config: ApprovalVotingConfig,
) -> Result<(Overseer<Spawner>, OverseerHandler), Error>
where
	RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
	RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block>,
	Spawner: 'static + SpawnNamed + Clone + Unpin,
{
	use polkadot_node_subsystem_util::metrics::Metrics;

	use polkadot_availability_distribution::AvailabilityDistributionSubsystem;
	use polkadot_node_core_av_store::AvailabilityStoreSubsystem;
	use polkadot_availability_bitfield_distribution::BitfieldDistribution as BitfieldDistributionSubsystem;
	use polkadot_node_core_bitfield_signing::BitfieldSigningSubsystem;
	use polkadot_node_core_backing::CandidateBackingSubsystem;
	use polkadot_node_core_candidate_selection::CandidateSelectionSubsystem;
	use polkadot_node_core_candidate_validation::CandidateValidationSubsystem;
	use polkadot_node_core_chain_api::ChainApiSubsystem;
	use polkadot_node_collation_generation::CollationGenerationSubsystem;
	use polkadot_collator_protocol::{CollatorProtocolSubsystem, ProtocolSide};
	use polkadot_network_bridge::NetworkBridge as NetworkBridgeSubsystem;
	use polkadot_pov_distribution::PoVDistribution as PoVDistributionSubsystem;
	use polkadot_node_core_provisioner::ProvisioningSubsystem as ProvisionerSubsystem;
	use polkadot_node_core_runtime_api::RuntimeApiSubsystem;
	use polkadot_statement_distribution::StatementDistribution as StatementDistributionSubsystem;
	use polkadot_availability_recovery::AvailabilityRecoverySubsystem;
	use polkadot_approval_distribution::ApprovalDistribution as ApprovalDistributionSubsystem;
	use polkadot_node_core_approval_voting::ApprovalVotingSubsystem;
	use polkadot_gossip_support::GossipSupport as GossipSupportSubsystem;

	let all_subsystems = AllSubsystems {
		availability_distribution: AvailabilityDistributionSubsystem::new(
			keystore.clone(),
			Metrics::register(registry)?,
		),
		availability_recovery: AvailabilityRecoverySubsystem::with_chunks_only(
		),
		availability_store: AvailabilityStoreSubsystem::new_on_disk(
			availability_config,
			Metrics::register(registry)?,
		)?,
		bitfield_distribution: BitfieldDistributionSubsystem::new(
			Metrics::register(registry)?,
		),
		bitfield_signing: BitfieldSigningSubsystem::new(
			spawner.clone(),
			keystore.clone(),
			Metrics::register(registry)?,
		),
		candidate_backing: CandidateBackingSubsystem::new(
			spawner.clone(),
			keystore.clone(),
			Metrics::register(registry)?,
		),
		candidate_selection: CandidateSelectionSubsystem::new(
			spawner.clone(),
			keystore.clone(),
			Metrics::register(registry)?,
		),
		candidate_validation: CandidateValidationSubsystem::new(
			spawner.clone(),
			Metrics::register(registry)?,
			isolation_strategy,
		),
		chain_api: ChainApiSubsystem::new(
			runtime_client.clone(),
			Metrics::register(registry)?,
		),
		collation_generation: CollationGenerationSubsystem::new(
			Metrics::register(registry)?,
		),
		collator_protocol: {
			let side = match is_collator {
				IsCollator::Yes(id) => ProtocolSide::Collator(id, Metrics::register(registry)?),
				IsCollator::No => ProtocolSide::Validator(Metrics::register(registry)?),
			};
			CollatorProtocolSubsystem::new(
				side,
			)
		},
		network_bridge: NetworkBridgeSubsystem::new(
			network_service,
			authority_discovery,
			request_multiplexer,
		),
		pov_distribution: PoVDistributionSubsystem::new(
			Metrics::register(registry)?,
		),
		provisioner: ProvisionerSubsystem::new(
			spawner.clone(),
			(),
			Metrics::register(registry)?,
		),
		runtime_api: RuntimeApiSubsystem::new(
			runtime_client.clone(),
			Metrics::register(registry)?,
			spawner.clone(),
		),
		statement_distribution: StatementDistributionSubsystem::new(
			Metrics::register(registry)?,
		),
		approval_distribution: ApprovalDistributionSubsystem::new(
			Metrics::register(registry)?,
		),
		approval_voting: ApprovalVotingSubsystem::with_config(
			approval_voting_config,
			keystore.clone(),
		)?,
		gossip_support: GossipSupportSubsystem::new(),
	};

	Overseer::new(
		leaves,
		all_subsystems,
		registry,
		spawner,
	).map_err(|e| e.into())
}

#[cfg(feature = "full-node")]
pub struct NewFull<C> {
	pub task_manager: TaskManager,
	pub client: C,
	pub overseer_handler: Option<OverseerHandler>,
	pub network: Arc<sc_network::NetworkService<Block, <Block as BlockT>::Hash>>,
	pub network_status_sinks: service::NetworkStatusSinks<Block>,
	pub rpc_handlers: RpcHandlers,
	pub backend: Arc<FullBackend>,
}

#[cfg(feature = "full-node")]
impl<C> NewFull<C> {
	/// Convert the client type using the given `func`.
	pub fn with_client<NC>(self, func: impl FnOnce(C) -> NC) -> NewFull<NC> {
		NewFull {
			client: func(self.client),
			task_manager: self.task_manager,
			overseer_handler: self.overseer_handler,
			network: self.network,
			network_status_sinks: self.network_status_sinks,
			rpc_handlers: self.rpc_handlers,
			backend: self.backend,
		}
	}
}

/// Is this node a collator?
#[cfg(feature = "full-node")]
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum IsCollator {
	/// This node is a collator.
	Yes(CollatorId),
	/// This node is not a collator.
	No,
}

#[cfg(feature = "full-node")]
impl IsCollator {
	/// Is this a collator?
	fn is_collator(&self) -> bool {
		matches!(self, Self::Yes(_))
	}
}

/// Create a new full node of arbitrary runtime and executor.
///
/// This is an advanced feature and not recommended for general use. Generally, `build_full` is
/// a better choice.
#[cfg(feature = "full-node")]
pub fn new_full<RuntimeApi, Executor>(
	mut config: Configuration,
	is_collator: IsCollator,
	grandpa_pause: Option<(u32, u32)>,
	jaeger_agent: Option<std::net::SocketAddr>,
	isolation_strategy: IsolationStrategy,
) -> Result<NewFull<Arc<FullClient<RuntimeApi, Executor>>>, Error>
	where
		RuntimeApi: ConstructRuntimeApi<Block, FullClient<RuntimeApi, Executor>> + Send + Sync + 'static,
		RuntimeApi::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
		Executor: NativeExecutionDispatch + 'static,
{
	#[cfg(feature = "real-overseer")]
	info!("real-overseer feature is ENABLED");

	let telemetry_span = TelemetrySpan::new();
	let _telemetry_span_entered = telemetry_span.enter();

	let role = config.role.clone();
	let force_authoring = config.force_authoring;
	let backoff_authoring_blocks =
		Some(sc_consensus_slots::BackoffAuthoringOnFinalizedHeadLagging {
			#[cfg(feature = "real-overseer")]
			unfinalized_slack: 100,
			..Default::default()
		});

	let disable_grandpa = config.disable_grandpa;
	let name = config.network.node_name.clone();

	let service::PartialComponents {
		client,
		backend,
		mut task_manager,
		keystore_container,
		select_chain,
		import_queue,
		transaction_pool,
		inherent_data_providers,
		other: (rpc_extensions_builder, import_setup, rpc_setup, slot_duration)
	} = new_partial::<RuntimeApi, Executor>(&mut config, jaeger_agent)?;

	let prometheus_registry = config.prometheus_registry().cloned();

	let shared_voter_state = rpc_setup;

	// Note: GrandPa is pushed before the Polkadot-specific protocols. This doesn't change
	// anything in terms of behaviour, but makes the logs more consistent with the other
	// Substrate nodes.
	config.network.extra_sets.push(grandpa::grandpa_peers_set_config());
	#[cfg(feature = "real-overseer")]
	config.network.extra_sets.extend(polkadot_network_bridge::peer_sets_info());

	// TODO: At the moment, the collator protocol uses notifications protocols to download
	// collations. Because of DoS-protection measures, notifications protocols have a very limited
	// bandwidth capacity, resulting in the collation download taking a long time.
	// The lines of code below considerably relaxes this DoS protection in order to circumvent
	// this problem. This configuraiton change should preferably not reach any live network, and
	// should be removed once the collation protocol is finished.
	// Tracking issue: https://github.com/paritytech/polkadot/issues/2283
	#[cfg(feature = "real-overseer")]
	fn adjust_yamux(cfg: &mut sc_network::config::NetworkConfiguration) {
		cfg.yamux_window_size = Some(5 * 1024 * 1024);
	}
	#[cfg(not(feature = "real-overseer"))]
	fn adjust_yamux(_: &mut sc_network::config::NetworkConfiguration) {}
	adjust_yamux(&mut config.network);

	config.network.request_response_protocols.push(sc_finality_grandpa_warp_sync::request_response_config_for_chain(
		&config, task_manager.spawn_handle(), backend.clone(), import_setup.1.shared_authority_set().clone(),
	));
	#[cfg(feature = "real-overseer")]
	fn register_request_response(config: &mut sc_network::config::NetworkConfiguration) -> RequestMultiplexer {
		let (multiplexer, configs) = RequestMultiplexer::new();
		config.request_response_protocols.extend(configs);
		multiplexer
	}
	#[cfg(not(feature = "real-overseer"))]
	fn register_request_response(_: &mut sc_network::config::NetworkConfiguration) {}
	let request_multiplexer = register_request_response(&mut config.network);

	let (network, network_status_sinks, system_rpc_tx, network_starter) =
		service::build_network(service::BuildNetworkParams {
			config: &config,
			client: client.clone(),
			transaction_pool: transaction_pool.clone(),
			spawn_handle: task_manager.spawn_handle(),
			import_queue,
			on_demand: None,
			block_announce_validator_builder: None,
		})?;

	if config.offchain_worker.enabled {
		let _ = service::build_offchain_workers(
			&config, backend.clone(), task_manager.spawn_handle(), client.clone(), network.clone(),
		);
	}

	let availability_config = config.database.clone().try_into().map_err(Error::Availability)?;

	let approval_voting_config = ApprovalVotingConfig {
		path: config.database.path()
			.ok_or(Error::DatabasePathRequired)?
			.join("parachains").join("approval-voting"),
		slot_duration_millis: slot_duration,
		cache_size: None, // default is fine.
	};

	let (rpc_handlers, telemetry_connection_notifier) = service::spawn_tasks(service::SpawnTasksParams {
		config,
		backend: backend.clone(),
		client: client.clone(),
		keystore: keystore_container.sync_keystore(),
		network: network.clone(),
		rpc_extensions_builder: Box::new(rpc_extensions_builder),
		transaction_pool: transaction_pool.clone(),
		task_manager: &mut task_manager,
		on_demand: None,
		remote_blockchain: None,
		network_status_sinks: network_status_sinks.clone(),
		system_rpc_tx,
		telemetry_span: Some(telemetry_span.clone()),
	})?;

	let (block_import, link_half, babe_link) = import_setup;

	let overseer_client = client.clone();
	let spawner = task_manager.spawn_handle();
	let leaves: Vec<_> = select_chain.clone()
		.leaves()
		.unwrap_or_else(|_| vec![])
		.into_iter()
		.filter_map(|hash| {
			let number = client.number(hash).ok()??;
			let parent_hash = client.header(&BlockId::Hash(hash)).ok()??.parent_hash;

			Some(BlockInfo {
				hash,
				parent_hash,
				number,
			})
		})
		.collect();

	let authority_discovery_service = if role.is_authority() || is_collator.is_collator() {
		use sc_network::Event;
		use futures::StreamExt;

		let authority_discovery_role = if role.is_authority() {
			sc_authority_discovery::Role::PublishAndDiscover(
				keystore_container.keystore(),
			)
		} else {
			// don't publish our addresses when we're only a collator
			sc_authority_discovery::Role::Discover
		};
		let dht_event_stream = network.event_stream("authority-discovery")
			.filter_map(|e| async move { match e {
				Event::Dht(e) => Some(e),
				_ => None,
			}});
		let (worker, service) = sc_authority_discovery::new_worker_and_service(
			client.clone(),
			network.clone(),
			Box::pin(dht_event_stream),
			authority_discovery_role,
			prometheus_registry.clone(),
		);

		task_manager.spawn_handle().spawn("authority-discovery-worker", worker.run());
		Some(service)
	} else {
		None
	};

	// we'd say let overseer_handler = authority_discovery_service.map(|authority_discovery_service|, ...),
	// but in that case we couldn't use ? to propagate errors
	let local_keystore = keystore_container.local_keystore();
	if local_keystore.is_none() {
		tracing::info!("Cannot run as validator without local keystore.");
	}

	let maybe_params = local_keystore
		.and_then(move |k| authority_discovery_service.map(|a| (a, k)));

	let overseer_handler = if let Some((authority_discovery_service, keystore)) = maybe_params {
		let (overseer, overseer_handler) = real_overseer(
			leaves,
			keystore,
			overseer_client.clone(),
			availability_config,
			network.clone(),
			authority_discovery_service,
			request_multiplexer,
			prometheus_registry.as_ref(),
			spawner,
			is_collator,
			isolation_strategy,
			approval_voting_config,
		)?;
		let overseer_handler_clone = overseer_handler.clone();

		task_manager.spawn_essential_handle().spawn_blocking("overseer", Box::pin(async move {
			use futures::{pin_mut, select, FutureExt};

			let forward = polkadot_overseer::forward_events(overseer_client, overseer_handler_clone);

			let forward = forward.fuse();
			let overseer_fut = overseer.run().fuse();

			pin_mut!(overseer_fut);
			pin_mut!(forward);

			select! {
				_ = forward => (),
				_ = overseer_fut => (),
				complete => (),
			}
		}));

		Some(overseer_handler)
	} else { None };

	if role.is_authority() {
		let can_author_with =
			consensus_common::CanAuthorWithNativeVersion::new(client.executor().clone());

		let proposer = ProposerFactory::new(
			task_manager.spawn_handle(),
			client.clone(),
			transaction_pool,
			overseer_handler.as_ref().ok_or(Error::AuthoritiesRequireRealOverseer)?.clone(),
			prometheus_registry.as_ref(),
		);

		let babe_config = babe::BabeParams {
			keystore: keystore_container.sync_keystore(),
			client: client.clone(),
			select_chain,
			block_import,
			env: proposer,
			sync_oracle: network.clone(),
			inherent_data_providers: inherent_data_providers.clone(),
			force_authoring,
			backoff_authoring_blocks,
			babe_link,
			can_author_with,
		};

		let babe = babe::start_babe(babe_config)?;
		task_manager.spawn_essential_handle().spawn_blocking("babe", babe);
	}

	// if the node isn't actively participating in consensus then it doesn't
	// need a keystore, regardless of which protocol we use below.
	let keystore_opt = if role.is_authority() {
		Some(keystore_container.sync_keystore())
	} else {
		None
	};

	let config = grandpa::Config {
		// FIXME substrate#1578 make this available through chainspec
		gossip_duration: Duration::from_millis(1000),
		justification_period: 512,
		name: Some(name),
		observer_enabled: false,
		keystore: keystore_opt,
		is_authority: role.is_authority(),
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
		let builder = grandpa::VotingRulesBuilder::default();

		#[cfg(feature = "real-overseer")]
		let builder = if let Some(ref overseer) = overseer_handler {
			builder.add(grandpa_support::ApprovalCheckingDiagnostic::new(
				overseer.clone(),
				prometheus_registry.as_ref(),
			)?)
		} else {
			builder
		};

		let voting_rule = match grandpa_pause {
			Some((block, delay)) => {
				info!(
					block_number = %block,
					delay = %delay,
					"GRANDPA scheduled voting pause set for block #{} with a duration of {} blocks.",
					block,
					delay,
				);

				builder
					.add(grandpa_support::PauseAfterBlockFor(block, delay))
					.build()
			}
			None => builder.build(),
		};

		let grandpa_config = grandpa::GrandpaParams {
			config,
			link: link_half,
			network: network.clone(),
			telemetry_on_connect: telemetry_connection_notifier.map(|x| x.on_connect_stream()),
			voting_rule,
			prometheus_registry: prometheus_registry.clone(),
			shared_voter_state,
		};

		task_manager.spawn_essential_handle().spawn_blocking(
			"grandpa-voter",
			grandpa::run_grandpa_voter(grandpa_config)?
		);
	}

	network_starter.start_network();

	Ok(NewFull {
		task_manager,
		client,
		overseer_handler,
		network,
		network_status_sinks,
		rpc_handlers,
		backend,
	})
}

/// Builds a new service for a light client.
fn new_light<Runtime, Dispatch>(mut config: Configuration) -> Result<(
	TaskManager,
	RpcHandlers,
	Option<TelemetryConnectionNotifier>,
), Error>
	where
		Runtime: 'static + Send + Sync + ConstructRuntimeApi<Block, LightClient<Runtime, Dispatch>>,
		<Runtime as ConstructRuntimeApi<Block, LightClient<Runtime, Dispatch>>>::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<LightBackend, Block>>,
		Dispatch: NativeExecutionDispatch + 'static,
{
	set_prometheus_registry(&mut config)?;
	use sc_client_api::backend::RemoteBackend;

	let (client, backend, keystore_container, mut task_manager, on_demand) =
		service::new_light_parts::<Block, Runtime, Dispatch>(&config)?;

	let select_chain = sc_consensus::LongestChain::new(backend.clone());

	let transaction_pool = Arc::new(sc_transaction_pool::BasicPool::new_light(
		config.transaction_pool.clone(),
		config.prometheus_registry(),
		task_manager.spawn_handle(),
		client.clone(),
		on_demand.clone(),
	));

	let (grandpa_block_import, _) = grandpa::block_import(
		client.clone(),
		&(client.clone() as Arc<_>),
		select_chain.clone(),
	)?;
	let justification_import = grandpa_block_import.clone();

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
		Some(Box::new(justification_import)),
		client.clone(),
		select_chain.clone(),
		inherent_data_providers.clone(),
		&task_manager.spawn_essential_handle(),
		config.prometheus_registry(),
		consensus_common::NeverCanAuthor,
	)?;

	let (network, network_status_sinks, system_rpc_tx, network_starter) =
		service::build_network(service::BuildNetworkParams {
			config: &config,
			client: client.clone(),
			transaction_pool: transaction_pool.clone(),
			spawn_handle: task_manager.spawn_handle(),
			import_queue,
			on_demand: Some(on_demand.clone()),
			block_announce_validator_builder: None,
		})?;

	if config.offchain_worker.enabled {
		let _ = service::build_offchain_workers(
			&config,
			backend.clone(),
			task_manager.spawn_handle(),
			client.clone(),
			network.clone(),
		);
	}

	let light_deps = polkadot_rpc::LightDeps {
		remote_blockchain: backend.remote_blockchain(),
		fetcher: on_demand.clone(),
		client: client.clone(),
		pool: transaction_pool.clone(),
	};

	let rpc_extensions = polkadot_rpc::create_light(light_deps);

	let telemetry_span = TelemetrySpan::new();
	let _telemetry_span_entered = telemetry_span.enter();

	let (rpc_handlers, telemetry_connection_notifier) = service::spawn_tasks(service::SpawnTasksParams {
		on_demand: Some(on_demand),
		remote_blockchain: Some(backend.remote_blockchain()),
		rpc_extensions_builder: Box::new(service::NoopRpcExtensionBuilder(rpc_extensions)),
		task_manager: &mut task_manager,
		config,
		keystore: keystore_container.sync_keystore(),
		backend,
		transaction_pool,
		client,
		network,
		network_status_sinks,
		system_rpc_tx,
		telemetry_span: Some(telemetry_span.clone()),
	})?;

	network_starter.start_network();

	Ok((task_manager, rpc_handlers, telemetry_connection_notifier))
}

/// Builds a new object suitable for chain operations.
#[cfg(feature = "full-node")]
pub fn new_chain_ops(mut config: &mut Configuration, jaeger_agent: Option<std::net::SocketAddr>) -> Result<
	(
		Arc<Client>,
		Arc<FullBackend>,
		consensus_common::import_queue::BasicQueue<Block, PrefixedMemoryDB<BlakeTwo256>>,
		TaskManager,
	),
	Error
>
{
	config.keystore = service::config::KeystoreConfig::InMemory;
	if config.chain_spec.is_rococo() {
		let service::PartialComponents { client, backend, import_queue, task_manager, .. }
			= new_partial::<rococo_runtime::RuntimeApi, RococoExecutor>(config, jaeger_agent)?;
		Ok((Arc::new(Client::Rococo(client)), backend, import_queue, task_manager))
	} else if config.chain_spec.is_kusama() {
		let service::PartialComponents { client, backend, import_queue, task_manager, .. }
			= new_partial::<kusama_runtime::RuntimeApi, KusamaExecutor>(config, jaeger_agent)?;
		Ok((Arc::new(Client::Kusama(client)), backend, import_queue, task_manager))
	} else if config.chain_spec.is_westend() {
		let service::PartialComponents { client, backend, import_queue, task_manager, .. }
			= new_partial::<westend_runtime::RuntimeApi, WestendExecutor>(config, jaeger_agent)?;
		Ok((Arc::new(Client::Westend(client)), backend, import_queue, task_manager))
	} else {
		let service::PartialComponents { client, backend, import_queue, task_manager, .. }
			= new_partial::<polkadot_runtime::RuntimeApi, PolkadotExecutor>(config, jaeger_agent)?;
		Ok((Arc::new(Client::Polkadot(client)), backend, import_queue, task_manager))
	}
}

/// Build a new light node.
pub fn build_light(config: Configuration) -> Result<(
	TaskManager,
	RpcHandlers,
	Option<TelemetryConnectionNotifier>,
), Error> {
	if config.chain_spec.is_rococo() {
		new_light::<rococo_runtime::RuntimeApi, RococoExecutor>(config)
	} else if config.chain_spec.is_kusama() {
		new_light::<kusama_runtime::RuntimeApi, KusamaExecutor>(config)
	} else if config.chain_spec.is_westend() {
		new_light::<westend_runtime::RuntimeApi, WestendExecutor>(config)
	} else {
		new_light::<polkadot_runtime::RuntimeApi, PolkadotExecutor>(config)
	}
}

#[cfg(feature = "full-node")]
pub fn build_full(
	config: Configuration,
	is_collator: IsCollator,
	grandpa_pause: Option<(u32, u32)>,
	jaeger_agent: Option<std::net::SocketAddr>,
) -> Result<NewFull<Client>, Error> {
	let isolation_strategy = {
		#[cfg(not(any(target_os = "android", target_os = "unknown")))]
		{
			let cache_base_path = config.database.path();
			IsolationStrategy::external_process_with_caching(cache_base_path)
		}

		#[cfg(any(target_os = "android", target_os = "unknown"))]
		{
			IsolationStrategy::InProcess
		}
	};

	if config.chain_spec.is_rococo() {
		new_full::<rococo_runtime::RuntimeApi, RococoExecutor>(
			config,
			is_collator,
			grandpa_pause,
			jaeger_agent,
			isolation_strategy,
		).map(|full| full.with_client(Client::Rococo))
	} else if config.chain_spec.is_kusama() {
		new_full::<kusama_runtime::RuntimeApi, KusamaExecutor>(
			config,
			is_collator,
			grandpa_pause,
			jaeger_agent,
			isolation_strategy,
		).map(|full| full.with_client(Client::Kusama))
	} else if config.chain_spec.is_westend() {
		new_full::<westend_runtime::RuntimeApi, WestendExecutor>(
			config,
			is_collator,
			grandpa_pause,
			jaeger_agent,
			isolation_strategy,
		).map(|full| full.with_client(Client::Westend))
	} else {
		new_full::<polkadot_runtime::RuntimeApi, PolkadotExecutor>(
			config,
			is_collator,
			grandpa_pause,
			jaeger_agent,
			isolation_strategy,
		).map(|full| full.with_client(Client::Polkadot))
	}
}
