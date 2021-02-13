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

//! `ProtocolState` representing a running availability distribution subsystem.
//!
//! We keep track of [`FetchTask`]s, which get created on [`ActiveLeavesUpdate`]s for each occupied
//! core in the leaves, if we have not yet created it before. We keep track for which
//! relay parents a `FetchTask` is considered live (corresponding slot is occupied with the
//! candidate fetched). Once there is no relay parent left for which that task is considered live,
//! it gets removed.
//!
//! We keep that task around as long as its corresponding candidate is considered pending
//! availability, even if we fetched our chunk already. This is so we won't fetch our piece again,
//! just because the candidate is still pending availability in the next block.
//!
//! We are also dependent on session information. We need to know which validators are in a
//! particular validator group, backing our candidate, so we can request our erasure chunk from
//! them.
//!
//! We want to randomize the list of validators in each group, so we get a
//! random order of validators to try to get the chunk from. This is to ensure load balancing, each
//! requesting validator should have a different order, thus trying different validators.
//!
//! But We would like to keep that randomized order around for an entire session, so our particular
//! validator will always request from the same validators, thus making sure it will find an open
//! network connection on each request.
//!
//! (TODO: What to do on session boundaries? Initial delay acceptable? Connect with some fake
//! request to future validators? Use a peer set after all and connect that to the future session?)
//!
//! So we need to keep some customized session info around, which seems to be a good idea for
//! performance reasons anyway. That's where `SessionCache` comes into play. It is used to keep
//! session information around as long as we need it. But how long do we need it? How do we manage
//! that cache? We can't rely on `ActiveLeavesUpdate`s heads alone, as we might get occupied slots
//! for heads we never got an `ActiveLeavesUpdate` from, therefore we don't populate the session
//! cache with sessions our leaves correspond to, but directly with the sessions of the relay
//! parents of our `CandidateDescriptor`s. So, its clear how to populate the cache, but when can we
//! get rid of cached session information? If for sure is safe to do when there is no
//! candidate/FetchTask around anymore which references it. Thus the cache simply consists of
//! `Weak` pointers to the actual session infos and the `FetchTask`s keep `Rc`s, therefore we know
//! exactly when we can get rid of a cache entry by means of the Weak pointer evaluating to `None`.

use std::collections::{
	hash_map::{Entry, HashMap},
	hash_set::HashSet,
};
use std::iter::IntoIterator;
use std::sync::Arc;

use futures::channel::oneshot;
use jaeger::JaegerSpan;

use itertools::{Either, Itertools};

use super::{fetch_task::FetchTask, session_cache::SessionCache, Result, LOG_TARGET};
use polkadot_primitives::v1::{
	BlakeTwo256, CandidateDescriptor, CandidateHash, CoreState, ErasureChunk, Hash, HashT,
	OccupiedCore, SessionIndex, ValidatorId, ValidatorIndex, PARACHAIN_KEY_TYPE_ID,
};
use polkadot_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	jaeger, ActiveLeavesUpdate, FromOverseer, OverseerSignal, PerLeafSpan, SpawnedSubsystem,
	Subsystem, SubsystemContext, SubsystemError, messages::AvailabilityDistributionMessage,
};
use polkadot_node_subsystem_util::request_availability_cores_ctx;

/// A running instance of this subsystem.
pub struct ProtocolState {
	/// Candidates we need to fetch our chunk for.
	fetches: HashMap<CandidateHash, FetchTask>,

	/// Localized information about sessions we are currently interested in.
	///
	/// This is usually the current one and at session boundaries also the last one.
	session_cache: SessionCache,
}

impl ProtocolState {
	/// Update heads that need availability distribution.
	///
	/// For all active heads we will be fetching our chunk for availabilty distribution.
	pub(crate) fn update_fetching_heads<Context>(
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
		self.start_requesting_chunks(ctx, activated).await?;
		self.stop_requesting_chunks(ctx, deactivated)?;
	}

	/// Start requesting chunks for newly imported heads.
	async fn start_requesting_chunks<Context>(
		&mut self,
		ctx: &mut Context,
		new_heads: impl Iterator<Item = (Hash, Arc<JaegerSpan>)>,
	) -> Result<()>
	where
		Context: SubsystemContext<Message = AvailabilityDistributionMessage> + Sync + Send,
	{
		for (leaf, _) in new_heads {
			let cores = query_occupied_cores(ctx, leaf).await?;
			self.add_cores(ctx, leaf, cores).await?;
		}
		Ok(())
	}

	/// Stop requesting chunks for obsolete heads.
	///
	/// Returns relay_parents which became irrelevant for availability fetching (are not
	/// referenced by any candidate anymore).
	fn stop_requesting_chunks(
		&mut self,
		obsolete_leaves: impl Iterator<Item = (Hash, Arc<JaegerSpan>)>,
	) -> Result<HashSet<Hash>> {
		let obsolete_leaves: HashSet<_> = obsolete_leaves.into_iter().map(|h| h.0).collect();
		let new_fetches =
			self.fetches.into_iter().filter_map(|(c_hash, task)| {
				task.remove_leaves(HashSet::from(obsolete_leaves));
				if task.is_finished() {
					Some(task.get_relay_parent())
				}
				else {
					None
				}
			}).collect();
		self.fetches = new_fetches;
	}

	/// Add candidates corresponding for a particular relay parent.
	///
	/// Starting requests where necessary.
	///
	/// Note: The passed in `leaf` is not the same as CandidateDescriptor::relay_parent in the
	/// given cores. The latter is the relay_parent this candidate considers its parent, while the
	/// passed in leaf might be some later block where the candidate is still pending availability.
	fn add_cores<Context>(
		&mut self,
		ctx: &mut Context,
		leaf: Hash,
		cores: impl IntoIterator<Item = OccupiedCore>,
	) 
	where
		Context: SubsystemContext,
	{
		for core in cores {
			match self.fetches.entry(core.candidate_hash) {
				Entry::Occupied(e) =>
				// Just book keeping - we are already requesting that chunk:
					e.get_mut().add_leaf(leaf),
				Entry::Vacant(e) => {
					let session_info = self.session_cache.fetch_session_info(ctx, core.candidate_descriptor.relay_parent)?;
					if let Some(session_info) = session_info {
						e.insert(FetchTask::start(ctx, leaf, core, session_info))
					}
					// Not a validator, nothing to do.
				}
			}
		}
	}
}

///// Query all hashes and descriptors of candidates pending availability at a particular block.
// #[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
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
