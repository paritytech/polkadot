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
use sp_blockchain::{HeaderBackend, Result as ClientResult};
use block_builder::BlockBuilderApi;
use consensus::SelectChain;
use availability_store::Store as AvailabilityStore;
use futures::prelude::*;
use futures::{future::{ready, select}, task::{Spawn, SpawnExt}};
use polkadot_primitives::{Block, BlockId};
use polkadot_primitives::parachain::{CandidateReceipt, ParachainHost};
use runtime_primitives::traits::{ProvideRuntimeApi};
use babe_primitives::BabeApi;
use keystore::KeyStorePtr;
use sr_api::ApiExt;

use tokio::{runtime::Runtime as LocalRuntime};
use log::{warn, error};

use super::{Network, Collators};

type TaskExecutor = Arc<dyn Spawn + Send + Sync>;

/// Gets a list of the candidates in a block.
pub(crate) fn fetch_candidates<P: BlockBody<Block>>(client: &P, block: &BlockId)
	-> ClientResult<Option<impl Iterator<Item=CandidateReceipt>>>
{
	use codec::{Encode, Decode};
	use polkadot_runtime::{Call, ParachainsCall, UncheckedExtrinsic as RuntimeExtrinsic};

	let extrinsics = client.block_body(block)?;
	Ok(match extrinsics {
		Some(extrinsics) => extrinsics
			.into_iter()
			.filter_map(|ex| RuntimeExtrinsic::decode(&mut ex.encode().as_slice()).ok())
			.filter_map(|ex| match ex.function {
				Call::Parachains(ParachainsCall::set_heads(heads)) => {
					Some(heads.into_iter().map(|c| c.candidate))
				}
				_ => None,
			})
		.next(),
		None => None,
	})
}

// creates a task to prune redundant entries in availability store upon block finalization
//
// NOTE: this will need to be changed to finality notification rather than
// block import notifications when the consensus switches to non-instant finality.
fn prune_unneeded_availability<P>(client: Arc<P>, availability_store: AvailabilityStore)
	-> impl Future<Output=()> + Send + Unpin
	where P: Send + Sync + BlockchainEvents<Block> + BlockBody<Block> + 'static
{
	client.finality_notification_stream()
		.for_each(move |notification| {
			let hash = notification.hash;
			let parent_hash = notification.header.parent_hash;
			let candidate_hashes = match fetch_candidates(&*client, &BlockId::hash(hash)) {
				Ok(Some(candidates)) => candidates.map(|c| c.hash()).collect(),
				Ok(None) => {
					warn!("Could not extract candidates from block body of imported block {:?}", hash);
					return ready(())
				}
				Err(e) => {
					warn!("Failed to fetch block body for imported block {:?}: {:?}", hash, e);
					return ready(())
				}
			};

			if let Err(e) = availability_store.candidates_finalized(parent_hash, candidate_hashes) {
				warn!(target: "validation", "Failed to prune unneeded available data: {:?}", e);
			}

			ready(())
		})
}

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
	availability_store: AvailabilityStore,
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

		let prune_available = select(
			prune_unneeded_availability(client, availability_store),
			exit.clone()
		)
			.map(|_| ());

		// spawn this on the tokio executor since it's fine on a thread pool.
		if let Err(_) = thread_pool.spawn(prune_available) {
			error!("Failed to spawn available pruning task");
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
