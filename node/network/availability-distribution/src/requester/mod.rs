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

use std::{
	collections::{
		hash_map::{Entry, HashMap},
		hash_set::HashSet,
	},
	iter::IntoIterator,
	pin::Pin,
};

use futures::{
	channel::{mpsc, oneshot},
	task::{Context, Poll},
	Stream,
};

use polkadot_node_subsystem::{
	messages::{ChainApiMessage, RuntimeApiMessage},
	overseer, ActivatedLeaf, ActiveLeavesUpdate, LeafStatus,
};
use polkadot_node_subsystem_util::runtime::{get_occupied_cores, RuntimeInfo};
use polkadot_primitives::v2::{CandidateHash, Hash, OccupiedCore, SessionIndex};

use super::{FatalError, Metrics, Result, LOG_TARGET};

#[cfg(test)]
mod tests;

/// Cache for session information.
mod session_cache;
use session_cache::SessionCache;

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
	///
	/// We remove them on failure, so we get retries on the next block still pending availability.
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

#[overseer::contextbounds(AvailabilityDistribution, prefix = self::overseer)]
impl Requester {
	/// How many ancestors of the leaf should we consider along with it.
	pub(crate) const LEAF_ANCESTRY_LEN_WITHIN_SESSION: usize = 3;

	/// Create a new `Requester`.
	///
	/// You must feed it with `ActiveLeavesUpdate` via `update_fetching_heads` and make it progress
	/// by advancing the stream.
	pub fn new(metrics: Metrics) -> Self {
		let (tx, rx) = mpsc::channel(1);
		Requester { fetches: HashMap::new(), session_cache: SessionCache::new(), tx, rx, metrics }
	}

	/// Update heads that need availability distribution.
	///
	/// For all active heads we will be fetching our chunks for availability distribution.
	pub async fn update_fetching_heads<Context>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		update: ActiveLeavesUpdate,
	) -> Result<()> {
		gum::trace!(target: LOG_TARGET, ?update, "Update fetching heads");
		let ActiveLeavesUpdate { activated, deactivated } = update;
		// Stale leaves happen after a reversion - we don't want to re-run availability there.
		if let Some(leaf) = activated.filter(|leaf| leaf.status == LeafStatus::Fresh) {
			// Order important! We need to handle activated, prior to deactivated, otherwise we might
			// cancel still needed jobs.
			self.start_requesting_chunks(ctx, runtime, leaf).await?;
		}

		self.stop_requesting_chunks(deactivated.into_iter());
		Ok(())
	}

	/// Start requesting chunks for newly imported head.
	///
	/// This will also request [`SESSION_ANCESTRY_LEN`] leaf ancestors from the same session
	/// and start requesting chunks for them too.
	async fn start_requesting_chunks<Context>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut RuntimeInfo,
		new_head: ActivatedLeaf,
	) -> Result<()> {
		let sender = &mut ctx.sender().clone();
		let ActivatedLeaf { hash: leaf, .. } = new_head;
		let (leaf_session_index, ancestors_in_session) = get_block_ancestors_in_same_session(
			sender,
			runtime,
			leaf,
			Self::LEAF_ANCESTRY_LEN_WITHIN_SESSION,
		)
		.await?;
		// Also spawn or bump tasks for candidates in ancestry in the same session.
		for hash in std::iter::once(leaf).chain(ancestors_in_session) {
			let cores = get_occupied_cores(sender, hash).await?;
			gum::trace!(
				target: LOG_TARGET,
				occupied_cores = ?cores,
				"Query occupied core"
			);
			// Important:
			// We mark the whole ancestry as live in the **leaf** hash, so we don't need to track
			// any tasks separately.
			//
			// The next time the subsystem receives leaf update, some of spawned task will be bumped
			// to be live in fresh relay parent, while some might get dropped due to the current leaf
			// being deactivated.
			self.add_cores(ctx, runtime, leaf, leaf_session_index, cores).await?;
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
	/// Note: The passed in `leaf` is not the same as `CandidateDescriptor::relay_parent` in the
	/// given cores. The latter is the `relay_parent` this candidate considers its parent, while the
	/// passed in leaf might be some later block where the candidate is still pending availability.
	async fn add_cores<Context>(
		&mut self,
		context: &mut Context,
		runtime: &mut RuntimeInfo,
		leaf: Hash,
		leaf_session_index: SessionIndex,
		cores: impl IntoIterator<Item = OccupiedCore>,
	) -> Result<()> {
		for core in cores {
			match self.fetches.entry(core.candidate_hash) {
				Entry::Occupied(mut e) =>
				// Just book keeping - we are already requesting that chunk:
				{
					e.get_mut().add_leaf(leaf);
				},
				Entry::Vacant(e) => {
					let tx = self.tx.clone();
					let metrics = self.metrics.clone();

					let task_cfg = self
						.session_cache
						.with_session_info(
							context,
							runtime,
							// We use leaf here, the relay_parent must be in the same session as the
							// leaf. This is guaranteed by runtime which ensures that cores are cleared
							// at session boundaries. At the same time, only leaves are guaranteed to
							// be fetchable by the state trie.
							leaf,
							leaf_session_index,
							|info| FetchTaskConfig::new(leaf, &core, tx, metrics, info),
						)
						.await
						.map_err(|err| {
							gum::warn!(
								target: LOG_TARGET,
								error = ?err,
								"Failed to spawn a fetch task"
							);
							err
						});

					if let Ok(Some(task_cfg)) = task_cfg {
						e.insert(FetchTask::start(task_cfg, context).await?);
					}
					// Not a validator, nothing to do.
				},
			}
		}
		Ok(())
	}
}

impl Stream for Requester {
	type Item = overseer::AvailabilityDistributionOutgoingMessages;

	fn poll_next(mut self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Option<Self::Item>> {
		loop {
			match Pin::new(&mut self.rx).poll_next(ctx) {
				Poll::Ready(Some(FromFetchTask::Message(m))) => return Poll::Ready(Some(m)),
				Poll::Ready(Some(FromFetchTask::Concluded(Some(bad_boys)))) => {
					self.session_cache.report_bad_log(bad_boys);
					continue
				},
				Poll::Ready(Some(FromFetchTask::Concluded(None))) => continue,
				Poll::Ready(Some(FromFetchTask::Failed(candidate_hash))) => {
					// Make sure we retry on next block still pending availability.
					self.fetches.remove(&candidate_hash);
				},
				Poll::Ready(None) => return Poll::Ready(None),
				Poll::Pending => return Poll::Pending,
			}
		}
	}
}

/// Requests up to `limit` ancestor hashes of relay parent in the same session.
///
/// Also returns session index of the `head`.
async fn get_block_ancestors_in_same_session<Sender>(
	sender: &mut Sender,
	runtime: &mut RuntimeInfo,
	head: Hash,
	limit: usize,
) -> Result<(SessionIndex, Vec<Hash>)>
where
	Sender:
		overseer::SubsystemSender<RuntimeApiMessage> + overseer::SubsystemSender<ChainApiMessage>,
{
	// The order is parent, grandparent, ...
	//
	// `limit + 1` since a session index for the last element in ancestry
	// is obtained through its parent. It always gets truncated because
	// `session_ancestry_len` can only be incremented `ancestors.len() - 1` times.
	let mut ancestors = get_block_ancestors(sender, head, limit + 1).await?;
	let mut ancestors_iter = ancestors.iter();

	// `head` is the child of the first block in `ancestors`, request its session index.
	let head_session_index = match ancestors_iter.next() {
		Some(parent) => runtime.get_session_index_for_child(sender, *parent).await?,
		None => {
			// No first element, i.e. empty.
			return Ok((0, ancestors))
		},
	};

	let mut session_ancestry_len = 0;
	// The first parent is skipped.
	for parent in ancestors_iter {
		// Parent is the i-th ancestor, request session index for its child -- (i-1)th element.
		let session_index = runtime.get_session_index_for_child(sender, *parent).await?;
		if session_index == head_session_index {
			session_ancestry_len += 1;
		} else {
			break
		}
	}

	// Drop the rest.
	ancestors.truncate(session_ancestry_len);

	Ok((head_session_index, ancestors))
}

/// Request up to `limit` ancestor hashes of relay parent from the Chain API.
async fn get_block_ancestors<Sender>(
	sender: &mut Sender,
	relay_parent: Hash,
	limit: usize,
) -> Result<Vec<Hash>>
where
	Sender: overseer::SubsystemSender<ChainApiMessage>,
{
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(ChainApiMessage::Ancestors {
			hash: relay_parent,
			k: limit,
			response_channel: tx,
		})
		.await;

	let ancestors = rx
		.await
		.map_err(FatalError::ChainApiSenderDropped)?
		.map_err(FatalError::ChainApi)?;
	Ok(ancestors)
}
