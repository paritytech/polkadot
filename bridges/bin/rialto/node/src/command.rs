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

use crate::cli::{Cli, Subcommand};
use rialto_runtime::{Block, RuntimeApi};
use sc_cli::{ChainSpec, Role, RuntimeVersion, SubstrateCli};

impl SubstrateCli for Cli {
	fn impl_name() -> String {
		"Rialto Bridge Node".into()
	}

	fn impl_version() -> String {
		env!("CARGO_PKG_VERSION").into()
	}

	fn description() -> String {
		"Rialto Bridge Node".into()
	}

	fn author() -> String {
		"Parity Technologies".into()
	}

	fn support_url() -> String {
		"https://github.com/paritytech/parity-bridges-common/".into()
	}

	fn copyright_start_year() -> i32 {
		2019
	}

	fn executable_name() -> String {
		"rialto-bridge-node".into()
	}

	fn native_runtime_version(_: &Box<dyn ChainSpec>) -> &'static RuntimeVersion {
		&rialto_runtime::VERSION
	}

	fn load_spec(&self, id: &str) -> Result<Box<dyn sc_service::ChainSpec>, String> {
		Ok(Box::new(
			match id {
				"" | "dev" => crate::chain_spec::Alternative::Development,
				"local" => crate::chain_spec::Alternative::LocalTestnet,
				_ => return Err(format!("Unsupported chain specification: {}", id)),
			}
			.load(),
		))
	}
}

// Rialto native executor instance.
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

/// Parse and run command line arguments
pub fn run() -> sc_cli::Result<()> {
	let cli = Cli::from_args();
	sp_core::crypto::set_default_ss58_version(sp_core::crypto::Ss58AddressFormat::custom(
		rialto_runtime::SS58Prefix::get() as u16,
	));

	match &cli.subcommand {
		Some(Subcommand::Benchmark(cmd)) =>
			if cfg!(feature = "runtime-benchmarks") {
				let runner = cli.create_runner(cmd)?;

				runner.sync_run(|config| cmd.run::<Block, ExecutorDispatch>(config))
			} else {
				println!(
					"Benchmarking wasn't enabled when building the node. \
				You can enable it with `--features runtime-benchmarks`."
				);
				Ok(())
			},
		Some(Subcommand::Key(cmd)) => cmd.run(&cli),
		Some(Subcommand::Sign(cmd)) => cmd.run(),
		Some(Subcommand::Verify(cmd)) => cmd.run(),
		Some(Subcommand::Vanity(cmd)) => cmd.run(),
		Some(Subcommand::BuildSpec(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|config| cmd.run(config.chain_spec, config.network))
		},
		Some(Subcommand::CheckBlock(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|mut config| {
				let (client, _, import_queue, task_manager) =
					polkadot_service::new_chain_ops(&mut config, None).map_err(service_error)?;
				Ok((cmd.run(client, import_queue), task_manager))
			})
		},
		Some(Subcommand::ExportBlocks(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|mut config| {
				let (client, _, _, task_manager) =
					polkadot_service::new_chain_ops(&mut config, None).map_err(service_error)?;
				Ok((cmd.run(client, config.database), task_manager))
			})
		},
		Some(Subcommand::ExportState(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|mut config| {
				let (client, _, _, task_manager) =
					polkadot_service::new_chain_ops(&mut config, None).map_err(service_error)?;
				Ok((cmd.run(client, config.chain_spec), task_manager))
			})
		},
		Some(Subcommand::ImportBlocks(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|mut config| {
				let (client, _, import_queue, task_manager) =
					polkadot_service::new_chain_ops(&mut config, None).map_err(service_error)?;
				Ok((cmd.run(client, import_queue), task_manager))
			})
		},
		Some(Subcommand::PurgeChain(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|config| cmd.run(config.database))
		},
		Some(Subcommand::Revert(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.async_run(|mut config| {
				let (client, backend, _, task_manager) =
					polkadot_service::new_chain_ops(&mut config, None).map_err(service_error)?;
				Ok((cmd.run(client, backend), task_manager))
			})
		},
		Some(Subcommand::Inspect(cmd)) => {
			let runner = cli.create_runner(cmd)?;
			runner.sync_run(|config| cmd.run::<Block, RuntimeApi, ExecutorDispatch>(config))
		},
		Some(Subcommand::PvfPrepareWorker(cmd)) => {
			let mut builder = sc_cli::LoggerBuilder::new("");
			builder.with_colors(false);
			let _ = builder.init();

			polkadot_node_core_pvf::prepare_worker_entrypoint(&cmd.socket_path);
			Ok(())
		},
		Some(crate::cli::Subcommand::PvfExecuteWorker(cmd)) => {
			let mut builder = sc_cli::LoggerBuilder::new("");
			builder.with_colors(false);
			let _ = builder.init();

			polkadot_node_core_pvf::execute_worker_entrypoint(&cmd.socket_path);
			Ok(())
		},
		None => {
			let runner = cli.create_runner(&cli.run)?;

			// some parameters that are used by polkadot nodes, but that are not used by our binary
			// let jaeger_agent = None;
			// let grandpa_pause = None;
			// let no_beefy = true;
			// let telemetry_worker_handler = None;
			// let is_collator = crate::service::IsCollator::No;
			let overseer_gen = polkadot_service::overseer::RealOverseerGen;
			runner.run_node_until_exit(|config| async move {
				match config.role {
					Role::Light => Err(sc_cli::Error::Service(sc_service::Error::Other(
						"Light client is not supported by this node".into(),
					))),
					_ => {
						let is_collator = polkadot_service::IsCollator::No;
						let grandpa_pause = None;
						let enable_beefy = true;
						let jaeger_agent = None;
						let telemetry_worker_handle = None;
						let program_path = None;
						let overseer_enable_anyways = false;

						polkadot_service::new_full::<rialto_runtime::RuntimeApi, ExecutorDispatch, _>(
							config,
							is_collator,
							grandpa_pause,
							enable_beefy,
							jaeger_agent,
							telemetry_worker_handle,
							program_path,
							overseer_enable_anyways,
							overseer_gen,
						)
							.map(|full| full.task_manager)
							.map_err(service_error)
					},
				}
			})
		},
	}
}

// We don't want to change 'service.rs' too much to ease future updates => it'll keep using
// its own error enum like original polkadot service does.
fn service_error(err: polkadot_service::Error) -> sc_cli::Error {
	sc_cli::Error::Application(Box::new(err))
}
