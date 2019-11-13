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

//! Polkadot CLI

#![warn(missing_docs)]

use cli::{AbstractService, VersionInfo, TaskExecutor};
use futures::channel::oneshot;
use futures::{future, FutureExt};

use std::cell::RefCell;

// the regular polkadot worker simply does nothing until ctrl-c
struct Worker;
impl cli::IntoExit for Worker {
	type Exit = future::Map<oneshot::Receiver<()>, fn(Result<(), oneshot::Canceled>) -> ()>;
	fn into_exit(self) -> Self::Exit {
		// can't use signal directly here because CtrlC takes only `Fn`.
		let (exit_send, exit) = oneshot::channel();

		let exit_send_cell = RefCell::new(Some(exit_send));
		ctrlc::set_handler(move || {
			if let Some(exit_send) = exit_send_cell.try_borrow_mut().expect("signal handler not reentrant; qed").take() {
				exit_send.send(()).expect("Error sending exit notification");
			}
		}).expect("Error setting Ctrl-C handler");

		exit.map(drop)
	}
}

impl cli::Worker for Worker {
	type Work = <Self as cli::IntoExit>::Exit;
	fn work<S, SC, B, CE>(self, _: &S, _: TaskExecutor) -> Self::Work
	where S: AbstractService<Block = service::Block, RuntimeApi = service::RuntimeApi,
		Backend = B, SelectChain = SC,
		NetworkSpecialization = service::PolkadotProtocol, CallExecutor = CE>,
		SC: service::SelectChain<service::Block> + 'static,
		B: service::Backend<service::Block, service::Blake2Hasher> + 'static,
		CE: service::CallExecutor<service::Block, service::Blake2Hasher> + Clone + Send + Sync + 'static {
		use cli::IntoExit;
		self.into_exit()
	}
}

fn main() -> Result<(), cli::error::Error> {
	let version = VersionInfo {
		name: "Parity Polkadot",
		commit: env!("VERGEN_SHA_SHORT"),
		version: env!("CARGO_PKG_VERSION"),
		executable_name: "polkadot",
		author: "Parity Team <admin@parity.io>",
		description: "Polkadot Relay-chain Client Node",
		support_url: "https://github.com/paritytech/polkadot/issues/new",
	};

	cli::run(Worker, version)
}
