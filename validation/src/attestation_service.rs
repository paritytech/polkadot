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

//! Attestation service.

/// Attestation service. A long running service that creates and manages parachain attestation
/// instances.
///
/// This uses a handle to an underlying thread pool to dispatch heavy work
/// such as candidate verification while performing event-driven work
/// on a local event loop.

use std::{thread, time::Duration, sync::Arc};

use client::{BlockchainEvents, BlockBody};
use sp_blockchain::HeaderBackend;
use block_builder::BlockBuilderApi;
use consensus::SelectChain;
use futures::prelude::*;
use futures::{future::{ready, select}, task::{Spawn, SpawnExt}};
use polkadot_primitives::Block;
use polkadot_primitives::parachain::ParachainHost;
use runtime_primitives::traits::{ProvideRuntimeApi};
use babe_primitives::BabeApi;
use keystore::KeyStorePtr;
use sp_api::ApiExt;

use tokio::{runtime::Runtime as LocalRuntime};
use log::{warn, error};

use super::{Network, Collators};

type TaskExecutor = Arc<dyn Spawn + Send + Sync>;

/// Parachain candidate attestation service handle.
pub(crate) struct ServiceHandle {
	thread: Option<thread::JoinHandle<()>>,
	exit_signal: Option<::exit_future::Signal>,
}

/// Create and start a new instance of the attestation service.
pub(crate) fn start<C, N, P, SC>(
	client: Arc<P>,
	select_chain: SC,
	parachain_validation: Arc<crate::ParachainValidation<C, N, P>>,
	thread_pool: TaskExecutor,
	keystore: KeyStorePtr,
	max_block_data_size: Option<u64>,
) -> ServiceHandle
	where
		C: Collators + Send + Sync + Unpin + 'static,
		C::Collation: Send + Unpin + 'static,
		P: BlockchainEvents<Block> + BlockBody<Block>,
		P: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync + 'static,
		P::Api: ParachainHost<Block> +
			BlockBuilderApi<Block> +
			BabeApi<Block> +
			ApiExt<Block, Error = sp_blockchain::Error>,
		N: Network + Send + Sync + 'static,
		N::TableRouter: Send + 'static,
		N::BuildTableRouter: Send + Unpin + 'static,
		SC: SelectChain<Block> + 'static,
{
	const TIMER_INTERVAL: Duration = Duration::from_secs(30);

	let (signal, exit) = ::exit_future::signal();
	let thread = thread::spawn(move || {
		let mut runtime = LocalRuntime::new().expect("Could not create local runtime");
		let notifications = {
			let client = client.clone();
			let validation = parachain_validation.clone();

			let keystore = keystore.clone();

			let notifications = client.import_notification_stream()
				.for_each(move |notification| {
					let parent_hash = notification.hash;
					if notification.is_new_best {
						let res = validation.get_or_instantiate(
							parent_hash,
							&keystore,
							max_block_data_size,
						);

						if let Err(e) = res {
							warn!(
								"Unable to start parachain validation on top of {:?}: {}",
								parent_hash, e
							);
						}
					}
					ready(())
				});

			select(notifications, exit.clone())
		};

		let prune_old_sessions = {
			let select_chain = select_chain.clone();
			let interval = crate::interval(TIMER_INTERVAL)
				.for_each(move |_| match select_chain.leaves() {
					Ok(leaves) => {
						parachain_validation.retain(|h| leaves.contains(h));
						ready(())
					}
					Err(e) => {
						warn!("Error fetching leaves from client: {:?}", e);
						ready(())
					}
				});

			select(interval, exit.clone()).map(|_| ())
		};

		runtime.spawn(notifications);
		if let Err(_) = thread_pool.spawn(prune_old_sessions) {
			error!("Failed to spawn old sessions pruning task");
		}

		runtime.block_on(exit);
	});

	ServiceHandle {
		thread: Some(thread),
		exit_signal: Some(signal),
	}
}

impl Drop for ServiceHandle {
	fn drop(&mut self) {
		if let Some(signal) = self.exit_signal.take() {
			let _ = signal.fire();
		}

		if let Some(thread) = self.thread.take() {
			thread.join().expect("The service thread has panicked");
		}
	}
}
