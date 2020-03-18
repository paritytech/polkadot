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

use sc_client::LongestChain;
use std::sync::Arc;
use std::time::Duration;
use polkadot_primitives::{parachain, Hash, BlockId, AccountId, Nonce, Balance};
#[cfg(feature = "full-node")]
use polkadot_network::{legacy::gossip::Known, protocol as network_protocol};
use service::{error::{Error as ServiceError}, ServiceBuilder};
use grandpa::{self, FinalityProofProvider as GrandpaFinalityProofProvider};
use inherents::InherentDataProviders;
use sc_executor::native_executor_instance;
use log::info;
pub use service::{
	AbstractService, Roles, PruningMode, TransactionPoolOptions, Error, RuntimeGenesis, ServiceBuilderCommand,
	TFullClient, TLightClient, TFullBackend, TLightBackend, TFullCallExecutor, TLightCallExecutor,
	Configuration, ChainSpec,
};
pub use service::config::{DatabaseConfig, PrometheusConfig, full_version_from_strs};
pub use sc_executor::NativeExecutionDispatch;
pub use sc_client::{ExecutionStrategy, CallExecutor, Client};
pub use sc_client_api::backend::Backend;
pub use sp_api::{Core as CoreApi, ConstructRuntimeApi, ProvideRuntimeApi, StateBackend};
pub use sp_runtime::traits::{HashFor, NumberFor};
pub use consensus_common::SelectChain;
pub use polkadot_primitives::parachain::{CollatorId, ParachainHost};
pub use polkadot_primitives::Block;
pub use sp_runtime::traits::{Block as BlockT, self as runtime_traits, BlakeTwo256};
pub use chain_spec::{PolkadotChainSpec, KusamaChainSpec};
#[cfg(not(target_os = "unknown"))]
pub use consensus::run_validation_worker;
pub use codec::Codec;
pub use polkadot_runtime;
pub use kusama_runtime;
use prometheus_endpoint::Registry;

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

/// A set of APIs that polkadot-like runtimes must implement.
pub trait RuntimeApiCollection<Extrinsic: codec::Codec + Send + Sync + 'static> :
	sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block>
	+ sp_api::ApiExt<Block, Error = sp_blockchain::Error>
	+ babe_primitives::BabeApi<Block>
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

pub trait RuntimeExtrinsic: codec::Codec + Send + Sync + 'static
{}

impl<E> RuntimeExtrinsic for E where E: codec::Codec + Send + Sync + 'static
{}

/// Can be called for a `Configuration` to check if it is a configuration for the `Kusama` network.
pub trait IsKusama {
	/// Returns if this is a configuration for the `Kusama` network.
	fn is_kusama(&self) -> bool;
}

impl IsKusama for &dyn ChainSpec {
	fn is_kusama(&self) -> bool {
		self.id().starts_with("kusama") || self.id().starts_with("ksm")
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
		let inherent_data_providers = inherents::InherentDataProviders::new();
		let builder = service::ServiceBuilder::new_full::<
			Block, $runtime, $executor
		>($config)?
			.with_select_chain(|_, backend| {
				Ok(sc_client::LongestChain::new(backend.clone()))
			})?
			.with_transaction_pool(|config, client, _fetcher| {
				let pool_api = sc_transaction_pool::FullChainApi::new(client.clone());
				let pool = sc_transaction_pool::BasicPool::new(config, std::sync::Arc::new(pool_api));
				Ok(pool)
			})?
			.with_import_queue(|config, client, mut select_chain, _| {
				let select_chain = select_chain.take()
					.ok_or_else(|| service::Error::SelectChainRequired)?;

				let grandpa_hard_forks = if config.expect_chain_spec().is_kusama() {
					grandpa_kusama_hard_forks()
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
				)?;

				import_setup = Some((block_import, grandpa_link, babe_link));
				Ok(import_queue)
			})?
			.with_rpc_extensions(|builder| -> Result<polkadot_rpc::RpcExtension, _> {
				Ok(polkadot_rpc::create_full(builder.client().clone(), builder.pool()))
			})?;

		(builder, import_setup, inherent_data_providers)
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
	config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
)
	-> Result<(
		impl AbstractService<
			Block = Block,
			RuntimeApi = polkadot_runtime::RuntimeApi,
			Backend = TFullBackend<Block>,
			SelectChain = LongestChain<TFullBackend<Block>, Block>,
			CallExecutor = TFullCallExecutor<Block, PolkadotExecutor>,
		>,
		FullNodeHandles,
	), ServiceError>
{
	new_full(
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		grandpa_pause,
	)
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
)
	-> Result<(
		impl AbstractService<
			Block = Block,
			RuntimeApi = kusama_runtime::RuntimeApi,
			Backend = TFullBackend<Block>,
			SelectChain = LongestChain<TFullBackend<Block>, Block>,
			CallExecutor = TFullCallExecutor<Block, KusamaExecutor>,
		>,
		FullNodeHandles,
	), ServiceError>
{
	new_full(
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		grandpa_pause,
	)
}

/// Handles to other sub-services that full nodes instantiate, which consumers
/// of the node may use.
#[cfg(feature = "full-node")]
pub struct FullNodeHandles {
	/// A handle to the Polkadot networking protocol.
	pub polkadot_network: Option<network_protocol::Service>,
}

/// Builds a new service for a full client.
#[cfg(feature = "full-node")]
pub fn new_full<Runtime, Dispatch, Extrinsic>(
	mut config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
)
	-> Result<(
		impl AbstractService<
			Block = Block,
			RuntimeApi = Runtime,
			Backend = TFullBackend<Block>,
			SelectChain = LongestChain<TFullBackend<Block>, Block>,
			CallExecutor = TFullCallExecutor<Block, Dispatch>,
		>,
		FullNodeHandles,
	), ServiceError>
	where
		Runtime: ConstructRuntimeApi<Block, service::TFullClient<Block, Runtime, Dispatch>> + Send + Sync + 'static,
		Runtime::RuntimeApi:
			RuntimeApiCollection<Extrinsic, StateBackend = sc_client_api::StateBackendFor<TFullBackend<Block>, Block>>,
		Dispatch: NativeExecutionDispatch + 'static,
		Extrinsic: RuntimeExtrinsic,
		// Rust bug: https://github.com/rust-lang/rust/issues/24159
		<Runtime::RuntimeApi as sp_api::ApiExt<Block>>::StateBackend: sp_api::StateBackend<BlakeTwo256>,
{
	use sc_network::Event;
	use sc_client_api::ExecutorProvider;
	use futures::stream::StreamExt;

	let is_collator = collating_for.is_some();
	let is_authority = config.roles.is_authority() && !is_collator;
	let force_authoring = config.force_authoring;
	let max_block_data_size = max_block_data_size;
	let db_path = if let DatabaseConfig::Path { ref path, .. } = config.expect_database() {
		path.clone()
	} else {
		return Err("Starting a Polkadot service with a custom database isn't supported".to_string().into());
	};
	let disable_grandpa = config.disable_grandpa;
	let name = config.name.clone();
	let authority_discovery_enabled = authority_discovery_enabled;
	let sentry_nodes = config.network.sentry_nodes.clone();
	let slot_duration = slot_duration;

	// sentry nodes announce themselves as authorities to the network
	// and should run the same protocols authorities do, but it should
	// never actively participate in any consensus process.
	let participates_in_consensus = is_authority && !config.sentry_mode;

	let (builder, mut import_setup, inherent_data_providers) = new_full_start!(config, Runtime, Dispatch);

	let backend = builder.backend().clone();

	let service = builder
		.with_finality_proof_provider(|client, backend| {
			let provider = client as Arc<dyn grandpa::StorageAndProofProvider<_, _>>;
			Ok(Arc::new(GrandpaFinalityProofProvider::new(backend, provider)) as _)
		})?
		.build()?;

	let (block_import, link_half, babe_link) = import_setup.take()
		.expect("Link Half and Block Import are present for Full Services or setup failed before. qed");

	let client = service.client();
	let known_oracle = client.clone();

	let mut handles = FullNodeHandles { polkadot_network: None };
	let select_chain = if let Some(select_chain) = service.select_chain() {
		select_chain
	} else {
		info!("The node cannot start as an authority because it can't select chain.");
		return Ok((service, handles));
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
			collating_for,
		},
		(is_known, client.clone()),
		client.clone(),
		service.spawn_task_handle(),
	).map_err(|e| format!("Could not spawn network worker: {:?}", e))?;

	if participates_in_consensus {
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

		let proposer = consensus::ProposerFactory::new(
			client.clone(),
			service.transaction_pool(),
			validation_service_handle,
			slot_duration,
			backend,
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
			client,
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

		if authority_discovery_enabled {
			let network = service.network();
			let network_event_stream = network.event_stream();
			let dht_event_stream = network_event_stream.filter_map(|e| async move { match e {
				Event::Dht(e) => Some(e),
				_ => None,
			}}).boxed();
			let authority_discovery = authority_discovery::AuthorityDiscovery::new(
				service.client(),
				network,
				sentry_nodes,
				service.keystore(),
				dht_event_stream,
				service.prometheus_registry(),
			);
			service.spawn_task("authority-discovery", authority_discovery);
		}
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
		gossip_duration: Duration::from_millis(1000),
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
					.add(PauseAfterBlockFor(block, delay))
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
		};

		service.spawn_essential_task(
			"grandpa-voter",
			grandpa::run_grandpa_voter(grandpa_config)?
		);
	} else {
		grandpa::setup_disabled_grandpa(
			service.client(),
			&inherent_data_providers,
			service.network(),
		)?;
	}

	handles.polkadot_network = Some(polkadot_network_service);
	Ok((service, handles))
}

fn grandpa_kusama_hard_forks() -> Vec<(
	grandpa_primitives::SetId,
	(Hash, polkadot_primitives::BlockNumber),
	grandpa_primitives::AuthorityList,
)> {
	use sp_core::crypto::Ss58Codec;
	use std::str::FromStr;

	let forks = vec![
		(
			623,
			"01e94e1e7e9cf07b3b0bf4e1717fce7448e5563901c2ef2e3b8e9ecaeba088b1",
			1492283,
		),
		(
			624,
			"ddc4323c5e8966844dfaa87e0c2f74ef6b43115f17bf8e4ff38845a62d02b9a9",
			1492436,
		),
		(
			625,
			"38ba115b296663e424e32d7b1655cd795719cef4fd7d579271a6d01086cf1628",
			1492586,
		),
		(
			626,
			"f3172b6b8497c10fc772f5dada4eeb1f4c4919c97de9de2e1a439444d5a057ff",
			1492955,
		),
		(
			627,
			"b26526aea299e9d24af29fdacd5cf4751a663d24894e3d0a37833aa14c58424a",
			1493338,
		),
		(
			628,
			"3980d024327d53b8d01ef0d198a052cd058dd579508d8ed6283fe3614e0a3694",
			1493913,
		),
		(
			629,
			"31f22997a786c25ee677786373368cae6fd501fd1bc4b212b8e267235c88179d",
			1495083,
		),
		(
			630,
			"1c65eb250cf54b466c64f1a4003d1415a7ee275e49615450c0e0525179857eef",
			1497404,
		),
		(
			631,
			"9e44116467cc9d7e224e36487bf2cf571698cae16b25f54a7430f1278331fdd8",
			1498598,
		),
		(
			632,
			"ed09126031512c4368f2b7bc1ecd73ee0234e8aba6cfe0308f5fdf27d0ce7dc8",
			1500988,
		),
	];

	let authorities = vec![
		"CwjLJ1zPWK5Ao9WChAFp7rWGEgN3AyXXjTRPrqgm5WwBpoS",
		"Dp8FHpZTzvoKXztkfrUAkF6xNf6sjVU5ZLZ29NEGUazouou",
		"DtK7YfkhNWU6wEPF1dShsFdhtosVAuJPLkoGhKhG1r5LjKq",
		"FLnHYBuoyThzqJ45tdb8P6yMLdocM7ir27Pg1AnpYoygm1K",
		"FWEfJ5UMghr52UopgYjawAg6hQg3ztbQek75pfeRtLVi8pB",
		"ECoLHAu7HKWGTB9od82HAtequYj6hvNHigkGSB9g3ApxAwB",
		"GL1Tg3Uppo8GYL9NjKj4dWKcS6tW98REop9G5hpu7HgFwTa",
		"ExnjU5LZMktrgtQBE3An6FsQfvaKG1ukxPqwhJydgdgarmY",
		"CagLpgCBu5qJqYF2tpFX6BnU4yHvMGSjc7r3Ed1jY3tMbQt",
		"DsrtmMsD4ijh3n4uodxPoiW9NZ7v7no5wVvPVj8fL1dfrWB",
		"HQB4EctrVR68ozZDyBiRJzLRAEGh1YKgCkAsFjJcegL9RQA",
		"H2YTYbXTFkDY1cGnv164ecnDT3hsD2bQXtyiDbcQuXcQZUV",
		"H5WL8jXmbkCoEcLfvqJkbLUeGrDFsJiMXkhhRWn3joct1tE",
		"DpB37GDrJDYcmg2df2eqsrPKMay1u8hyZ6sQi2FuUiUeNLu",
		"FR8yjKRA9MTjvFGK8kfzrdC23Fr6xd7rfBvZXSjAsmuxURE",
		"DxHPty3B9fpj3duu6Gc6gCSCAvsydJHJEY5G3oVYT8S5BYJ",
		"DbVKC8ZJjevrhqSnZyJMMvmPL7oPPL4ed1roxawYnHVgyin",
		"DVJV81kab2J6oTyRJ9T3NCwW2DSrysbWCssvMcE6cwZHnAd",
		"Fg4rDAyzoVzf39Zo8JFPo4W314ntNWNwm3shr4xKe8M1fJg",
		"GUaNcnAruMVxHGTs7gGpSUpigRJboQYQBBQyPohkFcP6NMH",
		"J4BMGF4W9yWiJz4pkhQW73X6QMGpKUzmPppVnqzBCqw5dQq",
		"E1cR61L1tdDEop4WdWVqcq1H1x6VqsDpSHvFyUeC41uruVJ",
		"GoWLzBsj1f23YtdDpyntnvN1LwXKhF5TEeZvBeTVxofgWGR",
		"CwHwmbogSwtRbrkajVBNubPvWmHBGU4bhMido54M9CjuKZD",
		"FLT63y9oVXJnyiWMAL4RvWxsQx21Vymw9961Z7NRFmSG7rw",
		"FoQ2y6JuHuHTG4rHFL3f2hCxfJMvtrq8wwPWdv8tsdkcyA8",
		"D7QQKqqs8ocGorRA12h4QoBSHDia1DkHeXT4eMfjWQ483QH",
		"J6z7FP35F9DiiU985bhkDTS3WxyeTBeoo9MtLdLoD3GiWPj",
		"EjapydCK25AagodRbDECavHAy8yQY1tmeRhwUXhVWx4cFPv",
		"H8admATcRkGCrF1dTDDBCjQDsYjMkuPaN9YwR2mSCj4DWMQ",
		"FtHMRU1fxsoswJjBvyCGvECepC7gP2X77QbNpyikYSqqR6k",
		"DzY5gwr45GVRUFzRMmeg8iffpqYF47nm3XbJhmjG97FijaE",
		"D3HKWAihSUmg8HrfeFrftSwNK7no261yA9RNr3LUUdsuzuJ",
		"D82DwwGJGTcSvtB3SmNrZejnSertbPzpkYvDUp3ibScL3ne",
		"FTPxLXLQvMDQYFA6VqNLGwWPKhemMYP791XVj8TmDpFuV3b",
		"FzGfKmS7N8Z1tvCBU5JH1eBXZQ9pCtRNoMUnNVv38wZNq72",
		"GDfm1MyLAQ7Rh8YPtF6FtMweV4hz91zzeDy2sSABNNqAbmg",
		"DiVQbq7sozeKp7PXPM1HLFc2m7ih8oepKLRK99oBY3QZak1",
		"HErWh7D2RzrjWWB2fTJfcAejD9MJpadeWWZM2Wnk7LiNWfG",
		"Es4DbDauYZYyRJbr6VxrhdcM1iufP9GtdBYf3YtSEvdwNyb",
		"EBgXT6FaVo4WsN2LmfnB2jnpDFf4zay3E492RGSn6v1tY99",
		"Dr9Zg4fxZurexParztL9SezFeHsPwdP8uGgULeRMbk8DDHJ",
		"JEnSTZJpLh91cSryptj57RtFxq9xXqf4U5wBH3qoP91ZZhN",
		"DqtRkrmtPANa8wrYR7Ce2LxJxk2iNFtiCxv1cXbx54uqdTN",
		"GaxmF53xbuTFKopVEseWiaCTa8fC6f99n4YfW8MGPSPYX3s",
		"EiCesgkAaighBKMpwFSAUdvwE4mRjBjNmmd5fP6d4FG8DAx",
		"HVbwWGUx7kCgUGap1Mfcs37g6JAZ5qsfsM7TsDRcSqvfxmd",
		"G45bc8Ajrd6YSXav77gQwjjGoAsR2qiGd1aLzkMy7o1RLwd",
		"Cqix2rD93Mdf7ytg8tBavAig2TvhXPgPZ2mejQvkq7qgRPq",
		"GpodE2S5dPeVjzHB4Drm8R9rEwcQPtwAspXqCVz1ooFWf5K",
		"CwfmfRmzPKLj3ntSCejuVwYmQ1F9iZWY4meQrAVoJ2G8Kce",
		"Fhp5NPvutRCJ4Gx3G8vCYGaveGcU3KgTwfrn5Zr8sLSgwVx",
		"GeYRRPkyi23wSF3cJGjq82117fKJZUbWsAGimUnzb5RPbB1",
		"DzCJ4y5oT611dfKQwbBDVbtCfENTdMCjb4KGMU3Mq6nyUMu",
	];

	let authorities = authorities
		.into_iter()
		.map(|address| {
			(
				grandpa_primitives::AuthorityId::from_ss58check(address)
					.expect("hard fork authority addresses are static and they should be carefully defined; qed."),
				1,
			)
		})
		.collect::<Vec<_>>();

	forks
		.into_iter()
		.map(|(set_id, hash, number)| {
			let hash = Hash::from_str(hash)
				.expect("hard fork hashes are static and they should be carefully defined; qed.");

			(set_id, (hash, number), authorities.clone())
		})
		.collect()
}

/// Create a new Polkadot service for a light client.
pub fn polkadot_new_light(
	config: Configuration,
)
	-> Result<impl AbstractService<
		Block = Block,
		RuntimeApi = polkadot_runtime::RuntimeApi,
		Backend = TLightBackend<Block>,
		SelectChain = LongestChain<TLightBackend<Block>, Block>,
		CallExecutor = TLightCallExecutor<Block, PolkadotExecutor>,
	>, ServiceError>
{
	new_light(config)
}

/// Create a new Kusama service for a light client.
pub fn kusama_new_light(
	config: Configuration,
)
	-> Result<impl AbstractService<
		Block = Block,
		RuntimeApi = kusama_runtime::RuntimeApi,
		Backend = TLightBackend<Block>,
		SelectChain = LongestChain<TLightBackend<Block>, Block>,
		CallExecutor = TLightCallExecutor<Block, KusamaExecutor>,
	>, ServiceError>
{
	new_light(config)
}

// We can't use service::TLightClient due to
// Rust bug: https://github.com/rust-lang/rust/issues/43580
type TLocalLightClient<Runtime, Dispatch> =  Client<
	sc_client::light::backend::Backend<sc_client_db::light::LightStorage<Block>, BlakeTwo256>,
	sc_client::light::call_executor::GenesisCallExecutor<
		sc_client::light::backend::Backend<sc_client_db::light::LightStorage<Block>, BlakeTwo256>,
		sc_client::LocalCallExecutor<
			sc_client::light::backend::Backend<
				sc_client_db::light::LightStorage<Block>,
				BlakeTwo256
			>,
			sc_executor::NativeExecutor<Dispatch>
		>
	>,
	Block,
	Runtime
>;

/// Builds a new service for a light client.
pub fn new_light<Runtime, Dispatch, Extrinsic>(
	mut config: Configuration,
)
	-> Result<impl AbstractService<
		Block = Block,
		RuntimeApi = Runtime,
		Backend = TLightBackend<Block>,
		SelectChain = LongestChain<TLightBackend<Block>, Block>,
		CallExecutor = TLightCallExecutor<Block, Dispatch>,
	>, ServiceError>
where
	Runtime: Send + Sync + 'static,
	Runtime::RuntimeApi: RuntimeApiCollection<
		Extrinsic,
		StateBackend = sc_client_api::StateBackendFor<TLightBackend<Block>, Block>
	>,
	Dispatch: NativeExecutionDispatch + 'static,
	Extrinsic: RuntimeExtrinsic,
	Runtime: sp_api::ConstructRuntimeApi<
		Block,
		TLocalLightClient<Runtime, Dispatch>,
	>,
{
	set_prometheus_registry(&mut config)?;

	let inherent_data_providers = InherentDataProviders::new();

	ServiceBuilder::new_light::<Block, Runtime, Dispatch>(config)?
		.with_select_chain(|_, backend| {
			Ok(LongestChain::new(backend.clone()))
		})?
		.with_transaction_pool(|config, client, fetcher| {
			let fetcher = fetcher
				.ok_or_else(|| "Trying to start light transaction pool without active fetcher")?;
			let pool_api = sc_transaction_pool::LightChainApi::new(client.clone(), fetcher.clone());
			let pool = sc_transaction_pool::BasicPool::with_revalidation_type(
				config, Arc::new(pool_api), sc_transaction_pool::RevalidationType::Light,
			);
			Ok(pool)
		})?
		.with_import_queue_and_fprb(|_config, client, backend, fetcher, _select_chain, _| {
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
			)?;

			Ok((import_queue, finality_proof_request_builder))
		})?
		.with_finality_proof_provider(|client, backend| {
			let provider = client as Arc<dyn grandpa::StorageAndProofProvider<_, _>>;
			Ok(Arc::new(GrandpaFinalityProofProvider::new(backend, provider)) as _)
		})?
		.with_rpc_extensions(|builder|
			-> Result<polkadot_rpc::RpcExtension, _> {
			let fetcher = builder.fetcher()
				.ok_or_else(|| "Trying to start node RPC without active fetcher")?;
			let remote_blockchain = builder.remote_backend()
				.ok_or_else(|| "Trying to start node RPC without active remote blockchain")?;

			Ok(polkadot_rpc::create_light(builder.client().clone(), remote_blockchain, fetcher, builder.pool()))
		})?
		.build()
}

/// A custom GRANDPA voting rule that "pauses" voting (i.e. keeps voting for the
/// same last finalized block) after a given block at height `N` has been
/// finalized and for a delay of `M` blocks, i.e. until the best block reaches
/// `N` + `M`, the voter will keep voting for block `N`.
struct PauseAfterBlockFor<N>(N, N);

impl<Block, B> grandpa::VotingRule<Block, B> for PauseAfterBlockFor<NumberFor<Block>> where
	Block: BlockT,
	B: sp_blockchain::HeaderBackend<Block>,
{
	fn restrict_vote(
		&self,
		backend: &B,
		base: &Block::Header,
		best_target: &Block::Header,
		current_target: &Block::Header,
	) -> Option<(Block::Hash, NumberFor<Block>)> {
		use sp_runtime::generic::BlockId;
		use sp_runtime::traits::Header as _;

		// walk backwards until we find the target block
		let find_target = |
			target_number: NumberFor<Block>,
			current_header: &Block::Header
		| {
			let mut target_hash = current_header.hash();
			let mut target_header = current_header.clone();

			loop {
				if *target_header.number() < target_number {
					unreachable!(
						"we are traversing backwards from a known block; \
						 blocks are stored contiguously; \
						 qed"
					);
				}

				if *target_header.number() == target_number {
					return Some((target_hash, target_number));
				}

				target_hash = *target_header.parent_hash();
				target_header = backend.header(BlockId::Hash(target_hash)).ok()?
					.expect("Header known to exist due to the existence of one of its descendents; qed");
			}
		};

		// only restrict votes targeting a block higher than the block
		// we've set for the pause
		if *current_target.number() > self.0 {
			// if we're past the pause period (i.e. `self.0 + self.1`)
			// then we no longer need to restrict any votes
			if *best_target.number() > self.0 + self.1 {
				return None;
			}

			// if we've finalized the pause block, just keep returning it
			// until best number increases enough to pass the condition above
			if *base.number() >= self.0 {
				return Some((base.hash(), *base.number()));
			}

			// otherwise find the target header at the pause block
			// to vote on
			return find_target(self.0, current_target);
		}

		None
	}
}

#[cfg(test)]
mod tests {
	use polkadot_test_runtime_client::prelude::*;
	use polkadot_test_runtime_client::sp_consensus::BlockOrigin;
	use sc_block_builder::BlockBuilderProvider;
	use grandpa::VotingRule;
	use sp_blockchain::HeaderBackend;
	use sp_runtime::generic::BlockId;
	use sp_runtime::traits::Header;
	use std::sync::Arc;

	#[test]
	fn grandpa_pause_voting_rule_works() {
		let client = Arc::new(polkadot_test_runtime_client::new());

		let mut push_blocks = {
			let mut client = client.clone();
			move |n| {
				for _ in 0..n {
					let block = client.new_block(Default::default()).unwrap().build().unwrap().block;
					client.import(BlockOrigin::Own, block).unwrap();
				}
			}
		};

		let get_header = {
			let client = client.clone();
			move |n| client.header(&BlockId::Number(n)).unwrap().unwrap()
		};

		// the rule should filter all votes after block #20
		// is finalized until block #50 is imported.
		let voting_rule = super::PauseAfterBlockFor(20, 30);

		// add 10 blocks
		push_blocks(10);
		assert_eq!(
			client.info().best_number,
			10,
		);

		// we have not reached the pause block
		// therefore nothing should be restricted
		assert_eq!(
			voting_rule.restrict_vote(
				&*client,
				&get_header(0),
				&get_header(10),
				&get_header(10),
			),
			None,
		);

		// add 15 more blocks
		// best block: #25
		push_blocks(15);

		// we are targeting the pause block,
		// the vote should not be restricted
		assert_eq!(
			voting_rule.restrict_vote(
				&*client,
				&get_header(10),
				&get_header(20),
				&get_header(20),
			),
			None,
		);

		// we are past the pause block, votes should
		// be limited to the pause block.
		let pause_block = get_header(20);
		assert_eq!(
			voting_rule.restrict_vote(
				&*client,
				&get_header(10),
				&get_header(21),
				&get_header(21),
			),
			Some((pause_block.hash(), *pause_block.number())),
		);

		// we've finalized the pause block, so we'll keep
		// restricting our votes to it.
		assert_eq!(
			voting_rule.restrict_vote(
				&*client,
				&pause_block, // #20
				&get_header(21),
				&get_header(21),
			),
			Some((pause_block.hash(), *pause_block.number())),
		);

		// add 30 more blocks
		// best block: #55
		push_blocks(30);

		// we're at the last block of the pause, this block
		// should still be considered in the pause period
		assert_eq!(
			voting_rule.restrict_vote(
				&*client,
				&pause_block, // #20
				&get_header(50),
				&get_header(50),
			),
			Some((pause_block.hash(), *pause_block.number())),
		);

		// we're past the pause period, no votes should be filtered
		assert_eq!(
			voting_rule.restrict_vote(
				&*client,
				&pause_block, // #20
				&get_header(51),
				&get_header(51),
			),
			None,
		);
	}
}
