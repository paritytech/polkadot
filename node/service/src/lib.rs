// Copyright (C) Parity Technologies (UK) Ltd.
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

pub mod benchmarking;
pub mod chain_spec;
mod fake_runtime_api;
mod grandpa_support;
mod parachains_db;
mod relay_chain_selection;

#[cfg(feature = "full-node")]
pub mod overseer;

#[cfg(feature = "full-node")]
pub use self::overseer::{OverseerGen, OverseerGenArgs, RealOverseerGen};

#[cfg(test)]
mod tests;

#[cfg(feature = "full-node")]
use {
	grandpa::{self, FinalityProofProvider as GrandpaFinalityProofProvider},
	gum::info,
	polkadot_node_core_approval_voting::{
		self as approval_voting_subsystem, Config as ApprovalVotingConfig,
	},
	polkadot_node_core_av_store::Config as AvailabilityConfig,
	polkadot_node_core_av_store::Error as AvailabilityError,
	polkadot_node_core_candidate_validation::Config as CandidateValidationConfig,
	polkadot_node_core_chain_selection::{
		self as chain_selection_subsystem, Config as ChainSelectionConfig,
	},
	polkadot_node_core_dispute_coordinator::Config as DisputeCoordinatorConfig,
	polkadot_node_network_protocol::{
		peer_set::PeerSetProtocolNames, request_response::ReqProtocolNames,
	},
	sc_client_api::BlockBackend,
	sp_core::traits::SpawnNamed,
	sp_trie::PrefixedMemoryDB,
};

use polkadot_node_subsystem_util::database::Database;

#[cfg(feature = "full-node")]
pub use {
	polkadot_overseer::{Handle, Overseer, OverseerConnector, OverseerHandle},
	polkadot_primitives::runtime_api::ParachainHost,
	relay_chain_selection::SelectRelayChain,
	sc_client_api::AuxStore,
	sp_authority_discovery::AuthorityDiscoveryApi,
	sp_blockchain::{HeaderBackend, HeaderMetadata},
	sp_consensus_babe::BabeApi,
};

#[cfg(feature = "full-node")]
use polkadot_node_subsystem::jaeger;

use std::{path::PathBuf, sync::Arc, time::Duration};

use prometheus_endpoint::Registry;
#[cfg(feature = "full-node")]
use service::KeystoreContainer;
use service::RpcHandlers;
use telemetry::TelemetryWorker;
#[cfg(feature = "full-node")]
use telemetry::{Telemetry, TelemetryWorkerHandle};

pub use chain_spec::{KusamaChainSpec, PolkadotChainSpec, RococoChainSpec, WestendChainSpec};
pub use consensus_common::{Proposal, SelectChain};
use frame_benchmarking_cli::SUBSTRATE_REFERENCE_HARDWARE;
use mmr_gadget::MmrGadget;
pub use polkadot_primitives::{Block, BlockId, BlockNumber, CollatorPair, Hash, Id as ParaId};
pub use sc_client_api::{Backend, CallExecutor, ExecutionStrategy};
pub use sc_consensus::{BlockImport, LongestChain};
pub use sc_executor::NativeExecutionDispatch;
use sc_executor::{HeapAllocStrategy, WasmExecutor, DEFAULT_HEAP_ALLOC_STRATEGY};
pub use service::{
	config::{DatabaseSource, PrometheusConfig},
	ChainSpec, Configuration, Error as SubstrateServiceError, PruningMode, Role, RuntimeGenesis,
	TFullBackend, TFullCallExecutor, TFullClient, TaskManager, TransactionPoolOptions,
};
pub use sp_api::{ApiRef, ConstructRuntimeApi, Core as CoreApi, ProvideRuntimeApi, StateBackend};
pub use sp_runtime::{
	generic,
	traits::{
		self as runtime_traits, BlakeTwo256, Block as BlockT, HashFor, Header as HeaderT, NumberFor,
	},
};

#[cfg(feature = "kusama-native")]
pub use {kusama_runtime, kusama_runtime_constants};
#[cfg(feature = "polkadot-native")]
pub use {polkadot_runtime, polkadot_runtime_constants};
#[cfg(feature = "rococo-native")]
pub use {rococo_runtime, rococo_runtime_constants};
#[cfg(feature = "westend-native")]
pub use {westend_runtime, westend_runtime_constants};

pub use fake_runtime_api::{GetLastTimestamp, RuntimeApi};

#[cfg(feature = "full-node")]
pub type FullBackend = service::TFullBackend<Block>;

#[cfg(feature = "full-node")]
pub type FullClient = service::TFullClient<
	Block,
	RuntimeApi,
	WasmExecutor<(sp_io::SubstrateHostFunctions, frame_benchmarking::benchmarking::HostFunctions)>,
>;

/// Provides the header and block number for a hash.
///
/// Decouples `sc_client_api::Backend` and `sp_blockchain::HeaderBackend`.
pub trait HeaderProvider<Block, Error = sp_blockchain::Error>: Send + Sync + 'static
where
	Block: BlockT,
	Error: std::fmt::Debug + Send + Sync + 'static,
{
	/// Obtain the header for a hash.
	fn header(
		&self,
		hash: <Block as BlockT>::Hash,
	) -> Result<Option<<Block as BlockT>::Header>, Error>;
	/// Obtain the block number for a hash.
	fn number(
		&self,
		hash: <Block as BlockT>::Hash,
	) -> Result<Option<<<Block as BlockT>::Header as HeaderT>::Number>, Error>;
}

impl<Block, T> HeaderProvider<Block> for T
where
	Block: BlockT,
	T: sp_blockchain::HeaderBackend<Block> + 'static,
{
	fn header(
		&self,
		hash: Block::Hash,
	) -> sp_blockchain::Result<Option<<Block as BlockT>::Header>> {
		<Self as sp_blockchain::HeaderBackend<Block>>::header(self, hash)
	}
	fn number(
		&self,
		hash: Block::Hash,
	) -> sp_blockchain::Result<Option<<<Block as BlockT>::Header as HeaderT>::Number>> {
		<Self as sp_blockchain::HeaderBackend<Block>>::number(self, hash)
	}
}

/// Decoupling the provider.
///
/// Mandated since `trait HeaderProvider` can only be
/// implemented once for a generic `T`.
pub trait HeaderProviderProvider<Block>: Send + Sync + 'static
where
	Block: BlockT,
{
	type Provider: HeaderProvider<Block> + 'static;

	fn header_provider(&self) -> &Self::Provider;
}

impl<Block, T> HeaderProviderProvider<Block> for T
where
	Block: BlockT,
	T: sc_client_api::Backend<Block> + 'static,
{
	type Provider = <T as sc_client_api::Backend<Block>>::Blockchain;

	fn header_provider(&self) -> &Self::Provider {
		self.blockchain()
	}
}

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
	Jaeger(#[from] polkadot_node_subsystem::jaeger::JaegerError),

	#[cfg(feature = "full-node")]
	#[error(transparent)]
	Availability(#[from] AvailabilityError),

	#[error("Authorities require the real overseer implementation")]
	AuthoritiesRequireRealOverseer,

	#[cfg(feature = "full-node")]
	#[error("Creating a custom database is required for validators")]
	DatabasePathRequired,

	#[cfg(feature = "full-node")]
	#[error("Expected at least one of polkadot, kusama, westend or rococo runtime feature")]
	NoRuntime,

	#[cfg(feature = "full-node")]
	#[error("Worker binaries not executable, prepare binary: {0:?}, execute binary: {1:?}")]
	InvalidWorkerBinaries(PathBuf, PathBuf),

	#[cfg(feature = "full-node")]
	#[error("Worker binaries could not be found at workers path ({0:?}), polkadot binary directory, or /usr/lib/polkadot")]
	MissingWorkerBinaries(Option<PathBuf>),

	#[cfg(feature = "full-node")]
	#[error("Version of worker binaries ({0}) is different from node version ({1})")]
	WorkerBinaryVersionMismatch(String, String),
}

/// Identifies the variant of the chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Chain {
	/// Polkadot.
	Polkadot,
	/// Kusama.
	Kusama,
	/// Rococo or one of its derivations.
	Rococo,
	/// Westend.
	Westend,
	/// Unknown chain?
	Unknown,
}

/// Can be called for a `Configuration` to identify which network the configuration targets.
pub trait IdentifyVariant {
	/// Returns if this is a configuration for the `Polkadot` network.
	fn is_polkadot(&self) -> bool;

	/// Returns if this is a configuration for the `Kusama` network.
	fn is_kusama(&self) -> bool;

	/// Returns if this is a configuration for the `Westend` network.
	fn is_westend(&self) -> bool;

	/// Returns if this is a configuration for the `Rococo` network.
	fn is_rococo(&self) -> bool;

	/// Returns if this is a configuration for the `Wococo` test network.
	fn is_wococo(&self) -> bool;

	/// Returns if this is a configuration for the `Versi` test network.
	fn is_versi(&self) -> bool;

	/// Returns true if this configuration is for a development network.
	fn is_dev(&self) -> bool;

	/// Identifies the variant of the chain.
	fn identify_chain(&self) -> Chain;
}

impl IdentifyVariant for Box<dyn ChainSpec> {
	fn is_polkadot(&self) -> bool {
		self.id().starts_with("polkadot") || self.id().starts_with("dot")
	}
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
	fn is_versi(&self) -> bool {
		self.id().starts_with("versi") || self.id().starts_with("vrs")
	}
	fn is_dev(&self) -> bool {
		self.id().ends_with("dev")
	}
	fn identify_chain(&self) -> Chain {
		if self.is_polkadot() {
			Chain::Polkadot
		} else if self.is_kusama() {
			Chain::Kusama
		} else if self.is_westend() {
			Chain::Westend
		} else if self.is_rococo() || self.is_versi() || self.is_wococo() {
			Chain::Rococo
		} else {
			Chain::Unknown
		}
	}
}

#[cfg(feature = "full-node")]
pub fn open_database(db_source: &DatabaseSource) -> Result<Arc<dyn Database>, Error> {
	let parachains_db = match db_source {
		DatabaseSource::RocksDb { path, .. } => parachains_db::open_creating_rocksdb(
			path.clone(),
			parachains_db::CacheSizes::default(),
		)?,
		DatabaseSource::ParityDb { path, .. } => parachains_db::open_creating_paritydb(
			path.parent().ok_or(Error::DatabasePathRequired)?.into(),
			parachains_db::CacheSizes::default(),
		)?,
		DatabaseSource::Auto { paritydb_path, rocksdb_path, .. } => {
			if paritydb_path.is_dir() && paritydb_path.exists() {
				parachains_db::open_creating_paritydb(
					paritydb_path.parent().ok_or(Error::DatabasePathRequired)?.into(),
					parachains_db::CacheSizes::default(),
				)?
			} else {
				parachains_db::open_creating_rocksdb(
					rocksdb_path.clone(),
					parachains_db::CacheSizes::default(),
				)?
			}
		},
		DatabaseSource::Custom { .. } => {
			unimplemented!("No polkadot subsystem db for custom source.");
		},
	};
	Ok(parachains_db)
}

/// Initialize the `Jeager` collector. The destination must listen
/// on the given address and port for `UDP` packets.
#[cfg(any(test, feature = "full-node"))]
fn jaeger_launch_collector_with_agent(
	spawner: impl SpawnNamed,
	config: &Configuration,
	agent: Option<std::net::SocketAddr>,
) -> Result<(), Error> {
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
type FullSelectChain = relay_chain_selection::SelectRelayChain<FullBackend>;
#[cfg(feature = "full-node")]
type FullGrandpaBlockImport<ChainSelection = FullSelectChain> =
	grandpa::GrandpaBlockImport<FullBackend, Block, FullClient, ChainSelection>;
#[cfg(feature = "full-node")]
type FullBeefyBlockImport<InnerBlockImport> =
	beefy::import::BeefyBlockImport<Block, FullBackend, FullClient, InnerBlockImport>;

#[cfg(feature = "full-node")]
struct Basics {
	task_manager: TaskManager,
	client: Arc<FullClient>,
	backend: Arc<FullBackend>,
	keystore_container: KeystoreContainer,
	telemetry: Option<Telemetry>,
}

#[cfg(feature = "full-node")]
fn new_partial_basics(
	config: &mut Configuration,
	jaeger_agent: Option<std::net::SocketAddr>,
	telemetry_worker_handle: Option<TelemetryWorkerHandle>,
) -> Result<Basics, Error> {
	let telemetry = config
		.telemetry_endpoints
		.clone()
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

	let heap_pages = config
		.default_heap_pages
		.map_or(DEFAULT_HEAP_ALLOC_STRATEGY, |h| HeapAllocStrategy::Static { extra_pages: h as _ });

	let executor = WasmExecutor::builder()
		.with_execution_method(config.wasm_method)
		.with_onchain_heap_alloc_strategy(heap_pages)
		.with_offchain_heap_alloc_strategy(heap_pages)
		.with_max_runtime_instances(config.max_runtime_instances)
		.with_runtime_cache_size(config.runtime_cache_size)
		.build();

	let (client, backend, keystore_container, task_manager) =
		service::new_full_parts::<Block, RuntimeApi, _>(
			&config,
			telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
			executor,
		)?;
	let client = Arc::new(client);

	let telemetry = telemetry.map(|(worker, telemetry)| {
		if let Some(worker) = worker {
			task_manager.spawn_handle().spawn(
				"telemetry",
				Some("telemetry"),
				Box::pin(worker.run()),
			);
		}
		telemetry
	});

	jaeger_launch_collector_with_agent(task_manager.spawn_handle(), &*config, jaeger_agent)?;

	Ok(Basics { task_manager, client, backend, keystore_container, telemetry })
}

#[cfg(feature = "full-node")]
fn new_partial<ChainSelection>(
	config: &mut Configuration,
	Basics { task_manager, backend, client, keystore_container, telemetry }: Basics,
	select_chain: ChainSelection,
) -> Result<
	service::PartialComponents<
		FullClient,
		FullBackend,
		ChainSelection,
		sc_consensus::DefaultImportQueue<Block, FullClient>,
		sc_transaction_pool::FullPool<Block, FullClient>,
		(
			impl Fn(
				polkadot_rpc::DenyUnsafe,
				polkadot_rpc::SubscriptionTaskExecutor,
			) -> Result<polkadot_rpc::RpcExtension, SubstrateServiceError>,
			(
				babe::BabeBlockImport<
					Block,
					FullClient,
					FullBeefyBlockImport<FullGrandpaBlockImport<ChainSelection>>,
				>,
				grandpa::LinkHalf<Block, FullClient, ChainSelection>,
				babe::BabeLink<Block>,
				beefy::BeefyVoterLinks<Block>,
			),
			grandpa::SharedVoterState,
			sp_consensus_babe::SlotDuration,
			Option<Telemetry>,
		),
	>,
	Error,
>
where
	ChainSelection: 'static + SelectChain<Block>,
{
	let transaction_pool = sc_transaction_pool::BasicPool::new_full(
		config.transaction_pool.clone(),
		config.role.is_authority().into(),
		config.prometheus_registry(),
		task_manager.spawn_essential_handle(),
		client.clone(),
	);

	let grandpa_hard_forks = if config.chain_spec.is_kusama() {
		grandpa_support::kusama_hard_forks()
	} else {
		Vec::new()
	};

	let (grandpa_block_import, grandpa_link) = grandpa::block_import_with_authority_set_hard_forks(
		client.clone(),
		&(client.clone() as Arc<_>),
		select_chain.clone(),
		grandpa_hard_forks,
		telemetry.as_ref().map(|x| x.handle()),
	)?;
	let justification_import = grandpa_block_import.clone();

	let (beefy_block_import, beefy_voter_links, beefy_rpc_links) =
		beefy::beefy_block_import_and_links(
			grandpa_block_import,
			backend.clone(),
			client.clone(),
			config.prometheus_registry().cloned(),
		);

	let babe_config = babe::configuration(&*client)?;
	let (block_import, babe_link) =
		babe::block_import(babe_config.clone(), beefy_block_import, client.clone())?;

	let slot_duration = babe_link.config().slot_duration();
	let (import_queue, babe_worker_handle) = babe::import_queue(
		babe_link.clone(),
		block_import.clone(),
		Some(Box::new(justification_import)),
		client.clone(),
		select_chain.clone(),
		move |_, ()| async move {
			let timestamp = sp_timestamp::InherentDataProvider::from_system_time();

			let slot =
				sp_consensus_babe::inherents::InherentDataProvider::from_timestamp_and_slot_duration(
					*timestamp,
					slot_duration,
				);

			Ok((slot, timestamp))
		},
		&task_manager.spawn_essential_handle(),
		config.prometheus_registry(),
		telemetry.as_ref().map(|x| x.handle()),
	)?;

	let justification_stream = grandpa_link.justification_stream();
	let shared_authority_set = grandpa_link.shared_authority_set().clone();
	let shared_voter_state = grandpa::SharedVoterState::empty();
	let finality_proof_provider = GrandpaFinalityProofProvider::new_for_service(
		backend.clone(),
		Some(shared_authority_set.clone()),
	);

	let import_setup = (block_import, grandpa_link, babe_link, beefy_voter_links);
	let rpc_setup = shared_voter_state.clone();

	let rpc_extensions_builder = {
		let client = client.clone();
		let keystore = keystore_container.keystore();
		let transaction_pool = transaction_pool.clone();
		let select_chain = select_chain.clone();
		let chain_spec = config.chain_spec.cloned_box();
		let backend = backend.clone();

		move |deny_unsafe,
		      subscription_executor: polkadot_rpc::SubscriptionTaskExecutor|
		      -> Result<polkadot_rpc::RpcExtension, service::Error> {
			let deps = polkadot_rpc::FullDeps {
				client: client.clone(),
				pool: transaction_pool.clone(),
				select_chain: select_chain.clone(),
				chain_spec: chain_spec.cloned_box(),
				deny_unsafe,
				babe: polkadot_rpc::BabeDeps {
					babe_worker_handle: babe_worker_handle.clone(),
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
					beefy_finality_proof_stream: beefy_rpc_links.from_voter_justif_stream.clone(),
					beefy_best_block_stream: beefy_rpc_links.from_voter_best_beefy_stream.clone(),
					subscription_executor,
				},
			};

			polkadot_rpc::create_full(deps, backend.clone()).map_err(Into::into)
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
		other: (rpc_extensions_builder, import_setup, rpc_setup, slot_duration, telemetry),
	})
}

#[cfg(feature = "full-node")]
pub struct NewFullParams<OverseerGenerator: OverseerGen> {
	pub is_collator: IsCollator,
	pub grandpa_pause: Option<(u32, u32)>,
	pub enable_beefy: bool,
	pub jaeger_agent: Option<std::net::SocketAddr>,
	pub telemetry_worker_handle: Option<TelemetryWorkerHandle>,
	pub workers_path: Option<std::path::PathBuf>,
	pub overseer_enable_anyways: bool,
	pub overseer_gen: OverseerGenerator,
	pub overseer_message_channel_capacity_override: Option<usize>,
	#[allow(dead_code)]
	pub malus_finality_delay: Option<u32>,
	pub hwbench: Option<sc_sysinfo::HwBench>,
}

#[cfg(feature = "full-node")]
pub struct NewFull {
	pub task_manager: TaskManager,
	pub client: Arc<FullClient>,
	pub overseer_handle: Option<Handle>,
	pub network: Arc<sc_network::NetworkService<Block, <Block as BlockT>::Hash>>,
	pub sync_service: Arc<sc_network_sync::SyncingService<Block>>,
	pub rpc_handlers: RpcHandlers,
	pub backend: Arc<FullBackend>,
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

pub const AVAILABILITY_CONFIG: AvailabilityConfig = AvailabilityConfig {
	col_data: parachains_db::REAL_COLUMNS.col_availability_data,
	col_meta: parachains_db::REAL_COLUMNS.col_availability_meta,
};

/// Create a new full node of arbitrary runtime and executor.
///
/// This is an advanced feature and not recommended for general use. Generally, `build_full` is
/// a better choice.
///
/// `overseer_enable_anyways` always enables the overseer, based on the provided `OverseerGenerator`,
/// regardless of the role the node has. The relay chain selection (longest or disputes-aware) is
/// still determined based on the role of the node. Likewise for authority discovery.
///
/// `workers_path` is used to get the path to the directory where auxiliary worker binaries reside.
/// If not specified, the main binary's directory is searched first, then `/usr/lib/polkadot` is
/// searched. If the path points to an executable rather then directory, that executable is used
/// both as preparation and execution worker (supposed to be used for tests only).
#[cfg(feature = "full-node")]
pub fn new_full<OverseerGenerator: OverseerGen>(
	mut config: Configuration,
	NewFullParams {
		is_collator,
		grandpa_pause,
		enable_beefy,
		jaeger_agent,
		telemetry_worker_handle,
		workers_path,
		overseer_enable_anyways,
		overseer_gen,
		overseer_message_channel_capacity_override,
		malus_finality_delay: _malus_finality_delay,
		hwbench,
	}: NewFullParams<OverseerGenerator>,
) -> Result<NewFull, Error> {
	use polkadot_node_network_protocol::request_response::IncomingRequest;
	use sc_network_common::sync::warp::WarpSyncParams;

	let is_offchain_indexing_enabled = config.offchain_worker.indexing_enabled;
	let role = config.role.clone();
	let force_authoring = config.force_authoring;
	let backoff_authoring_blocks = {
		let mut backoff = sc_consensus_slots::BackoffAuthoringOnFinalizedHeadLagging::default();

		if config.chain_spec.is_rococo() ||
			config.chain_spec.is_wococo() ||
			config.chain_spec.is_versi()
		{
			// it's a testnet that's in flux, finality has stalled sometimes due
			// to operational issues and it's annoying to slow down block
			// production to 1 block per hour.
			backoff.max_interval = 10;
		}

		Some(backoff)
	};

	// If not on a known test network, warn the user that BEEFY is still experimental.
	if enable_beefy &&
		!config.chain_spec.is_rococo() &&
		!config.chain_spec.is_wococo() &&
		!config.chain_spec.is_versi()
	{
		gum::warn!("BEEFY is still experimental, usage on a production network is discouraged.");
	}

	let disable_grandpa = config.disable_grandpa;
	let name = config.network.node_name.clone();

	let basics = new_partial_basics(&mut config, jaeger_agent, telemetry_worker_handle)?;

	let prometheus_registry = config.prometheus_registry().cloned();

	let overseer_connector = OverseerConnector::default();
	let overseer_handle = Handle::new(overseer_connector.handle());

	let chain_spec = config.chain_spec.cloned_box();

	let keystore = basics.keystore_container.local_keystore();
	let auth_or_collator = role.is_authority() || is_collator.is_collator();
	let pvf_checker_enabled = role.is_authority() && !is_collator.is_collator();

	let select_chain = if auth_or_collator {
		let metrics =
			polkadot_node_subsystem_util::metrics::Metrics::register(prometheus_registry.as_ref())?;

		SelectRelayChain::new_with_overseer(
			basics.backend.clone(),
			overseer_handle.clone(),
			metrics,
			Some(basics.task_manager.spawn_handle()),
		)
	} else {
		SelectRelayChain::new_longest_chain(basics.backend.clone())
	};

	let service::PartialComponents::<_, _, SelectRelayChain<_>, _, _, _> {
		client,
		backend,
		mut task_manager,
		keystore_container,
		select_chain,
		import_queue,
		transaction_pool,
		other: (rpc_extensions_builder, import_setup, rpc_setup, slot_duration, mut telemetry),
	} = new_partial::<SelectRelayChain<_>>(&mut config, basics, select_chain)?;

	let shared_voter_state = rpc_setup;
	let auth_disc_publish_non_global_ips = config.network.allow_non_globals_in_dht;
	let mut net_config = sc_network::config::FullNetworkConfiguration::new(&config.network);

	let genesis_hash = client.block_hash(0).ok().flatten().expect("Genesis block exists; qed");

	// Note: GrandPa is pushed before the Polkadot-specific protocols. This doesn't change
	// anything in terms of behaviour, but makes the logs more consistent with the other
	// Substrate nodes.
	let grandpa_protocol_name = grandpa::protocol_standard_name(&genesis_hash, &config.chain_spec);
	net_config.add_notification_protocol(grandpa::grandpa_peers_set_config(
		grandpa_protocol_name.clone(),
	));

	let beefy_gossip_proto_name =
		beefy::gossip_protocol_name(&genesis_hash, config.chain_spec.fork_id());
	// `beefy_on_demand_justifications_handler` is given to `beefy-gadget` task to be run,
	// while `beefy_req_resp_cfg` is added to `config.network.request_response_protocols`.
	let (beefy_on_demand_justifications_handler, beefy_req_resp_cfg) =
		beefy::communication::request_response::BeefyJustifsRequestHandler::new(
			&genesis_hash,
			config.chain_spec.fork_id(),
			client.clone(),
			prometheus_registry.clone(),
		);
	if enable_beefy {
		net_config.add_notification_protocol(beefy::communication::beefy_peers_set_config(
			beefy_gossip_proto_name.clone(),
		));
		net_config.add_request_response_protocol(beefy_req_resp_cfg);
	}

	let peerset_protocol_names =
		PeerSetProtocolNames::new(genesis_hash, config.chain_spec.fork_id());

	{
		use polkadot_network_bridge::{peer_sets_info, IsAuthority};
		let is_authority = if role.is_authority() { IsAuthority::Yes } else { IsAuthority::No };
		for config in peer_sets_info(is_authority, &peerset_protocol_names) {
			net_config.add_notification_protocol(config);
		}
	}

	let req_protocol_names = ReqProtocolNames::new(&genesis_hash, config.chain_spec.fork_id());

	let (pov_req_receiver, cfg) = IncomingRequest::get_config_receiver(&req_protocol_names);
	net_config.add_request_response_protocol(cfg);
	let (chunk_req_receiver, cfg) = IncomingRequest::get_config_receiver(&req_protocol_names);
	net_config.add_request_response_protocol(cfg);
	let (collation_req_receiver, cfg) = IncomingRequest::get_config_receiver(&req_protocol_names);
	net_config.add_request_response_protocol(cfg);
	let (available_data_req_receiver, cfg) =
		IncomingRequest::get_config_receiver(&req_protocol_names);
	net_config.add_request_response_protocol(cfg);
	let (statement_req_receiver, cfg) = IncomingRequest::get_config_receiver(&req_protocol_names);
	net_config.add_request_response_protocol(cfg);
	let (dispute_req_receiver, cfg) = IncomingRequest::get_config_receiver(&req_protocol_names);
	net_config.add_request_response_protocol(cfg);

	let grandpa_hard_forks = if config.chain_spec.is_kusama() {
		grandpa_support::kusama_hard_forks()
	} else {
		Vec::new()
	};

	let warp_sync = Arc::new(grandpa::warp_proof::NetworkProvider::new(
		backend.clone(),
		import_setup.1.shared_authority_set().clone(),
		grandpa_hard_forks,
	));

	let (network, system_rpc_tx, tx_handler_controller, network_starter, sync_service) =
		service::build_network(service::BuildNetworkParams {
			config: &config,
			net_config,
			client: client.clone(),
			transaction_pool: transaction_pool.clone(),
			spawn_handle: task_manager.spawn_handle(),
			import_queue,
			block_announce_validator_builder: None,
			warp_sync_params: Some(WarpSyncParams::WithProvider(warp_sync)),
		})?;

	if config.offchain_worker.enabled {
		let offchain_workers = Arc::new(sc_offchain::OffchainWorkers::new_with_options(
			client.clone(),
			sc_offchain::OffchainWorkerOptions { enable_http_requests: false },
		));

		// Start the offchain workers to have
		task_manager.spawn_handle().spawn(
			"offchain-notifications",
			None,
			sc_offchain::notification_future(
				config.role.is_authority(),
				client.clone(),
				offchain_workers,
				task_manager.spawn_handle().clone(),
				network.clone(),
			),
		);
	}

	let parachains_db = open_database(&config.database)?;

	let approval_voting_config = ApprovalVotingConfig {
		col_approval_data: parachains_db::REAL_COLUMNS.col_approval_data,
		slot_duration_millis: slot_duration.as_millis() as u64,
	};

	let (prep_worker_path, exec_worker_path) = workers::determine_workers_paths(workers_path)?;

	let candidate_validation_config = CandidateValidationConfig {
		artifacts_cache_path: config
			.database
			.path()
			.ok_or(Error::DatabasePathRequired)?
			.join("pvf-artifacts"),
		prep_worker_path,
		exec_worker_path,
	};

	let chain_selection_config = ChainSelectionConfig {
		col_data: parachains_db::REAL_COLUMNS.col_chain_selection_data,
		stagnant_check_interval: Default::default(),
		stagnant_check_mode: chain_selection_subsystem::StagnantCheckMode::PruneOnly,
	};

	let dispute_coordinator_config = DisputeCoordinatorConfig {
		col_dispute_data: parachains_db::REAL_COLUMNS.col_dispute_coordinator_data,
	};

	let rpc_handlers = service::spawn_tasks(service::SpawnTasksParams {
		config,
		backend: backend.clone(),
		client: client.clone(),
		keystore: keystore_container.keystore(),
		network: network.clone(),
		sync_service: sync_service.clone(),
		rpc_builder: Box::new(rpc_extensions_builder),
		transaction_pool: transaction_pool.clone(),
		task_manager: &mut task_manager,
		system_rpc_tx,
		tx_handler_controller,
		telemetry: telemetry.as_mut(),
	})?;

	if let Some(hwbench) = hwbench {
		sc_sysinfo::print_hwbench(&hwbench);
		if !SUBSTRATE_REFERENCE_HARDWARE.check_hardware(&hwbench) && role.is_authority() {
			log::warn!(
				"⚠️  The hardware does not meet the minimal requirements for role 'Authority' find out more at:\n\
				https://wiki.polkadot.network/docs/maintain-guides-how-to-validate-polkadot#reference-hardware"
			);
		}

		if let Some(ref mut telemetry) = telemetry {
			let telemetry_handle = telemetry.handle();
			task_manager.spawn_handle().spawn(
				"telemetry_hwbench",
				None,
				sc_sysinfo::initialize_hwbench_telemetry(telemetry_handle, hwbench),
			);
		}
	}

	let (block_import, link_half, babe_link, beefy_links) = import_setup;

	let overseer_client = client.clone();
	let spawner = task_manager.spawn_handle();

	let authority_discovery_service = if auth_or_collator || overseer_enable_anyways {
		use futures::StreamExt;
		use sc_network::{Event, NetworkEventStream};

		let authority_discovery_role = if role.is_authority() {
			sc_authority_discovery::Role::PublishAndDiscover(keystore_container.keystore())
		} else {
			// don't publish our addresses when we're not an authority (collator, cumulus, ..)
			sc_authority_discovery::Role::Discover
		};
		let dht_event_stream =
			network.event_stream("authority-discovery").filter_map(|e| async move {
				match e {
					Event::Dht(e) => Some(e),
					_ => None,
				}
			});
		let (worker, service) = sc_authority_discovery::new_worker_and_service_with_config(
			sc_authority_discovery::WorkerConfig {
				publish_non_global_ips: auth_disc_publish_non_global_ips,
				// Require that authority discovery records are signed.
				strict_record_validation: true,
				..Default::default()
			},
			client.clone(),
			network.clone(),
			Box::pin(dht_event_stream),
			authority_discovery_role,
			prometheus_registry.clone(),
		);

		task_manager.spawn_handle().spawn(
			"authority-discovery-worker",
			Some("authority-discovery"),
			Box::pin(worker.run()),
		);
		Some(service)
	} else {
		None
	};

	let overseer_handle = if let Some(authority_discovery_service) = authority_discovery_service {
		let (overseer, overseer_handle) = overseer_gen
			.generate::<service::SpawnTaskHandle, FullClient>(
				overseer_connector,
				OverseerGenArgs {
					keystore,
					runtime_client: overseer_client.clone(),
					parachains_db,
					network_service: network.clone(),
					sync_service: sync_service.clone(),
					authority_discovery_service,
					pov_req_receiver,
					chunk_req_receiver,
					collation_req_receiver,
					available_data_req_receiver,
					statement_req_receiver,
					dispute_req_receiver,
					registry: prometheus_registry.as_ref(),
					spawner,
					is_collator,
					approval_voting_config,
					availability_config: AVAILABILITY_CONFIG,
					candidate_validation_config,
					chain_selection_config,
					dispute_coordinator_config,
					pvf_checker_enabled,
					overseer_message_channel_capacity_override,
					req_protocol_names,
					peerset_protocol_names,
				},
			)
			.map_err(|e| {
				gum::error!("Failed to init overseer: {}", e);
				e
			})?;
		let handle = Handle::new(overseer_handle.clone());

		{
			let handle = handle.clone();
			task_manager.spawn_essential_handle().spawn_blocking(
				"overseer",
				None,
				Box::pin(async move {
					use futures::{pin_mut, select, FutureExt};

					let forward = polkadot_overseer::forward_events(overseer_client, handle);

					let forward = forward.fuse();
					let overseer_fut = overseer.run().fuse();

					pin_mut!(overseer_fut);
					pin_mut!(forward);

					select! {
						() = forward => (),
						() = overseer_fut => (),
						complete => (),
					}
				}),
			);
		}
		Some(handle)
	} else {
		assert!(
			!auth_or_collator,
			"Precondition congruence (false) is guaranteed by manual checking. qed"
		);
		None
	};

	if role.is_authority() {
		let proposer = sc_basic_authorship::ProposerFactory::new(
			task_manager.spawn_handle(),
			client.clone(),
			transaction_pool,
			prometheus_registry.as_ref(),
			telemetry.as_ref().map(|x| x.handle()),
		);

		let client_clone = client.clone();
		let overseer_handle =
			overseer_handle.as_ref().ok_or(Error::AuthoritiesRequireRealOverseer)?.clone();
		let slot_duration = babe_link.config().slot_duration();
		let babe_config = babe::BabeParams {
			keystore: keystore_container.keystore(),
			client: client.clone(),
			select_chain,
			block_import,
			env: proposer,
			sync_oracle: sync_service.clone(),
			justification_sync_link: sync_service.clone(),
			create_inherent_data_providers: move |parent, ()| {
				let client_clone = client_clone.clone();
				let overseer_handle = overseer_handle.clone();

				async move {
					let parachain =
						polkadot_node_core_parachains_inherent::ParachainsInherentDataProvider::new(
							client_clone,
							overseer_handle,
							parent,
						);

					let timestamp = sp_timestamp::InherentDataProvider::from_system_time();

					let slot =
						sp_consensus_babe::inherents::InherentDataProvider::from_timestamp_and_slot_duration(
							*timestamp,
							slot_duration,
						);

					Ok((slot, timestamp, parachain))
				}
			},
			force_authoring,
			backoff_authoring_blocks,
			babe_link,
			block_proposal_slot_portion: babe::SlotProportion::new(2f32 / 3f32),
			max_block_proposal_slot_portion: None,
			telemetry: telemetry.as_ref().map(|x| x.handle()),
		};

		let babe = babe::start_babe(babe_config)?;
		task_manager.spawn_essential_handle().spawn_blocking("babe", None, babe);
	}

	// if the node isn't actively participating in consensus then it doesn't
	// need a keystore, regardless of which protocol we use below.
	let keystore_opt = if role.is_authority() { Some(keystore_container.keystore()) } else { None };

	if enable_beefy {
		let justifications_protocol_name = beefy_on_demand_justifications_handler.protocol_name();
		let network_params = beefy::BeefyNetworkParams {
			network: network.clone(),
			sync: sync_service.clone(),
			gossip_protocol_name: beefy_gossip_proto_name,
			justifications_protocol_name,
			_phantom: core::marker::PhantomData::<Block>,
		};
		let payload_provider = beefy_primitives::mmr::MmrRootProvider::new(client.clone());
		let beefy_params = beefy::BeefyParams {
			client: client.clone(),
			backend: backend.clone(),
			payload_provider,
			runtime: client.clone(),
			key_store: keystore_opt.clone(),
			network_params,
			min_block_delta: if chain_spec.is_wococo() { 4 } else { 8 },
			prometheus_registry: prometheus_registry.clone(),
			links: beefy_links,
			on_demand_justifications_handler: beefy_on_demand_justifications_handler,
		};

		let gadget = beefy::start_beefy_gadget::<_, _, _, _, _, _, _>(beefy_params);

		// BEEFY currently only runs on testnets, if it fails we'll
		// bring the node down with it to make sure it is noticed.
		task_manager
			.spawn_essential_handle()
			.spawn_blocking("beefy-gadget", None, gadget);

		if is_offchain_indexing_enabled {
			task_manager.spawn_handle().spawn_blocking(
				"mmr-gadget",
				None,
				MmrGadget::start(
					client.clone(),
					backend.clone(),
					sp_mmr_primitives::INDEXING_PREFIX.to_vec(),
				),
			);
		}
	}

	let config = grandpa::Config {
		// FIXME substrate#1578 make this available through chainspec
		// Grandpa performance can be improved a bit by tuning this parameter, see:
		// https://github.com/paritytech/polkadot/issues/5464
		gossip_duration: Duration::from_millis(1000),
		justification_period: 512,
		name: Some(name),
		observer_enabled: false,
		keystore: keystore_opt,
		local_role: role,
		telemetry: telemetry.as_ref().map(|x| x.handle()),
		protocol_name: grandpa_protocol_name,
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
		let mut builder = grandpa::VotingRulesBuilder::default();

		#[cfg(not(feature = "malus"))]
		let _malus_finality_delay = None;

		if let Some(delay) = _malus_finality_delay {
			info!(?delay, "Enabling malus finality delay",);
			builder = builder.add(grandpa::BeforeBestBlockBy(delay));
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

				builder.add(grandpa_support::PauseAfterBlockFor(block, delay)).build()
			},
			None => builder.build(),
		};

		let grandpa_config = grandpa::GrandpaParams {
			config,
			link: link_half,
			network: network.clone(),
			sync: sync_service.clone(),
			voting_rule,
			prometheus_registry: prometheus_registry.clone(),
			shared_voter_state,
			telemetry: telemetry.as_ref().map(|x| x.handle()),
		};

		task_manager.spawn_essential_handle().spawn_blocking(
			"grandpa-voter",
			None,
			grandpa::run_grandpa_voter(grandpa_config)?,
		);
	}

	network_starter.start_network();

	Ok(NewFull {
		task_manager,
		client,
		overseer_handle,
		network,
		sync_service,
		rpc_handlers,
		backend,
	})
}

#[cfg(feature = "full-node")]
mod workers {
	use super::Error;
	use is_executable::IsExecutable;
	use std::{path::PathBuf, process::Command};

	#[cfg(test)]
	use std::sync::{Mutex, OnceLock};

	/// Override the workers polkadot binary directory path, used for testing.
	#[cfg(test)]
	fn workers_exe_path_override() -> &'static Mutex<Option<PathBuf>> {
		static OVERRIDE: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
		OVERRIDE.get_or_init(|| Mutex::new(None))
	}
	/// Override the workers lib directory path, used for testing.
	#[cfg(test)]
	fn workers_lib_path_override() -> &'static Mutex<Option<PathBuf>> {
		static OVERRIDE: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
		OVERRIDE.get_or_init(|| Mutex::new(None))
	}

	/// 1. Get the binaries from the workers path if it is passed in, or consider all possible
	/// locations on the filesystem in order and get all sets of paths at which the binaries exist.
	///
	/// 2. If no paths exist, error out. We can't proceed without workers.
	///
	/// 3. Log a warning if more than one set of paths exists. Continue with the first set of paths.
	///
	/// 4. Check if the returned paths are executable. If not it's evidence of a borked installation
	/// so error out.
	///
	/// 5. Do the version check, if mismatch error out.
	///
	/// 6. At this point the final set of paths should be good to use.
	pub fn determine_workers_paths(
		given_workers_path: Option<PathBuf>,
	) -> Result<(PathBuf, PathBuf), Error> {
		let mut workers_paths = list_workers_paths(given_workers_path.clone())?;
		if workers_paths.is_empty() {
			return Err(Error::MissingWorkerBinaries(given_workers_path))
		} else if workers_paths.len() > 1 {
			log::warn!("multiple sets of worker binaries found ({:?})", workers_paths,);
		}

		let (prep_worker_path, exec_worker_path) = workers_paths.swap_remove(0);
		if !prep_worker_path.is_executable() || !exec_worker_path.is_executable() {
			return Err(Error::InvalidWorkerBinaries(prep_worker_path, exec_worker_path))
		}

		// Do the version check.
		let node_version = env!("SUBSTRATE_CLI_IMPL_VERSION").to_string();
		let prep_worker_version =
			Command::new(&prep_worker_path).args(["--version"]).output()?.stdout;
		let prep_worker_version = std::str::from_utf8(&prep_worker_version)
			.expect("version is printed as a string; qed")
			.trim()
			.to_string();
		if prep_worker_version != node_version {
			return Err(Error::WorkerBinaryVersionMismatch(prep_worker_version, node_version))
		}
		let exec_worker_version =
			Command::new(&exec_worker_path).args(["--version"]).output()?.stdout;
		let exec_worker_version = std::str::from_utf8(&exec_worker_version)
			.expect("version is printed as a string; qed")
			.trim()
			.to_string();
		if exec_worker_version != node_version {
			return Err(Error::WorkerBinaryVersionMismatch(exec_worker_version, node_version))
		}

		// Paths are good to use.
		log::info!("using prepare-worker binary at: {:?}", prep_worker_path);
		log::info!("using execute-worker binary at: {:?}", exec_worker_path);

		Ok((prep_worker_path, exec_worker_path))
	}

	/// Get list of workers paths by considering the passed-in `given_workers_path` option, or possible
	/// locations on the filesystem. See `new_full`.
	fn list_workers_paths(
		given_workers_path: Option<PathBuf>,
	) -> Result<Vec<(PathBuf, PathBuf)>, Error> {
		if let Some(path) = given_workers_path {
			log::trace!("Using explicitly provided workers path {:?}", path);

			if path.is_executable() {
				return Ok(vec![(path.clone(), path)])
			}

			let mut prep_worker = path.clone();
			let mut exec_worker = path.clone();
			prep_worker.push(polkadot_node_core_pvf::PREPARE_BINARY_NAME);
			exec_worker.push(polkadot_node_core_pvf::EXECUTE_BINARY_NAME);

			// Check if both workers exist. Otherwise return an empty vector which results in an error.
			return if prep_worker.exists() && exec_worker.exists() {
				Ok(vec![(prep_worker, exec_worker)])
			} else {
				Ok(vec![])
			}
		}

		// Workers path not provided, check all possible valid locations.

		let mut workers_paths = vec![];

		{
			let mut exe_path = std::env::current_exe()?;
			let _ = exe_path.pop(); // executable file will always have a parent directory.
			#[cfg(test)]
			if let Some(ref path_override) = *workers_exe_path_override().lock().unwrap() {
				exe_path = path_override.clone();
			}

			let mut prep_worker = exe_path.clone();
			prep_worker.push(polkadot_node_core_pvf::PREPARE_BINARY_NAME);
			let mut exec_worker = exe_path.clone();
			exec_worker.push(polkadot_node_core_pvf::EXECUTE_BINARY_NAME);

			// Add to set if both workers exist. Warn on partial installs.
			let (prep_worker_exists, exec_worker_exists) =
				(prep_worker.exists(), exec_worker.exists());
			if prep_worker_exists && exec_worker_exists {
				log::trace!("Worker binaries found at current exe path: {:?}", exe_path);
				workers_paths.push((prep_worker, exec_worker));
			} else if prep_worker_exists {
				log::warn!("Worker binary found at {:?} but not {:?}", prep_worker, exec_worker);
			} else if exec_worker_exists {
				log::warn!("Worker binary found at {:?} but not {:?}", exec_worker, prep_worker);
			}
		}

		{
			#[allow(unused_mut)]
			let mut lib_path = PathBuf::from("/usr/lib/polkadot");
			#[cfg(test)]
			if let Some(ref path_override) = *workers_lib_path_override().lock().unwrap() {
				lib_path = path_override.clone();
			}

			let mut prep_worker = lib_path.clone();
			prep_worker.push(polkadot_node_core_pvf::PREPARE_BINARY_NAME);
			let mut exec_worker = lib_path.clone();
			exec_worker.push(polkadot_node_core_pvf::EXECUTE_BINARY_NAME);

			// Add to set if both workers exist. Warn on partial installs.
			let (prep_worker_exists, exec_worker_exists) =
				(prep_worker.exists(), exec_worker.exists());
			if prep_worker_exists && exec_worker_exists {
				log::trace!("Worker binaries found at /usr/lib/polkadot");
				workers_paths.push((prep_worker, exec_worker));
			} else if prep_worker_exists {
				log::warn!("Worker binary found at {:?} but not {:?}", prep_worker, exec_worker);
			} else if exec_worker_exists {
				log::warn!("Worker binary found at {:?} but not {:?}", exec_worker, prep_worker);
			}
		}

		Ok(workers_paths)
	}

	// Tests that set up a temporary directory tree according to what scenario we want to test and
	// run worker detection.
	#[cfg(test)]
	mod tests {
		use super::*;

		use assert_matches::assert_matches;
		use serial_test::serial;
		use std::{env::temp_dir, fs, os::unix::fs::PermissionsExt, path::Path};

		const NODE_VERSION: &'static str = env!("SUBSTRATE_CLI_IMPL_VERSION");

		/// Write a dummy executable to the path which satisfies the version check.
		fn write_worker_exe(path: impl AsRef<Path>) -> Result<(), Box<dyn std::error::Error>> {
			let program = get_program(NODE_VERSION);
			fs::write(&path, program)?;
			Ok(fs::set_permissions(&path, fs::Permissions::from_mode(0o744))?)
		}

		fn write_worker_exe_invalid_version(
			path: impl AsRef<Path>,
			version: &str,
		) -> Result<(), Box<dyn std::error::Error>> {
			let program = get_program(version);
			fs::write(&path, program)?;
			Ok(fs::set_permissions(&path, fs::Permissions::from_mode(0o744))?)
		}

		fn get_program(version: &str) -> String {
			format!(
				"#!/bin/bash

if [[ $# -ne 1 ]] ; then
    echo \"unexpected number of arguments: $#\"
    exit 1
fi

if [[ \"$1\" != \"--version\" ]] ; then
    echo \"unexpected argument: $1\"
    exit 1
fi

echo {}
",
				version
			)
		}

		fn with_temp_dir_structure(
			f: impl FnOnce(PathBuf) -> Result<(), Box<dyn std::error::Error>>,
		) -> Result<(), Box<dyn std::error::Error>> {
			// Set up <tmp>/usr/lib/polkadot and <tmp>/usr/bin, both empty.

			let tempdir = temp_dir();
			let lib_path = tempdir.join("usr/lib/polkadot");
			let _ = fs::remove_dir_all(&lib_path);
			fs::create_dir_all(&lib_path)?;
			*workers_lib_path_override().lock()? = Some(lib_path);

			let exe_path = tempdir.join("usr/bin");
			let _ = fs::remove_dir_all(&exe_path);
			fs::create_dir_all(&exe_path)?;
			*workers_exe_path_override().lock()? = Some(exe_path);

			// Set up custom path at <tmp>/usr/local/bin.
			let custom_path = tempdir.join("usr/local/bin");
			let _ = fs::remove_dir_all(&custom_path);
			fs::create_dir_all(&custom_path)?;

			f(tempdir)
		}

		#[test]
		#[serial]
		fn test_given_worker_path() {
			with_temp_dir_structure(|tempdir| {
				let given_workers_path = tempdir.join("usr/local/bin");

				// Try with provided workers path that has missing binaries.
				assert_matches!(
					determine_workers_paths(Some(given_workers_path.clone())),
					Err(Error::MissingWorkerBinaries(Some(p))) if p == given_workers_path
				);

				// Try with provided workers path that has not executable binaries.
				let prepare_worker_path = given_workers_path.join("polkadot-prepare-worker");
				write_worker_exe(&prepare_worker_path)?;
				fs::set_permissions(&prepare_worker_path, fs::Permissions::from_mode(0o644))?;
				let execute_worker_path = given_workers_path.join("polkadot-execute-worker");
				write_worker_exe(&execute_worker_path)?;
				fs::set_permissions(&execute_worker_path, fs::Permissions::from_mode(0o644))?;
				assert_matches!(
					determine_workers_paths(Some(given_workers_path.clone())),
					Err(Error::InvalidWorkerBinaries(p1, p2)) if p1 == prepare_worker_path && p2 == execute_worker_path
				);

				// Try with valid workers directory path that has executable binaries.
				fs::set_permissions(&prepare_worker_path, fs::Permissions::from_mode(0o744))?;
				fs::set_permissions(&execute_worker_path, fs::Permissions::from_mode(0o744))?;
				assert_matches!(
					determine_workers_paths(Some(given_workers_path)),
					Ok((p1, p2)) if p1 == prepare_worker_path && p2 == execute_worker_path
				);

				// Try with valid provided workers path that is a binary file.
				let given_workers_path = tempdir.join("usr/local/bin/puppet-worker");
				write_worker_exe(&given_workers_path)?;
				assert_matches!(
					determine_workers_paths(Some(given_workers_path.clone())),
					Ok((p1, p2)) if p1 == given_workers_path && p2 == given_workers_path
				);

				Ok(())
			})
			.unwrap();
		}

		#[test]
		#[serial]
		fn missing_workers_paths_throws_error() {
			with_temp_dir_structure(|tempdir| {
				// Try with both binaries missing.
				assert_matches!(
					determine_workers_paths(None),
					Err(Error::MissingWorkerBinaries(None))
				);

				// Try with only prep worker (at bin location).
				let prepare_worker_path = tempdir.join("usr/bin/polkadot-prepare-worker");
				write_worker_exe(&prepare_worker_path)?;
				assert_matches!(
					determine_workers_paths(None),
					Err(Error::MissingWorkerBinaries(None))
				);

				// Try with only exec worker (at bin location).
				fs::remove_file(&prepare_worker_path)?;
				let execute_worker_path = tempdir.join("usr/bin/polkadot-execute-worker");
				write_worker_exe(&execute_worker_path)?;
				assert_matches!(
					determine_workers_paths(None),
					Err(Error::MissingWorkerBinaries(None))
				);

				// Try with only prep worker (at lib location).
				fs::remove_file(&execute_worker_path)?;
				let prepare_worker_path = tempdir.join("usr/lib/polkadot/polkadot-prepare-worker");
				write_worker_exe(&prepare_worker_path)?;
				assert_matches!(
					determine_workers_paths(None),
					Err(Error::MissingWorkerBinaries(None))
				);

				// Try with only exec worker (at lib location).
				fs::remove_file(&prepare_worker_path)?;
				let execute_worker_path = tempdir.join("usr/lib/polkadot/polkadot-execute-worker");
				write_worker_exe(execute_worker_path)?;
				assert_matches!(
					determine_workers_paths(None),
					Err(Error::MissingWorkerBinaries(None))
				);

				Ok(())
			})
			.unwrap()
		}

		#[test]
		#[serial]
		fn should_find_workers_at_all_locations() {
			with_temp_dir_structure(|tempdir| {
				let prepare_worker_bin_path = tempdir.join("usr/bin/polkadot-prepare-worker");
				write_worker_exe(&prepare_worker_bin_path)?;

				let execute_worker_bin_path = tempdir.join("usr/bin/polkadot-execute-worker");
				write_worker_exe(&execute_worker_bin_path)?;

				let prepare_worker_lib_path = tempdir.join("usr/lib/polkadot/polkadot-prepare-worker");
				write_worker_exe(&prepare_worker_lib_path)?;

				let execute_worker_lib_path = tempdir.join("usr/lib/polkadot/polkadot-execute-worker");
				write_worker_exe(&execute_worker_lib_path)?;

				assert_matches!(
					list_workers_paths(None),
					Ok(v) if v == vec![(prepare_worker_bin_path, execute_worker_bin_path), (prepare_worker_lib_path, execute_worker_lib_path)]
				);

				Ok(())
			})
			.unwrap();
		}

		#[test]
		#[serial]
		fn workers_version_mismatch_throws_error() {
			let bad_version = "v9.9.9.9";

			with_temp_dir_structure(|tempdir| {
				// Workers at bin location return bad version.
				let prepare_worker_bin_path = tempdir.join("usr/bin/polkadot-prepare-worker");
				let execute_worker_bin_path = tempdir.join("usr/bin/polkadot-execute-worker");
				write_worker_exe_invalid_version(&prepare_worker_bin_path, bad_version)?;
				write_worker_exe(&execute_worker_bin_path)?;
				assert_matches!(
					determine_workers_paths(None),
					Err(Error::WorkerBinaryVersionMismatch(v1, v2)) if v1 == bad_version && v2 == NODE_VERSION
				);

				// Workers at lib location return bad version.
				let prepare_worker_lib_path = tempdir.join("usr/lib/polkadot-prepare-worker");
				let execute_worker_lib_path = tempdir.join("usr/lib/polkadot-execute-worker");
				write_worker_exe(&prepare_worker_lib_path)?;
				write_worker_exe_invalid_version(&execute_worker_lib_path, bad_version)?;
				assert_matches!(
					determine_workers_paths(None),
					Err(Error::WorkerBinaryVersionMismatch(v1, v2)) if v1 == bad_version && v2 == NODE_VERSION
				);

				// Workers at provided workers location return bad version.
				let given_workers_path = tempdir.join("usr/local/bin");
				let prepare_worker_path = given_workers_path.join("polkadot-prepare-worker");
				let execute_worker_path = given_workers_path.join("polkadot-execute-worker");
				write_worker_exe_invalid_version(&prepare_worker_path, bad_version)?;
				write_worker_exe_invalid_version(&execute_worker_path, bad_version)?;
				assert_matches!(
					determine_workers_paths(Some(given_workers_path)),
					Err(Error::WorkerBinaryVersionMismatch(v1, v2)) if v1 == bad_version && v2 == NODE_VERSION
				);

				// Given worker binary returns bad version.
				let given_workers_path = tempdir.join("usr/local/bin/puppet-worker");
				write_worker_exe_invalid_version(&given_workers_path, bad_version)?;
				assert_matches!(
					determine_workers_paths(Some(given_workers_path)),
					Err(Error::WorkerBinaryVersionMismatch(v1, v2)) if v1 == bad_version && v2 == NODE_VERSION
				);

				Ok(())
			})
			.unwrap();
		}

		#[test]
		#[serial]
		fn should_find_valid_workers() {
			// Test bin location.
			with_temp_dir_structure(|tempdir| {
				let prepare_worker_bin_path = tempdir.join("usr/bin/polkadot-prepare-worker");
				write_worker_exe(&prepare_worker_bin_path)?;

				let execute_worker_bin_path = tempdir.join("usr/bin/polkadot-execute-worker");
				write_worker_exe(&execute_worker_bin_path)?;

				assert_matches!(
					determine_workers_paths(None),
					Ok((p1, p2)) if p1 == prepare_worker_bin_path && p2 == execute_worker_bin_path
				);

				Ok(())
			})
			.unwrap();

			// Test lib location.
			with_temp_dir_structure(|tempdir| {
				let prepare_worker_lib_path =
					tempdir.join("usr/lib/polkadot/polkadot-prepare-worker");
				write_worker_exe(&prepare_worker_lib_path)?;

				let execute_worker_lib_path =
					tempdir.join("usr/lib/polkadot/polkadot-execute-worker");
				write_worker_exe(&execute_worker_lib_path)?;

				assert_matches!(
					determine_workers_paths(None),
					Ok((p1, p2)) if p1 == prepare_worker_lib_path && p2 == execute_worker_lib_path
				);

				Ok(())
			})
			.unwrap();
		}
	}
}

#[cfg(feature = "full-node")]
macro_rules! chain_ops {
	($config:expr, $jaeger_agent:expr, $telemetry_worker_handle:expr) => {{
		let telemetry_worker_handle = $telemetry_worker_handle;
		let jaeger_agent = $jaeger_agent;
		let mut config = $config;
		let basics = new_partial_basics(config, jaeger_agent, telemetry_worker_handle)?;

		use ::sc_consensus::LongestChain;
		// use the longest chain selection, since there is no overseer available
		let chain_selection = LongestChain::new(basics.backend.clone());

		let service::PartialComponents { client, backend, import_queue, task_manager, .. } =
			new_partial::<LongestChain<_, Block>>(&mut config, basics, chain_selection)?;
		Ok((client, backend, import_queue, task_manager))
	}};
}

/// Builds a new object suitable for chain operations.
#[cfg(feature = "full-node")]
pub fn new_chain_ops(
	config: &mut Configuration,
	jaeger_agent: Option<std::net::SocketAddr>,
) -> Result<
	(
		Arc<FullClient>,
		Arc<FullBackend>,
		sc_consensus::BasicQueue<Block, PrefixedMemoryDB<BlakeTwo256>>,
		TaskManager,
	),
	Error,
> {
	config.keystore = service::config::KeystoreConfig::InMemory;

	if config.chain_spec.is_rococo() ||
		config.chain_spec.is_wococo() ||
		config.chain_spec.is_versi()
	{
		chain_ops!(config, jaeger_agent, None)
	} else if config.chain_spec.is_kusama() {
		chain_ops!(config, jaeger_agent, None)
	} else if config.chain_spec.is_westend() {
		return chain_ops!(config, jaeger_agent, None)
	} else {
		chain_ops!(config, jaeger_agent, None)
	}
}

/// Build a full node.
///
/// The actual "flavor", aka if it will use `Polkadot`, `Rococo` or `Kusama` is determined based on
/// [`IdentifyVariant`] using the chain spec.
///
/// `overseer_enable_anyways` always enables the overseer, based on the provided `OverseerGenerator`,
/// regardless of the role the node has. The relay chain selection (longest or disputes-aware) is
/// still determined based on the role of the node. Likewise for authority discovery.
#[cfg(feature = "full-node")]
pub fn build_full<OverseerGenerator: OverseerGen>(
	config: Configuration,
	mut params: NewFullParams<OverseerGenerator>,
) -> Result<NewFull, Error> {
	let is_polkadot = config.chain_spec.is_polkadot();

	params.overseer_message_channel_capacity_override =
		params.overseer_message_channel_capacity_override.map(move |capacity| {
			if is_polkadot {
				gum::warn!("Channel capacity should _never_ be tampered with on polkadot!");
			}
			capacity
		});

	new_full(config, params)
}

/// Reverts the node state down to at most the last finalized block.
///
/// In particular this reverts:
/// - `ApprovalVotingSubsystem` data in the parachains-db;
/// - `ChainSelectionSubsystem` data in the parachains-db;
/// - Low level Babe and Grandpa consensus data.
#[cfg(feature = "full-node")]
pub fn revert_backend(
	client: Arc<FullClient>,
	backend: Arc<FullBackend>,
	blocks: BlockNumber,
	config: Configuration,
) -> Result<(), Error> {
	let best_number = client.info().best_number;
	let finalized = client.info().finalized_number;
	let revertible = blocks.min(best_number - finalized);

	if revertible == 0 {
		return Ok(())
	}

	let number = best_number - revertible;
	let hash = client.block_hash_from_id(&BlockId::Number(number))?.ok_or(
		sp_blockchain::Error::Backend(format!(
			"Unexpected hash lookup failure for block number: {}",
			number
		)),
	)?;

	let parachains_db = open_database(&config.database)
		.map_err(|err| sp_blockchain::Error::Backend(err.to_string()))?;

	revert_approval_voting(parachains_db.clone(), hash)?;
	revert_chain_selection(parachains_db, hash)?;
	// Revert Substrate consensus related components
	babe::revert(client.clone(), backend, blocks)?;
	grandpa::revert(client, blocks)?;

	Ok(())
}

fn revert_chain_selection(db: Arc<dyn Database>, hash: Hash) -> sp_blockchain::Result<()> {
	let config = chain_selection_subsystem::Config {
		col_data: parachains_db::REAL_COLUMNS.col_chain_selection_data,
		stagnant_check_interval: chain_selection_subsystem::StagnantCheckInterval::never(),
		stagnant_check_mode: chain_selection_subsystem::StagnantCheckMode::PruneOnly,
	};

	let chain_selection = chain_selection_subsystem::ChainSelectionSubsystem::new(config, db);

	chain_selection
		.revert_to(hash)
		.map_err(|err| sp_blockchain::Error::Backend(err.to_string()))
}

fn revert_approval_voting(db: Arc<dyn Database>, hash: Hash) -> sp_blockchain::Result<()> {
	let config = approval_voting_subsystem::Config {
		col_approval_data: parachains_db::REAL_COLUMNS.col_approval_data,
		slot_duration_millis: Default::default(),
	};

	let approval_voting = approval_voting_subsystem::ApprovalVotingSubsystem::with_config(
		config,
		db,
		Arc::new(sc_keystore::LocalKeystore::in_memory()),
		Box::new(consensus_common::NoNetwork),
		approval_voting_subsystem::Metrics::default(),
	);

	approval_voting
		.revert_to(hash)
		.map_err(|err| sp_blockchain::Error::Backend(err.to_string()))
}
