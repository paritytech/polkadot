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

use itertools::{Itertools, Either}

use super::{Result, LOG_TARGET};

/// A running instance of this subsystem.
struct ProtocolState {
	/// Candidates we need to fetch our chunk for.
	fetches: HashMap<CandidateHash, FetchTask>,

	/// Localized information about sessions we are currently interested in.
	///
	/// This is usually the current one and at session boundaries also the last one.
	live_sessions: HashMap<SessionIndex, SessionInfo>,
}

/// Localized session information, tailored for the needs of availability distribution.
struct SessionInfo {
	/// Validator groups of the current session.
	///
	/// Each group's order is randomized. This way we achieve load balancing when requesting
	/// chunks, as the validators in a group will be tried in that randomized order. Each node
	/// should arrive at a different order, therefore we distribute the load.
	validator_groups: Vec<Vec<ValidatorIndex>>,

	/// Information about ourself:
	validator_id: ValidatorId,

	/// The relay parents we are keeping this entry for.
	live_in: HashSet<Hash>,
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
