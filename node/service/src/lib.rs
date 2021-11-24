// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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
mod relay_chain_selection;

#[cfg(feature = "full-node")]
pub mod overseer;

#[cfg(feature = "full-node")]
pub use self::overseer::{OverseerGen, OverseerGenArgs, RealOverseerGen};

#[cfg(all(test, feature = "disputes"))]
mod tests;

#[cfg(feature = "full-node")]
use {
	grandpa::{self, FinalityProofProvider as GrandpaFinalityProofProvider},
	polkadot_node_core_approval_voting::Config as ApprovalVotingConfig,
	polkadot_node_core_av_store::Config as AvailabilityConfig,
	polkadot_node_core_av_store::Error as AvailabilityError,
	polkadot_node_core_candidate_validation::Config as CandidateValidationConfig,
	polkadot_node_core_chain_selection::{
		self as chain_selection_subsystem, Config as ChainSelectionConfig,
	},
	polkadot_node_core_dispute_coordinator::Config as DisputeCoordinatorConfig,
	polkadot_overseer::BlockInfo,
	sc_client_api::ExecutorProvider,
	sp_trie::PrefixedMemoryDB,
	tracing::info,
};

pub use sp_core::traits::SpawnNamed;
#[cfg(feature = "full-node")]
pub use {
	polkadot_overseer::{Handle, Overseer, OverseerConnector, OverseerHandle},
	polkadot_primitives::v1::ParachainHost,
	relay_chain_selection::SelectRelayChain,
	sc_client_api::AuxStore,
	sp_authority_discovery::AuthorityDiscoveryApi,
	sp_blockchain::HeaderBackend,
	sp_consensus_babe::BabeApi,
};

#[cfg(feature = "full-node")]
use polkadot_subsystem::jaeger;

use std::{sync::Arc, time::Duration};

use prometheus_endpoint::Registry;
#[cfg(feature = "full-node")]
use service::KeystoreContainer;
use service::RpcHandlers;
use telemetry::TelemetryWorker;
#[cfg(feature = "full-node")]
use telemetry::{Telemetry, TelemetryWorkerHandle};

#[cfg(feature = "rococo-native")]
pub use polkadot_client::RococoExecutorDispatch;

#[cfg(feature = "westend-native")]
pub use polkadot_client::WestendExecutorDispatch;

#[cfg(feature = "kusama-native")]
pub use polkadot_client::KusamaExecutorDispatch;

#[cfg(feature = "polkadot-native")]
pub use polkadot_client::PolkadotExecutorDispatch;

pub use chain_spec::{KusamaChainSpec, PolkadotChainSpec, RococoChainSpec, WestendChainSpec};
pub use consensus_common::{block_validation::Chain, Proposal, SelectChain};
#[cfg(feature = "full-node")]
pub use polkadot_client::{
	AbstractClient, Client, ClientHandle, ExecuteWithClient, FullBackend, FullClient,
	RuntimeApiCollection,
};
pub use polkadot_primitives::v1::{Block, BlockId, CollatorPair, Hash, Id as ParaId};
pub use sc_client_api::{Backend, CallExecutor, ExecutionStrategy};
pub use sc_consensus::{BlockImport, LongestChain};
use sc_executor::NativeElseWasmExecutor;
pub use sc_executor::NativeExecutionDispatch;
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
pub use kusama_runtime;
#[cfg(feature = "polkadot-native")]
pub use polkadot_runtime;
#[cfg(feature = "rococo-native")]
pub use rococo_runtime;
#[cfg(feature = "westend-native")]
pub use westend_runtime;

/// The maximum number of active leaves we forward to the [`Overseer`] on startup.
#[cfg(any(test, feature = "full-node"))]
const MAX_ACTIVE_LEAVES: usize = 4;

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
		<Self as sp_blockchain::HeaderBackend<Block>>::header(
			self,
			generic::BlockId::<Block>::Hash(hash),
		)
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
	Jaeger(#[from] polkadot_subsystem::jaeger::JaegerError),

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
type FullGrandpaBlockImport<RuntimeApi, ExecutorDispatch, ChainSelection = FullSelectChain> =
	grandpa::GrandpaBlockImport<
		FullBackend,
		Block,
		FullClient<RuntimeApi, ExecutorDispatch>,
		ChainSelection,
	>;

#[cfg(feature = "full-node")]
struct Basics<RuntimeApi, ExecutorDispatch>
where
	RuntimeApi: ConstructRuntimeApi<Block, FullClient<RuntimeApi, ExecutorDispatch>>
		+ Send
		+ Sync
		+ 'static,
	RuntimeApi::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
	ExecutorDispatch: NativeExecutionDispatch + 'static,
{
	task_manager: TaskManager,
	client: Arc<FullClient<RuntimeApi, ExecutorDispatch>>,
	backend: Arc<FullBackend>,
	keystore_container: KeystoreContainer,
	telemetry: Option<Telemetry>,
}

#[cfg(feature = "full-node")]
fn new_partial_basics<RuntimeApi, ExecutorDispatch>(
	config: &mut Configuration,
	jaeger_agent: Option<std::net::SocketAddr>,
	telemetry_worker_handle: Option<TelemetryWorkerHandle>,
) -> Result<Basics<RuntimeApi, ExecutorDispatch>, Error>
where
	RuntimeApi: ConstructRuntimeApi<Block, FullClient<RuntimeApi, ExecutorDispatch>>
		+ Send
		+ Sync
		+ 'static,
	RuntimeApi::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
	ExecutorDispatch: NativeExecutionDispatch + 'static,
{
	set_prometheus_registry(config)?;

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

	let executor = NativeElseWasmExecutor::<ExecutorDispatch>::new(
		config.wasm_method,
		config.default_heap_pages,
		config.max_runtime_instances,
	);

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
fn new_partial<RuntimeApi, ExecutorDispatch, ChainSelection>(
	config: &mut Configuration,
	Basics { task_manager, backend, client, keystore_container, telemetry }: Basics<
		RuntimeApi,
		ExecutorDispatch,
	>,
	select_chain: ChainSelection,
) -> Result<
	service::PartialComponents<
		FullClient<RuntimeApi, ExecutorDispatch>,
		FullBackend,
		ChainSelection,
		sc_consensus::DefaultImportQueue<Block, FullClient<RuntimeApi, ExecutorDispatch>>,
		sc_transaction_pool::FullPool<Block, FullClient<RuntimeApi, ExecutorDispatch>>,
		(
			impl service::RpcExtensionBuilder,
			(
				babe::BabeBlockImport<
					Block,
					FullClient<RuntimeApi, ExecutorDispatch>,
					FullGrandpaBlockImport<RuntimeApi, ExecutorDispatch, ChainSelection>,
				>,
				grandpa::LinkHalf<Block, FullClient<RuntimeApi, ExecutorDispatch>, ChainSelection>,
				babe::BabeLink<Block>,
				beefy_gadget::notification::BeefySignedCommitmentSender<Block>,
			),
			grandpa::SharedVoterState,
			std::time::Duration, // slot-duration
			Option<Telemetry>,
		),
	>,
	Error,
>
where
	RuntimeApi: ConstructRuntimeApi<Block, FullClient<RuntimeApi, ExecutorDispatch>>
		+ Send
		+ Sync
		+ 'static,
	RuntimeApi::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
	ExecutorDispatch: NativeExecutionDispatch + 'static,
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

	let babe_config = babe::Config::get_or_compute(&*client)?;
	let (block_import, babe_link) =
		babe::block_import(babe_config.clone(), grandpa_block_import, client.clone())?;

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

			polkadot_rpc::create_full(deps).map_err(Into::into)
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
pub struct NewFull<C> {
	pub task_manager: TaskManager,
	pub client: C,
	pub overseer_handle: Option<Handle>,
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
			overseer_handle: self.overseer_handle,
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
async fn active_leaves<RuntimeApi, ExecutorDispatch>(
	select_chain: &impl SelectChain<Block>,
	client: &FullClient<RuntimeApi, ExecutorDispatch>,
) -> Result<Vec<BlockInfo>, Error>
where
	RuntimeApi: ConstructRuntimeApi<Block, FullClient<RuntimeApi, ExecutorDispatch>>
		+ Send
		+ Sync
		+ 'static,
	RuntimeApi::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
	ExecutorDispatch: NativeExecutionDispatch + 'static,
{
	let best_block = select_chain.best_chain().await?;

	let mut leaves = select_chain
		.leaves()
		.await
		.unwrap_or_default()
		.into_iter()
		.filter_map(|hash| {
			let number = HeaderBackend::number(client, hash).ok()??;

			// Only consider leaves that are in maximum an uncle of the best block.
			if number < best_block.number().saturating_sub(1) {
				return None
			} else if hash == best_block.hash() {
				return None
			};

			let parent_hash = client.header(&BlockId::Hash(hash)).ok()??.parent_hash;

			Some(BlockInfo { hash, parent_hash, number })
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
pub fn new_full<RuntimeApi, ExecutorDispatch, OverseerGenerator>(
	mut config: Configuration,
	is_collator: IsCollator,
	grandpa_pause: Option<(u32, u32)>,
	enable_beefy: bool,
	jaeger_agent: Option<std::net::SocketAddr>,
	telemetry_worker_handle: Option<TelemetryWorkerHandle>,
	program_path: Option<std::path::PathBuf>,
	overseer_gen: OverseerGenerator,
) -> Result<NewFull<Arc<FullClient<RuntimeApi, ExecutorDispatch>>>, Error>
where
	RuntimeApi: ConstructRuntimeApi<Block, FullClient<RuntimeApi, ExecutorDispatch>>
		+ Send
		+ Sync
		+ 'static,
	RuntimeApi::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
	ExecutorDispatch: NativeExecutionDispatch + 'static,
	OverseerGenerator: OverseerGen,
{
	use polkadot_node_network_protocol::request_response::IncomingRequest;

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

	let basics = new_partial_basics::<RuntimeApi, ExecutorDispatch>(
		&mut config,
		jaeger_agent,
		telemetry_worker_handle,
	)?;

	let prometheus_registry = config.prometheus_registry().cloned();

	let overseer_connector = OverseerConnector::default();
	let overseer_handle = Handle::new(overseer_connector.handle());

	let chain_spec = config.chain_spec.cloned_box();

	let local_keystore = basics.keystore_container.local_keystore();
	let requires_overseer_for_chain_sel =
		local_keystore.is_some() && (role.is_authority() || is_collator.is_collator());

	let select_chain = SelectRelayChain::new(
		basics.backend.clone(),
		overseer_handle.clone(),
		requires_overseer_for_chain_sel,
		polkadot_node_subsystem_util::metrics::Metrics::register(prometheus_registry.as_ref())?,
	);

	let service::PartialComponents::<_, _, SelectRelayChain<_>, _, _, _> {
		client,
		backend,
		mut task_manager,
		keystore_container,
		select_chain,
		import_queue,
		transaction_pool,
		other: (rpc_extensions_builder, import_setup, rpc_setup, slot_duration, mut telemetry),
	} = new_partial::<RuntimeApi, ExecutorDispatch, SelectRelayChain<_>>(
		&mut config,
		basics,
		select_chain,
	)?;

	let shared_voter_state = rpc_setup;
	let auth_disc_publish_non_global_ips = config.network.allow_non_globals_in_dht;

	// Note: GrandPa is pushed before the Polkadot-specific protocols. This doesn't change
	// anything in terms of behaviour, but makes the logs more consistent with the other
	// Substrate nodes.
	config.network.extra_sets.push(grandpa::grandpa_peers_set_config());

	if chain_spec.is_rococo() || chain_spec.is_wococo() {
		config.network.extra_sets.push(beefy_gadget::beefy_peers_set_config());
	}

	{
		use polkadot_network_bridge::{peer_sets_info, IsAuthority};
		let is_authority = if role.is_authority() { IsAuthority::Yes } else { IsAuthority::No };
		config.network.extra_sets.extend(peer_sets_info(is_authority));
	}

	let (pov_req_receiver, cfg) = IncomingRequest::get_config_receiver();
	config.network.request_response_protocols.push(cfg);
	let (chunk_req_receiver, cfg) = IncomingRequest::get_config_receiver();
	config.network.request_response_protocols.push(cfg);
	let (collation_req_receiver, cfg) = IncomingRequest::get_config_receiver();
	config.network.request_response_protocols.push(cfg);
	let (available_data_req_receiver, cfg) = IncomingRequest::get_config_receiver();
	config.network.request_response_protocols.push(cfg);
	let (statement_req_receiver, cfg) = IncomingRequest::get_config_receiver();
	config.network.request_response_protocols.push(cfg);
	let (dispute_req_receiver, cfg) = IncomingRequest::get_config_receiver();
	config.network.request_response_protocols.push(cfg);

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

	let (network, system_rpc_tx, network_starter) =
		service::build_network(service::BuildNetworkParams {
			config: &config,
			client: client.clone(),
			transaction_pool: transaction_pool.clone(),
			spawn_handle: task_manager.spawn_handle(),
			import_queue,
			block_announce_validator_builder: None,
			warp_sync: Some(warp_sync),
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
		artifacts_cache_path: config
			.database
			.path()
			.ok_or(Error::DatabasePathRequired)?
			.join("pvf-artifacts"),
		program_path: match program_path {
			None => std::env::current_exe()?,
			Some(p) => p,
		},
	};

	let chain_selection_config = ChainSelectionConfig {
		col_data: crate::parachains_db::REAL_COLUMNS.col_chain_selection_data,
		stagnant_check_interval: chain_selection_subsystem::StagnantCheckInterval::never(),
	};

	let dispute_coordinator_config = DisputeCoordinatorConfig {
		col_data: crate::parachains_db::REAL_COLUMNS.col_dispute_coordinator_data,
	};

	let rpc_handlers = service::spawn_tasks(service::SpawnTasksParams {
		config,
		backend: backend.clone(),
		client: client.clone(),
		keystore: keystore_container.sync_keystore(),
		network: network.clone(),
		rpc_extensions_builder: Box::new(rpc_extensions_builder),
		transaction_pool: transaction_pool.clone(),
		task_manager: &mut task_manager,
		system_rpc_tx,
		telemetry: telemetry.as_mut(),
	})?;

	let (block_import, link_half, babe_link, beefy_link) = import_setup;

	let overseer_client = client.clone();
	let spawner = task_manager.spawn_handle();
	// Cannot use the `RelayChainSelection`, since that'd require a setup _and running_ overseer
	// which we are about to setup.
	let active_leaves =
		futures::executor::block_on(active_leaves(select_chain.as_longest_chain(), &*client))?;

	let authority_discovery_service = if role.is_authority() || is_collator.is_collator() {
		use futures::StreamExt;
		use sc_network::Event;

		let authority_discovery_role = if role.is_authority() {
			sc_authority_discovery::Role::PublishAndDiscover(keystore_container.keystore())
		} else {
			// don't publish our addresses when we're only a collator
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

	if local_keystore.is_none() {
		tracing::info!("Cannot run as validator without local keystore.");
	}

	let maybe_params =
		local_keystore.and_then(move |k| authority_discovery_service.map(|a| (a, k)));

	let overseer_handle = if let Some((authority_discovery_service, keystore)) = maybe_params {
		let (overseer, overseer_handle) = overseer_gen
			.generate::<service::SpawnTaskHandle, FullClient<RuntimeApi, ExecutorDispatch>>(
				overseer_connector,
				OverseerGenArgs {
					leaves: active_leaves,
					keystore,
					runtime_client: overseer_client.clone(),
					parachains_db,
					network_service: network.clone(),
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
					availability_config,
					candidate_validation_config,
					chain_selection_config,
					dispute_coordinator_config,
				},
			)
			.map_err(|e| {
				tracing::error!("Failed to init overseer: {}", e);
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
						_ = forward => (),
						_ = overseer_fut => (),
						complete => (),
					}
				}),
			);
		}
		Some(handle)
	} else {
		assert!(
			!requires_overseer_for_chain_sel,
			"Precondition congruence (false) is guaranteed by manual checking. qed"
		);
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
		let overseer_handle =
			overseer_handle.as_ref().ok_or(Error::AuthoritiesRequireRealOverseer)?.clone();
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
				let overseer_handle = overseer_handle.clone();

				async move {
					let parachain = polkadot_node_core_parachains_inherent::ParachainsInherentDataProvider::create(
						&*client_clone,
						overseer_handle,
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
			max_block_proposal_slot_portion: None,
			telemetry: telemetry.as_ref().map(|x| x.handle()),
		};

		let babe = babe::start_babe(babe_config)?;
		task_manager.spawn_essential_handle().spawn_blocking("babe", None, babe);
	}

	// if the node isn't actively participating in consensus then it doesn't
	// need a keystore, regardless of which protocol we use below.
	let keystore_opt =
		if role.is_authority() { Some(keystore_container.sync_keystore()) } else { None };

	// We currently only run the BEEFY gadget on the Rococo and Wococo testnets.
	if enable_beefy && (chain_spec.is_rococo() || chain_spec.is_wococo()) {
		let beefy_params = beefy_gadget::BeefyParams {
			client: client.clone(),
			backend: backend.clone(),
			key_store: keystore_opt.clone(),
			network: network.clone(),
			signed_commitment_sender: beefy_link,
			min_block_delta: if chain_spec.is_wococo() { 4 } else { 8 },
			prometheus_registry: prometheus_registry.clone(),
		};

		let gadget = beefy_gadget::start_beefy_gadget::<_, _, _, _>(beefy_params);

		// Wococo's purpose is to be a testbed for BEEFY, so if it fails we'll
		// bring the node down with it to make sure it is noticed.
		if chain_spec.is_wococo() {
			task_manager
				.spawn_essential_handle()
				.spawn_blocking("beefy-gadget", None, gadget);
		} else {
			task_manager.spawn_handle().spawn_blocking("beefy-gadget", None, gadget);
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

	Ok(NewFull { task_manager, client, overseer_handle, network, rpc_handlers, backend })
}

#[cfg(feature = "full-node")]
macro_rules! chain_ops {
	($config:expr, $jaeger_agent:expr, $telemetry_worker_handle:expr; $scope:ident, $executor:ident, $variant:ident) => {{
		let telemetry_worker_handle = $telemetry_worker_handle;
		let jaeger_agent = $jaeger_agent;
		let mut config = $config;
		let basics = new_partial_basics::<$scope::RuntimeApi, $executor>(
			config,
			jaeger_agent,
			telemetry_worker_handle,
		)?;

		use ::sc_consensus::LongestChain;
		// use the longest chain selection, since there is no overseer available
		let chain_selection = LongestChain::new(basics.backend.clone());

		let service::PartialComponents { client, backend, import_queue, task_manager, .. } =
			new_partial::<$scope::RuntimeApi, $executor, LongestChain<_, Block>>(
				&mut config,
				basics,
				chain_selection,
			)?;
		Ok((Arc::new(Client::$variant(client)), backend, import_queue, task_manager))
	}};
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
		sc_consensus::BasicQueue<Block, PrefixedMemoryDB<BlakeTwo256>>,
		TaskManager,
	),
	Error,
> {
	config.keystore = service::config::KeystoreConfig::InMemory;

	let telemetry_worker_handle = None;

	#[cfg(feature = "rococo-native")]
	if config.chain_spec.is_rococo() || config.chain_spec.is_wococo() {
		return chain_ops!(config, jaeger_agent, telemetry_worker_handle; rococo_runtime, RococoExecutorDispatch, Rococo)
	}

	#[cfg(feature = "kusama-native")]
	if config.chain_spec.is_kusama() {
		return chain_ops!(config, jaeger_agent, telemetry_worker_handle; kusama_runtime, KusamaExecutorDispatch, Kusama)
	}

	#[cfg(feature = "westend-native")]
	if config.chain_spec.is_westend() {
		return chain_ops!(config, jaeger_agent, telemetry_worker_handle; westend_runtime, WestendExecutorDispatch, Westend)
	}

	#[cfg(feature = "polkadot-native")]
	{
		return chain_ops!(config, jaeger_agent, telemetry_worker_handle; polkadot_runtime, PolkadotExecutorDispatch, Polkadot)
	}
	#[cfg(not(feature = "polkadot-native"))]
	Err(Error::NoRuntime)
}

#[cfg(feature = "full-node")]
pub fn build_full(
	config: Configuration,
	is_collator: IsCollator,
	grandpa_pause: Option<(u32, u32)>,
	enable_beefy: bool,
	jaeger_agent: Option<std::net::SocketAddr>,
	telemetry_worker_handle: Option<TelemetryWorkerHandle>,
	overseer_gen: impl OverseerGen,
) -> Result<NewFull<Client>, Error> {
	#[cfg(feature = "rococo-native")]
	if config.chain_spec.is_rococo() || config.chain_spec.is_wococo() {
		return new_full::<rococo_runtime::RuntimeApi, RococoExecutorDispatch, _>(
			config,
			is_collator,
			grandpa_pause,
			enable_beefy,
			jaeger_agent,
			telemetry_worker_handle,
			None,
			overseer_gen,
		)
		.map(|full| full.with_client(Client::Rococo))
	}

	#[cfg(feature = "kusama-native")]
	if config.chain_spec.is_kusama() {
		return new_full::<kusama_runtime::RuntimeApi, KusamaExecutorDispatch, _>(
			config,
			is_collator,
			grandpa_pause,
			enable_beefy,
			jaeger_agent,
			telemetry_worker_handle,
			None,
			overseer_gen,
		)
		.map(|full| full.with_client(Client::Kusama))
	}

	#[cfg(feature = "westend-native")]
	if config.chain_spec.is_westend() {
		return new_full::<westend_runtime::RuntimeApi, WestendExecutorDispatch, _>(
			config,
			is_collator,
			grandpa_pause,
			enable_beefy,
			jaeger_agent,
			telemetry_worker_handle,
			None,
			overseer_gen,
		)
		.map(|full| full.with_client(Client::Westend))
	}

	#[cfg(feature = "polkadot-native")]
	{
		return new_full::<polkadot_runtime::RuntimeApi, PolkadotExecutorDispatch, _>(
			config,
			is_collator,
			grandpa_pause,
			enable_beefy,
			jaeger_agent,
			telemetry_worker_handle,
			None,
			overseer_gen,
		)
		.map(|full| full.with_client(Client::Polkadot))
	}

	#[cfg(not(feature = "polkadot-native"))]
	Err(Error::NoRuntime)
}
