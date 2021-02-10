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
use itertools::{Itertools, Either}

use super::{Result, LOG_TARGET, session_cache::SessionCache};

/// A running instance of this subsystem.
struct ProtocolState {
	/// Candidates we need to fetch our chunk for.
	fetches: HashMap<CandidateHash, FetchTask>,

	/// Localized information about sessions we are currently interested in.
	///
	/// This is usually the current one and at session boundaries also the last one.
	session_cache: SessionCache,
}


struct ChunkFetchingInfo {
	descriptor: CandidateDescriptor,
	/// Validators that backed the candidate and hopefully have our chunk.
	backing_group: Vec<ValidatorIndex>,
}

impl ProtocolState {
	/// Update heads that need availability distribution.
	///
	/// For all active heads we will be fetching our chunk for availabilty distribution.
	pub(crate) fn update_fetching_heads(
		&mut self,
		ctx: &mut Context,
		update: ActiveLeavesUpdate,
	) -> Result<()> {
		let ActiveLeavesUpdate {
			activated,
			deactivated,
		} = update;
		// Order important! We need to handle activated, prior to deactivated, otherwise we might
		// cancel still needed jobs.
		self.start_requesting_chunks(ctx, activated)?;
		let dead_parents = self.stop_requesting_chunks(ctx, deactivated)?;
	}

	/// Start requesting chunks for newly imported heads.
	fn start_requesting_chunks(
		&mut self,
		ctx: &mut Context,
		new_heads: &SmallVec<[(Hash, Arc<JaegerSpan>)]>,
	) -> Result<()> {
		for (leaf, _) in new_heads {
			let cores = query_occupied_cores(ctx, leaf).await?;
			add_cores(cores)?;
		}
		Ok(())
	}

	/// Stop requesting chunks for obsolete heads.
	///
	/// Returns relay_parents which became irrelevant for availability fetching (are not
	/// referenced by any candidate anymore).
	fn stop_requesting_chunks(
		&mut self,
		ctx: &mut Context,
		obsolete_leaves: &SmallVec<[(Hash, Arc<JaegerSpan>)]>,
	) -> Result<HashSet<Hash>> {
		let obsolete_leaves: HashSet<_> = obsolete_leaves.into_iter().map(|h| h.0).collect();
		let (obsolete_parents, new_fetches): (HashSet<_>, HashMap<_>) =
			self.fetches.into_iter().partition_map(|(c_hash, task)| {
				task.remove_leaves(HashSet::from(obsolete_leaves));
				if task.is_finished() {
					Either::Left(task.get_relay_parent())
				} else {
					Either::Right((c_hash, task))
				}
			});
		self.fetches = new_fetches;
		obsolete_parents
	}

	/// Add candidates corresponding for a particular relay parent.
	///
	/// Starting requests where necessary.
	///
	/// Note: The passed in `leaf` is not the same as CandidateDescriptor::relay_parent in the
	/// given cores. The latter is the relay_parent this candidate considers its parent, while the
	/// passed in leaf might be some later block where the candidate is still pending availability.
	fn add_cores(
		&mut self,
		ctx: &mut Context,
		leaf: Hash,
		cores: impl IntoIter<Item = OccupiedCore>,
	) {
		for core in cores {
			match self.fetches.entry(core.candidate_hash) {
				Entry::Occupied(e) =>
				// Just book keeping - we are already requesting that chunk:
					e.relay_parents.insert(leaf),
				Entry::Vacant(e) => {
					e.insert(FetchTask::start(ctx, leaf, core))
				}
			}
		}
	}
}

/// Start requesting our chunk for the given core.
fn start_request_chunk(core: OccupiedCore) -> FetchTask {
	panic!("TODO: To be implemented!");
}

/// Query all hashes and descriptors of candidates pending availability at a particular block.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_occupied_cores<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
) -> Result<Vec<OccupiedCore>>
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		relay_parent,
		RuntimeApiRequest::AvailabilityCores(tx),
	)))
	.await;

	let cores: Vec<_> = rx
		.await
		.map_err(|e| Error::AvailabilityCoresResponseChannel(e))?
		.map_err(|e| Error::AvailabilityCores(e))?;

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
