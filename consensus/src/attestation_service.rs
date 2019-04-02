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

use std::thread;
use std::time::{Duration, Instant};
use std::sync::Arc;

use client::{BlockchainEvents, ChainHead, BlockBody};
use client::block_builder::api::BlockBuilder;
use client::blockchain::HeaderBackend;
use client::runtime_api::{ApiExt, Core};
use primitives::ed25519;
use futures::prelude::*;
use polkadot_primitives::{Block, BlockId};
use polkadot_primitives::parachain::ParachainHost;
use extrinsic_store::Store as ExtrinsicStore;
use runtime_primitives::traits::ProvideRuntimeApi;
use consensus_authorities::AuthoritiesApi;

use tokio::runtime::TaskExecutor;
use tokio::runtime::current_thread::Runtime as LocalRuntime;
use tokio::timer::Interval;

use super::{Network, Collators};

// creates a task to prune redundant entries in availability store upon block finalization
//
// NOTE: this will need to be changed to finality notification rather than
// block import notifications when the consensus switches to non-instant finality.
fn prune_unneeded_availability<P>(client: Arc<P>, extrinsic_store: ExtrinsicStore)
	-> impl Future<Item=(),Error=()> + Send
	where P: Send + Sync + BlockchainEvents<Block> + BlockBody<Block> + 'static
{
	use codec::{Encode, Decode};
	use polkadot_primitives::BlockId;

	enum NotifyError {
		BodyFetch(::client::error::Error),
	}

	impl NotifyError {
		fn log(&self, hash: &::polkadot_primitives::Hash) {
			match *self {
				NotifyError::BodyFetch(ref err) => warn!("Failed to fetch block body for imported block {:?}: {:?}", hash, err),
			}
		}
	}

	client.finality_notification_stream()
		.for_each(move |notification| {
			use polkadot_runtime::{Call, ParachainsCall, UncheckedExtrinsic as RuntimeExtrinsic};

			let hash = notification.hash;
			let parent_hash = notification.header.parent_hash;
			let extrinsics = client.block_body(&BlockId::hash(hash))
				.map_err(NotifyError::BodyFetch);

			let extrinsics = match extrinsics {
				Ok(r) => r,
				Err(e) => { e.log(&hash); return Ok(()) }
			};

			let candidate_hashes = match extrinsics
				.iter()
				.filter_map(|ex| RuntimeExtrinsic::decode(&mut ex.encode().as_slice()))
				.filter_map(|ex| match ex.function {
					Call::Parachains(ParachainsCall::set_heads(ref heads)) =>
						Some(heads.iter().map(|c| c.candidate.hash()).collect()),
					_ => None,
				})
				.next()
			{
				Some(x) => x,
				None => return Ok(()),
			};

			if let Err(e) = extrinsic_store.candidates_finalized(parent_hash, candidate_hashes) {
				warn!(target: "consensus", "Failed to prune unneeded available data: {:?}", e);
			}

			Ok(())
		})
}

/// Parachain candidate attestation service handle.
pub(crate) struct ServiceHandle {
	thread: Option<thread::JoinHandle<()>>,
	exit_signal: Option<::exit_future::Signal>,
}

/// Create and start a new instance of the attestation service.
pub(crate) fn start<C, N, P>(
	client: Arc<P>,
	parachain_consensus: Arc<::ParachainConsensus<C, N, P>>,
	thread_pool: TaskExecutor,
	key: Arc<ed25519::Pair>,
	extrinsic_store: ExtrinsicStore,
) -> ServiceHandle
	where
		C: Collators + Send + Sync + 'static,
		<C::Collation as IntoFuture>::Future: Send + 'static,
		P: BlockchainEvents<Block> + ChainHead<Block> + BlockBody<Block>,
		P: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync + 'static,
		P::Api: ParachainHost<Block> + BlockBuilder<Block> + AuthoritiesApi<Block>,
		N: Network + Send + Sync + 'static,
		N::TableRouter: Send + 'static,
{
	const TIMER_DELAY: Duration = Duration::from_secs(5);
	const TIMER_INTERVAL: Duration = Duration::from_secs(30);

	let (signal, exit) = ::exit_future::signal();
	let thread = thread::spawn(move || {
		let mut runtime = LocalRuntime::new().expect("Could not create local runtime");
		let notifications = {
			let consensus = parachain_consensus.clone();
			let key = key.clone();
			let auth_client = client.clone();
			#[allow(deprecated)]
			let authorities = move |at: &BlockId| {
				if auth_client.runtime_api().has_api::<AuthoritiesApi<Block>>(at).unwrap_or(false) {
					AuthoritiesApi::authorities(&*auth_client.runtime_api(), at)
				} else {
					Core::authorities(&*auth_client.runtime_api(), at)
				}
			};
			let client = client.clone();

			client.import_notification_stream()
				.for_each(move |notification| {
					let parent_hash = notification.hash;
					if notification.is_new_best {
						let res = authorities(&BlockId::hash(parent_hash))
							.map_err(Into::into)
							.and_then(|authorities| {
								consensus.get_or_instantiate(
									parent_hash,
									&authorities,
									key.clone(),
								)
							});

						if let Err(e) = res {
							warn!("Unable to start parachain consensus on top of {:?}: {}",
								parent_hash, e);
						}
					}
					Ok(())
				})
				.select(exit.clone())
				.then(|_| Ok(()))
		};

		let prune_old_sessions = {
			let client = client.clone();
			let interval = Interval::new(
				Instant::now() + TIMER_DELAY,
				TIMER_INTERVAL,
			);

			interval
				.for_each(move |_| match client.leaves() {
					Ok(leaves) => {
						parachain_consensus.retain(|h| leaves.contains(h));
						Ok(())
					}
					Err(e) => {
						warn!("Error fetching leaves from client: {:?}", e);
						Ok(())
					}
				})
				.map_err(|e| warn!("Timer error {:?}", e))
				.select(exit.clone())
				.then(|_| Ok(()))
		};

		runtime.spawn(notifications);
		thread_pool.spawn(prune_old_sessions);

		let prune_available = prune_unneeded_availability(client, extrinsic_store)
			.select(exit.clone())
			.then(|_| Ok(()));

		// spawn this on the tokio executor since it's fine on a thread pool.
		thread_pool.spawn(prune_available);

		if let Err(e) = runtime.block_on(exit) {
			debug!("BFT event loop error {:?}", e);
		}
	});

	ServiceHandle {
		thread: Some(thread),
		exit_signal: Some(signal),
	}
}

impl Drop for ServiceHandle {
	fn drop(&mut self) {
		if let Some(signal) = self.exit_signal.take() {
			signal.fire();
		}

		if let Some(thread) = self.thread.take() {
			thread.join().expect("The service thread has panicked");
		}
	}
}
