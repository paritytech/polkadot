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

use log::info;
use service::{IdentifyVariant, self};
use sc_cli::{SubstrateCli, RuntimeVersion, Role};
use crate::cli::{Cli, Subcommand};
use futures::future::TryFutureExt;

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error(transparent)]
	PolkadotService(#[from] service::Error),

	#[error(transparent)]
	SubstrateCli(#[from] sc_cli::Error),

	#[error(transparent)]
	SubstrateService(#[from] sc_service::Error),

	#[error("Other: {0}")]
	Other(String),
}

impl std::convert::From<String> for Error {
	fn from(s: String) -> Self {
		Self::Other(s)
	}
}

type Result<T> = std::result::Result<T, Error>;

fn get_exec_name() -> Option<String> {
	std::env::current_exe()
		.ok()
		.and_then(|pb| pb.file_name().map(|s| s.to_os_string()))
		.and_then(|s| s.into_string().ok())
}

impl SubstrateCli for Cli {
	fn impl_name() -> String { "Parity Polkadot".into() }

	fn impl_version() -> String { env!("SUBSTRATE_CLI_IMPL_VERSION").into() }

	fn description() -> String { env!("CARGO_PKG_DESCRIPTION").into() }

	fn author() -> String { env!("CARGO_PKG_AUTHORS").into() }

	fn support_url() -> String { "https://github.com/paritytech/polkadot/issues/new".into() }

	fn copyright_start_year() -> i32 { 2017 }

	fn executable_name() -> String { "polkadot".into() }

	fn load_spec(&self, id: &str) -> std::result::Result<Box<dyn sc_service::ChainSpec>, String> {
		let id = if id == "" {
			let n = get_exec_name().unwrap_or_default();
			["polkadot", "kusama", "westend", "rococo"].iter()
				.cloned()
				.find(|&chain| n.starts_with(chain))
				.unwrap_or("polkadot")
		} else { id };
		Ok(match id {
			"kusama" => Box::new(service::chain_spec::kusama_config()?),
			#[cfg(feature = "kusama-native")]
			"kusama-dev" => Box::new(service::chain_spec::kusama_development_config()?),
			#[cfg(feature = "kusama-native")]
			"kusama-local" => Box::new(service::chain_spec::kusama_local_testnet_config()?),
			#[cfg(feature = "kusama-native")]
			"kusama-staging" => Box::new(service::chain_spec::kusama_staging_testnet_config()?),
			#[cfg(not(feature = "kusama-native"))]
			name if name.starts_with("kusama-") && !name.ends_with(".json") =>
				Err(format!("`{}` only supported with `kusama-native` feature enabled.", name))?,
			"polkadot" => Box::new(service::chain_spec::polkadot_config()?),
			"polkadot-dev" | "dev" => Box::new(service::chain_spec::polkadot_development_config()?),
			"polkadot-local" => Box::new(service::chain_spec::polkadot_local_testnet_config()?),
			"polkadot-staging" => Box::new(service::chain_spec::polkadot_staging_testnet_config()?),
			"rococo" => Box::new(service::chain_spec::rococo_config()?),
			#[cfg(feature = "rococo-native")]
			"rococo-dev" => Box::new(service::chain_spec::rococo_development_config()?),
			#[cfg(feature = "rococo-native")]
			"rococo-local" => Box::new(service::chain_spec::rococo_local_testnet_config()?),
			#[cfg(feature = "rococo-native")]
			"rococo-staging" => Box::new(service::chain_spec::rococo_staging_testnet_config()?),
			#[cfg(not(feature = "rococo-native"))]
			name if name.starts_with("rococo-") && !name.ends_with(".json") =>
				Err(format!("`{}` only supported with `rococo-native` feature enabled.", name))?,
			"westend" => Box::new(service::chain_spec::westend_config()?),
			#[cfg(feature = "westend-native")]
			"westend-dev" => Box::new(service::chain_spec::westend_development_config()?),
			#[cfg(feature = "westend-native")]
			"westend-local" => Box::new(service::chain_spec::westend_local_testnet_config()?),
			#[cfg(feature = "westend-native")]
			"westend-staging" => Box::new(service::chain_spec::westend_staging_testnet_config()?),
			#[cfg(not(feature = "westend-native"))]
			name if name.starts_with("westend-") && !name.ends_with(".json") =>
				Err(format!("`{}` only supported with `westend-native` feature enabled.", name))?,
			"wococo" => Box::new(service::chain_spec::wococo_config()?),
			#[cfg(feature = "rococo-native")]
			"wococo-dev" => Box::new(service::chain_spec::wococo_development_config()?),
			#[cfg(not(feature = "rococo-native"))]
			name if name.starts_with("wococo-") =>
				Err(format!("`{}` only supported with `rococo-native` feature enabled.", name))?,
			path => {
				let path = std::path::PathBuf::from(path);

				let chain_spec = Box::new(service::PolkadotChainSpec::from_json_file(path.clone())?) as Box<dyn service::ChainSpec>;

				// When `force_*` is given or the file name starts with the name of one of the known chains,
				// we use the chain spec for the specific chain.
				if self.run.force_rococo || chain_spec.is_rococo() || chain_spec.is_wococo() {
					Box::new(service::RococoChainSpec::from_json_file(path)?)
				} else if self.run.force_kusama || chain_spec.is_kusama() {
					Box::new(service::KusamaChainSpec::from_json_file(path)?)
				} else if self.run.force_westend || chain_spec.is_westend() {
					Box::new(service::WestendChainSpec::from_json_file(path)?)
				} else {
					chain_spec
				}
			},
		})
	}

	fn native_runtime_version(spec: &Box<dyn service::ChainSpec>) -> &'static RuntimeVersion {
		#[cfg(feature = "kusama-native")]
		if spec.is_kusama() {
			return &service::kusama_runtime::VERSION
		}

		#[cfg(feature = "westend-native")]
		if spec.is_westend() {
			return &service::westend_runtime::VERSION
		}

		#[cfg(feature = "rococo-native")]
		if spec.is_rococo() || spec.is_wococo() {
			return &service::rococo_runtime::VERSION
		}

		#[cfg(not(all(feature = "rococo-native", feature = "westend-native", feature = "kusama-native")))]
		let _ = spec;

		&service::polkadot_runtime::VERSION
	}
}

fn set_default_ss58_version(spec: &Box<dyn service::ChainSpec>) {
	use sp_core::crypto::Ss58AddressFormat;

	let ss58_version = if spec.is_kusama() {
		Ss58AddressFormat::KusamaAccount
	} else if spec.is_westend() {
		Ss58AddressFormat::SubstrateAccount
	} else {
		Ss58AddressFormat::PolkadotAccount
	};

	sp_core::crypto::set_default_ss58_version(ss58_version);
}

const DEV_ONLY_ERROR_PATTERN: &'static str =
	"can only use subcommand with --chain [polkadot-dev, kusama-dev, westend-dev, rococo-dev, wococo-dev], got ";

fn ensure_dev(spec: &Box<dyn service::ChainSpec>) -> std::result::Result<(), String> {
	if spec.is_dev() {
		Ok(())
	} else {
		Err(format!("{}{}", DEV_ONLY_ERROR_PATTERN, spec.id()))
	}
}

/// Parses polkadot specific CLI arguments and run the service.
pub fn run() -> Result<()> {
	let cli = Cli::from_args();

	match &cli.subcommand {
		None => {
			let runner = cli.create_runner(&cli.run.base)
				.map_err(Error::from)?;
			let chain_spec = &runner.config().chain_spec;

			set_default_ss58_version(chain_spec);

			let grandpa_pause = if cli.run.grandpa_pause.is_empty() {
				None
			} else {
				Some((cli.run.grandpa_pause[0], cli.run.grandpa_pause[1]))
			};

			if chain_spec.is_kusama() {
				info!("----------------------------");
				info!("This chain is not in any way");
				info!("      endorsed by the       ");
				info!("     KUSAMA FOUNDATION      ");
				info!("----------------------------");
			}

			let jaeger_agent = cli.run.jaeger_agent;

			runner.run_node_until_exit(move |config| async move {
				let role = config.role.clone();

				match role {
					#[cfg(feature = "browser")]
					Role::Light => service::build_light(config).map(|(task_manager, _)| task_manager).map_err(Into::into),
					#[cfg(not(feature = "browser"))]
					Role::Light => Err(Error::Other("Light client not enabled".into())),
					_ => service::build_full(
						config,
						service::IsCollator::No,
						grandpa_pause,
						cli.run.no_beefy,
						jaeger_agent,
						None,
					).map(|full| full.task_manager).map_err(Into::into)
				}
			})
		},
		Some(Subcommand::BuildSpec(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			Ok(runner.sync_run(|config| {
				cmd.run(config.chain_spec, config.network)
			})?)
		},
		Some(Subcommand::CheckBlock(cmd)) => {
			let runner = cli.create_runner(cmd)
				.map_err(Error::SubstrateCli)?;
			let chain_spec = &runner.config().chain_spec;

			set_default_ss58_version(chain_spec);

			runner.async_run(|mut config| {
				let (client, _, import_queue, task_manager) = service::new_chain_ops(&mut config, None)?;
				Ok((cmd.run(client, import_queue).map_err(Error::SubstrateCli), task_manager))
			})
		},
		Some(Subcommand::ExportBlocks(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			let chain_spec = &runner.config().chain_spec;

			set_default_ss58_version(chain_spec);

			Ok(runner.async_run(|mut config| {
				let (client, _, _, task_manager) = service::new_chain_ops(&mut config, None)
					.map_err(Error::PolkadotService)?;
				Ok((cmd.run(client, config.database).map_err(Error::SubstrateCli), task_manager))
			})?)
		},
		Some(Subcommand::ExportState(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			let chain_spec = &runner.config().chain_spec;

			set_default_ss58_version(chain_spec);

			Ok(runner.async_run(|mut config| {
				let (client, _, _, task_manager) = service::new_chain_ops(&mut config, None)?;
				Ok((cmd.run(client, config.chain_spec).map_err(Error::SubstrateCli), task_manager))
			})?)
		},
		Some(Subcommand::ImportBlocks(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			let chain_spec = &runner.config().chain_spec;

			set_default_ss58_version(chain_spec);

			Ok(runner.async_run(|mut config| {
				let (client, _, import_queue, task_manager) = service::new_chain_ops(&mut config, None)?;
				Ok((cmd.run(client, import_queue).map_err(Error::SubstrateCli), task_manager))
			})?)
		},
		Some(Subcommand::PurgeChain(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			Ok(runner.sync_run(|config| cmd.run(config.database))?)
		},
		Some(Subcommand::Revert(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			let chain_spec = &runner.config().chain_spec;

			set_default_ss58_version(chain_spec);

			Ok(runner.async_run(|mut config| {
				let (client, backend, _, task_manager) = service::new_chain_ops(&mut config, None)?;
				Ok((cmd.run(client, backend).map_err(Error::SubstrateCli), task_manager))
			})?)
		},
		Some(Subcommand::PvfPrepareWorker(cmd)) => {
			let mut builder = sc_cli::LoggerBuilder::new("");
			builder.with_colors(false);
			let _ = builder.init();

			#[cfg(any(target_os = "android", feature = "browser"))]
			{
				return Err(
					sc_cli::Error::Input("PVF preparation workers are not supported under this platform".into()).into()
				);
			}

			#[cfg(not(any(target_os = "android", feature = "browser")))]
			{
				polkadot_node_core_pvf::prepare_worker_entrypoint(&cmd.socket_path);
				Ok(())
			}
		},
		Some(Subcommand::PvfExecuteWorker(cmd)) => {
			let mut builder = sc_cli::LoggerBuilder::new("");
			builder.with_colors(false);
			let _ = builder.init();

			#[cfg(any(target_os = "android", feature = "browser"))]
			{
				return Err(
					sc_cli::Error::Input("PVF execution workers are not supported under this platform".into()).into()
				);
			}

			#[cfg(not(any(target_os = "android", feature = "browser")))]
			{
				polkadot_node_core_pvf::execute_worker_entrypoint(&cmd.socket_path);
				Ok(())
			}
		},
		Some(Subcommand::Benchmark(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			let chain_spec = &runner.config().chain_spec;
			set_default_ss58_version(chain_spec);

			ensure_dev(chain_spec).map_err(Error::Other)?;

			#[cfg(feature = "kusama-native")]
			if chain_spec.is_kusama() {
				return Ok(runner.sync_run(|config| {
					cmd.run::<service::kusama_runtime::Block, service::KusamaExecutor>(config)
						.map_err(|e| Error::SubstrateCli(e))
				})?)
			}

			#[cfg(feature = "westend-native")]
			if chain_spec.is_westend() {
				return Ok(runner.sync_run(|config| {
					cmd.run::<service::westend_runtime::Block, service::WestendExecutor>(config)
						.map_err(|e| Error::SubstrateCli(e))
				})?)
			}

			// else we assume it is polkadot.
			Ok(runner.sync_run(|config| {
				cmd.run::<service::polkadot_runtime::Block, service::PolkadotExecutor>(config)
					.map_err(|e| Error::SubstrateCli(e))
			})?)
		},
		Some(Subcommand::Key(cmd)) => Ok(cmd.run(&cli)?),
		#[cfg(feature = "try-runtime")]
		Some(Subcommand::TryRuntime(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			let chain_spec = &runner.config().chain_spec;
			set_default_ss58_version(chain_spec);

			use sc_service::TaskManager;
			let registry = &runner.config().prometheus_config.as_ref().map(|cfg| &cfg.registry);
			let task_manager = TaskManager::new(
				runner.config().task_executor.clone(),
				*registry,
			).map_err(|e| Error::SubstrateService(sc_service::Error::Prometheus(e)))?;

			ensure_dev(chain_spec).map_err(Error::Other)?;

			#[cfg(feature = "kusama-native")]
			if chain_spec.is_kusama() {
				return runner.async_run(|config| {
					Ok((cmd.run::<
						service::kusama_runtime::Block,
						service::KusamaExecutor,
					>(config).map_err(Error::SubstrateCli), task_manager))
				})
			}

			#[cfg(feature = "westend-native")]
			if chain_spec.is_westend() {
				return runner.async_run(|config| {
					Ok((cmd.run::<
						service::westend_runtime::Block,
						service::WestendExecutor,
					>(config).map_err(Error::SubstrateCli), task_manager))
				})
			}
			// else we assume it is polkadot.
			runner.async_run(|config| {
				Ok((cmd.run::<
					service::polkadot_runtime::Block,
					service::PolkadotExecutor,
				>(config).map_err(Error::SubstrateCli), task_manager))
			})
		},
		#[cfg(not(feature = "try-runtime"))]
		Some(Subcommand::TryRuntime) => {
			Err(Error::Other("TryRuntime wasn't enabled when building the node. \
				You can enable it with `--features try-runtime`.".into()).into())
		},
	}?;
	Ok(())
}
