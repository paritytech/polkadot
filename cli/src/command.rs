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

use std::path::PathBuf;
use std::net::SocketAddr;
use log::info;
use sp_runtime::traits::BlakeTwo256;
use service::{IsKusama, Block, self, RuntimeApiCollection, TFullClient};
use sc_service::{
	config::{
		DatabaseConfig, ExecutionStrategies, NodeKeyConfig, WasmExecutionMethod, PrometheusConfig, TelemetryEndpoints,
	}, PruningMode, Roles, TracingReceiver, TransactionPoolOptions,
};
use sp_api::ConstructRuntimeApi;
use sc_cli::{spec_factory, SubstrateCLI, CliConfiguration, Result, substrate_cli_params};
use sc_executor::NativeExecutionDispatch;
use crate::cli::{Cli, Subcommand, BaseSubcommand, RunCmd};

#[spec_factory(
	impl_name = "parity-polkadot",
	support_url = "https://github.com/paritytech/polkadot/issues/new",
	copyright_start_year = 2017,
	executable_name = "polkadot",
)]
fn spec_factory(_id: &str) -> std::result::Result<Box<dyn sc_service::ChainSpec>, String> {
	unreachable!()
}

// TODO: merge this load_spec to spec_factory somehow
fn load_spec(s: &str, is_kusama: bool) -> std::result::Result<Box<dyn service::ChainSpec>, String> {
	Ok(match s {
		"polkadot-dev" | "dev" => Box::new(service::chain_spec::polkadot_development_config()),
		"polkadot-local" => Box::new(service::chain_spec::polkadot_local_testnet_config()),
		"polkadot-staging" => Box::new(service::chain_spec::polkadot_staging_testnet_config()),
		"kusama-dev" => Box::new(service::chain_spec::kusama_development_config()),
		"kusama-local" => Box::new(service::chain_spec::kusama_local_testnet_config()),
		"kusama-staging" => Box::new(service::chain_spec::kusama_staging_testnet_config()),
		"westend" => Box::new(service::chain_spec::westend_config()?),
		"kusama" | "" => Box::new(service::chain_spec::kusama_config()?),
		path if is_kusama => Box::new(service::KusamaChainSpec::from_json_file(std::path::PathBuf::from(path))?),
		path => Box::new(service::PolkadotChainSpec::from_json_file(std::path::PathBuf::from(path))?),
	})
}

/// Parses polkadot specific CLI arguments and run the service.
pub fn run() -> Result<()> {
	let opt = Cli::from_args();

	// let force_kusama = opt.run.force_kusama; // TODO

	let grandpa_pause = if opt.grandpa_pause.is_empty() {
		None
	} else {
		// should be enforced by cli parsing
		assert_eq!(opt.grandpa_pause.len(), 2);
		Some((opt.grandpa_pause[0], opt.grandpa_pause[1]))
	};

	match opt.subcommand {
		None => {
			let runtime = Cli::create_runtime(&opt.run)?;
			let config = runtime.config();
			let is_kusama = config.chain_spec.is_kusama();

			if is_kusama {
				info!("Native runtime: {}", service::KusamaExecutor::native_version().runtime_version);
				info!("----------------------------");
				info!("This chain is not in any way");
				info!("      endorsed by the       ");
				info!("     KUSAMA FOUNDATION      ");
				info!("----------------------------");

				run_node::<
					service::kusama_runtime::RuntimeApi,
					service::KusamaExecutor,
					service::kusama_runtime::UncheckedExtrinsic,
				>(runtime, opt.authority_discovery_enabled, grandpa_pause)
			} else {
				info!("Native runtime: {}", service::PolkadotExecutor::native_version().runtime_version);

				run_node::<
					service::polkadot_runtime::RuntimeApi,
					service::PolkadotExecutor,
					service::polkadot_runtime::UncheckedExtrinsic,
				>(runtime, opt.authority_discovery_enabled, grandpa_pause)
			}
		},
		Some(Subcommand::Base(subcommand)) => {
			let runtime = Cli::create_runtime(&subcommand)?;
			let is_kusama = runtime.config().chain_spec.is_kusama();

			if is_kusama {
				runtime.run_subcommand(subcommand.subcommand, |config|
					service::new_chain_ops::<
						service::kusama_runtime::RuntimeApi,
						service::KusamaExecutor,
						service::kusama_runtime::UncheckedExtrinsic,
					>(config)
				)
			} else {
				runtime.run_subcommand(subcommand.subcommand, |config|
					service::new_chain_ops::<
						service::polkadot_runtime::RuntimeApi,
						service::PolkadotExecutor,
						service::polkadot_runtime::UncheckedExtrinsic,
					>(config)
				)
			}
		},
		Some(Subcommand::ValidationWorker(cmd)) => {
			sc_cli::init_logger("");

			if cfg!(feature = "browser") {
				Err(sc_cli::Error::Input("Cannot run validation worker in browser".into()))
			} else {
				#[cfg(not(feature = "browser"))]
				service::run_validation_worker(&cmd.mem_id)?;
				Ok(())
			}
		},
		Some(Subcommand::Benchmark(cmd)) => {
			let runtime = Cli::create_runtime(&cmd)?;
			let is_kusama = runtime.config().chain_spec.is_kusama();

			if is_kusama {
				runtime.sync_run(|config| {
					cmd.run::<service::kusama_runtime::Block, service::KusamaExecutor>(config)
				})
			} else {
				runtime.sync_run(|config| {
					cmd.run::<service::polkadot_runtime::Block, service::PolkadotExecutor>(config)
				})
			}
		},
	}
}

fn run_node<R, D, E>(
	runtime: sc_cli::Runtime<Cli>,
	authority_discovery_enabled: bool,
	grandpa_pause: Option<(u32, u32)>,
) -> sc_cli::Result<()>
where
	R: ConstructRuntimeApi<Block, service::TFullClient<Block, R, D>>
		+ Send + Sync + 'static,
	<R as ConstructRuntimeApi<Block, service::TFullClient<Block, R, D>>>::RuntimeApi:
		RuntimeApiCollection<E, StateBackend = sc_client_api::StateBackendFor<service::TFullBackend<Block>, Block>>,
	<R as ConstructRuntimeApi<Block, service::TLightClient<Block, R, D>>>::RuntimeApi:
		RuntimeApiCollection<E, StateBackend = sc_client_api::StateBackendFor<service::TLightBackend<Block>, Block>>,
	E: service::Codec + Send + Sync + 'static,
	D: service::NativeExecutionDispatch + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	<<R as ConstructRuntimeApi<Block, TFullClient<Block, R, D>>>::RuntimeApi as sp_api::ApiExt<Block>>::StateBackend:
		sp_api::StateBackend<BlakeTwo256>,
	// Rust bug: https://github.com/rust-lang/rust/issues/43580
	R: ConstructRuntimeApi<
		Block,
		TLightClient<R, D>
	>,
{
	runtime.run_node(
		|config| service::new_light::<R, D, E>(config),
		|config| service::new_full::<R, D, E>(
			config,
			None,
			None,
			authority_discovery_enabled,
			6000,
			grandpa_pause,
		).map(|(s, _)| s),
	)
	//
}

// We can't simply use `service::TLightClient` due to a
// Rust bug: https://github.com/rust-lang/rust/issues/43580
type TLightClient<Runtime, Dispatch> = sc_client::Client<
	sc_client::light::backend::Backend<sc_client_db::light::LightStorage<Block>, BlakeTwo256>,
	sc_client::light::call_executor::GenesisCallExecutor<
		sc_client::light::backend::Backend<sc_client_db::light::LightStorage<Block>, BlakeTwo256>,
		sc_client::LocalCallExecutor<
			sc_client::light::backend::Backend<sc_client_db::light::LightStorage<Block>, BlakeTwo256>,
			sc_executor::NativeExecutor<Dispatch>
		>
	>,
	Block,
	Runtime
>;

// TODO: reduce boilerplate?
impl CliConfiguration for BaseSubcommand {
	fn base_path(&self) -> sc_cli::Result<Option<&PathBuf>> { self.subcommand.base_path() }

	fn is_dev(&self) -> Result<bool> { self.subcommand.is_dev() }

	fn database_config(&self, base_path: &PathBuf, cache_size: Option<usize>) -> Result<DatabaseConfig> {
		self.subcommand.database_config(base_path, cache_size)
	}

	// TODO: only relevant override
	fn chain_spec<C: SubstrateCLI>(&self) -> Result<Box<dyn sc_service::ChainSpec>> {
		let id = match self.subcommand.get_shared_params().chain {
			Some(ref chain) => chain.clone(),
			None => {
				if self.subcommand.get_shared_params().dev {
					"dev".into()
				} else {
					"".into()
				}
			}
		};

		Ok(load_spec(id.as_str(), self.force_kusama.force_kusama)?)
	}

	fn init<C: SubstrateCLI>(&self) -> Result<()> { self.subcommand.init::<C>() }

	fn pruning(&self, is_dev: bool, roles: Roles) -> Result<PruningMode> { self.subcommand.pruning(is_dev, roles) }

	fn tracing_receiver(&self) -> Result<TracingReceiver> { self.subcommand.tracing_receiver() }

	fn tracing_targets(&self) -> Result<Option<String>> { self.subcommand.tracing_targets() }

	fn state_cache_size(&self) -> Result<usize> { self.subcommand.state_cache_size() }

	fn wasm_method(&self) -> Result<WasmExecutionMethod> { self.subcommand.wasm_method() }

	fn execution_strategies(&self, is_dev: bool) -> Result<ExecutionStrategies> {
		self.subcommand.execution_strategies(is_dev)
	}

	fn database_cache_size(&self) -> Result<Option<usize>> { self.subcommand.database_cache_size() }

	fn node_key(&self, net_config_dir: &PathBuf) -> Result<NodeKeyConfig> { self.subcommand.node_key(net_config_dir) }
}

// TODO: reduce boilerplate?
#[substrate_cli_params(
	shared_params = base.shared_params, import_params = base.import_params, network_params = base.network_config,
	keystore_params = base.keystore_params,
)]
impl CliConfiguration for RunCmd {
	// TODO: only relevant override
	fn chain_spec<C: SubstrateCLI>(&self) -> Result<Box<dyn sc_service::ChainSpec>> {
		let id = match self.base.shared_params.chain {
			Some(ref chain) => chain.clone(),
			None => {
				if self.base.shared_params.dev {
					"dev".into()
				} else {
					"".into()
				}
			}
		};

		Ok(load_spec(id.as_str(), self.force_kusama.force_kusama)?)
	}

	fn node_name(&self) -> Result<String> { self.base.node_name() }

	fn dev_key_seed(&self, is_dev: bool) -> Result<Option<String>> { self.base.dev_key_seed(is_dev) }

	fn telemetry_endpoints(
		&self,
		chain_spec: &Box<dyn sc_service::ChainSpec>,
	) -> Result<Option<TelemetryEndpoints>> { self.base.telemetry_endpoints(chain_spec) }

	fn sentry_mode(&self) -> Result<bool> { self.base.sentry_mode() }

	fn roles(&self, is_dev: bool) -> Result<Roles> { self.base.roles(is_dev) }

	fn force_authoring(&self) -> Result<bool> { self.base.force_authoring() }

	fn prometheus_config(&self) -> Result<Option<PrometheusConfig>> { self.base.prometheus_config() }

	fn disable_grandpa(&self) -> Result<bool> { self.base.disable_grandpa() }

	fn rpc_ws_max_connections(&self) -> Result<Option<usize>> { self.base.rpc_ws_max_connections() }

	fn rpc_cors(&self, is_dev: bool) -> Result<Option<Vec<String>>> { self.base.rpc_cors(is_dev) }

	fn rpc_http(&self) -> Result<Option<SocketAddr>> { self.base.rpc_http() }

	fn rpc_ws(&self) -> Result<Option<SocketAddr>> { self.base.rpc_ws() }

	fn offchain_worker(&self, roles: Roles) -> Result<bool> { self.base.offchain_worker(roles) }

	fn transaction_pool(&self) -> Result<TransactionPoolOptions> { self.base.transaction_pool() }

	fn max_runtime_instances(&self) -> Result<Option<usize>> { self.base.max_runtime_instances() }
}
