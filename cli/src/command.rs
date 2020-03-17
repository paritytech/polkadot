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
use sp_runtime::traits::BlakeTwo256;
use service::{IsKusama, Block, self, RuntimeApiCollection, TFullClient};
use sp_api::ConstructRuntimeApi;
use sc_executor::NativeExecutionDispatch;
use crate::chain_spec::load_spec;
use crate::cli::{Cli, Subcommand};
use sc_cli::VersionInfo;

/// Parses polkadot specific CLI arguments and run the service.
pub fn run(version: VersionInfo) -> sc_cli::Result<()> {
	let opt = sc_cli::from_args::<Cli>(&version);

	let mut config = service::Configuration::from_version(&version);
	config.impl_name = "parity-polkadot";
	let force_kusama = opt.run.force_kusama;

	let grandpa_pause = if opt.grandpa_pause.is_empty() {
		None
	} else {
		// should be enforced by cli parsing
		assert_eq!(opt.grandpa_pause.len(), 2);
		Some((opt.grandpa_pause[0], opt.grandpa_pause[1]))
	};

	match opt.subcommand {
		None => {
			opt.run.base.init(&version)?;
			opt.run.base.update_config(
				&mut config,
				|id| load_spec(id, force_kusama),
				&version
			)?;

			let is_kusama = config.expect_chain_spec().is_kusama();

			info!("{}", version.name);
			info!("  version {}", config.full_version());
			info!("  by {}, 2017-2020", version.author);
			info!("Chain specification: {}", config.expect_chain_spec().name());
			info!("Node name: {}", config.name);
			info!("Roles: {}", config.display_role());

			if is_kusama {
				info!("Native runtime: {}", service::KusamaExecutor::native_version().runtime_version);
				info!("----------------------------");
				info!("This chain is not in any way");
				info!("      endorsed by the       ");
				info!("     KUSAMA FOUNDATION      ");
				info!("----------------------------");

				run_service_until_exit::<
					service::kusama_runtime::RuntimeApi,
					service::KusamaExecutor,
					service::kusama_runtime::UncheckedExtrinsic,
				>(config, opt.authority_discovery_enabled, grandpa_pause)
			} else {
				info!("Native runtime: {}", service::PolkadotExecutor::native_version().runtime_version);

				run_service_until_exit::<
					service::polkadot_runtime::RuntimeApi,
					service::PolkadotExecutor,
					service::polkadot_runtime::UncheckedExtrinsic,
				>(config, opt.authority_discovery_enabled, grandpa_pause)
			}
		},
		Some(Subcommand::Base(cmd)) => {
			cmd.init(&version)?;
			cmd.update_config(
				&mut config,
				|id| load_spec(id, force_kusama),
				&version
			)?;

			let is_kusama = config.expect_chain_spec().is_kusama();

			if is_kusama {
				cmd.run(config, service::new_chain_ops::<
					service::kusama_runtime::RuntimeApi,
					service::KusamaExecutor,
					service::kusama_runtime::UncheckedExtrinsic,
				>)
			} else {
				cmd.run(config, service::new_chain_ops::<
					service::polkadot_runtime::RuntimeApi,
					service::PolkadotExecutor,
					service::polkadot_runtime::UncheckedExtrinsic,
				>)
			}
		},
		Some(Subcommand::ValidationWorker(args)) => {
			sc_cli::init_logger("");

			if cfg!(feature = "browser") {
				Err(sc_cli::Error::Input("Cannot run validation worker in browser".into()))
			} else {
				#[cfg(not(feature = "browser"))]
				service::run_validation_worker(&args.mem_id)?;
				Ok(())
			}
		},
		Some(Subcommand::Benchmark(cmd)) => {
			cmd.init(&version)?;
			cmd.update_config(&mut config, |id| load_spec(id, force_kusama), &version)?;
			let is_kusama = config.expect_chain_spec().is_kusama();
			if is_kusama {
				cmd.run::<service::kusama_runtime::Block, service::KusamaExecutor>(config)
			} else {
				cmd.run::<service::polkadot_runtime::Block, service::PolkadotExecutor>(config)
			}
		},
	}
}

fn run_service_until_exit<R, D, E>(
	config: service::Configuration,
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
	match config.roles {
		service::Roles::LIGHT =>
			sc_cli::run_service_until_exit(
				config,
				|config| service::new_light::<R, D, E>(config),
			),
		_ =>
			sc_cli::run_service_until_exit(
				config,
				|config| service::new_full::<R, D, E>(
					config,
					None,
					None,
					authority_discovery_enabled,
					6000,
					grandpa_pause,
				)
					.map(|(s, _)| s),
			),
	}
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
