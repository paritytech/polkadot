// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Requester takes care of requesting erasure chunks for candidates that are pending
//! availability.

use std::collections::{
	hash_map::{Entry, HashMap},
	hash_set::HashSet,
};
use std::iter::IntoIterator;
use std::pin::Pin;
use std::sync::Arc;

use futures::{
	channel::mpsc,
	task::{Context, Poll},
	Stream,
};

use sp_keystore::SyncCryptoStorePtr;

use polkadot_node_subsystem_util::request_availability_cores_ctx;
use polkadot_primitives::v1::{CandidateHash, CoreState, Hash, OccupiedCore};
use polkadot_subsystem::{
	messages::AllMessages, ActiveLeavesUpdate, jaeger, SubsystemContext,
};

use super::{error::recv_runtime, session_cache::SessionCache, Result, LOG_TARGET, Metrics};

/// A task fetching a particular chunk.
mod fetch_task;
use fetch_task::{FetchTask, FetchTaskConfig, FromFetchTask};

/// Requester takes care of requesting erasure chunks from backing groups and stores them in the
/// av store.
///
/// It implements a stream that needs to be advanced for it making progress.
pub struct Requester {
	/// Candidates we need to fetch our chunk for.
	///
	/// We keep those around as long as a candidate is pending availability on some leaf, so we
	/// won't fetch chunks multiple times.
	fetches: HashMap<CandidateHash, FetchTask>,

	/// Localized information about sessions we are currently interested in.
	session_cache: SessionCache,

	/// Sender to be cloned for `FetchTask`s.
	tx: mpsc::Sender<FromFetchTask>,

	/// Receive messages from `FetchTask`.
	rx: mpsc::Receiver<FromFetchTask>,

	/// Prometheus Metrics
	metrics: Metrics,
}

impl Requester {
	/// Create a new `Requester`.
	///
	/// You must feed it with `ActiveLeavesUpdate` via `update_fetching_heads` and make it progress
	/// by advancing the stream.
	#[tracing::instrument(level = "trace", skip(keystore, metrics), fields(subsystem = LOG_TARGET))]
	pub fn new(keystore: SyncCryptoStorePtr, metrics: Metrics) -> Self {
		// All we do is forwarding messages, no need to make this big.
		// Each sender will get one slot, see
		// [here](https://docs.rs/futures/0.3.13/futures/channel/mpsc/fn.channel.html).
		let (tx, rx) = mpsc::channel(0);
		Requester {
			fetches: HashMap::new(),
			session_cache: SessionCache::new(keystore),
			tx,
			rx,
			metrics,
		}
	}
	/// Update heads that need availability distribution.
	///
	/// For all active heads we will be fetching our chunks for availabilty distribution.
	#[tracing::instrument(level = "trace", skip(self, ctx, update), fields(subsystem = LOG_TARGET))]
	pub async fn update_fetching_heads<Context>(
		&mut self,
		ctx: &mut Context,
		update: ActiveLeavesUpdate,
	) -> Result<()>
	where
		Context: SubsystemContext,
	{
		let ActiveLeavesUpdate {
			activated,
			deactivated,
		} = update;
		// Order important! We need to handle activated, prior to deactivated, otherwise we might
		// cancel still needed jobs.
		self.start_requesting_chunks(ctx, activated.into_iter())
			.await?;
		self.stop_requesting_chunks(deactivated.into_iter());
		Ok(())
	}

	/// Start requesting chunks for newly imported heads.
	async fn start_requesting_chunks<Context>(
		&mut self,
		ctx: &mut Context,
		new_heads: impl Iterator<Item = (Hash, Arc<jaeger::Span>)>,
	) -> Result<()>
	where
		Context: SubsystemContext,
	{
		for (leaf, _) in new_heads {
			let cores = query_occupied_cores(ctx, leaf).await?;
			self.add_cores(ctx, leaf, cores).await?;
		}
		Ok(())
	}

	/// Stop requesting chunks for obsolete heads.
	///
	fn stop_requesting_chunks(&mut self, obsolete_leaves: impl Iterator<Item = Hash>) {
		let obsolete_leaves: HashSet<_> = obsolete_leaves.collect();
		self.fetches.retain(|_, task| {
			task.remove_leaves(&obsolete_leaves);
			task.is_live()
		})
	}

	/// Add candidates corresponding for a particular relay parent.
	///
	/// Starting requests where necessary.
	///
	/// Note: The passed in `leaf` is not the same as CandidateDescriptor::relay_parent in the
	/// given cores. The latter is the relay_parent this candidate considers its parent, while the
	/// passed in leaf might be some later block where the candidate is still pending availability.
	async fn add_cores<Context>(
		&mut self,
		ctx: &mut Context,
		leaf: Hash,
		cores: impl IntoIterator<Item = OccupiedCore>,
	) -> Result<()>
	where
		Context: SubsystemContext,
	{
		for core in cores {
			match self.fetches.entry(core.candidate_hash) {
				Entry::Occupied(mut e) =>
				// Just book keeping - we are already requesting that chunk:
				{
					e.get_mut().add_leaf(leaf);
				}
				Entry::Vacant(e) => {
					let tx = self.tx.clone();
					let metrics = self.metrics.clone();

					let task_cfg = self
						.session_cache
						.with_session_info(
							ctx,
							// We use leaf here, as relay_parent must be in the same session as the
							// leaf. (Cores are dropped at session boundaries.) At the same time,
							// only leaves are guaranteed to be fetchable by the state trie.
							leaf,
							|info| FetchTaskConfig::new(leaf, &core, tx, metrics, info),
						)
						.await?;

					if let Some(task_cfg) = task_cfg {
						e.insert(FetchTask::start(task_cfg, ctx).await?);
					}
					// Not a validator, nothing to do.
				}
			}
		}
		Ok(())
	}
}

impl Stream for Requester {
	type Item = AllMessages;

	fn poll_next(
		mut self: Pin<&mut Self>,
		ctx: &mut Context,
	) -> Poll<Option<AllMessages>> {
		loop {
			match Pin::new(&mut self.rx).poll_next(ctx) {
				Poll::Ready(Some(FromFetchTask::Message(m))) =>
					return Poll::Ready(Some(m)),
				Poll::Ready(Some(FromFetchTask::Concluded(Some(bad_boys)))) => {
					self.session_cache.report_bad_log(bad_boys);
					continue
				}
				Poll::Ready(Some(FromFetchTask::Concluded(None))) =>
					continue,
				Poll::Ready(None) =>
					return Poll::Ready(None),
				Poll::Pending =>
					return Poll::Pending,
			}
		}
	}
}

/// Query all hashes and descriptors of candidates pending availability at a particular block.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_occupied_cores<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
) -> Result<Vec<OccupiedCore>>
where
	Context: SubsystemContext,
{
	let cores = recv_runtime(request_availability_cores_ctx(relay_parent, ctx).await).await?;

	Ok(cores
		.into_iter()
		.filter_map(|core_state| {
			if let CoreState::Occupied(occupied) = core_state {
				Some(occupied)
			} else {
				None
			}
		})
		.collect())
}
