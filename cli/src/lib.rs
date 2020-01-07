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

//! Polkadot CLI library.

#![warn(missing_docs)]
#![warn(unused_extern_crates)]

mod chain_spec;
#[cfg(feature = "browser")]
mod browser;

use chain_spec::ChainSpec;
use futures::{
	Future, FutureExt, TryFutureExt, future::select, channel::oneshot, compat::Future01CompatExt,
};
use tokio::runtime::Runtime;
use log::info;
use structopt::StructOpt;

pub use service::{
	AbstractService, CustomConfiguration, ProvideRuntimeApi, CoreApi, ParachainHost, IsKusama, self,
	WrappedExecutor
};

pub use cli::{VersionInfo, IntoExit, NoCustom, SharedParams};
pub use cli::{display_role, error};

/// Load the `ChainSpec` for the given `id`.
pub fn load_spec(id: &str) -> Result<Option<service::ChainSpec>, String> {
	Ok(match ChainSpec::from(id) {
		Some(spec) => Some(spec.load()?),
		None => None,
	})
}

#[derive(Debug, StructOpt, Clone)]
enum PolkadotSubCommands {
	#[structopt(name = "validation-worker", setting = structopt::clap::AppSettings::Hidden)]
	ValidationWorker(ValidationWorkerCommand),
}

impl cli::GetSharedParams for PolkadotSubCommands {
	fn shared_params(&self) -> Option<&cli::SharedParams> { None }
}

#[derive(Debug, StructOpt, Clone)]
struct ValidationWorkerCommand {
	#[structopt()]
	pub mem_id: String,
}

#[derive(Debug, StructOpt, Clone)]
struct PolkadotSubParams {
	#[structopt(long = "enable-authority-discovery")]
	pub authority_discovery_enabled: bool,
}

/// Parses polkadot specific CLI arguments and run the service.
pub fn run<E: IntoExit>(exit: E, version: cli::VersionInfo) -> error::Result<()> {
	let cmd = cli::parse_and_prepare::<PolkadotSubCommands, PolkadotSubParams, _>(
		&version,
		"parity-polkadot",
		std::env::args(),
	);

	// Preload spec to select native runtime
	let spec = match cmd.shared_params() {
		Some(params) => Some(cli::load_spec(params, &load_spec)?),
		None => None,
	};
	if spec.as_ref().map_or(false, |c| c.is_kusama()) {
		execute_cmd_with_runtime::<
			service::kusama_runtime::RuntimeApi,
			service::KusamaExecutor,
			service::kusama_runtime::UncheckedExtrinsic,
			_
		>(exit, &version, cmd, spec)
	} else {
		execute_cmd_with_runtime::<
			service::polkadot_runtime::RuntimeApi,
			service::PolkadotExecutor,
			service::polkadot_runtime::UncheckedExtrinsic,
			_
		>(exit, &version, cmd, spec)
	}
}

/// Execute the given `cmd` with the given runtime.
fn execute_cmd_with_runtime<R, D, E, X>(
	exit: X,
	version: &cli::VersionInfo,
	cmd: cli::ParseAndPrepare<PolkadotSubCommands, PolkadotSubParams>,
	spec: Option<service::ChainSpec>,
) -> error::Result<()>
where
	R: service::ConstructRuntimeApi<service::Block, service::TFullClient<service::Block, R, D>>
		+ service::ConstructRuntimeApi<service::Block, service::TLightClient<service::Block, R, D>>
		+ Send + Sync + 'static,
	<R as service::ConstructRuntimeApi<service::Block, service::TFullClient<service::Block, R, D>>>::RuntimeApi: service::RuntimeApiCollection<E>,
	<R as service::ConstructRuntimeApi<service::Block, service::TLightClient<service::Block, R, D>>>::RuntimeApi: service::RuntimeApiCollection<E>,
	E: service::Codec + Send + Sync + 'static,
	D: service::NativeExecutionDispatch + 'static,
	X: IntoExit,
{
	let is_kusama = spec.as_ref().map_or(false, |s| s.is_kusama());
	// Use preloaded spec
	let load_spec = |_: &str| Ok(spec);
	match cmd {
		cli::ParseAndPrepare::Run(cmd) => cmd.run(load_spec, exit,
			|exit, _cli_args, custom_args, mut config| {
				info!("{}", version.name);
				info!("  version {}", config.full_version());
				info!("  by {}, 2017-2019", version.author);
				info!("Chain specification: {}", config.chain_spec.name());
				info!("Native runtime: {}", D::native_version().runtime_version);
				if is_kusama {
					info!("----------------------------");
					info!("This chain is not in any way");
					info!("      endorsed by the       ");
					info!("     KUSAMA FOUNDATION      ");
					info!("----------------------------");
				}
				info!("Node name: {}", config.name);
				info!("Roles: {}", display_role(&config));
				config.custom = service::CustomConfiguration::default();
				config.custom.authority_discovery_enabled = custom_args.authority_discovery_enabled;
				let runtime = Runtime::new().map_err(|e| format!("{:?}", e))?;
				match config.roles {
					service::Roles::LIGHT =>
						run_until_exit(
							runtime,
							service::new_light::<R, D, E>(config).map_err(|e| format!("{:?}", e))?,
							exit.into_exit(),
						),
					_ =>
						run_until_exit(
							runtime,
							service::new_full::<R, D, E>(config).map_err(|e| format!("{:?}", e))?,
							exit.into_exit(),
						),
				}.map_err(|e| format!("{:?}", e))
			}),
			cli::ParseAndPrepare::BuildSpec(cmd) => cmd.run::<NoCustom, _, _, _>(load_spec),
			cli::ParseAndPrepare::ExportBlocks(cmd) => cmd.run_with_builder::<_, _, _, _, _, _, _>(|config|
				Ok(service::new_chain_ops::<R, D, E>(config)?), load_spec, exit),
			cli::ParseAndPrepare::ImportBlocks(cmd) => cmd.run_with_builder::<_, _, _, _, _, _, _>(|config|
				Ok(service::new_chain_ops::<R, D, E>(config)?), load_spec, exit),
			cli::ParseAndPrepare::CheckBlock(cmd) => cmd.run_with_builder::<_, _, _, _, _, _, _>(|config|
				Ok(service::new_chain_ops::<R, D, E>(config)?), load_spec, exit),
			cli::ParseAndPrepare::PurgeChain(cmd) => cmd.run(load_spec),
			cli::ParseAndPrepare::RevertChain(cmd) => cmd.run_with_builder::<_, _, _, _, _, _>(|config|
				Ok(service::new_chain_ops::<R, D, E>(config)?), load_spec),
			cli::ParseAndPrepare::CustomCommand(PolkadotSubCommands::ValidationWorker(args)) => {
				if cfg!(feature = "browser") {
					Err(error::Error::Input("Cannot run validation worker in browser".into()))
				} else {
					#[cfg(not(feature = "browser"))]
					service::run_validation_worker(&args.mem_id)?;
					Ok(())
				}
			}
	}
}

/// Run the given `service` using the `runtime` until it exits or `e` fires.
pub fn run_until_exit(
	mut runtime: Runtime,
	service: impl AbstractService,
	e: impl Future<Output = ()> + Send + Unpin + 'static,
) -> error::Result<()> {
	let (exit_send, exit) = oneshot::channel();

	let executor = runtime.executor();
	let informant = cli::informant::build(&service);
	let future = select(exit, informant)
		.map(|_| Ok(()))
		.compat();

	executor.spawn(future);

	// we eagerly drop the service so that the internal exit future is fired,
	// but we need to keep holding a reference to the global telemetry guard
	let _telemetry = service.telemetry();

	let service_res = {
		let service = service
			.map_err(|err| error::Error::Service(err))
			.compat();
		let select = select(service, e)
			.map(|_| Ok(()))
			.compat();
		runtime.block_on(select)
	};

	let _ = exit_send.send(());

	use futures01::Future;
	// TODO [andre]: timeout this future substrate/#1318
	let _ = runtime.shutdown_on_idle().wait();

	service_res
}
