// Copyright 2017 Parity Technologies (UK) Ltd.
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

extern crate futures;
extern crate tokio;

extern crate substrate_cli as cli;
extern crate polkadot_service as service;
extern crate exit_future;

#[macro_use]
extern crate log;

mod chain_spec;

use std::ops::Deref;
use chain_spec::ChainSpec;
use futures::Future;
use tokio::runtime::Runtime;
use service::Service as BareService;

pub use service::{
	Components as ServiceComponents, PolkadotService, CustomConfiguration, ServiceFactory, Factory,
	ProvideRuntimeApi, CoreApi, ParachainHost,
};

pub use cli::{VersionInfo, IntoExit, NoCustom};
pub use cli::error;
pub use tokio::runtime::TaskExecutor;

fn load_spec(id: &str) -> Result<Option<service::ChainSpec>, String> {
	Ok(match ChainSpec::from(id) {
		Some(spec) => Some(spec.load()?),
		None => None,
	})
}

/// Additional worker making use of the node, to run asynchronously before shutdown.
///
/// This will be invoked with the service and spawn a future that resolves
/// when complete.
pub trait Worker: IntoExit {
	/// A future that resolves when the work is done or the node should exit.
	/// This will be run on a tokio runtime.
	type Work: Future<Item=(),Error=()> + Send + 'static;

	/// Return configuration for the polkadot node.
	// TODO: make this the full configuration, so embedded nodes don't need
	// string CLI args
	fn configuration(&self) -> service::CustomConfiguration { Default::default() }

	/// Do work and schedule exit.
	fn work<S: PolkadotService>(self, service: &S, executor: TaskExecutor) -> Self::Work;
}

/// Parse command line arguments into service configuration.
///
/// IANA unassigned port ranges that we could use:
/// 6717-6766		Unassigned
/// 8504-8553		Unassigned
/// 9556-9591		Unassigned
/// 9803-9874		Unassigned
/// 9926-9949		Unassigned
pub fn run<I, T, W>(args: I, worker: W, version: cli::VersionInfo) -> error::Result<()> where
	I: IntoIterator<Item = T>,
	T: Into<std::ffi::OsString> + Clone,
	W: Worker,
{
	cli::parse_and_execute::<service::Factory, NoCustom, NoCustom, _, _, _, _, _>(
		load_spec, &version, "parity-polkadot", args, worker,
		|worker, _custom_args, mut config| {
			info!("{}", version.name);
			info!("  version {}", config.full_version());
			info!("  by {}, 2017-2019", version.author);
			info!("Chain specification: {}", config.chain_spec.name());
			info!("Node name: {}", config.name);
			info!("Roles: {:?}", config.roles);
			config.custom = worker.configuration();
			let runtime = Runtime::new().map_err(|e| format!("{:?}", e))?;
			let executor = runtime.executor();
			match config.roles {
				service::Roles::LIGHT =>
					run_until_exit(
						runtime,
						Factory::new_light(config, executor).map_err(|e| format!("{:?}", e))?,
						worker
					),
				_ => run_until_exit(
						runtime,
						Factory::new_full(config, executor).map_err(|e| format!("{:?}", e))?,
						worker
					),
			}.map_err(|e| format!("{:?}", e))
		}
	).map_err(Into::into).map(|_| ())
}

fn run_until_exit<T, C, W>(
	mut runtime: Runtime,
	service: T,
	worker: W,
) -> error::Result<()>
	where
	    T: Deref<Target=BareService<C>>,
		C: service::Components,
		BareService<C>: PolkadotService,
		W: Worker,
{
	let (exit_send, exit) = exit_future::signal();

	let executor = runtime.executor();
	cli::informant::start(&service, exit.clone(), executor.clone());

	let _ = runtime.block_on(worker.work(&*service, executor.clone()));
	exit_send.fire();

	// we eagerly drop the service so that the internal exit future is fired,
	// but we need to keep holding a reference to the global telemetry guard
	let _telemetry = service.telemetry();
	drop(service);

	// TODO [andre]: timeout this future (https://github.com/paritytech/substrate/issues/1318)
	let _ = runtime.shutdown_on_idle().wait();

	Ok(())
}
