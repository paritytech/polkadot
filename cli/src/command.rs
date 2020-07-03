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
#[cfg(not(feature = "service-rewr"))]
use service::{IdentifyVariant, self};
#[cfg(feature = "service-rewr")]
use service_new::{IdentifyVariant, self as service};
use sc_cli::{SubstrateCli, Result, RuntimeVersion, Role};
use crate::cli::{Cli, Subcommand};

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
			["polkadot", "kusama", "westend"].iter()
				.cloned()
				.find(|&chain| n.starts_with(chain))
				.unwrap_or("polkadot")
		} else { id };
		Ok(match id {
			"polkadot-dev" | "dev" => Box::new(service::chain_spec::polkadot_development_config()),
			"polkadot-local" => Box::new(service::chain_spec::polkadot_local_testnet_config()),
			"polkadot-staging" => Box::new(service::chain_spec::polkadot_staging_testnet_config()),
			"kusama-dev" => Box::new(service::chain_spec::kusama_development_config()),
			"kusama-local" => Box::new(service::chain_spec::kusama_local_testnet_config()),
			"kusama-staging" => Box::new(service::chain_spec::kusama_staging_testnet_config()),
			"polkadot" => Box::new(service::chain_spec::polkadot_config()?),
			"westend" => Box::new(service::chain_spec::westend_config()?),
			"kusama" => Box::new(service::chain_spec::kusama_config()?),
			"westend-dev" => Box::new(service::chain_spec::westend_development_config()),
			"westend-local" => Box::new(service::chain_spec::westend_local_testnet_config()),
			"westend-staging" => Box::new(service::chain_spec::westend_staging_testnet_config()),
			path if self.run.force_kusama => {
				Box::new(service::KusamaChainSpec::from_json_file(std::path::PathBuf::from(path))?)
			},
			path if self.run.force_westend => {
				Box::new(service::WestendChainSpec::from_json_file(std::path::PathBuf::from(path))?)
			},
			path => Box::new(service::PolkadotChainSpec::from_json_file(std::path::PathBuf::from(path))?),
		})
	}

	fn native_runtime_version(spec: &Box<dyn service::ChainSpec>) -> &'static RuntimeVersion {
		if spec.is_kusama() {
			&service::kusama_runtime::VERSION
		} else if spec.is_westend() {
			&service::westend_runtime::VERSION
		} else {
			&service::polkadot_runtime::VERSION
		}
	}
}

/// Parses polkadot specific CLI arguments and run the service.
pub fn run() -> Result<()> {
	let cli = Cli::from_args();

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
	};

	match &cli.subcommand {
		None => {
			let runtime = cli.create_runner(&cli.run.base)?;
			let chain_spec = &runtime.config().chain_spec;

			set_default_ss58_version(chain_spec);

			let authority_discovery_enabled = cli.run.authority_discovery_enabled;
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

				runtime.run_node_until_exit(|config| match config.role {
					Role::Light => service::kusama_new_light(config)
						.map(|(components, _)| components),
					_ => service::kusama_new_full(
						config,
						None,
						None,
						authority_discovery_enabled,
						6000,
						grandpa_pause,
					).map(|(components, _, _)| components)
				})
			} else if chain_spec.is_westend() {
				runtime.run_node_until_exit(|config| match config.role {
					Role::Light => service::westend_new_light(config)
						.map(|(components, _)| components),
					_ => service::westend_new_full(
						config,
						None,
						None,
						authority_discovery_enabled,
						6000,
						grandpa_pause,
					).map(|(components, _, _)| components)
				})
			} else {
				runtime.run_node_until_exit(|config| match config.role {
					Role::Light => service::polkadot_new_light(config)
						.map(|(components, _)| components),
					_ => service::polkadot_new_full(
						config,
						None,
						None,
						authority_discovery_enabled,
						6000,
						grandpa_pause,
					).map(|(components, _, _)| components)
				})
			}
		},
		Some(Subcommand::Base(subcommand)) => {
			let runtime = cli.create_runner(subcommand)?;
			let chain_spec = &runtime.config().chain_spec;

			set_default_ss58_version(chain_spec);

			if chain_spec.is_kusama() {
				runtime.run_subcommand(subcommand, |config|
					service::new_chain_ops::<
						service::kusama_runtime::RuntimeApi,
						service::KusamaExecutor,
						service::kusama_runtime::UncheckedExtrinsic,
					>(config)
				)
			} else if chain_spec.is_westend() {
				runtime.run_subcommand(subcommand, |config|
					service::new_chain_ops::<
						service::westend_runtime::RuntimeApi,
						service::WestendExecutor,
						service::westend_runtime::UncheckedExtrinsic,
					>(config)
				)
			} else {
				runtime.run_subcommand(subcommand, |config|
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
				#[cfg(all(not(feature = "browser"), not(feature = "service-rewr")))]
				service::run_validation_worker(&cmd.mem_id)?;
				Ok(())
			}
		},
		Some(Subcommand::Benchmark(cmd)) => {
			let runtime = cli.create_runner(cmd)?;
			let chain_spec = &runtime.config().chain_spec;

			set_default_ss58_version(chain_spec);

			if chain_spec.is_kusama() {
				runtime.sync_run(|config| {
					cmd.run::<service::kusama_runtime::Block, service::KusamaExecutor>(config)
				})
			} else if chain_spec.is_westend() {
				runtime.sync_run(|config| {
					cmd.run::<service::westend_runtime::Block, service::WestendExecutor>(config)
				})
			} else {
				runtime.sync_run(|config| {
					cmd.run::<service::polkadot_runtime::Block, service::PolkadotExecutor>(config)
				})
			}
		},
	}
}
