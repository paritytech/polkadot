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
mod parachains_db;

#[cfg(feature = "full-node")]
use {
	tracing::info,
	polkadot_network_bridge::RequestMultiplexer,
	polkadot_node_core_av_store::Config as AvailabilityConfig,
	polkadot_node_core_av_store::Error as AvailabilityError,
	polkadot_node_core_approval_voting::Config as ApprovalVotingConfig,
	polkadot_node_core_candidate_validation::Config as CandidateValidationConfig,
	polkadot_overseer::{AllSubsystems, BlockInfo, Overseer, OverseerHandler},
	polkadot_primitives::v1::ParachainHost,
	sc_authority_discovery::Service as AuthorityDiscoveryService,
	sp_authority_discovery::AuthorityDiscoveryApi,
	sp_blockchain::HeaderBackend,
	sp_trie::PrefixedMemoryDB,
	sc_client_api::{AuxStore, ExecutorProvider},
	sc_keystore::LocalKeystore,
	sp_consensus_babe::BabeApi,
	grandpa::{self, FinalityProofProvider as GrandpaFinalityProofProvider},
	beefy_primitives::ecdsa::AuthoritySignature as BeefySignature,
	sp_runtime::traits::Header as HeaderT,
};

use sp_core::traits::SpawnNamed;

use polkadot_subsystem::jaeger;

use std::sync::Arc;
use std::time::Duration;

use prometheus_endpoint::Registry;
use service::RpcHandlers;
use telemetry::{Telemetry, TelemetryWorker, TelemetryWorkerHandle};

#[cfg(feature = "rococo-native")]
pub use polkadot_client::RococoExecutor;

#[cfg(feature = "westend-native")]
pub use polkadot_client::WestendExecutor;

#[cfg(feature = "kusama-native")]
pub use polkadot_client::KusamaExecutor;

pub use polkadot_client::{
	PolkadotExecutor, FullBackend, FullClient, AbstractClient, Client, ClientHandle, ExecuteWithClient,
	RuntimeApiCollection,
};
pub use chain_spec::{PolkadotChainSpec, KusamaChainSpec, WestendChainSpec, RococoChainSpec};
pub use consensus_common::{Proposal, SelectChain, BlockImport, block_validation::Chain};
pub use polkadot_primitives::v1::{Block, BlockId, CollatorPair, Hash, Id as ParaId};
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

#[cfg(feature = "kusama-native")]
pub use kusama_runtime;
pub use polkadot_runtime;
#[cfg(feature = "rococo-native")]
pub use rococo_runtime;
#[cfg(feature = "westend-native")]
pub use westend_runtime;

/// The maximum number of active leaves we forward to the [`Overseer`] on startup.
const MAX_ACTIVE_LEAVES: usize = 4;

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
	Telemetry(#[from] telemetry::Error),

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

/// Can be called for a `Configuration` to identify which network the configuration targets.
pub trait IdentifyVariant {
	/// Returns if this is a configuration for the `Kusama` network.
	fn is_kusama(&self) -> bool;

	/// Returns if this is a configuration for the `Westend` network.
	fn is_westend(&self) -> bool;

	/// Returns if this is a configuration for the `Rococo` network.
	fn is_rococo(&self) -> bool;

	/// Returns if this is a configuration for the `Wococo` test network.
	fn is_wococo(&self) -> bool;

	/// Returns true if this configuration is for a development network.
	fn is_dev(&self) -> bool;
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
	fn is_wococo(&self) -> bool {
		self.id().starts_with("wococo") || self.id().starts_with("wco")
	}
	fn is_dev(&self) -> bool {
		self.id().ends_with("dev")
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

#[cfg(feature = "full-node")]
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;
#[cfg(feature = "full-node")]
type FullGrandpaBlockImport<RuntimeApi, Executor> = grandpa::GrandpaBlockImport<
	FullBackend, Block, FullClient<RuntimeApi, Executor>, FullSelectChain
>;

#[cfg(feature = "light-node")]
type LightBackend = service::TLightBackendWithHash<Block, sp_runtime::traits::BlakeTwo256>;

#[cfg(feature = "light-node")]
type LightClient<RuntimeApi, Executor> =
	service::TLightClientWithBackend<Block, RuntimeApi, Executor, LightBackend>;

#[cfg(feature = "full-node")]
fn new_partial<RuntimeApi, Executor>(
	config: &mut Configuration,
	jaeger_agent: Option<std::net::SocketAddr>,
	telemetry_worker_handle: Option<TelemetryWorkerHandle>,
) -> Result<
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
				babe::BabeLink<Block>,
				beefy_gadget::notification::BeefySignedCommitmentSender<Block, BeefySignature>,
			),
			grandpa::SharedVoterState,
			std::time::Duration, // slot-duration
			Option<Telemetry>,
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


	let telemetry = config.telemetry_endpoints.clone()
		.filter(|x| !x.is_empty())
		.map(move |endpoints| -> Result<_, telemetry::Error> {
			let (worker, mut worker_handle) = if let Some(worker_handle) = telemetry_worker_handle {
				(None, worker_handle)
			} else {
				let worker = TelemetryWorker::new(16)?;
				let worker_handle = worker.handle();
				(Some(worker), worker_handle)
			};
			let telemetry = worker_handle.new_telemetry(endpoints);
			Ok((worker, telemetry))
		})
		.transpose()?;

	let (client, backend, keystore_container, task_manager) =
		service::new_full_parts::<Block, RuntimeApi, Executor>(
			&config,
			telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
		)?;
	let client = Arc::new(client);

	let telemetry = telemetry
		.map(|(worker, telemetry)| {
			if let Some(worker) = worker {
				task_manager.spawn_handle().spawn("telemetry", worker.run());
			}
			telemetry
		});

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
			telemetry.as_ref().map(|x| x.handle()),
		)?;

	let justification_import = grandpa_block_import.clone();

	let babe_config = babe::Config::get_or_compute(&*client)?;
	let (block_import, babe_link) = babe::block_import(
		babe_config.clone(),
		grandpa_block_import,
		client.clone(),
	)?;

	let slot_duration = babe_link.config().slot_duration();
	let import_queue = babe::import_queue(
		babe_link.clone(),
		block_import.clone(),
		Some(Box::new(justification_import)),
		client.clone(),
		select_chain.clone(),
		move |_, ()| async move {
			let timestamp = sp_timestamp::InherentDataProvider::from_system_time();

			let slot =
				sp_consensus_babe::inherents::InherentDataProvider::from_timestamp_and_duration(
					*timestamp,
					slot_duration,
				);

			Ok((timestamp, slot))
		},
		&task_manager.spawn_essential_handle(),
		config.prometheus_registry(),
		consensus_common::CanAuthorWithNativeVersion::new(client.executor().clone()),
		telemetry.as_ref().map(|x| x.handle()),
	)?;

	let (beefy_link, beefy_commitment_stream) =
		beefy_gadget::notification::BeefySignedCommitmentStream::channel();

	let justification_stream = grandpa_link.justification_stream();
	let shared_authority_set = grandpa_link.shared_authority_set().clone();
	let shared_voter_state = grandpa::SharedVoterState::empty();
	let finality_proof_provider = GrandpaFinalityProofProvider::new_for_service(
		backend.clone(),
		Some(shared_authority_set.clone()),
	);

	let import_setup = (block_import.clone(), grandpa_link, babe_link.clone(), beefy_link);
	let rpc_setup = shared_voter_state.clone();

	let shared_epoch_changes = babe_link.epoch_changes().clone();
	let slot_duration = babe_config.slot_duration();

	let rpc_extensions_builder = {
		let client = client.clone();
		let keystore = keystore_container.sync_keystore();
		let transaction_pool = transaction_pool.clone();
		let select_chain = select_chain.clone();
		let chain_spec = config.chain_spec.cloned_box();

		move |deny_unsafe, subscription_executor: polkadot_rpc::SubscriptionTaskExecutor|
			-> polkadot_rpc::RpcExtension
		{
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
					subscription_executor: subscription_executor.clone(),
					finality_provider: finality_proof_provider.clone(),
				},
				beefy: polkadot_rpc::BeefyDeps {
					beefy_commitment_stream: beefy_commitment_stream.clone(),
					subscription_executor,
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
		other: (rpc_extensions_builder, import_setup, rpc_setup, slot_duration, telemetry)
	})
}

#[cfg(feature = "full-node")]
fn real_overseer<Spawner, RuntimeClient>(
	leaves: impl IntoIterator<Item = BlockInfo>,
	keystore: Arc<LocalKeystore>,
	runtime_client: Arc<RuntimeClient>,
	parachains_db: Arc<dyn kvdb::KeyValueDB>,
	availability_config: AvailabilityConfig,
	approval_voting_config: ApprovalVotingConfig,
	network_service: Arc<sc_network::NetworkService<Block, Hash>>,
	authority_discovery: AuthorityDiscoveryService,
	request_multiplexer: RequestMultiplexer,
	registry: Option<&Registry>,
	spawner: Spawner,
	is_collator: IsCollator,
	candidate_validation_config: CandidateValidationConfig,
) -> Result<(Overseer<Spawner, Arc<RuntimeClient>>, OverseerHandler), Error>
where
	RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
	RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
	Spawner: 'static + SpawnNamed + Clone + Unpin,
{
	use polkadot_node_subsystem_util::metrics::Metrics;

	use polkadot_availability_distribution::AvailabilityDistributionSubsystem;
	use polkadot_node_core_av_store::AvailabilityStoreSubsystem;
	use polkadot_availability_bitfield_distribution::BitfieldDistribution as BitfieldDistributionSubsystem;
	use polkadot_node_core_bitfield_signing::BitfieldSigningSubsystem;
	use polkadot_node_core_backing::CandidateBackingSubsystem;
	use polkadot_node_core_candidate_validation::CandidateValidationSubsystem;
	use polkadot_node_core_chain_api::ChainApiSubsystem;
	use polkadot_node_collation_generation::CollationGenerationSubsystem;
	use polkadot_collator_protocol::{CollatorProtocolSubsystem, ProtocolSide};
	use polkadot_network_bridge::NetworkBridge as NetworkBridgeSubsystem;
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
		availability_store: AvailabilityStoreSubsystem::new(
			parachains_db.clone(),
			availability_config,
			Metrics::register(registry)?,
		),
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
		candidate_validation: CandidateValidationSubsystem::with_config(
			candidate_validation_config,
			Metrics::register(registry)?,
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
				IsCollator::Yes(collator_pair) => ProtocolSide::Collator(
					network_service.local_peer_id().clone(),
					collator_pair,
					Metrics::register(registry)?,
				),
				IsCollator::No => ProtocolSide::Validator {
					keystore: keystore.clone(),
					eviction_policy: Default::default(),
					metrics: Metrics::register(registry)?,
				},
			};
			CollatorProtocolSubsystem::new(
				side,
			)
		},
		network_bridge: NetworkBridgeSubsystem::new(
			network_service.clone(),
			authority_discovery,
			request_multiplexer,
			Box::new(network_service.clone()),
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
			keystore.clone(),
			Metrics::register(registry)?,
		),
		approval_distribution: ApprovalDistributionSubsystem::new(
			Metrics::register(registry)?,
		),
		approval_voting: ApprovalVotingSubsystem::with_config(
			approval_voting_config,
			parachains_db,
			keystore.clone(),
			Box::new(network_service.clone()),
			Metrics::register(registry)?,
		),
		gossip_support: GossipSupportSubsystem::new(
			keystore.clone(),
		),
	};

	Overseer::new(
		leaves,
		all_subsystems,
		registry,
		runtime_client.clone(),
		spawner,
	).map_err(|e| e.into())
}

#[cfg(feature = "full-node")]
pub struct NewFull<C> {
	pub task_manager: TaskManager,
	pub client: C,
	pub overseer_handler: Option<OverseerHandler>,
	pub network: Arc<sc_network::NetworkService<Block, <Block as BlockT>::Hash>>,
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
			rpc_handlers: self.rpc_handlers,
			backend: self.backend,
		}
	}
}

/// Is this node a collator?
#[cfg(feature = "full-node")]
#[derive(Clone)]
pub enum IsCollator {
	/// This node is a collator.
	Yes(CollatorPair),
	/// This node is not a collator.
	No,
}

#[cfg(feature = "full-node")]
impl std::fmt::Debug for IsCollator {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		use sp_core::Pair;
		match self {
			IsCollator::Yes(pair) => write!(fmt, "Yes({})", pair.public()),
			IsCollator::No => write!(fmt, "No"),
		}
	}
}

#[cfg(feature = "full-node")]
impl IsCollator {
	/// Is this a collator?
	fn is_collator(&self) -> bool {
		matches!(self, Self::Yes(_))
	}
}

/// Returns the active leaves the overseer should start with.
#[cfg(feature = "full-node")]
fn active_leaves<RuntimeApi, Executor>(
	select_chain: &sc_consensus::LongestChain<FullBackend, Block>,
	client: &FullClient<RuntimeApi, Executor>,
) -> Result<Vec<BlockInfo>, Error>
where
		RuntimeApi: ConstructRuntimeApi<Block, FullClient<RuntimeApi, Executor>> + Send + Sync + 'static,
		RuntimeApi::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
		Executor: NativeExecutionDispatch + 'static,
{
	let best_block = select_chain.best_chain()?;

	let mut leaves = select_chain
		.leaves()
		.unwrap_or_default()
		.into_iter()
		.filter_map(|hash| {
			let number = client.number(hash).ok()??;

			// Only consider leaves that are in maximum an uncle of the best block.
			if number < best_block.number().saturating_sub(1) {
				return None
			} else if hash == best_block.hash() {
				return None
			};

			let parent_hash = client.header(&BlockId::Hash(hash)).ok()??.parent_hash;

			Some(BlockInfo {
				hash,
				parent_hash,
				number,
			})
		})
		.collect::<Vec<_>>();

	// Sort by block number and get the maximum number of leaves
	leaves.sort_by_key(|b| b.number);

	leaves.push(BlockInfo {
		hash: best_block.hash(),
		parent_hash: *best_block.parent_hash(),
		number: *best_block.number(),
	});

	Ok(leaves.into_iter().rev().take(MAX_ACTIVE_LEAVES).collect())
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
	disable_beefy: bool,
	jaeger_agent: Option<std::net::SocketAddr>,
	telemetry_worker_handle: Option<TelemetryWorkerHandle>,
	program_path: Option<std::path::PathBuf>,
) -> Result<NewFull<Arc<FullClient<RuntimeApi, Executor>>>, Error>
	where
		RuntimeApi: ConstructRuntimeApi<Block, FullClient<RuntimeApi, Executor>> + Send + Sync + 'static,
		RuntimeApi::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
		Executor: NativeExecutionDispatch + 'static,
{
	let role = config.role.clone();
	let force_authoring = config.force_authoring;
	let backoff_authoring_blocks = {
		let mut backoff = sc_consensus_slots::BackoffAuthoringOnFinalizedHeadLagging::default();

		if config.chain_spec.is_rococo() || config.chain_spec.is_wococo() {
			// it's a testnet that's in flux, finality has stalled sometimes due
			// to operational issues and it's annoying to slow down block
			// production to 1 block per hour.
			backoff.max_interval = 10;
		}

		Some(backoff)
	};

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
		other: (rpc_extensions_builder, import_setup, rpc_setup, slot_duration, mut telemetry)
	} = new_partial::<RuntimeApi, Executor>(&mut config, jaeger_agent, telemetry_worker_handle)?;

	let prometheus_registry = config.prometheus_registry().cloned();

	let shared_voter_state = rpc_setup;
	let auth_disc_publish_non_global_ips = config.network.allow_non_globals_in_dht;

	// Note: GrandPa is pushed before the Polkadot-specific protocols. This doesn't change
	// anything in terms of behaviour, but makes the logs more consistent with the other
	// Substrate nodes.
	config.network.extra_sets.push(grandpa::grandpa_peers_set_config());

	if config.chain_spec.is_rococo() || config.chain_spec.is_wococo() {
		config.network.extra_sets.push(beefy_gadget::beefy_peers_set_config());
	}

	{
		use polkadot_network_bridge::{peer_sets_info, IsAuthority};
		let is_authority = if role.is_authority() {
			IsAuthority::Yes
		} else {
			IsAuthority::No
		};
		config.network.extra_sets.extend(peer_sets_info(is_authority));
	}

	config.network.request_response_protocols.push(sc_finality_grandpa_warp_sync::request_response_config_for_chain(
		&config, task_manager.spawn_handle(), backend.clone(), import_setup.1.shared_authority_set().clone(),
	));
	let request_multiplexer = {
		let (multiplexer, configs) = RequestMultiplexer::new();
		config.network.request_response_protocols.extend(configs);
		multiplexer
	};

	let (network, system_rpc_tx, network_starter) =
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
			&config, task_manager.spawn_handle(), client.clone(), network.clone(),
		);
	}

	let parachains_db = crate::parachains_db::open_creating(
		config.database.path().ok_or(Error::DatabasePathRequired)?.into(),
		crate::parachains_db::CacheSizes::default(),
	)?;

	let availability_config = AvailabilityConfig {
		col_data: crate::parachains_db::REAL_COLUMNS.col_availability_data,
		col_meta: crate::parachains_db::REAL_COLUMNS.col_availability_meta,
	};

	let approval_voting_config = ApprovalVotingConfig {
		col_data: crate::parachains_db::REAL_COLUMNS.col_approval_data,
		slot_duration_millis: slot_duration.as_millis() as u64,
	};

	let candidate_validation_config = CandidateValidationConfig {
		artifacts_cache_path: config.database
			.path()
			.ok_or(Error::DatabasePathRequired)?
			.join("pvf-artifacts"),
		program_path: match program_path {
			None => std::env::current_exe()?,
			Some(p) => p,
		},
	};

	let chain_spec = config.chain_spec.cloned_box();
	let rpc_handlers = service::spawn_tasks(service::SpawnTasksParams {
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
		system_rpc_tx,
		telemetry: telemetry.as_mut(),
	})?;

	let (block_import, link_half, babe_link, beefy_link) = import_setup;

	let overseer_client = client.clone();
	let spawner = task_manager.spawn_handle();
	let active_leaves = active_leaves(&select_chain, &*client)?;

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
		let (worker, service) = sc_authority_discovery::new_worker_and_service_with_config(
			sc_authority_discovery::WorkerConfig {
				publish_non_global_ips: auth_disc_publish_non_global_ips,
				..Default::default()
			},
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
			active_leaves,
			keystore,
			overseer_client.clone(),
			parachains_db,
			availability_config,
			approval_voting_config,
			network.clone(),
			authority_discovery_service,
			request_multiplexer,
			prometheus_registry.as_ref(),
			spawner,
			is_collator,
			candidate_validation_config,
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
	} else {
		None
	};

	if role.is_authority() {
		let can_author_with =
			consensus_common::CanAuthorWithNativeVersion::new(client.executor().clone());

		let proposer = sc_basic_authorship::ProposerFactory::new(
			task_manager.spawn_handle(),
			client.clone(),
			transaction_pool,
			prometheus_registry.as_ref(),
			telemetry.as_ref().map(|x| x.handle()),
		);

		let client_clone = client.clone();
		let overseer_handler = overseer_handler.as_ref().ok_or(Error::AuthoritiesRequireRealOverseer)?.clone();
		let slot_duration = babe_link.config().slot_duration();
		let babe_config = babe::BabeParams {
			keystore: keystore_container.sync_keystore(),
			client: client.clone(),
			select_chain,
			block_import,
			env: proposer,
			sync_oracle: network.clone(),
			justification_sync_link: network.clone(),
			create_inherent_data_providers: move |parent, ()| {
				let client_clone = client_clone.clone();
				let overseer_handler = overseer_handler.clone();
				async move {
					let parachain = polkadot_node_core_parachains_inherent::ParachainsInherentDataProvider::create(
						&*client_clone,
						overseer_handler,
						parent,
					).await.map_err(|e| Box::new(e))?;

					let uncles = sc_consensus_uncles::create_uncles_inherent_data_provider(
						&*client_clone,
						parent,
					)?;

					let timestamp = sp_timestamp::InherentDataProvider::from_system_time();

					let slot =
						sp_consensus_babe::inherents::InherentDataProvider::from_timestamp_and_duration(
							*timestamp,
							slot_duration,
						);

					Ok((timestamp, slot, uncles, parachain))
				}
			},
			force_authoring,
			backoff_authoring_blocks,
			babe_link,
			can_author_with,
			block_proposal_slot_portion: babe::SlotProportion::new(2f32 / 3f32),
			telemetry: telemetry.as_ref().map(|x| x.handle()),
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

	// We currently only run the BEEFY gadget on the Rococo and Wococo testnets.
	if !disable_beefy && (chain_spec.is_rococo() || chain_spec.is_wococo()) {
		let beefy_params = beefy_gadget::BeefyParams {
			client: client.clone(),
			backend: backend.clone(),
			key_store: keystore_opt.clone(),
			network: network.clone(),
			signed_commitment_sender: beefy_link,
			min_block_delta: if chain_spec.is_wococo() { 4 } else { 8 },
			prometheus_registry: prometheus_registry.clone(),
		};

		let gadget = beefy_gadget::start_beefy_gadget::<_, beefy_primitives::ecdsa::AuthorityPair, _, _, _>(
			beefy_params
		);

		// Wococo's purpose is to be a testbed for BEEFY, so if it fails we'll
		// bring the node down with it to make sure it is noticed.
		if chain_spec.is_wococo() {
			task_manager.spawn_essential_handle().spawn_blocking("beefy-gadget", gadget);
		} else {
			task_manager.spawn_handle().spawn_blocking("beefy-gadget", gadget);
		}
	}

	let config = grandpa::Config {
		// FIXME substrate#1578 make this available through chainspec
		gossip_duration: Duration::from_millis(1000),
		justification_period: 512,
		name: Some(name),
		observer_enabled: false,
		keystore: keystore_opt,
		local_role: role,
		telemetry: telemetry.as_ref().map(|x| x.handle()),
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

		let builder = if let Some(ref overseer) = overseer_handler {
			builder.add(grandpa_support::ApprovalCheckingVotingRule::new(
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
			voting_rule,
			prometheus_registry: prometheus_registry.clone(),
			shared_voter_state,
			telemetry: telemetry.as_ref().map(|x| x.handle()),
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
		rpc_handlers,
		backend,
	})
}

/// Builds a new service for a light client.
#[cfg(feature = "light-node")]
fn new_light<Runtime, Dispatch>(mut config: Configuration) -> Result<(
	TaskManager,
	RpcHandlers,
), Error>
	where
		Runtime: 'static + Send + Sync + ConstructRuntimeApi<Block, LightClient<Runtime, Dispatch>>,
		<Runtime as ConstructRuntimeApi<Block, LightClient<Runtime, Dispatch>>>::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<LightBackend, Block>>,
		Dispatch: NativeExecutionDispatch + 'static,
{
	set_prometheus_registry(&mut config)?;
	use sc_client_api::backend::RemoteBackend;

	let telemetry = config.telemetry_endpoints.clone()
		.filter(|x| !x.is_empty())
		.map(|endpoints| -> Result<_, telemetry::Error> {
			let worker = TelemetryWorker::new(16)?;
			let telemetry = worker.handle().new_telemetry(endpoints);
			Ok((worker, telemetry))
		})
		.transpose()?;

	let (client, backend, keystore_container, mut task_manager, on_demand) =
		service::new_light_parts::<Block, Runtime, Dispatch>(
			&config,
			telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
		)?;

	let mut telemetry = telemetry
		.map(|(worker, telemetry)| {
			task_manager.spawn_handle().spawn("telemetry", worker.run());
			telemetry
		});

	config.network.extra_sets.push(grandpa::grandpa_peers_set_config());

	let select_chain = sc_consensus::LongestChain::new(backend.clone());

	let transaction_pool = Arc::new(sc_transaction_pool::BasicPool::new_light(
		config.transaction_pool.clone(),
		config.prometheus_registry(),
		task_manager.spawn_handle(),
		client.clone(),
		on_demand.clone(),
	));

	let (grandpa_block_import, grandpa_link) = grandpa::block_import(
		client.clone(),
		&(client.clone() as Arc<_>),
		select_chain.clone(),
		telemetry.as_ref().map(|x| x.handle()),
	)?;
	let justification_import = grandpa_block_import.clone();

	let (babe_block_import, babe_link) = babe::block_import(
		babe::Config::get_or_compute(&*client)?,
		grandpa_block_import,
		client.clone(),
	)?;

	// FIXME: pruning task isn't started since light client doesn't do `AuthoritySetup`.
	let slot_duration = babe_link.config().slot_duration();
	let import_queue = babe::import_queue(
		babe_link,
		babe_block_import,
		Some(Box::new(justification_import)),
		client.clone(),
		select_chain.clone(),
		move |_, ()| async move {
			let timestamp = sp_timestamp::InherentDataProvider::from_system_time();

			let slot =
				sp_consensus_babe::inherents::InherentDataProvider::from_timestamp_and_duration(
					*timestamp,
					slot_duration,
				);

			Ok((timestamp, slot))
		},
		&task_manager.spawn_essential_handle(),
		config.prometheus_registry(),
		consensus_common::NeverCanAuthor,
		telemetry.as_ref().map(|x| x.handle()),
	)?;

	let (network, system_rpc_tx, network_starter) =
		service::build_network(service::BuildNetworkParams {
			config: &config,
			client: client.clone(),
			transaction_pool: transaction_pool.clone(),
			spawn_handle: task_manager.spawn_handle(),
			import_queue,
			on_demand: Some(on_demand.clone()),
			block_announce_validator_builder: None,
		})?;

	let enable_grandpa = !config.disable_grandpa;
	if enable_grandpa {
		let name = config.network.node_name.clone();

		let config = grandpa::Config {
			gossip_duration: Duration::from_millis(1000),
			justification_period: 512,
			name: Some(name),
			observer_enabled: false,
			keystore: None,
			local_role: config.role.clone(),
			telemetry: telemetry.as_ref().map(|x| x.handle()),
		};

		task_manager.spawn_handle().spawn_blocking(
			"grandpa-observer",
			grandpa::run_grandpa_observer(config, grandpa_link, network.clone())?,
		);
	}

	if config.offchain_worker.enabled {
		let _ = service::build_offchain_workers(
			&config,
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

	let rpc_handlers = service::spawn_tasks(service::SpawnTasksParams {
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
		system_rpc_tx,
		telemetry: telemetry.as_mut(),
	})?;

	network_starter.start_network();

	Ok((task_manager, rpc_handlers))
}

/// Builds a new object suitable for chain operations.
#[cfg(feature = "full-node")]
pub fn new_chain_ops(
	mut config: &mut Configuration,
	jaeger_agent: Option<std::net::SocketAddr>,
) -> Result<
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

	#[cfg(feature = "rococo-native")]
	if config.chain_spec.is_rococo() || config.chain_spec.is_wococo() {
		let service::PartialComponents { client, backend, import_queue, task_manager, .. }
			= new_partial::<rococo_runtime::RuntimeApi, RococoExecutor>(config, jaeger_agent, None)?;
		return Ok((Arc::new(Client::Rococo(client)), backend, import_queue, task_manager))
	}

	#[cfg(feature = "kusama-native")]
	if config.chain_spec.is_kusama() {
		let service::PartialComponents { client, backend, import_queue, task_manager, .. }
			= new_partial::<kusama_runtime::RuntimeApi, KusamaExecutor>(config, jaeger_agent, None)?;
		return Ok((Arc::new(Client::Kusama(client)), backend, import_queue, task_manager))
	}

	#[cfg(feature = "westend-native")]
	if config.chain_spec.is_westend() {
		let service::PartialComponents { client, backend, import_queue, task_manager, .. }
			= new_partial::<westend_runtime::RuntimeApi, WestendExecutor>(config, jaeger_agent, None)?;
		return Ok((Arc::new(Client::Westend(client)), backend, import_queue, task_manager))
	}

	let service::PartialComponents { client, backend, import_queue, task_manager, .. }
		= new_partial::<polkadot_runtime::RuntimeApi, PolkadotExecutor>(config, jaeger_agent, None)?;
	Ok((Arc::new(Client::Polkadot(client)), backend, import_queue, task_manager))
}


/// Build a new light node.
#[cfg(feature = "light-node")]
pub fn build_light(config: Configuration) -> Result<(
	TaskManager,
	RpcHandlers,
), Error> {
	#[cfg(feature = "rococo-native")]
	if config.chain_spec.is_rococo() || config.chain_spec.is_wococo() {
		return new_light::<rococo_runtime::RuntimeApi, RococoExecutor>(config)
	}

	#[cfg(feature = "kusama-native")]
	if config.chain_spec.is_kusama() {
		return new_light::<kusama_runtime::RuntimeApi, KusamaExecutor>(config)
	}

	#[cfg(feature = "westend-native")]
	if config.chain_spec.is_westend() {
		return new_light::<westend_runtime::RuntimeApi, WestendExecutor>(config)
	}

	new_light::<polkadot_runtime::RuntimeApi, PolkadotExecutor>(config)
}

#[cfg(feature = "full-node")]
pub fn build_full(
	config: Configuration,
	is_collator: IsCollator,
	grandpa_pause: Option<(u32, u32)>,
	disable_beefy: bool,
	jaeger_agent: Option<std::net::SocketAddr>,
	telemetry_worker_handle: Option<TelemetryWorkerHandle>,
) -> Result<NewFull<Client>, Error> {
	#[cfg(feature = "rococo-native")]
	if config.chain_spec.is_rococo() || config.chain_spec.is_wococo() {
		return new_full::<rococo_runtime::RuntimeApi, RococoExecutor>(
			config,
			is_collator,
			grandpa_pause,
			disable_beefy,
			jaeger_agent,
			telemetry_worker_handle,
			None,
		).map(|full| full.with_client(Client::Rococo))
	}

	#[cfg(feature = "kusama-native")]
	if config.chain_spec.is_kusama() {
		return new_full::<kusama_runtime::RuntimeApi, KusamaExecutor>(
			config,
			is_collator,
			grandpa_pause,
			disable_beefy,
			jaeger_agent,
			telemetry_worker_handle,
			None,
		).map(|full| full.with_client(Client::Kusama))
	}

	#[cfg(feature = "westend-native")]
	if config.chain_spec.is_westend() {
		return new_full::<westend_runtime::RuntimeApi, WestendExecutor>(
			config,
			is_collator,
			grandpa_pause,
			disable_beefy,
			jaeger_agent,
			telemetry_worker_handle,
			None,
		).map(|full| full.with_client(Client::Westend))
	}

	new_full::<polkadot_runtime::RuntimeApi, PolkadotExecutor>(
		config,
		is_collator,
		grandpa_pause,
		disable_beefy,
		jaeger_agent,
		telemetry_worker_handle,
		None,
	).map(|full| full.with_client(Client::Polkadot))
}
