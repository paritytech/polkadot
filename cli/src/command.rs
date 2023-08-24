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

use crate::cli::{Cli, Subcommand, NODE_VERSION};
use frame_benchmarking_cli::{BenchmarkCmd, ExtrinsicFactory, SUBSTRATE_REFERENCE_HARDWARE};
use futures::future::TryFutureExt;
use log::{info, warn};
use sc_cli::SubstrateCli;
use service::{
	self,
	benchmarking::{benchmark_inherent_data, RemarkBuilder, TransferKeepAliveBuilder},
	HeaderBackend, IdentifyVariant,
};
use sp_core::crypto::Ss58AddressFormatRegistry;
use sp_keyring::Sr25519Keyring;
use std::net::ToSocketAddrs;

pub use crate::{error::Error, service::BlockId};
#[cfg(feature = "hostperfcheck")]
pub use polkadot_performance_test::PerfCheckError;
#[cfg(feature = "pyroscope")]
use pyroscope_pprofrs::{pprof_backend, PprofConfig};

impl From<String> for Error {
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
	fn impl_name() -> String {
		"Parity Polkadot".into()
	}

	fn impl_version() -> String {
		NODE_VERSION.into()
	}

	fn description() -> String {
		env!("CARGO_PKG_DESCRIPTION").into()
	}

	fn author() -> String {
		env!("CARGO_PKG_AUTHORS").into()
	}

	fn support_url() -> String {
		"https://github.com/paritytech/polkadot/issues/new".into()
	}

	fn copyright_start_year() -> i32 {
		2017
	}

	fn executable_name() -> String {
		"polkadot".into()
	}

	fn load_spec(&self, id: &str) -> std::result::Result<Box<dyn sc_service::ChainSpec>, String> {
		let id = if id == "" {
			let n = get_exec_name().unwrap_or_default();
			["polkadot", "kusama", "westend", "rococo", "versi"]
				.iter()
				.cloned()
				.find(|&chain| n.starts_with(chain))
				.unwrap_or("polkadot")
		} else {
			id
		};
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
			#[cfg(feature = "polkadot-native")]
			"polkadot-dev" | "dev" => Box::new(service::chain_spec::polkadot_development_config()?),
			#[cfg(feature = "polkadot-native")]
			"polkadot-local" => Box::new(service::chain_spec::polkadot_local_testnet_config()?),
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
			#[cfg(feature = "rococo-native")]
			"wococo-local" => Box::new(service::chain_spec::wococo_local_testnet_config()?),
			#[cfg(not(feature = "rococo-native"))]
			name if name.starts_with("wococo-") =>
				Err(format!("`{}` only supported with `rococo-native` feature enabled.", name))?,
			#[cfg(feature = "rococo-native")]
			"versi-dev" => Box::new(service::chain_spec::versi_development_config()?),
			#[cfg(feature = "rococo-native")]
			"versi-local" => Box::new(service::chain_spec::versi_local_testnet_config()?),
			#[cfg(feature = "rococo-native")]
			"versi-staging" => Box::new(service::chain_spec::versi_staging_testnet_config()?),
			#[cfg(not(feature = "rococo-native"))]
			name if name.starts_with("versi-") =>
				Err(format!("`{}` only supported with `rococo-native` feature enabled.", name))?,
			path => {
				let path = std::path::PathBuf::from(path);

				let chain_spec = Box::new(service::PolkadotChainSpec::from_json_file(path.clone())?)
					as Box<dyn service::ChainSpec>;

				// When `force_*` is given or the file name starts with the name of one of the known
				// chains, we use the chain spec for the specific chain.
				if self.run.force_rococo ||
					chain_spec.is_rococo() ||
					chain_spec.is_wococo() ||
					chain_spec.is_versi()
				{
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
}

fn set_default_ss58_version(spec: &Box<dyn service::ChainSpec>) {
	let ss58_version = if spec.is_kusama() {
		Ss58AddressFormatRegistry::KusamaAccount
	} else if spec.is_westend() {
		Ss58AddressFormatRegistry::SubstrateAccount
	} else {
		Ss58AddressFormatRegistry::PolkadotAccount
	}
	.into();

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

/// Runs performance checks.
/// Should only be used in release build since the check would take too much time otherwise.
fn host_perf_check() -> Result<()> {
	#[cfg(not(feature = "hostperfcheck"))]
	{
		return Err(Error::FeatureNotEnabled { feature: "hostperfcheck" }.into())
	}

	#[cfg(all(not(build_type = "release"), feature = "hostperfcheck"))]
	{
		return Err(PerfCheckError::WrongBuildType.into())
	}

	#[cfg(all(feature = "hostperfcheck", build_type = "release"))]
	{
		crate::host_perf_check::host_perf_check()?;
		return Ok(())
	}
}

/// Launch a node, accepting arguments just like a regular node,
/// accepts an alternative overseer generator, to adjust behavior
/// for integration tests as needed.
/// `malus_finality_delay` restrict finality votes of this node
/// to be at most `best_block - malus_finality_delay` height.
#[cfg(feature = "malus")]
pub fn run_node(
	run: Cli,
	overseer_gen: impl service::OverseerGen,
	malus_finality_delay: Option<u32>,
) -> Result<()> {
	run_node_inner(run, overseer_gen, malus_finality_delay, |_logger_builder, _config| {})
}

fn run_node_inner<F>(
	cli: Cli,
	overseer_gen: impl service::OverseerGen,
	maybe_malus_finality_delay: Option<u32>,
	logger_hook: F,
) -> Result<()>
where
	F: FnOnce(&mut sc_cli::LoggerBuilder, &sc_service::Configuration),
{
	let runner = cli
		.create_runner_with_logger_hook::<sc_cli::RunCmd, F>(&cli.run.base, logger_hook)
		.map_err(Error::from)?;
	let chain_spec = &runner.config().chain_spec;

	// By default, enable BEEFY on all networks except Polkadot (for now), unless
	// explicitly disabled through CLI.
	let mut enable_beefy = !chain_spec.is_polkadot() && !cli.run.no_beefy;
	// BEEFY doesn't (yet) support warp sync:
	// Until we implement https://github.com/paritytech/substrate/issues/14756
	// - disallow warp sync for validators,
	// - disable BEEFY when warp sync for non-validators.
	if enable_beefy && runner.config().network.sync_mode.is_warp() {
		if runner.config().role.is_authority() {
			return Err(Error::Other(
				"Warp sync not supported for validator nodes running BEEFY.".into(),
			))
		} else {
			// disable BEEFY for non-validator nodes that are warp syncing
			warn!("🥩 BEEFY not supported when warp syncing. Disabling BEEFY.");
			enable_beefy = false;
		}
	}

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

	let jaeger_agent = if let Some(ref jaeger_agent) = cli.run.jaeger_agent {
		Some(
			jaeger_agent
				.to_socket_addrs()
				.map_err(Error::AddressResolutionFailure)?
				.next()
				.ok_or_else(|| Error::AddressResolutionMissing)?,
		)
	} else {
		None
	};

	let node_version =
		if cli.run.disable_worker_version_check { None } else { Some(NODE_VERSION.to_string()) };

	runner.run_node_until_exit(move |config| async move {
		let hwbench = (!cli.run.no_hardware_benchmarks)
			.then_some(config.database.path().map(|database_path| {
				let _ = std::fs::create_dir_all(&database_path);
				sc_sysinfo::gather_hwbench(Some(database_path))
			}))
			.flatten();

		let database_source = config.database.clone();
		let task_manager = service::build_full(
			config,
			service::NewFullParams {
				is_parachain_node: service::IsParachainNode::No,
				grandpa_pause,
				enable_beefy,
				jaeger_agent,
				telemetry_worker_handle: None,
				node_version,
				workers_path: cli.run.workers_path,
				workers_names: None,
				overseer_gen,
				overseer_message_channel_capacity_override: cli
					.run
					.overseer_channel_capacity_override,
				malus_finality_delay: maybe_malus_finality_delay,
				hwbench,
			},
		)
		.map(|full| full.task_manager)?;

		sc_storage_monitor::StorageMonitorService::try_spawn(
			cli.storage_monitor,
			database_source,
			&task_manager.spawn_essential_handle(),
		)?;

		Ok(task_manager)
	})
}

/// Parses polkadot specific CLI arguments and run the service.
pub fn run() -> Result<()> {
	let cli: Cli = Cli::from_args();

	#[cfg(feature = "pyroscope")]
	let mut pyroscope_agent_maybe = if let Some(ref agent_addr) = cli.run.pyroscope_server {
		let address = agent_addr
			.to_socket_addrs()
			.map_err(Error::AddressResolutionFailure)?
			.next()
			.ok_or_else(|| Error::AddressResolutionMissing)?;
		// The pyroscope agent requires a `http://` prefix, so we just do that.
		let agent = pyro::PyroscopeAgent::builder(
			"http://".to_owned() + address.to_string().as_str(),
			"polkadot".to_owned(),
		)
		.backend(pprof_backend(PprofConfig::new().sample_rate(113)))
		.build()?;
		Some(agent.start()?)
	} else {
		None
	};

	#[cfg(not(feature = "pyroscope"))]
	if cli.run.pyroscope_server.is_some() {
		return Err(Error::PyroscopeNotCompiledIn)
	}

	match &cli.subcommand {
		None => run_node_inner(
			cli,
			service::RealOverseerGen,
			None,
			polkadot_node_metrics::logger_hook(),
		),
		Some(Subcommand::BuildSpec(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			Ok(runner.sync_run(|config| cmd.run(config.chain_spec, config.network))?)
		},
		Some(Subcommand::CheckBlock(cmd)) => {
			let runner = cli.create_runner(cmd).map_err(Error::SubstrateCli)?;
			let chain_spec = &runner.config().chain_spec;

			set_default_ss58_version(chain_spec);

			runner.async_run(|mut config| {
				let (client, _, import_queue, task_manager) =
					service::new_chain_ops(&mut config, None)?;
				Ok((cmd.run(client, import_queue).map_err(Error::SubstrateCli), task_manager))
			})
		},
		Some(Subcommand::ExportBlocks(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			let chain_spec = &runner.config().chain_spec;

			set_default_ss58_version(chain_spec);

			Ok(runner.async_run(|mut config| {
				let (client, _, _, task_manager) =
					service::new_chain_ops(&mut config, None).map_err(Error::PolkadotService)?;
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
				let (client, _, import_queue, task_manager) =
					service::new_chain_ops(&mut config, None)?;
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
				let aux_revert = Box::new(|client, backend, blocks| {
					service::revert_backend(client, backend, blocks, config).map_err(|err| {
						match err {
							service::Error::Blockchain(err) => err.into(),
							// Generic application-specific error.
							err => sc_cli::Error::Application(err.into()),
						}
					})
				});
				Ok((
					cmd.run(client, backend, Some(aux_revert)).map_err(Error::SubstrateCli),
					task_manager,
				))
			})?)
		},
		Some(Subcommand::Benchmark(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			let chain_spec = &runner.config().chain_spec;

			match cmd {
				#[cfg(not(feature = "runtime-benchmarks"))]
				BenchmarkCmd::Storage(_) =>
					return Err(sc_cli::Error::Input(
						"Compile with --features=runtime-benchmarks \
						to enable storage benchmarks."
							.into(),
					)
					.into()),
				#[cfg(feature = "runtime-benchmarks")]
				BenchmarkCmd::Storage(cmd) => runner.sync_run(|mut config| {
					let (client, backend, _, _) = service::new_chain_ops(&mut config, None)?;
					let db = backend.expose_db();
					let storage = backend.expose_storage();

					cmd.run(config, client.clone(), db, storage).map_err(Error::SubstrateCli)
				}),
				BenchmarkCmd::Block(cmd) => runner.sync_run(|mut config| {
					let (client, _, _, _) = service::new_chain_ops(&mut config, None)?;

					cmd.run(client.clone()).map_err(Error::SubstrateCli)
				}),
				// These commands are very similar and can be handled in nearly the same way.
				BenchmarkCmd::Extrinsic(_) | BenchmarkCmd::Overhead(_) => {
					ensure_dev(chain_spec).map_err(Error::Other)?;
					runner.sync_run(|mut config| {
						let (client, _, _, _) = service::new_chain_ops(&mut config, None)?;
						let header = client.header(client.info().genesis_hash).unwrap().unwrap();
						let inherent_data = benchmark_inherent_data(header)
							.map_err(|e| format!("generating inherent data: {:?}", e))?;
						let remark_builder =
							RemarkBuilder::new(client.clone(), config.chain_spec.identify_chain());

						match cmd {
							BenchmarkCmd::Extrinsic(cmd) => {
								let tka_builder = TransferKeepAliveBuilder::new(
									client.clone(),
									Sr25519Keyring::Alice.to_account_id(),
									config.chain_spec.identify_chain(),
								);

								let ext_factory = ExtrinsicFactory(vec![
									Box::new(remark_builder),
									Box::new(tka_builder),
								]);

								cmd.run(client.clone(), inherent_data, Vec::new(), &ext_factory)
									.map_err(Error::SubstrateCli)
							},
							BenchmarkCmd::Overhead(cmd) => cmd
								.run(
									config,
									client.clone(),
									inherent_data,
									Vec::new(),
									&remark_builder,
								)
								.map_err(Error::SubstrateCli),
							_ => unreachable!("Ensured by the outside match; qed"),
						}
					})
				},
				BenchmarkCmd::Pallet(cmd) => {
					set_default_ss58_version(chain_spec);
					ensure_dev(chain_spec).map_err(Error::Other)?;

					if cfg!(feature = "runtime-benchmarks") {
						runner.sync_run(|config| {
							cmd.run::<service::Block, ()>(config)
								.map_err(|e| Error::SubstrateCli(e))
						})
					} else {
						Err(sc_cli::Error::Input(
							"Benchmarking wasn't enabled when building the node. \
				You can enable it with `--features runtime-benchmarks`."
								.into(),
						)
						.into())
					}
				},
				BenchmarkCmd::Machine(cmd) => runner.sync_run(|config| {
					cmd.run(&config, SUBSTRATE_REFERENCE_HARDWARE.clone())
						.map_err(Error::SubstrateCli)
				}),
				// NOTE: this allows the Polkadot client to leniently implement
				// new benchmark commands.
				#[allow(unreachable_patterns)]
				_ => Err(Error::CommandNotImplemented),
			}
		},
		Some(Subcommand::HostPerfCheck) => {
			let mut builder = sc_cli::LoggerBuilder::new("");
			builder.with_colors(true);
			builder.init()?;

			host_perf_check()
		},
		Some(Subcommand::Key(cmd)) => Ok(cmd.run(&cli)?),
		#[cfg(feature = "try-runtime")]
		Some(Subcommand::TryRuntime) => Err(try_runtime_cli::DEPRECATION_NOTICE.to_owned().into()),
		#[cfg(not(feature = "try-runtime"))]
		Some(Subcommand::TryRuntime) => Err("TryRuntime wasn't enabled when building the node. \
				You can enable it with `--features try-runtime`."
			.to_owned()
			.into()),
		Some(Subcommand::ChainInfo(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			Ok(runner.sync_run(|config| cmd.run::<service::Block>(&config))?)
		},
	}?;

	#[cfg(feature = "pyroscope")]
	if let Some(pyroscope_agent) = pyroscope_agent_maybe.take() {
		let agent = pyroscope_agent.stop()?;
		agent.shutdown();
	}
	Ok(())
}
