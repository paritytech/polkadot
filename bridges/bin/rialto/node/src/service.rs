// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Rialto chain node service.
//!
//! The code is mostly copy of `service/src/lib.rs` file from Polkadot repository
//! without optional functions, and with BEEFY added on top.

use crate::overseer::{OverseerGen, OverseerGenArgs};

use polkadot_client::RuntimeApiCollection;
use polkadot_node_core_approval_voting::Config as ApprovalVotingConfig;
use polkadot_node_core_av_store::Config as AvailabilityConfig;
use polkadot_node_core_candidate_validation::Config as CandidateValidationConfig;
use polkadot_node_core_chain_selection::Config as ChainSelectionConfig;
use polkadot_node_core_dispute_coordinator::Config as DisputeCoordinatorConfig;
use polkadot_node_network_protocol::request_response::IncomingRequest;
use polkadot_overseer::{BlockInfo, OverseerConnector};
use polkadot_primitives::v1::BlockId;
use rialto_runtime::{self, opaque::Block, RuntimeApi};
use sc_client_api::ExecutorProvider;
use sc_executor::{NativeElseWasmExecutor, NativeExecutionDispatch};
use sc_finality_grandpa::FinalityProofProvider as GrandpaFinalityProofProvider;
use sc_service::{config::PrometheusConfig, Configuration, TaskManager};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sp_api::{ConstructRuntimeApi, HeaderT};
use sp_consensus::SelectChain;
use sp_runtime::traits::Block as BlockT;
use std::{sync::Arc, time::Duration};
use substrate_prometheus_endpoint::Registry;

pub use polkadot_overseer::Handle;
pub use polkadot_primitives::v1::ParachainHost;
pub use sc_client_api::AuxStore;
pub use sp_authority_discovery::AuthorityDiscoveryApi;
pub use sp_blockchain::HeaderBackend;
pub use sp_consensus_babe::BabeApi;

pub type Executor = NativeElseWasmExecutor<ExecutorDispatch>;

// Our native executor instance.
pub struct ExecutorDispatch;

impl sc_executor::NativeExecutionDispatch for ExecutorDispatch {
	type ExtendHostFunctions = frame_benchmarking::benchmarking::HostFunctions;

	fn dispatch(method: &str, data: &[u8]) -> Option<Vec<u8>> {
		rialto_runtime::api::dispatch(method, data)
	}

	fn native_version() -> sc_executor::NativeVersion {
		rialto_runtime::native_version()
	}
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error(transparent)]
	Io(#[from] std::io::Error),

	#[error(transparent)]
	Cli(#[from] sc_cli::Error),

	#[error(transparent)]
	Blockchain(#[from] sp_blockchain::Error),

	#[error(transparent)]
	Consensus(#[from] sp_consensus::Error),

	#[error(transparent)]
	Service(#[from] sc_service::Error),

	#[error(transparent)]
	Telemetry(#[from] sc_telemetry::Error),

	#[error("Failed to create an overseer")]
	Overseer(#[from] polkadot_overseer::SubsystemError),

	#[error(transparent)]
	Prometheus(#[from] substrate_prometheus_endpoint::PrometheusError),

	#[error("Authorities require the real overseer implementation")]
	AuthoritiesRequireRealOverseer,

	#[error("Creating a custom database is required for validators")]
	DatabasePathRequired,
}

type FullClient = sc_service::TFullClient<Block, RuntimeApi, Executor>;
type FullBackend = sc_service::TFullBackend<Block>;
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;
type FullGrandpaBlockImport =
	sc_finality_grandpa::GrandpaBlockImport<FullBackend, Block, FullClient, FullSelectChain>;
type FullTransactionPool = sc_transaction_pool::FullPool<Block, FullClient>;
type FullBabeBlockImport =
	sc_consensus_babe::BabeBlockImport<Block, FullClient, FullGrandpaBlockImport>;
type FullBabeLink = sc_consensus_babe::BabeLink<Block>;
type FullGrandpaLink = sc_finality_grandpa::LinkHalf<Block, FullClient, FullSelectChain>;

// If we're using prometheus, use a registry with a prefix of `polkadot`.
fn set_prometheus_registry(config: &mut Configuration) -> Result<(), Error> {
	if let Some(PrometheusConfig { registry, .. }) = config.prometheus_config.as_mut() {
		*registry = Registry::new_custom(Some("polkadot".into()), None)?;
	}

	Ok(())
}

// Needed here for complex return type while `impl Trait` in type aliases is unstable.
#[allow(clippy::type_complexity)]
pub fn new_partial(
	config: &mut Configuration,
) -> Result<
	sc_service::PartialComponents<
		FullClient,
		FullBackend,
		FullSelectChain,
		sc_consensus::DefaultImportQueue<Block, FullClient>,
		FullTransactionPool,
		(
			impl Fn(
				sc_rpc::DenyUnsafe,
				sc_rpc::SubscriptionTaskExecutor,
			) -> Result<jsonrpc_core::IoHandler<sc_service::RpcMetadata>, sc_service::Error>,
			(
				FullBabeBlockImport,
				FullGrandpaLink,
				FullBabeLink,
				beefy_gadget::notification::BeefySignedCommitmentSender<Block>,
			),
			sc_finality_grandpa::SharedVoterState,
			std::time::Duration,
			Option<Telemetry>,
		),
	>,
	Error,
>
where
	RuntimeApi: ConstructRuntimeApi<Block, FullClient> + Send + Sync + 'static,
	<RuntimeApi as ConstructRuntimeApi<Block, FullClient>>::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
	ExecutorDispatch: NativeExecutionDispatch + 'static,
{
	set_prometheus_registry(config)?;

	let telemetry = config
		.telemetry_endpoints
		.clone()
		.filter(|x| !x.is_empty())
		.map(|endpoints| -> Result<_, sc_telemetry::Error> {
			let worker = TelemetryWorker::new(16)?;
			let telemetry = worker.handle().new_telemetry(endpoints);
			Ok((worker, telemetry))
		})
		.transpose()?;

	let executor = NativeElseWasmExecutor::<ExecutorDispatch>::new(
		config.wasm_method,
		config.default_heap_pages,
		config.max_runtime_instances,
		config.runtime_cache_size,
	);

	let (client, backend, keystore_container, task_manager) =
		sc_service::new_full_parts::<Block, RuntimeApi, Executor>(
			config,
			telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
			executor,
		)?;
	let client = Arc::new(client);

	let telemetry = telemetry.map(|(worker, telemetry)| {
		task_manager.spawn_handle().spawn("telemetry", None, worker.run());
		telemetry
	});

	let select_chain = sc_consensus::LongestChain::new(backend.clone());

	let transaction_pool = sc_transaction_pool::BasicPool::new_full(
		config.transaction_pool.clone(),
		config.role.is_authority().into(),
		config.prometheus_registry(),
		task_manager.spawn_essential_handle(),
		client.clone(),
	);

	let (grandpa_block_import, grandpa_link) =
		sc_finality_grandpa::block_import_with_authority_set_hard_forks(
			client.clone(),
			&(client.clone() as Arc<_>),
			select_chain.clone(),
			Vec::new(),
			telemetry.as_ref().map(|x| x.handle()),
		)?;
	let justification_import = grandpa_block_import.clone();

	let babe_config = sc_consensus_babe::Config::get_or_compute(&*client)?;
	let (block_import, babe_link) =
		sc_consensus_babe::block_import(babe_config.clone(), grandpa_block_import, client.clone())?;

	let slot_duration = babe_link.config().slot_duration();
	let import_queue = sc_consensus_babe::import_queue(
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
		sp_consensus::CanAuthorWithNativeVersion::new(client.executor().clone()),
		telemetry.as_ref().map(|x| x.handle()),
	)?;

	let justification_stream = grandpa_link.justification_stream();
	let shared_authority_set = grandpa_link.shared_authority_set().clone();
	let shared_voter_state = sc_finality_grandpa::SharedVoterState::empty();

	let (signed_commitment_sender, signed_commitment_stream) =
		beefy_gadget::notification::BeefySignedCommitmentStream::channel();

	let import_setup = (block_import, grandpa_link, babe_link, signed_commitment_sender);
	let rpc_setup = shared_voter_state.clone();

	let slot_duration = babe_config.slot_duration();

	let rpc_extensions_builder = {
		let client = client.clone();
		let transaction_pool = transaction_pool.clone();
		let backend = backend.clone();

		move |deny_unsafe,
		      subscription_executor: sc_rpc::SubscriptionTaskExecutor|
		      -> Result<jsonrpc_core::IoHandler<sc_service::RpcMetadata>, sc_service::Error> {
			use pallet_transaction_payment_rpc::{TransactionPayment, TransactionPaymentApi};
			use sc_finality_grandpa_rpc::{GrandpaApi, GrandpaRpcHandler};
			use substrate_frame_rpc_system::{FullSystem, SystemApi};

			let backend = backend.clone();
			let client = client.clone();
			let pool = transaction_pool.clone();

			let shared_voter_state = shared_voter_state.clone();

			let finality_proof_provider = GrandpaFinalityProofProvider::new_for_service(
				backend,
				Some(shared_authority_set.clone()),
			);

			let mut io = jsonrpc_core::IoHandler::default();
			io.extend_with(SystemApi::to_delegate(FullSystem::new(
				client.clone(),
				pool,
				deny_unsafe,
			)));
			io.extend_with(TransactionPaymentApi::to_delegate(TransactionPayment::new(
				client.clone(),
			)));
			io.extend_with(GrandpaApi::to_delegate(GrandpaRpcHandler::new(
				shared_authority_set.clone(),
				shared_voter_state,
				justification_stream.clone(),
				subscription_executor.clone(),
				finality_proof_provider,
			)));
			io.extend_with(beefy_gadget_rpc::BeefyApi::to_delegate(
				beefy_gadget_rpc::BeefyRpcHandler::new(
					signed_commitment_stream.clone(),
					subscription_executor,
				),
			));
			io.extend_with(pallet_mmr_rpc::MmrApi::to_delegate(pallet_mmr_rpc::Mmr::new(client)));

			Ok(io)
		}
	};

	Ok(sc_service::PartialComponents {
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

pub struct NewFull<C> {
	pub task_manager: TaskManager,
	pub client: C,
	pub overseer_handle: Option<Handle>,
	pub network: Arc<sc_network::NetworkService<Block, <Block as BlockT>::Hash>>,
	pub rpc_handlers: sc_service::RpcHandlers,
	pub backend: Arc<FullBackend>,
}

/// The maximum number of active leaves we forward to the [`Overseer`] on start up.
const MAX_ACTIVE_LEAVES: usize = 4;

/// Returns the active leaves the overseer should start with.
async fn active_leaves(
	select_chain: &sc_consensus::LongestChain<FullBackend, Block>,
	client: &FullClient,
) -> Result<Vec<BlockInfo>, Error>
where
	RuntimeApi: ConstructRuntimeApi<Block, FullClient> + Send + Sync + 'static,
	<RuntimeApi as ConstructRuntimeApi<Block, FullClient>>::RuntimeApi:
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
			let number = client.number(hash).ok()??;

			// Only consider leaves that are in maximum an uncle of the best block.
			if number < best_block.number().saturating_sub(1) || hash == best_block.hash() {
				return None
			}

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

// Create a new full node.
pub fn new_full(
	mut config: Configuration,
	program_path: Option<std::path::PathBuf>,
	overseer_gen: impl OverseerGen,
) -> Result<NewFull<Arc<FullClient>>, Error>
where
	RuntimeApi: ConstructRuntimeApi<Block, FullClient> + Send + Sync + 'static,
	<RuntimeApi as ConstructRuntimeApi<Block, FullClient>>::RuntimeApi:
		RuntimeApiCollection<StateBackend = sc_client_api::StateBackendFor<FullBackend, Block>>,
	ExecutorDispatch: NativeExecutionDispatch + 'static,
{
	let is_collator = false;

	let role = config.role.clone();
	let force_authoring = config.force_authoring;
	let backoff_authoring_blocks =
		Some(sc_consensus_slots::BackoffAuthoringOnFinalizedHeadLagging::default());

	let disable_grandpa = config.disable_grandpa;
	let name = config.network.node_name.clone();

	let sc_service::PartialComponents {
		client,
		backend,
		mut task_manager,
		keystore_container,
		select_chain,
		import_queue,
		transaction_pool,
		other: (rpc_extensions_builder, import_setup, rpc_setup, slot_duration, mut telemetry),
	} = new_partial(&mut config)?;

	let prometheus_registry = config.prometheus_registry().cloned();

	let overseer_connector = OverseerConnector::default();

	let shared_voter_state = rpc_setup;
	let auth_disc_publish_non_global_ips = config.network.allow_non_globals_in_dht;

	// Note: GrandPa is pushed before the Polkadot-specific protocols. This doesn't change
	// anything in terms of behaviour, but makes the logs more consistent with the other
	// Substrate nodes.
	config.network.extra_sets.push(sc_finality_grandpa::grandpa_peers_set_config());

	config.network.extra_sets.push(beefy_gadget::beefy_peers_set_config());

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

	let warp_sync = Arc::new(sc_finality_grandpa::warp_proof::NetworkProvider::new(
		backend.clone(),
		import_setup.1.shared_authority_set().clone(),
		vec![],
	));

	let (network, system_rpc_tx, network_starter) =
		sc_service::build_network(sc_service::BuildNetworkParams {
			config: &config,
			client: client.clone(),
			transaction_pool: transaction_pool.clone(),
			spawn_handle: task_manager.spawn_handle(),
			import_queue,
			block_announce_validator_builder: None,
			warp_sync: Some(warp_sync),
		})?;

	if config.offchain_worker.enabled {
		let _ = sc_service::build_offchain_workers(
			&config,
			task_manager.spawn_handle(),
			client.clone(),
			network.clone(),
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
		stagnant_check_interval: polkadot_node_core_chain_selection::StagnantCheckInterval::never(),
	};

	let dispute_coordinator_config = DisputeCoordinatorConfig {
		col_data: crate::parachains_db::REAL_COLUMNS.col_dispute_coordinator_data,
	};

	let rpc_handlers = sc_service::spawn_tasks(sc_service::SpawnTasksParams {
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

	let (block_import, link_half, babe_link, signed_commitment_sender) = import_setup;

	let overseer_client = client.clone();
	let spawner = task_manager.spawn_handle();
	let active_leaves = futures::executor::block_on(active_leaves(&select_chain, &*client))?;

	let authority_discovery_service = if role.is_authority() || is_collator {
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

		task_manager
			.spawn_handle()
			.spawn("authority-discovery-worker", None, worker.run());
		Some(service)
	} else {
		None
	};

	// we'd say let overseer_handler =
	// authority_discovery_service.map(|authority_discovery_service|, ...), but in that case we
	// couldn't use ? to propagate errors
	let local_keystore = keystore_container.local_keystore();
	let maybe_params =
		local_keystore.and_then(move |k| authority_discovery_service.map(|a| (a, k)));

	let overseer_handle = if let Some((authority_discovery_service, keystore)) = maybe_params {
		let (overseer, overseer_handle) = overseer_gen
			.generate::<sc_service::SpawnTaskHandle, FullClient>(
				overseer_connector,
				OverseerGenArgs {
					leaves: active_leaves,
					keystore,
					runtime_client: overseer_client.clone(),
					parachains_db,
					availability_config,
					approval_voting_config,
					network_service: network.clone(),
					authority_discovery_service,
					registry: prometheus_registry.as_ref(),
					spawner,
					candidate_validation_config,
					available_data_req_receiver,
					chain_selection_config,
					chunk_req_receiver,
					collation_req_receiver,
					dispute_coordinator_config,
					dispute_req_receiver,
					pov_req_receiver,
					statement_req_receiver,
					disputes_enabled: false,
				},
			)?;
		let handle = Handle::new(overseer_handle);

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
		None
	};

	if role.is_authority() {
		let can_author_with =
			sp_consensus::CanAuthorWithNativeVersion::new(client.executor().clone());

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
		let babe_config = sc_consensus_babe::BabeParams {
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
					)
					.await
					.map_err(Box::new)?;

					let uncles = sc_consensus_uncles::create_uncles_inherent_data_provider(
						&*client_clone,
						parent,
					)?;

					let timestamp = sp_timestamp::InherentDataProvider::from_system_time();

					let slot = sp_consensus_babe::inherents::InherentDataProvider::from_timestamp_and_duration(
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
			block_proposal_slot_portion: sc_consensus_babe::SlotProportion::new(2f32 / 3f32),
			max_block_proposal_slot_portion: None,
			telemetry: telemetry.as_ref().map(|x| x.handle()),
		};

		let babe = sc_consensus_babe::start_babe(babe_config)?;
		task_manager.spawn_essential_handle().spawn_blocking("babe", None, babe);
	}

	// if the node isn't actively participating in consensus then it doesn't
	// need a keystore, regardless of which protocol we use below.
	let keystore_opt =
		if role.is_authority() { Some(keystore_container.sync_keystore()) } else { None };

	let beefy_params = beefy_gadget::BeefyParams {
		client: client.clone(),
		backend: backend.clone(),
		key_store: keystore_opt.clone(),
		network: network.clone(),
		signed_commitment_sender,
		min_block_delta: 2,
		prometheus_registry: prometheus_registry.clone(),
	};

	// Start the BEEFY bridge gadget.
	task_manager.spawn_essential_handle().spawn_blocking(
		"beefy-gadget",
		None,
		beefy_gadget::start_beefy_gadget::<_, _, _, _>(beefy_params),
	);

	let config = sc_finality_grandpa::Config {
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
		let builder = sc_finality_grandpa::VotingRulesBuilder::default();

		let voting_rule = builder.build();
		let grandpa_config = sc_finality_grandpa::GrandpaParams {
			config,
			link: link_half,
			network: network.clone(),
			voting_rule,
			prometheus_registry,
			shared_voter_state,
			telemetry: telemetry.as_ref().map(|x| x.handle()),
		};

		task_manager.spawn_essential_handle().spawn_blocking(
			"grandpa-voter",
			None,
			sc_finality_grandpa::run_grandpa_voter(grandpa_config)?,
		);
	}

	network_starter.start_network();

	Ok(NewFull { task_manager, client, overseer_handle, network, rpc_handlers, backend })
}

pub fn build_full(
	config: Configuration,
	overseer_gen: impl OverseerGen,
) -> Result<NewFull<Arc<FullClient>>, Error> {
	new_full(config, None, overseer_gen)
}
