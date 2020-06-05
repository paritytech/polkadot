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
mod grandpa_support;
mod client;

use std::sync::Arc;
use std::time::Duration;
use polkadot_primitives::{parachain, Hash, BlockId, AccountId, Nonce, Balance};
#[cfg(feature = "full-node")]
use polkadot_network::{legacy::gossip::Known, protocol as network_protocol};
use service::{error::Error as ServiceError, ServiceBuilder};
use grandpa::{self, FinalityProofProvider as GrandpaFinalityProofProvider};
use sc_executor::native_executor_instance;
use log::info;
pub use service::{
	AbstractService, Role, PruningMode, TransactionPoolOptions, Error, RuntimeGenesis,
	TFullClient, TLightClient, TFullBackend, TLightBackend, TFullCallExecutor, TLightCallExecutor,
	Configuration, ChainSpec, ServiceBuilderCommand,
};
pub use service::config::{DatabaseConfig, PrometheusConfig};
pub use sc_executor::NativeExecutionDispatch;
pub use sc_client_api::{Backend, ExecutionStrategy, CallExecutor};
pub use sc_consensus::LongestChain;
pub use sp_api::{Core as CoreApi, ConstructRuntimeApi, ProvideRuntimeApi, StateBackend};
pub use sp_runtime::traits::{HashFor, NumberFor};
pub use consensus_common::{SelectChain, BlockImport, block_validation::Chain};
pub use polkadot_primitives::parachain::{CollatorId, ParachainHost};
pub use polkadot_primitives::Block;
pub use sp_runtime::traits::{Block as BlockT, self as runtime_traits, BlakeTwo256};
pub use chain_spec::{PolkadotChainSpec, KusamaChainSpec, WestendChainSpec};
#[cfg(feature = "full-node")]
pub use consensus::run_validation_worker;
pub use codec::Codec;
pub use polkadot_runtime;
pub use kusama_runtime;
pub use westend_runtime;
use prometheus_endpoint::Registry;
pub use self::client::PolkadotClient;

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

/// A set of APIs that polkadot-like runtimes must implement.
pub trait RuntimeApiCollection<Extrinsic: codec::Codec + Send + Sync + 'static>:
	sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block>
	+ sp_api::ApiExt<Block, Error = sp_blockchain::Error>
	+ babe_primitives::BabeApi<Block>
	+ grandpa_primitives::GrandpaApi<Block>
	+ ParachainHost<Block>
	+ sp_block_builder::BlockBuilder<Block>
	+ system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce>
	+ pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance, Extrinsic>
	+ sp_api::Metadata<Block>
	+ sp_offchain::OffchainWorkerApi<Block>
	+ sp_session::SessionKeys<Block>
	+ authority_discovery_primitives::AuthorityDiscoveryApi<Block>
where
	Extrinsic: RuntimeExtrinsic,
	<Self as sp_api::ApiExt<Block>>::StateBackend: sp_api::StateBackend<BlakeTwo256>,
{}

impl<Api, Extrinsic> RuntimeApiCollection<Extrinsic> for Api
where
	Api:
	sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block>
	+ sp_api::ApiExt<Block, Error = sp_blockchain::Error>
	+ babe_primitives::BabeApi<Block>
	+ grandpa_primitives::GrandpaApi<Block>
	+ ParachainHost<Block>
	+ sp_block_builder::BlockBuilder<Block>
	+ system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce>
	+ pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance, Extrinsic>
	+ sp_api::Metadata<Block>
	+ sp_offchain::OffchainWorkerApi<Block>
	+ sp_session::SessionKeys<Block>
	+ authority_discovery_primitives::AuthorityDiscoveryApi<Block>,
	Extrinsic: RuntimeExtrinsic,
	<Self as sp_api::ApiExt<Block>>::StateBackend: sp_api::StateBackend<BlakeTwo256>,
{}

pub trait RuntimeExtrinsic: codec::Codec + Send + Sync + 'static {}

impl<E> RuntimeExtrinsic for E where E: codec::Codec + Send + Sync + 'static {}

/// Can be called for a `Configuration` to check if it is a configuration for the `Kusama` network.
pub trait IdentifyVariant {
	/// Returns if this is a configuration for the `Kusama` network.
	fn is_kusama(&self) -> bool;

	/// Returns if this is a configuration for the `Westend` network.
	fn is_westend(&self) -> bool;
}

impl IdentifyVariant for Box<dyn ChainSpec> {
	fn is_kusama(&self) -> bool {
		self.id().starts_with("kusama") || self.id().starts_with("ksm")
	}
	fn is_westend(&self) -> bool {
		self.id().starts_with("westend") || self.id().starts_with("wnd")
	}
}

// If we're using prometheus, use a registry with a prefix of `polkadot`.
fn set_prometheus_registry(config: &mut Configuration) -> Result<(), ServiceError> {
	if let Some(PrometheusConfig { registry, .. }) = config.prometheus_config.as_mut() {
		*registry = Registry::new_custom(Some("polkadot".into()), None)?;
	}

	Ok(())
}

/// Starts a `ServiceBuilder` for a full service.
///
/// Use this macro if you don't actually need the full service, but just the builder in order to
/// be able to perform chain operations.
macro_rules! new_full_start {
	($config:expr, $runtime:ty, $executor:ty) => {{
		set_prometheus_registry(&mut $config)?;

		let mut import_setup = None;
		let mut rpc_setup = None;
		let inherent_data_providers = inherents::InherentDataProviders::new();
		let builder = service::ServiceBuilder::new_full::<
			Block, $runtime, $executor
		>($config)?
			.with_select_chain(|_, backend| {
				Ok(sc_consensus::LongestChain::new(backend.clone()))
			})?
			.with_transaction_pool(|config, client, _fetcher, prometheus_registry| {
				let pool_api = sc_transaction_pool::FullChainApi::new(client.clone());
				let pool = sc_transaction_pool::BasicPool::new(config, std::sync::Arc::new(pool_api), prometheus_registry);
				Ok(pool)
			})?
			.with_import_queue(|
				config,
				client,
				mut select_chain,
				_,
				spawn_task_handle,
				registry,
			| {
				let select_chain = select_chain.take()
					.ok_or_else(|| service::Error::SelectChainRequired)?;

				let grandpa_hard_forks = if config.chain_spec.is_kusama() {
					grandpa_support::kusama_hard_forks()
				} else {
					Vec::new()
				};

				let (grandpa_block_import, grandpa_link) =
					grandpa::block_import_with_authority_set_hard_forks(
						client.clone(),
						&(client.clone() as Arc<_>),
						select_chain,
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
					client,
					inherent_data_providers.clone(),
					spawn_task_handle,
					registry,
				)?;

				import_setup = Some((block_import, grandpa_link, babe_link));
				Ok(import_queue)
			})?
			.with_rpc_extensions_builder(|builder| {
				let grandpa_link = import_setup.as_ref().map(|s| &s.1)
					.expect("GRANDPA LinkHalf is present for full services or set up failed; qed.");

				let shared_authority_set = grandpa_link.shared_authority_set().clone();
				let shared_voter_state = grandpa::SharedVoterState::empty();

				rpc_setup = Some((shared_voter_state.clone()));

				let babe_link = import_setup.as_ref().map(|s| &s.2)
					.expect("BabeLink is present for full services or set up faile; qed.");

				let babe_config = babe_link.config().clone();
				let shared_epoch_changes = babe_link.epoch_changes().clone();

				let client = builder.client().clone();
				let pool = builder.pool().clone();
				let select_chain = builder.select_chain().cloned()
					.expect("SelectChain is present for full services or set up failed; qed.");
				let keystore = builder.keystore().clone();

				Ok(move |deny_unsafe| -> polkadot_rpc::RpcExtension {
					let deps = polkadot_rpc::FullDeps {
						client: client.clone(),
						pool: pool.clone(),
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
						},
					};

					polkadot_rpc::create_full(deps)
				})
			})?;

		(builder, import_setup, inherent_data_providers, rpc_setup)
	}}
}

/// Builds a new service for a full client.
#[macro_export]
macro_rules! new_full {
	(
		$config:expr,
		$collating_for:expr,
		$max_block_data_size:expr,
		$authority_discovery_enabled:expr,
		$slot_duration:expr,
		$grandpa_pause:expr,
		$runtime:ty,
		$dispatch:ty
	) => {{
		use sc_network::Event;
		use sc_client_api::ExecutorProvider;
		use futures::stream::StreamExt;

		let is_collator = $collating_for.is_some();
		let role = $config.role.clone();
		let is_authority = role.is_authority() && !is_collator;
		let force_authoring = $config.force_authoring;
		let max_block_data_size = $max_block_data_size;
		let db_path = match $config.database.path() {
			Some(path) => std::path::PathBuf::from(path),
			None => return Err("Starting a Polkadot service with a custom database isn't supported".to_string().into()),
		};
		let disable_grandpa = $config.disable_grandpa;
		let name = $config.network.node_name.clone();
		let authority_discovery_enabled = $authority_discovery_enabled;
		let slot_duration = $slot_duration;

		let (builder, mut import_setup, inherent_data_providers, mut rpc_setup) =
			new_full_start!($config, $runtime, $dispatch);

		let service = builder
			.with_finality_proof_provider(|client, backend| {
				let provider = client as Arc<dyn grandpa::StorageAndProofProvider<_, _>>;
				Ok(Arc::new(GrandpaFinalityProofProvider::new(backend, provider)) as _)
			})?
			.build()?;

		let (block_import, link_half, babe_link) = import_setup.take()
			.expect("Link Half and Block Import are present for Full Services or setup failed before. qed");

		let shared_voter_state = rpc_setup.take()
			.expect("The SharedVoterState is present for Full Services or setup failed before. qed");

		let client = service.client();
		let known_oracle = client.clone();

		let mut handles = FullNodeHandles::default();
		let select_chain = if let Some(select_chain) = service.select_chain() {
			select_chain
		} else {
			info!("The node cannot start as an authority because it can't select chain.");
			return Ok((service, client, handles));
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

		let polkadot_network_service = network_protocol::start(
			service.network(),
			network_protocol::Config {
				collating_for: $collating_for,
			},
			(is_known, client.clone()),
			client.clone(),
			service.spawn_task_handle(),
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
				spawner: service.spawn_task_handle(),
				availability_store: availability_store.clone(),
				select_chain: select_chain.clone(),
				keystore: service.keystore(),
				max_block_data_size,
			}.build();

			service.spawn_essential_task("validation-service", Box::pin(validation_service));

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
				service.transaction_pool(),
				validation_service_handle,
				slot_duration,
				service.prometheus_registry().as_ref(),
			);

			let select_chain = service.select_chain().ok_or(ServiceError::SelectChainRequired)?;
			let can_author_with =
				consensus_common::CanAuthorWithNativeVersion::new(client.executor().clone());

			let block_import = availability_store.block_import(
				block_import,
				client.clone(),
				service.spawn_task_handle(),
				service.keystore(),
			)?;

			let babe_config = babe::BabeParams {
				keystore: service.keystore(),
				client: client.clone(),
				select_chain,
				block_import,
				env: proposer,
				sync_oracle: service.network(),
				inherent_data_providers: inherent_data_providers.clone(),
				force_authoring,
				babe_link,
				can_author_with,
			};

			let babe = babe::start_babe(babe_config)?;
			service.spawn_essential_task("babe", babe);
		}

		if matches!(role, Role::Authority{..} | Role::Sentry{..}) {
			if authority_discovery_enabled {
				let (sentries, authority_discovery_role) = match role {
					Role::Authority { ref sentry_nodes } => (
						sentry_nodes.clone(),
						authority_discovery::Role::Authority (
							service.keystore(),
						),
					),
					Role::Sentry {..} => (
						vec![],
						authority_discovery::Role::Sentry,
					),
					_ => unreachable!("Due to outer matches! constraint; qed."),
				};

				let network = service.network();
				let network_event_stream = network.event_stream("authority-discovery");
				let dht_event_stream = network_event_stream.filter_map(|e| async move { match e {
					Event::Dht(e) => Some(e),
					_ => None,
				}}).boxed();
				let authority_discovery = authority_discovery::AuthorityDiscovery::new(
					service.client(),
					network,
					sentries,
					dht_event_stream,
					authority_discovery_role,
					service.prometheus_registry(),
				);

				service.spawn_task("authority-discovery", authority_discovery);
			}
		}

		// if the node isn't actively participating in consensus then it doesn't
		// need a keystore, regardless of which protocol we use below.
		let keystore = if is_authority {
			Some(service.keystore())
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
			let voting_rule = match $grandpa_pause {
				Some((block, delay)) => {
					info!("GRANDPA scheduled voting pause set for block #{} with a duration of {} blocks.",
						block,
						delay,
					);

					grandpa::VotingRulesBuilder::default()
						.add(grandpa_support::PauseAfterBlockFor(block, delay))
						.build()
				},
				None =>
					grandpa::VotingRulesBuilder::default()
						.build(),
			};

			let grandpa_config = grandpa::GrandpaParams {
				config,
				link: link_half,
				network: service.network(),
				inherent_data_providers: inherent_data_providers.clone(),
				telemetry_on_connect: Some(service.telemetry_on_connect_stream()),
				voting_rule,
				prometheus_registry: service.prometheus_registry(),
				shared_voter_state,
			};

			service.spawn_essential_task(
				"grandpa-voter",
				grandpa::run_grandpa_voter(grandpa_config)?
			);
		} else {
			grandpa::setup_disabled_grandpa(
				client.clone(),
				&inherent_data_providers,
				service.network(),
			)?;
		}

		handles.polkadot_network = Some(polkadot_network_service);
		(service, client, handles)
	}}
}

/// Builds a new service for a light client.
#[macro_export]
macro_rules! new_light {
	($config:expr, $runtime:ty, $dispatch:ty) => {{
		crate::set_prometheus_registry(&mut $config)?;
		let inherent_data_providers = inherents::InherentDataProviders::new();

		ServiceBuilder::new_light::<Block, $runtime, $dispatch>($config)?
			.with_select_chain(|_, backend| {
				Ok(sc_consensus::LongestChain::new(backend.clone()))
			})?
			.with_transaction_pool(|config, client, fetcher, prometheus_registry| {
				let fetcher = fetcher
					.ok_or_else(|| "Trying to start light transaction pool without active fetcher")?;
				let pool_api = sc_transaction_pool::LightChainApi::new(client.clone(), fetcher.clone());
				let pool = sc_transaction_pool::BasicPool::with_revalidation_type(
					config, Arc::new(pool_api), prometheus_registry, sc_transaction_pool::RevalidationType::Light,
				);
				Ok(pool)
			})?
			.with_import_queue_and_fprb(|
				_config,
				client,
				backend,
				fetcher,
				_select_chain,
				_,
				spawn_task_handle,
				registry,
			| {
				let fetch_checker = fetcher
					.map(|fetcher| fetcher.checker().clone())
					.ok_or_else(|| "Trying to start light import queue without active fetch checker")?;
				let grandpa_block_import = grandpa::light_block_import(
					client.clone(), backend, &(client.clone() as Arc<_>), Arc::new(fetch_checker)
				)?;

				let finality_proof_import = grandpa_block_import.clone();
				let finality_proof_request_builder =
					finality_proof_import.create_finality_proof_request_builder();

				let (babe_block_import, babe_link) = babe::block_import(
					babe::Config::get_or_compute(&*client)?,
					grandpa_block_import,
					client.clone(),
				)?;

				// FIXME: pruning task isn't started since light client doesn't do `AuthoritySetup`.
				let import_queue = babe::import_queue(
					babe_link,
					babe_block_import,
					None,
					Some(Box::new(finality_proof_import)),
					client,
					inherent_data_providers.clone(),
					spawn_task_handle,
					registry,
				)?;

				Ok((import_queue, finality_proof_request_builder))
			})?
			.with_finality_proof_provider(|client, backend| {
				let provider = client as Arc<dyn grandpa::StorageAndProofProvider<_, _>>;
				Ok(Arc::new(grandpa::FinalityProofProvider::new(backend, provider)) as _)
			})?
			.with_rpc_extensions(|builder| {
				let fetcher = builder.fetcher()
					.ok_or_else(|| "Trying to start node RPC without active fetcher")?;
				let remote_blockchain = builder.remote_backend()
					.ok_or_else(|| "Trying to start node RPC without active remote blockchain")?;

				let light_deps = polkadot_rpc::LightDeps {
					remote_blockchain,
					fetcher,
					client: builder.client().clone(),
					pool: builder.pool(),
				};
				Ok(polkadot_rpc::create_light(light_deps))
			})?
			.build()
	}}
}

/// Builds a new object suitable for chain operations.
pub fn new_chain_ops<Runtime, Dispatch, Extrinsic>(mut config: Configuration)
	-> Result<impl ServiceBuilderCommand<Block=Block>, ServiceError>
where
	Runtime: ConstructRuntimeApi<Block, service::TFullClient<Block, Runtime, Dispatch>> + Send + Sync + 'static,
	Runtime::RuntimeApi:
	RuntimeApiCollection<Extrinsic, StateBackend = sc_client_api::StateBackendFor<TFullBackend<Block>, Block>>,
	Dispatch: NativeExecutionDispatch + 'static,
	Extrinsic: RuntimeExtrinsic,
	<Runtime::RuntimeApi as sp_api::ApiExt<Block>>::StateBackend: sp_api::StateBackend<BlakeTwo256>,
{
	config.keystore = service::config::KeystoreConfig::InMemory;
	Ok(new_full_start!(config, Runtime, Dispatch).0)
}

/// Create a new Polkadot service for a full node.
#[cfg(feature = "full-node")]
pub fn polkadot_new_full(
	mut config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
)
	-> Result<(
		impl AbstractService,
		Arc<impl PolkadotClient<
			Block,
			TFullBackend<Block>,
			polkadot_runtime::RuntimeApi
		>>,
		FullNodeHandles,
	), ServiceError>
{
	let (service, client, handles) = new_full!(
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		grandpa_pause,
		polkadot_runtime::RuntimeApi,
		PolkadotExecutor
	);

	Ok((service, client, handles))
}

/// Create a new Kusama service for a full node.
#[cfg(feature = "full-node")]
pub fn kusama_new_full(
	mut config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
) -> Result<(
		impl AbstractService,
		Arc<impl PolkadotClient<
			Block,
			TFullBackend<Block>,
			kusama_runtime::RuntimeApi
			>
		>,
		FullNodeHandles
	), ServiceError>
{
	let (service, client, handles) = new_full!(
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		grandpa_pause,
		kusama_runtime::RuntimeApi,
		KusamaExecutor
	);

	Ok((service, client, handles))
}

/// Create a new Kusama service for a full node.
#[cfg(feature = "full-node")]
pub fn westend_new_full(
	mut config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
)
	-> Result<(
		impl AbstractService,
		Arc<impl PolkadotClient<
			Block,
			TFullBackend<Block>,
			westend_runtime::RuntimeApi
		>>,
		FullNodeHandles,
	), ServiceError>
{
	let (service, client, handles) = new_full!(
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		grandpa_pause,
		westend_runtime::RuntimeApi,
		WestendExecutor
	);

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

/// Create a new Polkadot service for a light client.
pub fn polkadot_new_light(mut config: Configuration) -> Result<
	impl AbstractService<
		Block = Block,
		RuntimeApi = polkadot_runtime::RuntimeApi,
		Backend = TLightBackend<Block>,
		SelectChain = LongestChain<TLightBackend<Block>, Block>,
		CallExecutor = TLightCallExecutor<Block, PolkadotExecutor>,
	>, ServiceError>
{
	new_light!(config, polkadot_runtime::RuntimeApi, PolkadotExecutor)
}

/// Create a new Kusama service for a light client.
pub fn kusama_new_light(mut config: Configuration) -> Result<
	impl AbstractService<
		Block = Block,
		RuntimeApi = kusama_runtime::RuntimeApi,
		Backend = TLightBackend<Block>,
		SelectChain = LongestChain<TLightBackend<Block>, Block>,
		CallExecutor = TLightCallExecutor<Block, KusamaExecutor>,
	>, ServiceError>
{
	new_light!(config, kusama_runtime::RuntimeApi, KusamaExecutor)
}

/// Create a new Westend service for a light client.
pub fn westend_new_light(mut config: Configuration, ) -> Result<
	impl AbstractService<
		Block = Block,
		RuntimeApi = westend_runtime::RuntimeApi,
		Backend = TLightBackend<Block>,
		SelectChain = LongestChain<TLightBackend<Block>, Block>,
		CallExecutor = TLightCallExecutor<Block, KusamaExecutor>
	>,
	ServiceError>
{
	new_light!(config, westend_runtime::RuntimeApi, KusamaExecutor)
}
